//! Claude Code and Codex sessions started in a terminal (their TUIs), read
//! from the agents' own session stores so they can be listed and continued.
//!
//! - Claude Code: `~/.claude/projects/<dir>/<session>.jsonl`; TUI sessions
//!   have `entrypoint: "cli"`. Live ones are registered in
//!   `~/.claude/sessions/<pid>.json`.
//! - Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`; TUI sessions have
//!   `originator: "codex-tui"`. Titles live in `~/.codex/session_index.jsonl`;
//!   a running `codex` keeps its rollout file open.

use std::{
    collections::HashMap,
    io::{BufRead as _, BufReader},
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;

use crate::threads::{ChatItem, ToolStatus, describe_tool};

/// Sessions untouched for longer than this are not listed.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
/// Tool output kept when importing a transcript.
const MAX_OUTPUT: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub struct ExternalSession {
    pub agent: String,
    /// The agent's own session id.
    pub id: String,
    pub path: PathBuf,
    pub cwd: PathBuf,
    pub title: String,
    /// Unix seconds of the last write.
    pub updated: u64,
    /// Running in a terminal now: its status ("busy", "idle", "running").
    pub live: Option<String>,
}

#[derive(Clone)]
struct Header {
    agent: &'static str,
    id: String,
    cwd: PathBuf,
    title: String,
}

/// Headers by file, reread only when the file changes.
static HEADERS: Mutex<Option<HashMap<PathBuf, (u64, Option<Header>)>>> = Mutex::new(None);

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn mtime_secs(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Scratch and temp directories are not projects anyone wants listed.
fn is_scratch(cwd: &Path) -> bool {
    ["/private/tmp", "/tmp", "/var/folders", "/private/var/folders"]
        .iter()
        .any(|prefix| cwd.starts_with(prefix))
}

/// The first line of a message, as a title.
fn title_from(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    line.chars().take(80).collect()
}

/// "2026-10-21T08:47:40.047Z" as unix seconds.
fn parse_timestamp(text: &str) -> Option<u64> {
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.trim_end_matches('Z').split(':');
    let (h, min) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let sec = t.next()?.split('.').next()?.parse::<i64>().ok()?;
    // Days from civil date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + h * 3600 + min * 60 + sec).ok()
}

fn clip(text: &str) -> String {
    if text.len() <= MAX_OUTPUT {
        return text.to_string();
    }
    let mut end = MAX_OUTPUT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (truncated)", &text[..end])
}

/// Text from a string or a list of `{text}` parts.
fn text_of(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Messages the CLI writes on the user's behalf, not typed by them.
fn is_injected(text: &str) -> bool {
    let text = text.trim_start();
    text.is_empty()
        || text.starts_with('<')
        || text.starts_with("Caveat:")
        || text.starts_with("[Image: source:")
        || text.starts_with("# AGENTS.md")
}

// ---- Claude Code -----------------------------------------------------------

fn claude_header(path: &Path) -> Option<Header> {
    let file = std::fs::File::open(path).ok()?;
    let (mut cwd, mut entrypoint, mut title, mut first_prompt) = (None, None, None, None);
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        // Most lines are large blobs; look before parsing.
        let interesting = line.contains("\"ai-title\"")
            || (first_prompt.is_none() && line.contains("\"type\":\"user\""))
            || (cwd.is_none() && line.contains("\"cwd\""));
        if !interesting {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(t) = v.get("aiTitle").and_then(Value::as_str) {
            title = Some(t.to_string());
        }
        if cwd.is_none() {
            cwd = v.get("cwd").and_then(Value::as_str).map(PathBuf::from);
            entrypoint = v.get("entrypoint").and_then(Value::as_str).map(String::from);
        }
        let is_prompt = v["type"] == "user"
            && !v["isMeta"].as_bool().unwrap_or(false)
            && !v["isSidechain"].as_bool().unwrap_or(false);
        if first_prompt.is_none() && is_prompt {
            let text = text_of(&v["message"]["content"]);
            if !is_injected(&text) {
                first_prompt = Some(title_from(&text));
            }
        }
    }
    if entrypoint.as_deref() != Some("cli") {
        return None;
    }
    let id = path.file_stem()?.to_string_lossy().into_owned();
    Some(Header {
        agent: "claude",
        id,
        cwd: cwd?,
        title: title.or(first_prompt).unwrap_or_else(|| "Claude Code session".into()),
    })
}

/// Live Claude Code TUIs, by session id, with their status.
fn claude_live() -> HashMap<String, String> {
    let mut live = HashMap::new();
    let Ok(entries) = std::fs::read_dir(home().join(".claude/sessions")) else {
        return live;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(v) = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else {
            continue;
        };
        let pid = v["pid"].as_i64().or_else(|| v["pid"].as_str()?.parse().ok());
        // SAFETY: signal 0 only checks that the process exists.
        let alive = pid.is_some_and(|pid| unsafe { libc::kill(pid as libc::pid_t, 0) } == 0);
        if alive && v["kind"] == "interactive" {
            if let Some(id) = v["sessionId"].as_str() {
                let status = v["status"].as_str().unwrap_or("idle").to_string();
                live.insert(id.to_string(), status);
            }
        }
    }
    live
}

// ---- Codex -----------------------------------------------------------------

fn codex_header(path: &Path, names: &HashMap<String, String>) -> Option<Header> {
    let file = std::fs::File::open(path).ok()?;
    let mut lines = BufReader::new(file).lines().map_while(Result::ok);
    let meta: Value = serde_json::from_str(&lines.next()?).ok()?;
    let payload = &meta["payload"];
    if meta["type"] != "session_meta" || payload["originator"] != "codex-tui" {
        return None;
    }
    let id = payload["id"].as_str()?.to_string();
    let cwd = PathBuf::from(payload["cwd"].as_str()?);
    let title = names.get(&id).cloned().or_else(|| {
        lines.take(400).find_map(|line| {
            if !line.contains("\"role\":\"user\"") {
                return None;
            }
            let v: Value = serde_json::from_str(&line).ok()?;
            let text = text_of(&v["payload"]["content"]);
            (!is_injected(&text)).then(|| title_from(&text))
        })
    });
    Some(Header {
        agent: "codex",
        id,
        cwd,
        title: title.unwrap_or_else(|| "Codex session".into()),
    })
}

fn codex_names() -> HashMap<String, String> {
    let Ok(text) = std::fs::read_to_string(home().join(".codex/session_index.jsonl")) else {
        return HashMap::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|v| Some((v["id"].as_str()?.to_string(), v["thread_name"].as_str()?.to_string())))
        .collect()
}

/// Rollout files held open by running `codex` processes.
fn codex_live() -> HashMap<String, String> {
    let mut live = HashMap::new();
    let Ok(out) = Command::new("ps").args(["-axo", "pid=,comm="]).output() else {
        return live;
    };
    let pids: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, comm) = line.trim().split_once(' ')?;
            (comm.trim() == "codex" || comm.trim().ends_with("/codex")).then(|| pid.to_string())
        })
        .collect();
    if pids.is_empty() {
        return live;
    }
    let Ok(out) = Command::new("lsof").args(["-Fn", "-p", &pids.join(",")]).output() else {
        return live;
    };
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let Some(name) = line.strip_prefix('n') else { continue };
        if name.contains("/rollout-") && name.ends_with(".jsonl") {
            // rollout-<time>-<uuid>.jsonl
            let stem = name.trim_end_matches(".jsonl");
            if stem.len() > 36 {
                live.insert(stem[stem.len() - 36..].to_string(), "running".to_string());
            }
        }
    }
    live
}

