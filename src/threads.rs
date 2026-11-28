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
        "antigravity" => "Antigravity".into(),
        "kiro" => "Kiro CLI".into(),
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

/// What the user sent: text, and the files attached to it (`images`: pasted
/// images at first, any file since; the name stays for saved threads).
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
            let all_images = self.images.iter().all(|p| is_image_path(p));
            prompt.push_str(match (all_images, self.images.len() == 1) {
                (true, true) => "The user attached an image. Open it with your image-viewing or file-reading tool:",
                (true, false) => "The user attached images. Open them with your image-viewing or file-reading tool:",
                (false, true) => "The user attached a file. Open it with your file-reading tool:",
                (false, false) => "The user attached files. Open them with your file-reading tool:",
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
    /// The project folder resolved inside the checkout by the owning device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_dir: Option<PathBuf>,
}

// Preparing and deleting a worktree both touch the repository's worktree list.
static WORKTREE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl Worktree {
    /// A worktree for thread `id` of `project`, named after its title.
    #[cfg(test)]
    pub fn plan(project: &Path, id: u64, title: &str) -> Option<Self> {
        let git = crate::git::ProjectGit::read(project)?;
        Some(Self::from_project(&git, id, title))
    }

    pub fn from_project(git: &crate::git::ProjectGit, id: u64, title: &str) -> Self {
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
        // Join with the separator of the device that owns the checkout.
        let mut path = git.worktrees.clone().into_os_string();
        path.push(git.path_separator.to_string());
        path.push(id.to_string());
        Self { repo: git.repo.clone(), path: path.into(), branch, checkout_dir: None }
    }

    pub fn validate_on_host(&self, project: &Path, id: u64) -> Result<(), String> {
        let repo = crate::git::toplevel(None, project).ok_or("The project is not a Git repository")?;
        let managed_branch = self.branch.strip_prefix("codeany/").is_some_and(|name| {
            name.ends_with(&format!("-{}", id % 1_000_000))
                && name.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        });
        if self.repo != repo || self.path != crate::paths::worktree_dir(&repo, id) || !managed_branch {
            return Err("Invalid worktree for this project".into());
        }
        Ok(())
    }

    pub fn prepare(&self, base: Option<&str>) -> Result<(), String> {
        let _checkout = WORKTREE_LOCK.lock().unwrap();
        if !self.path.exists() {
            crate::git::add_worktree(&self.repo, &self.path, &self.branch, base)
                .map_err(|err| format!("Cannot create the worktree: {err}"))?;
        }
        Ok(())
    }

    pub fn cleanup(&self) {
        let _checkout = WORKTREE_LOCK.lock().unwrap();
        if let Err(err) = crate::git::remove_worktree(&self.repo, &self.path, &self.branch) {
            eprintln!("codeany: kept worktree {} ({}): {err}", self.path.display(), self.branch);
        }
    }

    /// The folder in the worktree that matches `project` in the main checkout.
    pub fn dir_for(&self, project: &Path) -> PathBuf {
        if let Ok(relative) = project.strip_prefix(&self.repo) {
            return self.path.join(relative);
        }
        // Git returns a physical checkout path even when the project was
        // opened through a symlink.
        if let (Ok(project), Ok(repo)) = (project.canonicalize(), self.repo.canonicalize())
            && let Ok(relative) = project.strip_prefix(&repo)
        {
            return self.path.join(relative);
        }
        self.path.clone()
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
    /// The workspace it belongs to; `None`: its device's first workspace
    /// (threads from before workspaces).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
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
    /// The starting branch for a new worktree or a pending project checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_branch: Option<String>,
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
            workspace: None,
            agent,
            access,
            model: None,
            effort: None,
            native_ids: HashMap::new(),
            seen: HashMap::new(),
            worktree: None,
            start_branch: None,
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
                Some(worktree) if self.machine.is_none() => worktree.dir_for(project),
                Some(_) => return None, // The owning device resolves this path.
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
    /// The agents offered when starting or switching a thread; the others
    /// stay installed but out of the way. Codex and Claude Code at first.
    #[serde(default = "default_enabled_agents")]
    pub enabled_agents: Vec<String>,
    /// Arguments appended to each agent's launch arguments on this machine,
    /// by agent id (Settings → Agents).
    #[serde(default)]
    pub agent_args: HashMap<String, Vec<String>>,
    /// The interface language's tag, or `None` to follow the system.
    #[serde(default)]
    pub language: Option<String>,
    /// Each project's row of open tabs.
    #[serde(default)]
    pub tabs: Vec<crate::chat_tabs::TabSet>,
    /// The folder last picked on each other machine, by device id.
    #[serde(default)]
    pub remote_dirs: std::collections::HashMap<String, PathBuf>,
    /// Workspaces, each on one device, as Termany has them.
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    /// The workspace a new window shows: the one used last.
    #[serde(default)]
    pub active_workspace: Option<String>,
    /// The workspaces open in windows when CodeAny last quit, to open again.
    #[serde(default)]
    pub windows: Vec<String>,
    /// Each workspace's theme, by workspace id, kept on this device; one with
    /// none shows `theme`.
    #[serde(default)]
    pub workspace_themes: HashMap<String, String>,
    #[serde(skip)]
    db: Option<Db>,
}

/// A workspace: projects and conversations on one device, under a name and
/// an icon. One on another device is that device's own record, mounted
/// here: its name, icon and projects come from there.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub title: String,
    /// An emoji, or `None` for the title's initial.
    #[serde(default)]
    pub icon: Option<String>,
    /// The device it lives on; `None` is this one.
    #[serde(default)]
    pub device: Option<String>,
    /// That device's name, for while it is offline.
    #[serde(default)]
    pub device_name: Option<String>,
    /// Its projects, beyond the folders its threads work in.
    #[serde(default)]
    pub projects: Vec<PathBuf>,
    /// The names given to its projects, by folder; one without a name
    /// shows its folder's.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub project_names: HashMap<PathBuf, String>,
    /// Projects removed from the list. Their folders and conversations are
    /// kept, so a folder is not listed again just for the threads that
    /// worked in it, until it is added back.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_projects: Vec<PathBuf>,
}

/// Every named project's name, by folder, for whatever shows a project:
/// kept current as the store loads and saves.
static PROJECT_NAMES: std::sync::RwLock<Option<HashMap<PathBuf, String>>> = std::sync::RwLock::new(None);

/// What a project is called: the name it was given, else its folder's.
pub fn project_label(project: &std::path::Path) -> String {
    if let Some(name) = PROJECT_NAMES.read().unwrap().as_ref().and_then(|names| names.get(project)) {
        return name.clone();
    }
    folder_label(project)
}

/// A folder's own name.
pub fn folder_label(folder: &std::path::Path) -> String {
    folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| folder.display().to_string())
}

