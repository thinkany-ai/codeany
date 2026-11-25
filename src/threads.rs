//! Agent conversations, persisted as one JSON file in Application Support.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use agl::protocol::{AgentEvent, ToolImage};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::db::Db;
use crate::i18n::{t, tf};

/// agl's access levels, with the names shown in the UI.
pub const ACCESS_LEVELS: [(&str, &str); 4] = [
    ("full", "Full access"),
    ("edit", "Auto edit"),
    ("ask", "Ask first"),
    ("read_only", "Read only"),
];

/// An access level's name in the current language.
pub fn access_name(access: &str) -> &'static str {
    t(ACCESS_LEVELS
        .iter()
        .find(|(id, _)| *id == access)
        .map(|(_, name)| *name)
        .unwrap_or("Full access"))
}

/// A thread's title for display. The titles CodeAny gives threads itself are
/// stored in English and shown in the current language.
pub fn display_title(title: &str) -> String {
    match title {
        "New thread" => t("New thread").into(),
        "Image" => t("Image").into(),
        other => other.to_string(),
    }
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

/// What the user sent: text, and images pasted with it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UserMessage {
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<PathBuf>,
}

impl From<String> for UserMessage {
    fn from(text: String) -> Self {
        Self { text, images: Vec::new() }
    }
}

impl From<&str> for UserMessage {
    fn from(text: &str) -> Self {
        text.to_string().into()
    }
}

