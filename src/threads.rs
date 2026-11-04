//! Agent conversations, persisted as one JSON file in Application Support.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use agl::protocol::{AgentEvent, ToolImage};
use base64::Engine as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::db::Db;

/// agl's access levels, with the names shown in the UI.
pub const ACCESS_LEVELS: [(&str, &str); 4] = [
    ("full", "Full access"),
    ("edit", "Auto edit"),
    ("ask", "Ask first"),
    ("read_only", "Read only"),
];

pub fn access_name(access: &str) -> &'static str {
    ACCESS_LEVELS
        .iter()
        .find(|(id, _)| *id == access)
        .map(|(_, name)| *name)
        .unwrap_or("Full access")
}

/// A display name for an agl agent id, for when detection has not run.
pub fn agent_name(id: &str) -> String {
    match id {
        "claude" => "Claude Code".into(),
        "codex" => "Codex".into(),
        "gemini" => "Gemini CLI".into(),
        "opencode" => "OpenCode".into(),
        "cursor" => "Cursor Agent".into(),
        "kimi" => "Kimi Code".into(),
        "grok" => "Grok Build".into(),
        other => other.into(),
    }
}

/// Earlier versions stored agents as `Claude`, `Codex`, … and access as
/// `Full`, `AutoEdit`, `ReadOnly`.
fn agent_id<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(String::deserialize(d)?.to_lowercase())
}

fn opt_agent_id<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(Option::<String>::deserialize(d)?.map(|s| s.to_lowercase()))
}