fn walk_jsonl(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                walk_jsonl(&path, depth - 1, out);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

/// Every recent TUI session of Claude Code and Codex, newest first. Blocks.
pub fn scan() -> Vec<ExternalSession> {
    let cutoff = SystemTime::now()
        .checked_sub(MAX_AGE)
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let mut claude_files = Vec::new();
    // Top-level files only: subagent transcripts live in subdirectories.
    if let Ok(dirs) = std::fs::read_dir(home().join(".claude/projects")) {
        for dir in dirs.flatten() {
            walk_jsonl(&dir.path(), 0, &mut claude_files);
        }
    }
    let mut codex_files = Vec::new();
    walk_jsonl(&home().join(".codex/sessions"), 3, &mut codex_files);

    let names = codex_names();
    let (claude_live, codex_live) = (claude_live(), codex_live());

    let mut guard = HEADERS.lock().unwrap();
    let cache = guard.get_or_insert_with(HashMap::new);
    let mut sessions = Vec::new();
    for (path, is_codex) in claude_files
        .into_iter()
        .map(|p| (p, false))
        .chain(codex_files.into_iter().map(|p| (p, true)))
    {
        let Some(updated) = mtime_secs(&path).filter(|&m| m >= cutoff) else { continue };
        let header = match cache.get(&path) {
            Some((seen, header)) if *seen == updated => header.clone(),
            _ => {
                let header = if is_codex { codex_header(&path, &names) } else { claude_header(&path) };
                cache.insert(path.clone(), (updated, header.clone()));
                header
            }
        };
        let Some(header) = header.filter(|h| !is_scratch(&h.cwd)) else { continue };
        let live = if is_codex { &codex_live } else { &claude_live };
        sessions.push(ExternalSession {
            agent: header.agent.to_string(),
            live: live.get(&header.id).cloned(),
            id: header.id,
            path,
            cwd: header.cwd,
            title: header.title,
            updated,
        });
    }
    sessions.sort_by(|a, b| b.updated.cmp(&a.updated));
    sessions
}

// ---- Transcripts -------------------------------------------------------------

fn set_tool_result(items: &mut [ChatItem], call_id: &str, text: String, failed: bool) {
    for item in items.iter_mut().rev() {
        if let ChatItem::Tool { id, output, status, .. } = item {
            if id == call_id {
                *output = (!text.trim().is_empty()).then(|| clip(&text));
                *status = if failed { ToolStatus::Failed } else { ToolStatus::Done };
                return;
            }
        }
    }
}

fn push_tool(items: &mut Vec<ChatItem>, id: String, name: &str, input: &Value) {
    let (title, detail, path) = describe_tool(name, input);
    let code = (name.contains("cua_repl")).then(|| input["code"].as_str().map(String::from)).flatten();
    items.push(ChatItem::Tool {
        id,
        title,
        detail,
        path,
        output: None,
        status: ToolStatus::Done,
        code,
        images: Vec::new(),
    });
}

fn claude_transcript(path: &Path) -> Vec<ChatItem> {
    let mut items = Vec::new();
    let Ok(file) = std::fs::File::open(path) else { return items };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if v["isSidechain"].as_bool().unwrap_or(false) {
            continue;
        }
        match v["type"].as_str().unwrap_or("") {
            "user" if !v["isMeta"].as_bool().unwrap_or(false) => match &v["message"]["content"] {
                Value::String(text) => {
                    if !is_injected(text) {
                        items.push(ChatItem::User(text.clone()));
                    }
                }
                Value::Array(parts) => {
                    let mut text = Vec::new();
                    for part in parts {
                        match part["type"].as_str().unwrap_or("") {
                            "text" => text.push(part["text"].as_str().unwrap_or("").to_string()),
                            "tool_result" => set_tool_result(
                                &mut items,
                                part["tool_use_id"].as_str().unwrap_or(""),
                                text_of(&part["content"]),
                                part["is_error"].as_bool().unwrap_or(false),
                            ),
                            _ => {}
                        }
                    }
                    let text = text.join("\n");
                    if !is_injected(&text) {
                        items.push(ChatItem::User(text));
                    }
                }
                _ => {}
            },
            "assistant" => {
                for part in v["message"]["content"].as_array().into_iter().flatten() {
                    match part["type"].as_str().unwrap_or("") {
                        "text" => {
                            let text = part["text"].as_str().unwrap_or("");
                            if !text.trim().is_empty() {
                                items.push(ChatItem::Assistant(text.to_string()));
                            }
                        }
                        "tool_use" => push_tool(
                            &mut items,
                            part["id"].as_str().unwrap_or("").to_string(),
                            part["name"].as_str().unwrap_or("Tool"),
                            &part["input"],
                        ),
                        _ => {}
                    }
                }
            }
            "system" if v["subtype"] == "turn_duration" => items.push(ChatItem::TurnEnd {
                ms: v["durationMs"].as_u64(),
                at: v["timestamp"].as_str().and_then(parse_timestamp),
            }),
            _ => {}
        }
    }
    items
}