impl UserMessage {
    /// The prompt for the agent: the text, then where to find each image.
    /// agl prompts are text only, so the agent reads the images from disk.
    pub fn prompt(&self) -> String {
        let mut prompt = self.text.clone();
        if !self.images.is_empty() {
            if !prompt.is_empty() {
                prompt.push_str("\n\n");
            }
            prompt.push_str(if self.images.len() == 1 {
                "The user attached an image. Open it with your image-viewing or file-reading tool:"
            } else {
                "The user attached images. Open them with your image-viewing or file-reading tool:"
            });
            for path in &self.images {
                prompt.push_str(&format!("\n- {}", path.display()));
            }
        }
        prompt
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ChatItem {
    User(UserMessage),
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

/// A git worktree a thread works in, on a branch of its own, so threads on
/// one project do not edit the same files.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Worktree {
    /// The repository's main checkout.
    pub repo: PathBuf,
    /// Where the worktree is checked out, under `~/.codeany/worktrees`.
    pub path: PathBuf,
    pub branch: String,
}

impl Worktree {
    /// A worktree for thread `id` of `project`, named after its title.
    pub fn plan(project: &Path, id: u64, title: &str) -> Option<Self> {
        let repo = crate::git::toplevel(project)?;
        let mut slug = String::new();
        for c in title.chars() {
            if c.is_ascii_alphanumeric() {
                slug.push(c.to_ascii_lowercase());
            } else if !slug.is_empty() && !slug.ends_with('-') {
                slug.push('-');
            }
            if slug.len() >= 32 {
                break;
            }
        }
        let slug = slug.trim_end_matches('-');
        // The id keeps branches apart; its last digits are enough for that.
        let suffix = id % 1_000_000;
        let branch = if slug.is_empty() {
            format!("codeany/thread-{suffix}")
        } else {
            format!("codeany/{slug}-{suffix}")
        };
        Some(Self {
            path: crate::paths::worktree_dir(&repo, id),
            repo,
            branch,
        })
    }

    /// The folder in the worktree that matches `project` in the main checkout.
    pub fn dir_for(&self, project: &Path) -> PathBuf {
        match project.strip_prefix(&self.repo) {
            Ok(rel) => self.path.join(rel),
            Err(_) => self.path.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Thread {
    pub id: u64,
    pub title: String,
    /// The folder the agent works in, on `machine`.
    pub project: Option<PathBuf>,
    /// The nex device the agent runs on; `None` is this machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// That device's name when the thread started, for while it is offline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_name: Option<String>,
    pub agent: String,
    pub access: String,
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
    /// The worktree the agent works in instead of `project` itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<Worktree>,
    /// Done with: kept, but listed apart.
    #[serde(default)]
    pub settled: bool,
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
            machine: None,
            machine_name: None,
            agent,
            access,
            model: None,
            effort: None,
            native_ids: HashMap::new(),
            seen: HashMap::new(),
            worktree: None,
            settled: false,
            items: Vec::new(),
            updated_at: now_secs(),
        }
    }

    /// Where the agent works: its worktree, the project, or for a local thread without
    /// one, its own folder under `~/.codeany/threads`. A remote thread
    /// without a folder lets the remote agl pick.
    pub fn cwd(&self) -> Option<PathBuf> {
        if let Some(project) = &self.project {
            return Some(match &self.worktree {
                Some(worktree) => worktree.dir_for(project),
                None => project.clone(),
            });
        }
        if self.machine.is_some() {
            return None;
        }
        let dir = crate::paths::thread_dir(self.id);
        match std::fs::create_dir_all(&dir) {
            Ok(()) => Some(dir),
            Err(err) => {
                eprintln!("codeany: cannot create {}: {err}", dir.display());
                None
            }
        }
    }

    pub fn session_name(&self) -> String {
        format!("codeany:{}:{}", self.id, self.agent)
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
                ChatItem::User(message) => lines.push(format!("User: {}", message.prompt())),
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
            // CodeAny adds the user's message itself, without the handoff context.
            AgentEvent::UserMessage { .. } => {}
            AgentEvent::TextDelta { text, .. } => match self.items.last_mut() {
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
                let local = self.machine.is_none();
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
                    *status = if ok { ToolStatus::Done } else { ToolStatus::Failed };
                    *images = end_images
                        .iter()
                        .enumerate()
                        .filter_map(|(ix, image)| save_image(thread_id, &call_id, ix, image))
                        .collect();
                    // An image too big to send inline (agl's limit is 4 MB of
                    // base64) is still on disk where the tool saved it, and
                    // the output names it (Codex's image generation does).
                    if images.is_empty() && local && end_images.iter().any(|image| image.omitted) {
                        let saved = PathBuf::from(end_output.trim());
                        if saved.is_absolute() && is_image_path(&saved) && saved.is_file() {
                            images.push(saved);
                        }
                    }
                    *output = (!end_output.is_empty()).then_some(end_output);
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
                    "completed" => {
                        // Some agents (OMP over ACP) report a provider's
                        // refusal as the answer and end the turn normally.
                        if let Some(ChatItem::Assistant(text)) = self.items.last()
                            && let Some(message) = provider_error(text)
                        {
                            *self.items.last_mut().unwrap() = ChatItem::Error(message);
                        }
                    }
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
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default = "default_access")]
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
    /// The project open when CodeAny last ran; reopened on a plain launch.
    #[serde(default)]
    pub last_project: Option<PathBuf>,
    /// The sidebar tab last shown: "code" or "chat".
    #[serde(default)]
    pub tab: Option<String>,
    /// The color theme: "system" or a built-in theme id.
    #[serde(default)]
    pub theme: Option<String>,
    #[serde(default)]
    pub typography: crate::style::Typography,
    #[serde(default)]
    pub threads: Vec<Thread>,
    /// The nex device new threads run on; `None` is this machine.
    #[serde(default)]
    pub machine: Option<String>,
    /// The interface language's tag, or `None` to follow the system.
    #[serde(default)]
    pub language: Option<String>,
    /// Each project's row of open tabs.
    #[serde(default)]
    pub tabs: Vec<crate::chat_tabs::TabSet>,
    /// The folder last picked on each other machine, by device id.
    #[serde(default)]
    pub remote_dirs: std::collections::HashMap<String, PathBuf>,
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
            last_project: None,
            tab: None,
            theme: None,
            typography: Default::default(),
            threads: Vec::new(),
            machine: None,
            tabs: Vec::new(),
            language: None,
            remote_dirs: Default::default(),
            db: None,
        }
    }
}

/// The message of a provider error an agent passed off as its answer, like
/// `400 {"type":"error","error":{"message":"Your credit balance is too low…"}}`
/// or `API Error: 529 {"error":{…}}`; `None` for anything else.
pub fn provider_error(text: &str) -> Option<String> {
    let text = text.trim();
    let brace = text.find('{')?;
    let prefix = text[..brace].trim();
    // Only a short lead-in: a status code, maybe "API Error:".
    if prefix.len() > 40 {
        return None;
    }
    let status = prefix
        .split(|c: char| !c.is_ascii_digit())
        .find(|n| n.len() == 3 && matches!(n.as_bytes()[0], b'4' | b'5'));
    if status.is_none() && !prefix.is_empty() && !prefix.to_ascii_lowercase().contains("error") {
        return None;
    }
    let mut values = serde_json::Deserializer::from_str(&text[brace..]).into_iter::<Value>();
    let json = values.next()?.ok()?;
    let error = json.get("error")?;
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .or_else(|| json.get("message").and_then(Value::as_str))?;
    Some(match status {
        Some(code) => format!("{message} ({code})"),
        None => message.to_string(),
    })
}

/// Remove what CodeAny keeps for a thread on disk: the images its tools
/// saved and the user pasted, and its working folder if it had no project.
pub fn delete_thread_files(thread: &Thread) {
    if thread.project.is_none() && thread.machine.is_none() {
        let _ = std::fs::remove_dir_all(crate::paths::thread_dir(thread.id));
    }
    for item in &thread.items {
        let images = match item {
            ChatItem::Tool { images, .. } => images,
            ChatItem::User(message) => &message.images,
            _ => continue,
        };
        for path in images {
            // Only files CodeAny saved; never an image the user pointed at.
            if path.starts_with(crate::paths::images()) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Save an image pasted into the composer under `~/.codeany/images`.
/// Formats agents may not read (TIFF, BMP, …) are converted to PNG.
pub fn save_pasted_image(image: &gpui_kit::Image) -> Option<PathBuf> {
    use gpui_kit::ImageFormat;
    let (bytes, ext) = match image.format {
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::Webp => {
            (image.bytes.clone(), image.format.extension())
        }
        _ => {
            let decoded = image::load_from_memory(&image.bytes).ok()?;
            let mut png = std::io::Cursor::new(Vec::new());
            decoded.write_to(&mut png, image::ImageFormat::Png).ok()?;
            (png.into_inner(), "png")
        }
    };
    let dir = images_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("paste-{}.{ext}", uuid::Uuid::new_v4()));
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// Whether a file looks like an image an agent can open.
pub fn is_image_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp"
            )
        })
}