impl Workspace {
    /// List `project`, first; one removed before is back.
    pub fn add_project(&mut self, project: &std::path::Path) {
        self.removed_projects.retain(|p| p != project);
        if !self.projects.iter().any(|p| p == project) {
            self.projects.insert(0, project.to_path_buf());
        }
    }

    /// Take `project` off the list, with the name it was given. Its
    /// conversations stay, out of sight until it is added again.
    pub fn remove_project(&mut self, project: &std::path::Path) {
        self.projects.retain(|p| p != project);
        self.project_names.remove(project);
        if !self.removed_projects.iter().any(|p| p == project) {
            self.removed_projects.push(project.to_path_buf());
        }
    }

    /// Whether `project` was removed from the list.
    pub fn removed(&self, project: &std::path::Path) -> bool {
        self.removed_projects.iter().any(|p| p == project)
    }

    pub fn new(title: String, icon: Option<String>, device: Option<String>, device_name: Option<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            title,
            icon,
            device,
            device_name,
            projects: Vec::new(),
            project_names: HashMap::new(),
            removed_projects: Vec::new(),
        }
    }

    /// Whether `id` may be deleted: every device keeps at least one workspace.
    pub fn deletable(all: &[Workspace], id: &str) -> bool {
        let Some(ws) = all.iter().find(|w| w.id == id) else { return false };
        all.iter().any(|w| w.id != ws.id && w.device == ws.device)
    }
}

impl Store {
    /// The workspace the window shows.
    pub fn active_workspace(&self) -> Option<&Workspace> {
        let id = self.active_workspace.as_ref()?;
        self.workspaces.iter().find(|w| &w.id == id)
    }