fn codex_transcript(path: &Path) -> Vec<ChatItem> {
    let mut items = Vec::new();
    let Ok(file) = std::fs::File::open(path) else { return items };
    let mut turn_started: Option<u64> = None;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let p = &v["payload"];
        let at = v["timestamp"].as_str().and_then(parse_timestamp);
        match (v["type"].as_str().unwrap_or(""), p["type"].as_str().unwrap_or("")) {
            ("response_item", "message") => {
                let text = text_of(&p["content"]);
                match p["role"].as_str() {
                    Some("user") if !is_injected(&text) => items.push(ChatItem::User(text)),
                    Some("assistant") if !text.trim().is_empty() => items.push(ChatItem::Assistant(text)),
                    _ => {}
                }
            }
            ("response_item", "function_call") => {
                let name = match p["namespace"].as_str() {
                    Some(ns) => format!("{ns}.{}", p["name"].as_str().unwrap_or("tool")),
                    None => p["name"].as_str().unwrap_or("tool").to_string(),
                };
                let input = p["arguments"]
                    .as_str()
                    .and_then(|a| serde_json::from_str(a).ok())
                    .unwrap_or(Value::Null);
                push_tool(&mut items, p["call_id"].as_str().unwrap_or("").into(), &name, &input);
            }
            ("response_item", "custom_tool_call") => {
                let name = p["name"].as_str().unwrap_or("tool");
                let raw = p["input"].as_str().unwrap_or("");
                let input = match name {
                    "apply_patch" => serde_json::json!({}),
                    _ => serde_json::json!({ "cmd": raw }),
                };
                push_tool(&mut items, p["call_id"].as_str().unwrap_or("").into(), name, &input);
            }
            ("response_item", "function_call_output" | "custom_tool_call_output") => {
                let output = match &p["output"] {
                    Value::String(s) => s.clone(),
                    other => text_of(other),
                };
                set_tool_result(&mut items, p["call_id"].as_str().unwrap_or(""), output, false);
            }
            ("event_msg", "task_started") => turn_started = at,
            ("event_msg", "task_complete") => items.push(ChatItem::TurnEnd {
                ms: at.zip(turn_started).map(|(end, start)| end.saturating_sub(start) * 1000),
                at,
            }),
            _ => {}
        }
    }
    items
}