impl Store {
    /// Open `~/.codeany/codeany.db`.
    pub fn load() -> Self {
        match Db::open(&crate::paths::database()) {
            Ok(db) => Store {
                agent: db.setting("agent").flatten(),
                access: db.setting("access").unwrap_or_else(default_access),
                model: db.setting("model").flatten(),
                effort: db.setting("effort").flatten(),
                favorites: db.setting("favorites").unwrap_or_default(),
                projects: db.setting("projects").unwrap_or_default(),
                last_project: db.setting("last_project").flatten(),
                tab: db.setting("sidebar_tab").flatten(),
                theme: db.setting("theme").flatten(),
                typography: db.setting("typography").unwrap_or_default(),
                threads: db.load_threads(),
                machine: db.setting("machine").flatten(),
                tabs: db.setting("tabs").unwrap_or_default(),
                language: db.setting("language").flatten(),
                remote_dirs: db.setting("remote_dirs").unwrap_or_default(),
                db: Some(db),
            },
            Err(err) => {
                eprintln!("codeany: cannot open the database: {err}");
                Store::default()
            }
        }
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
        db.set_setting("last_project", &self.last_project);
        db.set_setting("sidebar_tab", &self.tab);
        db.set_setting("theme", &self.theme);
        db.set_setting("typography", &self.typography);
        db.set_setting("machine", &self.machine);
        db.set_setting("tabs", &self.tabs);
        db.set_setting("language", &self.language);
        db.set_setting("remote_dirs", &self.remote_dirs);
        if let Err(err) = db.save_threads(&self.threads) {
            eprintln!("codeany: saving threads failed: {err}");
        }
        // Serve the threads that run here; write others back to their device.
        crate::sync::saved(&self.threads);
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
    let time = format!("{hour}:{:02}", tm.tm_min);
    let clock = tf(if tm.tm_hour < 12 { "{time} AM" } else { "{time} PM" }, &[("time", time)]);
    if (tm.tm_year, tm.tm_yday) == (today.tm_year, today.tm_yday) {
        return clock;
    }
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    tf(
        "{month} {day}, {clock}",
        &[
            ("month", t(MONTHS[tm.tm_mon.clamp(0, 11) as usize]).to_string()),
            ("day", tm.tm_mday.to_string()),
            ("clock", clock),
        ],
    )
}

pub fn relative_time(secs: u64) -> String {
    let elapsed = now_secs().saturating_sub(secs);
    match elapsed {
        0..60 => t("now").into(),
        60..3600 => tf("{n}m", &[("n", (elapsed / 60).to_string())]),
        3600..86400 => tf("{n}h", &[("n", (elapsed / 3600).to_string())]),
        _ => tf("{n}d", &[("n", (elapsed / 86400).to_string())]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        thread.apply(AgentEvent::TextDelta { text: reply.into(), phase: None });
        thread.apply(AgentEvent::TurnEnded {
            turn_id: "t".into(),
            outcome: "completed".into(),
            error: None,
            usage: None,
            timing: None,
            text: None,
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
    }

}


#[cfg(test)]
mod worktree_plan_tests {
    use super::*;

    #[test]
    fn names_the_branch_after_the_title() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let plan = Worktree::plan(&src, 1_700_000_123_456, "Fix the Login bug!").unwrap();
        assert_eq!(plan.branch, "codeany/fix-the-login-bug-123456");
        assert!(plan.path.ends_with("worktrees/codeany-desktop/1700000123456"));
        // A project below the repository's top maps to the same folder in the worktree.
        assert_eq!(plan.dir_for(&src), plan.path.join("src"));
        let plan = Worktree::plan(&src, 42, "修一下登录").unwrap();
        assert_eq!(plan.branch, "codeany/thread-42");
    }
}

#[cfg(test)]
mod provider_error_tests {
    use super::provider_error;

    #[test]
    fn spots_provider_errors_passed_off_as_answers() {
        let omp = r#"400 {"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."},"request_id":"req_1"} raw-http-request=/Users/x/.omp/logs/a.json"#;
        assert_eq!(
            provider_error(omp).as_deref(),
            Some("Your credit balance is too low to access the Anthropic API. (400)")
        );
        let claude = r#"API Error: 529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(provider_error(claude).as_deref(), Some("Overloaded (529)"));
        // Answers that merely contain JSON are left alone.
        assert_eq!(provider_error(r#"Here is the config: {"error": {"message": "x"}}"#), None);
        assert_eq!(provider_error(r#"{"name": "codeany"}"#), None);
        assert_eq!(provider_error("All done."), None);
    }
}