    /// Close the tabs showing a thread of another workspace: each workspace's
    /// row of tabs holds its own threads only.
    pub fn drop_foreign_tabs(&mut self) {
        let homes: std::collections::HashMap<u64, Option<String>> =
            self.threads.iter().map(|t| (t.id, self.workspace_of(t).map(|w| w.id.clone()))).collect();
        for set in &mut self.tabs {
            let Some(workspace) = set.workspace.clone() else { continue };
            // A thread not known (yet) is left to the sweep of gone threads.
            set.retain_threads(|id| homes.get(&id).is_none_or(|home| home.as_ref() == Some(&workspace)));
        }
    }

    /// The workspace `thread` shows in: the one it names when that is one
    /// of its device's here, else its device's first.
    pub fn workspace_of(&self, thread: &Thread) -> Option<&Workspace> {
        let mut on_device = self.workspaces.iter().filter(|w| w.device == thread.machine);
        let first = on_device.clone().next();
        thread
            .workspace
            .as_ref()
            .and_then(|id| on_device.find(|w| &w.id == id))
            .or(first)
    }

    /// Give every device that threads run on a workspace, so nothing from
    /// before workspaces goes missing; and pick one to show.
    pub fn ensure_workspaces(&mut self, this_device: &str) {
        if !self.workspaces.iter().any(|w| w.device.is_none()) {
            let mut local = Workspace::new(this_device.to_string(), Some("⭐".into()), None, None);
            local.projects = self.projects.clone();
            self.workspaces.insert(0, local);
        }
        let mut machines: Vec<(String, Option<String>)> = Vec::new();
        for thread in &self.threads {
            if let Some(machine) = &thread.machine {
                if !machines.iter().any(|(m, _)| m == machine) {
                    machines.push((machine.clone(), thread.machine_name.clone()));
                }
            }
        }
        for (machine, name) in machines {
            if !self.workspaces.iter().any(|w| w.device.as_deref() == Some(machine.as_str())) {
                let title = name.clone().unwrap_or_else(|| machine.clone());
                self.workspaces.push(Workspace::new(title, Some("💻".into()), Some(machine), name));
            }
        }
        if self.active_workspace().is_none() {
            // Where the window was: the device new threads ran on.
            let id = self
                .workspaces
                .iter()
                .find(|w| w.device == self.machine)
                .or(self.workspaces.first())
                .map(|w| w.id.clone());
            self.active_workspace = id;
        }
    }
}

fn default_enabled_agents() -> Vec<String> {
    vec!["codex".into(), "claude".into()]
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
            agent_args: HashMap::new(),
            enabled_agents: default_enabled_agents(),
            remote_dirs: Default::default(),
            workspaces: Vec::new(),
            active_workspace: None,
            windows: Vec::new(),
            workspace_themes: HashMap::new(),
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
        let path = crate::paths::database();
        let store = match Db::open(&path) {
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
                agent_args: db.setting("agent_args").unwrap_or_default(),
                enabled_agents: db.setting("enabled_agents").unwrap_or_else(default_enabled_agents),
                remote_dirs: db.setting("remote_dirs").unwrap_or_default(),
                workspaces: db.setting("workspaces").unwrap_or_default(),
                active_workspace: db.setting("active_workspace").flatten(),
                windows: db.setting("windows").unwrap_or_default(),
                workspace_themes: db.setting("workspace_themes").unwrap_or_default(),
                db: Some(db),
            },
            Err(err) => {
                eprintln!("codeany: cannot open the database: {err}");
                Store::default()
            }
        };
        // Threads are written off the UI thread from now on.
        if let Some(db) = &store.db {
            crate::db::start_writer(db, &path);
        }
        store.publish_project_names();
        store
    }

    /// Tell [`project_label`] what the projects are called. This device's
    /// names win where a folder of the same path is named elsewhere too.
    pub fn publish_project_names(&self) {
        *PROJECT_NAMES.write().unwrap() = Some(self.project_names());
    }

    fn project_names(&self) -> HashMap<PathBuf, String> {
        let mut names = HashMap::new();
        for remote in [true, false] {
            for ws in self.workspaces.iter().filter(|w| w.device.is_some() == remote) {
                names.extend(ws.project_names.iter().map(|(folder, name)| (folder.clone(), name.clone())));
            }
        }
        names
    }

    /// Write what changed.
    pub fn save(&self) {
        self.publish_project_names();
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
        db.set_setting("agent_args", &self.agent_args);
        db.set_setting("enabled_agents", &self.enabled_agents);
        db.set_setting("remote_dirs", &self.remote_dirs);
        db.set_setting("workspaces", &self.workspaces);
        db.set_setting("active_workspace", &self.active_workspace);
        db.set_setting("windows", &self.windows);
        db.set_setting("workspace_themes", &self.workspace_themes);
        // A copy is cheap next to serializing every item to see what changed;
        // that happens on the writer's thread.
        if !crate::db::save_in_background(self.threads.clone()) {
            if let Err(err) = db.save_threads(&self.threads) {
                eprintln!("codeany: saving threads failed: {err}");
            }
        }
        // Serve the threads that run here; write others back to their device.
        crate::sync::saved(&self.threads);
        crate::sync::saved_workspaces(&self.workspaces);
    }
}