/// A session's conversation as CodeAny items. Blocks.
pub fn transcript(session: &ExternalSession) -> Vec<ChatItem> {
    match session.agent.as_str() {
        "codex" => codex_transcript(&session.path),
        _ => claude_transcript(&session.path),
    }
}

/// The file's last write, for syncing an imported transcript.
pub fn modified(path: &Path) -> u64 {
    mtime_secs(path).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_parse_as_utc() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("2026-10-21T08:47:40.047Z"), Some(1_792_572_460));
    }

    #[test]
    fn injected_messages_are_not_prompts() {
        assert!(is_injected("<environment_context>\n<cwd>/x</cwd>"));
        assert!(is_injected("Caveat: The messages below were generated"));
        assert!(!is_injected("支持一下消息通知"));
    }

    #[test]
    fn claude_transcript_reads_turns_tools_and_titles() {
        let dir = std::env::temp_dir().join("codeany-external-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s1.jsonl");
        std::fs::write(
            &path,
            [
                r#"{"type":"user","entrypoint":"cli","cwd":"/Users/me/code/app","message":{"content":"Fix the bug"}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a.rs"}]}}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Fixed."}]}}"#,
                r#"{"type":"system","subtype":"turn_duration","durationMs":5000,"timestamp":"2026-10-21T08:47:40Z"}"#,
                r#"{"type":"ai-title","aiTitle":"Fix the bug in app"}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let header = claude_header(&path).unwrap();
        assert_eq!((header.title.as_str(), header.cwd.as_path()), ("Fix the bug in app", Path::new("/Users/me/code/app")));
        let items = claude_transcript(&path);
        assert!(matches!(&items[0], ChatItem::User(t) if t == "Fix the bug"));
        assert!(matches!(&items[2], ChatItem::Tool { output: Some(o), .. } if o == "a.rs"));
        assert!(matches!(items.last(), Some(ChatItem::TurnEnd { ms: Some(5000), .. })));
    }
}

#[cfg(test)]
mod live_tests {
    /// `cargo test scan_real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn scan_real() {
        let start = std::time::Instant::now();
        let sessions = super::scan();
        let first = start.elapsed();
        let again = std::time::Instant::now();
        super::scan();
        println!("{} sessions; first scan {first:?}, cached {:?}", sessions.len(), again.elapsed());
        for s in sessions.iter().take(12) {
            println!("{:6} {:?} {:<28} {}", s.agent, s.live, s.cwd.display().to_string().replace("/Users/idoubi", "~"), s.title);
        }
        if let Some(s) = sessions.iter().find(|s| s.agent == "codex") {
            println!("codex transcript items: {}", super::transcript(s).len());
        }
    }
}