fn access_id<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let raw = String::deserialize(d)?;
    Ok(match raw.as_str() {
        "Full" => "full".into(),
        "AutoEdit" => "edit".into(),
        "ReadOnly" => "read_only".into(),
        _ => raw,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolStatus {
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    pub status: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ChatItem {
    User(String),
    Assistant(String),
    Thinking(String),
    Tool {
        id: String,
        title: String,
        detail: Option<String>,
        path: Option<PathBuf>,
        output: Option<String>,
        status: ToolStatus,
        /// Code the tool ran (Codex computer/browser use).
        #[serde(default)]
        code: Option<String>,
        /// Images the tool returned, saved under Application Support.
        #[serde(default)]
        images: Vec<PathBuf>,
    },
    Approval {
        request_id: String,
        title: String,
        detail: Option<String>,
        options: Vec<String>,
        /// allow | allow_session | deny, once answered.
        decision: Option<String>,
    },
    Plan(Vec<PlanEntry>),
    /// The end of a turn: how long the agent worked, and when (unix secs).
    TurnEnd {
        ms: Option<u64>,
        #[serde(default)]
        at: Option<u64>,
    },
    /// The thread moved to another agent or model.
    Handoff {
        from_agent: String,
        from_label: String,
        to_agent: String,
        to_label: String,
    },
    Error(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Thread {
    pub id: u64,
    pub title: String,
    pub project: Option<PathBuf>,
    #[serde(deserialize_with = "agent_id")]
    pub agent: String,
    #[serde(deserialize_with = "access_id")]
    pub access: String,
    /// Earlier versions' native id of `agent`'s conversation; see `native_ids`.
    #[serde(default, skip_serializing)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// Each agent's own conversation id, to resume it in a fresh agl session.
    #[serde(default)]
    pub native_ids: HashMap<String, String>,
    /// How many items each agent has seen; a handoff sends it the rest.
    #[serde(default)]
    pub seen: HashMap<String, usize>,
    /// Done with: kept, but listed apart.
    #[serde(default)]
    pub settled: bool,
    /// Imported from a terminal session's transcript.
    #[serde(default)]
    pub external: Option<PathBuf>,
    /// The transcript's mtime last imported, while the thread still follows
    /// it; cleared once the conversation continues in dscode.
    #[serde(default)]
    pub synced: Option<u64>,
    pub items: Vec<ChatItem>,
    pub updated_at: u64,
}

/// Transcripts handed to another agent keep at most this many characters.
const MAX_HANDOFF_CHARS: usize = 60_000;

impl Thread {
    pub fn new(project: Option<PathBuf>, agent: String, access: String) -> Self {
        Self {
            id: now_millis(),
            title: "New thread".into(),
            project,
            agent,
            access,
            session_id: None,
            model: None,
            effort: None,
            native_ids: HashMap::new(),
            seen: HashMap::new(),
            settled: false,
            external: None,
            synced: None,
            items: Vec::new(),
            updated_at: now_secs(),
        }
    }

    /// Threads from before agent switching had one agent that saw everything.
    fn migrate(&mut self) {
        if let Some(id) = self.session_id.take() {
            self.native_ids.entry(self.agent.clone()).or_insert(id);
        }
        if self.seen.is_empty() && !self.items.is_empty() {
            self.seen.insert(self.agent.clone(), self.items.len());
        }
    }

    /// The agl session name for this thread's current agent.
    pub fn session_name(&self) -> String {
        format!("dscode:{}:{}", self.id, self.agent)
    }

    /// Agents that took part, in order of first appearance.
    pub fn agents_used(&self) -> Vec<String> {
        let mut agents = Vec::new();
        for item in &self.items {
            if let ChatItem::Handoff { from_agent, .. } = item {
                if !agents.contains(from_agent) {
                    agents.push(from_agent.clone());
                }
            }
        }
        if !agents.contains(&self.agent) {
            agents.push(self.agent.clone());
        }
        agents
    }

    /// Switch to another agent or model, marking the point in the transcript.
    pub fn switch(&mut self, agent: String, model: Option<String>, from_label: String, to_label: String) {
        if agent == self.agent && model == self.model {
            return;
        }
        if !self.items.is_empty() {
            self.items.push(ChatItem::Handoff {
                from_agent: self.agent.clone(),
                from_label,
                to_agent: agent.clone(),
                to_label,
            });
        }
        if agent != self.agent {
            // Efforts are per model family; let the new agent use its default.
            self.effort = None;
        }
        self.agent = agent;
        self.model = model;
    }

    /// The part of the conversation the current agent has not seen, as a
    /// transcript to prepend to its next prompt.
    pub fn handoff_context(&self) -> Option<String> {
        let start = self.seen.get(&self.agent).copied().unwrap_or(0);
        let mut lines = Vec::new();
        for item in self.items.iter().skip(start) {
            match item {
                ChatItem::User(text) => lines.push(format!("User: {text}")),
                ChatItem::Assistant(text) => lines.push(format!("Assistant: {text}")),
                ChatItem::Tool {
                    title,
                    detail,
                    status,
                    ..
                } => lines.push(format!(
                    "[tool {title}{}{}]",
                    detail.as_ref().map(|d| format!(": {}", d.lines().next().unwrap_or(""))).unwrap_or_default(),
                    if *status == ToolStatus::Failed { " (failed)" } else { "" }
                )),
                ChatItem::Handoff { to_label, .. } => lines.push(format!("[switched to {to_label}]")),
                _ => {}
            }
        }
        if !lines.iter().any(|l| l.starts_with("User:") || l.starts_with("Assistant:")) {
            return None;
        }
        let mut transcript = lines.join("\n\n");
        if transcript.len() > MAX_HANDOFF_CHARS {
            let mut cut = transcript.len() - MAX_HANDOFF_CHARS;
            while !transcript.is_char_boundary(cut) {
                cut += 1;
            }
            transcript = format!("…\n{}", &transcript[cut..]);
        }
        Some(format!(
            "Context handoff: this conversation was carried on with another coding agent. \
             Below is what happened since you last took part, for context; continue from it.\n\
             <transcript>\n{transcript}\n</transcript>\n\n"
        ))
    }

    /// Fold one agl event into the transcript.
    pub fn apply(&mut self, event: AgentEvent) {
        self.updated_at = now_secs();
        match event {
            // dscode adds the user's message itself, without the handoff context.
            AgentEvent::UserMessage { .. } => {}
            AgentEvent::TextDelta { text } => match self.items.last_mut() {
                Some(ChatItem::Assistant(existing)) => existing.push_str(&text),
                _ => self.items.push(ChatItem::Assistant(text)),
            },
            AgentEvent::Reasoning { text } => match self.items.last_mut() {
                Some(ChatItem::Thinking(existing)) => existing.push_str(&text),
                _ => self.items.push(ChatItem::Thinking(text)),
            },
            AgentEvent::ToolStart {
                call_id,
                name,
                input,
            } => {
                let (title, detail, path) = describe_tool(&name, &input);
                let code = is_cua(&name, &input)
                    .then(|| input["code"].as_str().map(String::from))
                    .flatten();
                self.items.push(ChatItem::Tool {
                    id: call_id,
                    title,
                    detail,
                    path,
                    output: None,
                    status: ToolStatus::Running,
                    code,
                    images: Vec::new(),
                });
            }
            AgentEvent::ToolEnd {
                call_id,
                ok,
                output: end_output,
                images: end_images,
            } => {
                let thread_id = self.id;
                let tool = self.items.iter_mut().rev().find_map(|item| match item {
                    ChatItem::Tool {
                        id,
                        output,
                        status,
                        images,
                        ..
                    } if *id == call_id => Some((output, status, images)),
                    _ => None,
                });
                if let Some((output, status, images)) = tool {
                    *output = (!end_output.is_empty()).then_some(end_output);
                    *status = if ok { ToolStatus::Done } else { ToolStatus::Failed };
                    *images = end_images
                        .iter()
                        .enumerate()
                        .filter_map(|(ix, image)| save_image(thread_id, &call_id, ix, image))
                        .collect();
                }
            }
            AgentEvent::PermissionRequest {
                request_id,
                tool,
                title,
                input,
                options,
                ..
            } => {
                let (verb, detail, _) = describe_tool(&tool, &input);
                let title = if title.is_empty() { verb } else { title };
                self.items.push(ChatItem::Approval {
                    request_id,
                    title,
                    detail,
                    options,
                    decision: None,
                });
            }
            AgentEvent::PermissionResolved {
                request_id: resolved,
                decision: answer,
                ..
            } => {
                for item in self.items.iter_mut().rev() {
                    if let ChatItem::Approval {
                        request_id,
                        decision,
                        ..
                    } = item
                    {
                        if *request_id == resolved {
                            *decision = Some(answer);
                            break;
                        }
                    }
                }
            }
            AgentEvent::Plan { entries } => {
                let entries: Vec<PlanEntry> = entries
                    .as_array()
                    .map(|entries| {
                        entries
                            .iter()
                            .filter_map(|e| {
                                Some(PlanEntry {
                                    content: e["content"].as_str()?.to_string(),
                                    status: e["status"].as_str().unwrap_or("pending").to_string(),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // One plan per turn, updated in place.
                let turn_start = self
                    .items
                    .iter()
                    .rposition(|item| matches!(item, ChatItem::User(_)))
                    .unwrap_or(0);
                match self.items[turn_start..]
                    .iter_mut()
                    .find(|item| matches!(item, ChatItem::Plan(_)))
                {
                    Some(ChatItem::Plan(existing)) => *existing = entries,
                    _ => self.items.push(ChatItem::Plan(entries)),
                }
            }
            AgentEvent::Info { native_id, .. } => {
                if let Some(id) = native_id {
                    self.native_ids.insert(self.agent.clone(), id);
                }
            }
            AgentEvent::TurnEnded {
                outcome,
                error,
                timing,
                ..
            } => {
                self.finish_running_tools();
                match outcome.as_str() {
                    "completed" => {}
                    "aborted" => self.items.push(ChatItem::Error("Interrupted".into())),
                    _ => self.items.push(ChatItem::Error(
                        error.unwrap_or_else(|| "The turn failed".into()),
                    )),
                }
                let ms = timing.as_ref().and_then(|t| t["total_ms"].as_u64());
                self.items.push(ChatItem::TurnEnd {
                    ms,
                    at: Some(now_secs()),
                });
                self.seen.insert(self.agent.clone(), self.items.len());
            }
            AgentEvent::Error { message } => self.items.push(ChatItem::Error(message)),
            _ => {}
        }
    }

    /// Tools still running when a turn ends will never report back.
    pub fn finish_running_tools(&mut self) {
        for item in &mut self.items {
            if let ChatItem::Tool { status, .. } = item {
                if *status == ToolStatus::Running {
                    *status = ToolStatus::Done;
                }
            }
        }
    }
}

/// A Codex computer-use or browser-use call: JavaScript run by `cua_repl`.
fn is_cua(name: &str, input: &Value) -> bool {
    name.contains("cua_repl") || (name.ends_with(".js") && input["code"].is_string())
}

fn images_dir() -> Option<PathBuf> {
    Some(crate::paths::images())
}

/// Write a tool image to disk, so the transcript only keeps its path.
fn save_image(thread_id: u64, call_id: &str, ix: usize, image: &ToolImage) -> Option<PathBuf> {
    if image.omitted {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(image.data.as_bytes())
        .ok()?;
    let ext = match image.mime_type.as_str() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "png",
    };
    let call: String = call_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(48)
        .collect();
    let dir = images_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{thread_id}-{call}-{ix}.{ext}"));
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// A short title, detail and file path for a tool call from its name and input.
pub(crate) fn describe_tool(name: &str, input: &Value) -> (String, Option<String>, Option<PathBuf>) {
    if is_cua(name, input) {
        let code = input["code"].as_str().unwrap_or_default();
        let kind = if code.contains("Browser") || code.contains("browser") || code.contains("Tab") {
            "Browser"
        } else {
            "Computer"
        };
        let detail = str_field(input, "title")
            .map(String::from)
            .or_else(|| code.lines().next().map(String::from));
        return (kind.to_string(), detail, None);
    }
    let path = ["file_path", "path", "filePath", "absolute_path", "notebook_path"]
        .iter()
        .find_map(|key| str_field(input, key))
        .map(PathBuf::from);
    let detail = ["command", "cmd", "pattern", "query", "url", "description", "prompt"]
        .iter()
        .find_map(|key| match input.get(*key) {
            Some(Value::String(s)) => Some(s.clone()),
            // Codex sends commands as argv.
            Some(Value::Array(parts)) => Some(
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
        .map(|d| {
            d.strip_prefix("/bin/zsh -lc ")
                .or_else(|| d.strip_prefix("bash -lc "))
                .unwrap_or(&d)
                .trim_matches(['\'', '"'])
                .to_string()
        })
        .or_else(|| path.as_ref().map(|p| p.display().to_string()));

    let lower = name.to_lowercase();
    let title = if lower.contains("bash")
        || lower.contains("shell")
        || lower.contains("command")
        || lower.contains("exec")
    {
        "Run"
    } else if lower.contains("edit")
        || lower.contains("write")
        || lower.contains("replace")
        || lower.contains("patch")
        || lower.contains("file_change")
    {
        "Edit"
    } else if lower.contains("image_gen") || lower.contains("imagegen") {
        "Generate image"
    } else if lower.contains("imageview") || lower.contains("view_image") {
        "View image"
    } else if lower.contains("read") || lower == "view" {
        "Read"
    } else if lower.contains("grep") || lower.contains("glob") || lower.contains("search") {
        "Search"
    } else if lower.contains("fetch") || lower.contains("web") {
        "Web"
    } else if lower.contains("todo") {
        "Update todos"
    } else {
        name
    };
    (title.to_string(), detail, path)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Store {
    #[serde(default, deserialize_with = "opt_agent_id")]
    pub agent: Option<String>,
    #[serde(default = "default_access", deserialize_with = "access_id")]
    pub access: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// Starred models, as (agent, model id).
    #[serde(default)]
    pub favorites: Vec<(String, String)>,
    /// Projects opened, most recent first.
    #[serde(default)]
    pub projects: Vec<PathBuf>,
    /// The sidebar tab last shown: "code" or "chat".
    #[serde(default)]
    pub tab: Option<String>,
    /// The color theme: "system" or a built-in theme id.
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub threads: Vec<Thread>,
    #[serde(skip)]
    db: Option<Db>,
}

fn default_access() -> String {
    "full".into()
}

impl Default for Store {
    fn default() -> Self {
        Self {
            agent: None,
            access: default_access(),
            model: None,
            effort: None,
            favorites: Vec::new(),
            projects: Vec::new(),
            tab: None,
            theme: None,
            threads: Vec::new(),
            db: None,
        }
    }
}

/// Remove the images a thread's tools saved.
pub fn delete_thread_images(thread: &Thread) {
    for item in &thread.items {
        if let ChatItem::Tool { images, .. } = item {
            for path in images {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

impl Store {
    /// Open `~/.dscode/dscode.db`, moving data from earlier versions on first use.
    pub fn load() -> Self {
        let mut store = match Db::open(&crate::paths::database()) {
            Ok(db) => {
                if db.is_empty() {
                    migrate_legacy(&db);
                }
                Store {
                    agent: db.setting("agent").flatten(),
                    access: db.setting("access").unwrap_or_else(default_access),
                    model: db.setting("model").flatten(),
                    effort: db.setting("effort").flatten(),
                    favorites: db.setting("favorites").unwrap_or_default(),
                    projects: db.setting("projects").unwrap_or_default(),
                    tab: db.setting("sidebar_tab").flatten(),
                    theme: db.setting("theme").flatten(),
                    threads: db.load_threads(),
                    db: Some(db),
                }
            }
            Err(err) => {
                eprintln!("dscode: cannot open the database: {err}");
                Store::default()
            }
        };
        store.threads.iter_mut().for_each(Thread::migrate);
        // Projects that only appear in older threads.
        for thread in &store.threads {
            if let Some(project) = &thread.project {
                if !store.projects.contains(project) {
                    store.projects.push(project.clone());
                }
            }
        }
        store
    }

    /// Write what changed.
    pub fn save(&self) {
        let Some(db) = &self.db else { return };
        db.set_setting("agent", &self.agent);
        db.set_setting("access", &self.access);
        db.set_setting("model", &self.model);
        db.set_setting("effort", &self.effort);
        db.set_setting("favorites", &self.favorites);
        db.set_setting("projects", &self.projects);
        db.set_setting("sidebar_tab", &self.tab);
        db.set_setting("theme", &self.theme);
        if let Err(err) = db.save_threads(&self.threads) {
            eprintln!("dscode: saving threads failed: {err}");
        }
    }
}

/// Bring `threads.json` and its images from the pre-`~/.dscode` location
/// into the database. The old file is kept as `threads.json.migrated`.
fn migrate_legacy(db: &Db) {
    let Some(legacy) = crate::paths::legacy() else { return };
    let json_path = legacy.join("threads.json");
    let Ok(mut text) = std::fs::read_to_string(&json_path) else { return };

    // Screenshots move along; their paths inside the transcripts follow.
    let (old_images, new_images) = (legacy.join("images"), crate::paths::images());
    if old_images.is_dir() && !new_images.exists() {
        if let Some(parent) = new_images.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::rename(&old_images, &new_images).is_ok() {
            let (from, to) = (old_images.to_string_lossy(), new_images.to_string_lossy());
            // Paths sit inside JSON strings, where `/` may be escaped.
            text = text.replace(from.as_ref(), to.as_ref());
            text = text.replace(&from.replace('/', "\\/"), &to.replace('/', "\\/"));
        }
    }

    let Ok(old) = serde_json::from_str::<Store>(&text) else {
        eprintln!("dscode: could not read {}", json_path.display());
        return;
    };
    db.set_setting("agent", &old.agent);
    db.set_setting("access", &old.access);
    db.set_setting("model", &old.model);
    db.set_setting("effort", &old.effort);
    db.set_setting("favorites", &old.favorites);
    db.set_setting("projects", &old.projects);
    if db.save_threads(&old.threads).is_ok() {
        let _ = std::fs::rename(&json_path, legacy.join("threads.json.migrated"));
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// "4:38 PM" today, "Oct 12, 4:38 PM" before that, in local time.
pub fn clock_time(secs: u64) -> String {
    fn local(secs: u64) -> Option<libc::tm> {
        let t = secs as libc::time_t;
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: localtime_r only writes the provided struct.
        let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
        ok.then_some(tm)
    }
    let (Some(tm), Some(today)) = (local(secs), local(now_secs())) else {
        return String::new();
    };
    let hour = match tm.tm_hour % 12 {
        0 => 12,
        h => h,
    };
    let clock = format!("{hour}:{:02} {}", tm.tm_min, if tm.tm_hour < 12 { "AM" } else { "PM" });
    if (tm.tm_year, tm.tm_yday) == (today.tm_year, today.tm_yday) {
        return clock;
    }
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    format!("{} {}, {clock}", MONTHS[tm.tm_mon.clamp(0, 11) as usize], tm.tm_mday)
}

pub fn relative_time(secs: u64) -> String {
    let elapsed = now_secs().saturating_sub(secs);
    match elapsed {
        0..60 => "now".into(),
        60..3600 => format!("{}m", elapsed / 60),
        3600..86400 => format!("{}h", elapsed / 3600),
        _ => format!("{}d", elapsed / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_threads_saved_by_earlier_versions() {
        let store: Store = serde_json::from_str(
            r#"{"agent":"Codex","access":"ReadOnly","threads":[{"id":1,"title":"t","project":null,
            "agent":"OpenCode","access":"AutoEdit","session_id":"s","items":[{"User":"hi"}],"updated_at":0}]}"#,
        )
        .unwrap();
        assert_eq!(store.agent.as_deref(), Some("codex"));
        assert_eq!(store.access, "read_only");
        assert_eq!(store.threads[0].agent, "opencode");
        assert_eq!(store.threads[0].access, "edit");
    }

    #[test]
    fn plan_updates_in_place_within_a_turn() {
        let mut thread = Thread::new(None, "claude".into(), "full".into());
        thread.items.push(ChatItem::User("go".into()));
        for status in ["pending", "completed"] {
            thread.apply(AgentEvent::Plan {
                entries: serde_json::json!([{"content": "step", "status": status}]),
            });
        }
        let plans: Vec<_> = thread.items.iter().filter(|i| matches!(i, ChatItem::Plan(_))).collect();
        assert_eq!(plans.len(), 1);
        assert!(matches!(plans[0], ChatItem::Plan(e) if e[0].status == "completed"));
    }
}

#[cfg(test)]
mod cua_tests {
    use super::*;

    #[test]
    fn cua_calls_use_their_title_and_keep_their_code() {
        let mut thread = Thread::new(None, "codex".into(), "full".into());
        thread.apply(AgentEvent::ToolStart {
            call_id: "c1".into(),
            name: "cua_repl.js".into(),
            input: serde_json::json!({"code": "await cua.createBrowserTab(\"1\", url)", "title": "Open example.com"}),
        });
        let ChatItem::Tool { title, detail, code, .. } = &thread.items[0] else { panic!() };
        assert_eq!(title, "Browser");
        assert_eq!(detail.as_deref(), Some("Open example.com"));
        assert!(code.as_deref().unwrap().contains("createBrowserTab"));
    }
}

#[cfg(test)]
mod handoff_tests {
    use super::*;

    fn turn(thread: &mut Thread, user: &str, reply: &str) {
        thread.items.push(ChatItem::User(user.into()));
        thread.apply(AgentEvent::TextDelta { text: reply.into() });
        thread.apply(AgentEvent::TurnEnded {
            turn_id: "t".into(),
            outcome: "completed".into(),
            error: None,
            usage: None,
            timing: None,
        });
    }

    #[test]
    fn each_agent_gets_only_what_it_missed() {
        let mut thread = Thread::new(None, "codex".into(), "full".into());
        assert!(thread.handoff_context().is_none());
        turn(&mut thread, "hi", "hello from codex");

        thread.switch("claude".into(), None, "GPT".into(), "Claude".into());
        let context = thread.handoff_context().unwrap();
        assert!(context.contains("User: hi") && context.contains("hello from codex"));
        turn(&mut thread, "and you?", "hello from claude");
        assert!(thread.handoff_context().is_none());

        thread.switch("codex".into(), None, "Claude".into(), "GPT".into());
        let context = thread.handoff_context().unwrap();
        assert!(!context.contains("User: hi"), "codex already saw the first turn");
        assert!(context.contains("hello from claude"));
        assert_eq!(thread.agents_used(), vec!["codex".to_string(), "claude".to_string()]);
    }

    #[test]
    fn legacy_threads_need_no_handoff() {
        let mut thread: Thread = serde_json::from_str(
            r#"{"id":1,"title":"t","project":null,"agent":"Codex","access":"Full","session_id":"n1",
            "items":[{"User":"hi"},{"Assistant":"yo"}],"updated_at":0}"#,
        )
        .unwrap();
        thread.migrate();
        assert_eq!(thread.native_ids.get("codex").map(String::as_str), Some("n1"));
        assert!(thread.handoff_context().is_none());
    }
}