/// A cheap measure of how much an item shows, which changes whenever it
/// grows or changes state: to notice changes without comparing contents.
pub fn item_weight(item: &ChatItem) -> usize {
    let text = |t: &Option<String>| t.as_ref().map_or(0, String::len);
    match item {
        ChatItem::User(message) => message.text.len() + message.images.len(),
        ChatItem::Assistant(t) | ChatItem::Thinking(t) | ChatItem::Error(t) => t.len(),
        ChatItem::Tool { title, detail, output, status, code, images, .. } => {
            title.len() + text(detail) + text(output) + text(code) + images.len() + (*status as usize) * 1_000_003
        }
        ChatItem::Approval { title, detail, options, decision, .. } => {
            title.len() + text(detail) + options.len() + text(decision) * 7 + usize::from(decision.is_some())
        }
        ChatItem::Plan(entries) => entries.len() + format!("{entries:?}").len(),
        ChatItem::TurnEnd { ms, at } => ms.unwrap_or(0) as usize ^ at.unwrap_or(0) as usize,
        ChatItem::Handoff { from_label, to_label, .. } => from_label.len() + to_label.len(),
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
    use chrono::{Datelike as _, Local, TimeZone as _, Timelike as _};
    let local = |secs: u64| Local.timestamp_opt(secs as i64, 0).single();
    let (Some(tm), Some(today)) = (local(secs), local(now_secs())) else {
        return String::new();
    };
    let hour = match tm.hour() % 12 {
        0 => 12,
        h => h,
    };
    let time = format!("{hour}:{:02}", tm.minute());
    let clock = tf(if tm.hour() < 12 { "{time} AM" } else { "{time} PM" }, &[("time", time)]);
    if tm.date_naive() == today.date_naive() {
        return clock;
    }
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    tf(
        "{month} {day}, {clock}",
        &[
            ("month", t(MONTHS[tm.month0() as usize]).to_string()),
            ("day", tm.day().to_string()),
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
        assert!(plan.validate_on_host(&src, 1_700_000_123_456).is_ok());
        assert!(plan.validate_on_host(&src, 42).is_err());
        // Named after the checkout's folder, whatever it is called (CI's is `codeany`).
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).file_name().unwrap();
        assert!(plan.path.ends_with(Path::new("worktrees").join(repo).join("1700000123456")));
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

#[cfg(test)]
mod project_tests {
    use super::*;

    #[test]
    fn a_removed_project_stays_off_the_list_until_it_is_added_again() {
        let folder = PathBuf::from("/Users/me/code/api-server");
        let mut ws = Workspace::new("mbp".into(), None, None, None);
        ws.add_project(&folder);
        ws.project_names.insert(folder.clone(), "Billing API".into());
        ws.remove_project(&folder);
        assert!(ws.removed(&folder) && ws.projects.is_empty() && ws.project_names.is_empty());
        ws.add_project(&folder);
        ws.add_project(&folder);
        assert!(!ws.removed(&folder));
        assert_eq!(ws.projects, vec![folder]);
    }

    #[test]
    fn this_devices_name_for_a_folder_wins() {
        let folder = PathBuf::from("/Users/me/code/api-server");
        let mut store = Store::default();
        let mut here = Workspace::new("mbp".into(), None, None, None);
        here.project_names.insert(folder.clone(), "Billing API".into());
        let mut there = Workspace::new("mini".into(), None, Some("dev1".into()), None);
        there.project_names.insert(folder.clone(), "Mini's API".into());
        there.project_names.insert(PathBuf::from("/srv/site"), "Site".into());
        store.workspaces = vec![here, there];
        let names = store.project_names();
        assert_eq!(names[&folder], "Billing API");
        assert_eq!(names[std::path::Path::new("/srv/site")], "Site");
        assert_eq!(folder_label(&folder), "api-server");
    }
}

#[cfg(test)]
mod workspace_tests {
    #[test]
    fn every_device_keeps_a_workspace() {
        let mut a = super::Workspace::new("a".into(), None, None, None);
        a.id = "a".into();
        let mut b = super::Workspace::new("b".into(), None, None, None);
        b.id = "b".into();
        let mut m = super::Workspace::new("m".into(), None, Some("mini".into()), None);
        m.id = "m".into();
        let all = vec![a, b.clone(), m];
        assert!(super::Workspace::deletable(&all, "a"));
        assert!(!super::Workspace::deletable(&all, "m"));
        let left: Vec<_> = all.into_iter().filter(|w| w.id != "a").collect();
        assert!(!super::Workspace::deletable(&left, "b"));
    }

    use super::*;

    fn thread(machine: Option<&str>, workspace: Option<&str>) -> Thread {
        let mut t = Thread::new(None, "codex".into(), "full".into());
        t.machine = machine.map(str::to_string);
        t.machine_name = machine.map(|m| format!("{m}-name"));
        t.workspace = workspace.map(str::to_string);
        t
    }

    #[test]
    fn threads_from_before_workspaces_land_in_one_per_device() {
        let mut store = Store::default();
        store.projects = vec!["/code/a".into()];
        store.threads = vec![thread(None, None), thread(Some("mini"), None)];
        store.machine = Some("mini".into());
        store.ensure_workspaces("mbp");
        assert_eq!(store.workspaces.len(), 2);
        let local = &store.workspaces[0];
        assert_eq!((local.title.as_str(), local.device.as_deref()), ("mbp", None));
        assert_eq!(local.projects, vec![PathBuf::from("/code/a")]);
        assert_eq!(store.workspaces[1].title, "mini-name");
        // It opens where new threads last ran.
        assert_eq!(store.active_workspace().unwrap().device.as_deref(), Some("mini"));
        // Running it again changes nothing.
        let before = store.workspaces.clone();
        store.ensure_workspaces("mbp");
        assert_eq!(store.workspaces, before);
    }

    #[test]
    fn a_thread_shows_in_the_workspace_it_names_on_its_device() {
        let mut store = Store::default();
        store.ensure_workspaces("mbp");
        let first = store.workspaces[0].id.clone();
        let second = Workspace::new("client".into(), None, None, None);
        let second_id = second.id.clone();
        store.workspaces.push(second);
        let mounted = Workspace::new("mini".into(), None, Some("mini".into()), None);
        let mounted_id = mounted.id.clone();
        store.workspaces.push(mounted);

        assert_eq!(store.workspace_of(&thread(None, None)).unwrap().id, first);
        assert_eq!(store.workspace_of(&thread(None, Some(&second_id))).unwrap().id, second_id);
        // A workspace of another device does not take a local thread.
        assert_eq!(store.workspace_of(&thread(None, Some(&mounted_id))).unwrap().id, first);
        // An unknown id falls back to its device's first workspace.
        assert_eq!(store.workspace_of(&thread(Some("mini"), Some("gone"))).unwrap().id, mounted_id);
        assert!(store.workspace_of(&thread(Some("other"), None)).is_none());
    }

    #[test]
    fn a_workspaces_tabs_hold_its_own_threads_only() {
        let mut store = Store::default();
        store.ensure_workspaces("mini");
        let here = store.workspaces[0].id.clone();
        let mbp = Workspace::new("mbp".into(), None, Some("mbp".into()), None);
        let mbp_id = mbp.id.clone();
        store.workspaces.push(mbp);
        let (mut mine, mut theirs) = (thread(None, None), thread(Some("mbp"), None));
        (mine.id, theirs.id) = (1, 2);
        store.threads = vec![mine, theirs];
        // A tab of mbp's thread strayed into this machine's row; 3 is not synced yet.
        store.tabs = vec![
            crate::chat_tabs::TabSet { project: None, workspace: Some(here), tabs: vec![Some(1), Some(2), None], active: 2 },
            crate::chat_tabs::TabSet { project: None, workspace: Some(mbp_id), tabs: vec![Some(2), Some(3)], active: 0 },
        ];
        store.drop_foreign_tabs();
        assert_eq!((store.tabs[0].tabs.clone(), store.tabs[0].active), (vec![Some(1), None], 1));
        assert_eq!(store.tabs[1].tabs, [Some(2), Some(3)]);
    }
}
