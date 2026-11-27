//! Other devices, from an agent's shell.
//!
//! The agents run commands, so the way to reach the devices connected in
//! Settings → Devices is a command: `codeany devices`, `codeany send` and
//! `codeany fetch`. The command is this binary; it asks the running app,
//! over a local socket (`cli.json` in CodeAny's home holds its port and a
//! token), and the app does the work over the link it already keeps with
//! each device, so the agent never holds a key.
//!
//! Moving a file follows the thread's access, as chosen under the input:
//! Full access and Auto edit move it, Ask first asks (once, or for the rest
//! of the thread), Read only refuses. Each new session of a thread on this
//! machine is told about the command (`briefing`).

use std::{
    collections::HashSet,
    io::{BufRead as _, BufReader, Write as _},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::Duration,
};

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use futures::{
    StreamExt as _,
    channel::{mpsc, oneshot},
};
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

use crate::{
    agl_bridge,
    i18n::{t, tf},
    remote,
};

/// The most a file may weigh: what agl moves in one call.
const MAX_BYTES: u64 = 100 * 1024 * 1024;
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// What the command sends the app: one JSON line.
#[derive(Debug, Serialize, Deserialize)]
struct Request {
    token: String,
    #[serde(default)]
    thread: Option<u64>,
    #[serde(flatten)]
    command: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Command {
    Devices,
    /// `local` is absolute; `remote` is absolute or starts with `~/`.
    Send { device: String, local: PathBuf, remote: String },
    Fetch { device: String, remote: String, local: PathBuf },
}

#[derive(Debug, Serialize, Deserialize)]
struct Reply {
    ok: bool,
    text: String,
}

#[derive(Serialize, Deserialize)]
struct Socket {
    port: u16,
    token: String,
}

fn socket_file() -> PathBuf {
    crate::paths::home().join("cli.json")
}

// ---- The command ---------------------------------------------------------

/// The subcommands this binary takes before it is an app.
pub fn is_command(arg: &str) -> bool {
    matches!(arg, "devices" | "send" | "fetch")
}

const USAGE: &str = "usage:
  codeany devices
  codeany send <device> <file> [destination, default ~/Downloads/]
  codeany fetch <device> <path on the device> [local destination, default .]
options: --thread <id> (or CODEANY_THREAD), the thread asking";

/// Run `codeany devices|send|fetch …`; the exit code.
pub fn run_cli(args: &[String]) -> i32 {
    match cli(args) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(err) => {
            eprintln!("codeany: {err:#}");
            1
        }
    }
}

fn cli(args: &[String]) -> Result<String> {
    let mut thread = std::env::var("CODEANY_THREAD").ok().and_then(|v| v.parse().ok());
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--thread" {
            thread = Some(it.next().and_then(|v| v.parse().ok()).ok_or_else(|| anyhow!("--thread takes a number"))?);
        } else if let Some(v) = arg.strip_prefix("--thread=") {
            thread = Some(v.parse().map_err(|_| anyhow!("--thread takes a number"))?);
        } else if arg == "-h" || arg == "--help" {
            return Ok(USAGE.into());
        } else {
            rest.push(arg.as_str());
        }
    }
    let cwd = std::env::current_dir().context("no current folder")?;
    let command = match rest.as_slice() {
        ["devices"] => Command::Devices,
        ["send", device, file, dest @ ..] if dest.len() <= 1 => {
            let local = absolute(&cwd, file);
            let meta = std::fs::metadata(&local).with_context(|| format!("cannot read {}", local.display()))?;
            if !meta.is_file() {
                bail!("{} is not a file (send one file at a time; zip a folder first)", local.display());
            }
            if meta.len() > MAX_BYTES {
                bail!("{} is larger than 100 MB", local.display());
            }
            let name = file_name(&local)?;
            Command::Send { device: device.to_string(), remote: remote_destination(dest.first().copied(), &name)?, local }
        }
        ["fetch", device, path, dest @ ..] if dest.len() <= 1 => {
            let name = path.rsplit('/').next().filter(|n| !n.is_empty() && *n != "~").ok_or_else(|| anyhow!("{path} names no file"))?;
            let local = absolute(&cwd, dest.first().copied().unwrap_or("."));
            let local = if local.is_dir() || dest.first().is_some_and(|d| d.ends_with('/')) { local.join(name) } else { local };
            Command::Fetch { device: device.to_string(), remote: path.to_string(), local }
        }
        _ => bail!("{USAGE}"),
    };
    let socket: Socket = std::fs::read(socket_file())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(|| anyhow!("CodeAny is not running"))?;
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", socket.port)).map_err(|_| anyhow!("CodeAny is not running"))?;
    let request = Request { token: socket.token, thread, command };
    let mut line = serde_json::to_vec(&request)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    let reply: Reply = serde_json::from_str(&reply).map_err(|_| anyhow!("CodeAny closed the connection"))?;
    if reply.ok { Ok(reply.text) } else { Err(anyhow!(reply.text)) }
}

fn absolute(cwd: &Path, path: &str) -> PathBuf {
    let path = match path.strip_prefix("~/") {
        Some(rest) => crate::paths::user_home().join(rest),
        None if path == "~" => crate::paths::user_home(),
        None => PathBuf::from(path),
    };
    if path.is_absolute() { path } else { cwd.join(path) }
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{} names no file", path.display()))
}

/// Where a sent file lands on the device: `dest` as given, or in it when it
/// is a folder (ends in `/`, or is the home or one of its usual folders).
fn remote_destination(dest: Option<&str>, name: &str) -> Result<String> {
    let dest = dest.unwrap_or("~/Downloads/");
    if !(dest.starts_with('/') || dest.starts_with("~/") || dest == "~") {
        bail!("a path on the other device starts with / or ~/ (got {dest})");
    }
    let folder = dest.ends_with('/')
        || matches!(dest.trim_end_matches('/'), "~" | "~/Downloads" | "~/Desktop" | "~/Documents");
    Ok(if folder {
        let base = if dest == "~" { "~/".to_string() } else { format!("{}/", dest.trim_end_matches('/')) };
        format!("{base}{name}")
    } else {
        dest.to_string()
    })
}

// ---- In the app ------------------------------------------------------------

/// A move the app's side must allow.
struct Ask {
    thread: Option<u64>,
    command: Command,
    device: String,
    reply: oneshot::Sender<Result<(), String>>,
}

static ASKS: OnceLock<mpsc::UnboundedSender<Ask>> = OnceLock::new();
/// Threads where "Always allow in this thread" was picked, until quit.
static ALWAYS: Mutex<Option<HashSet<u64>>> = Mutex::new(None);

/// Listen for the command; call once the app runs.
pub fn start(cx: &mut App) {
    let (tx, mut rx) = mpsc::unbounded::<Ask>();
    if ASKS.set(tx).is_err() {
        return;
    }
    cx.spawn(async move |cx| {
        while let Some(ask) = rx.next().await {
            cx.update(|cx| authorize(ask, cx));
        }
    })
    .detach();
    agl_bridge::runtime().spawn(async {
        if let Err(err) = serve().await {
            eprintln!("codeany: the device command is unavailable: {err:#}");
        }
    });
}

async fn serve() -> Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let socket = Socket { port: listener.local_addr()?.port(), token: token.clone() };
    write_private(&socket_file(), &serde_json::to_vec(&socket)?)?;
    loop {
        let (stream, _) = listener.accept().await?;
        let token = token.clone();
        tokio::spawn(async move {
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            if tokio::io::BufReader::new(read).read_line(&mut line).await.is_err() {
                return;
            }
            let reply = match serde_json::from_str::<Request>(&line) {
                Ok(r) if r.token == token => match handle(r.thread, r.command).await {
                    Ok(text) => Reply { ok: true, text },
                    Err(err) => Reply { ok: false, text: format!("{err:#}") },
                },
                _ => Reply { ok: false, text: "not a request from this CodeAny".into() },
            };
            let mut out = serde_json::to_vec(&reply).unwrap_or_default();
            out.push(b'\n');
            let _ = write.write_all(&out).await;
        });
    }
}

/// Only this user may read the token.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)?;
    Ok(())
}

async fn handle(thread: Option<u64>, command: Command) -> Result<String> {
    let devices: Vec<remote::Device> =
        remote::devices().await.unwrap_or_else(|_| remote::cached_devices()).into_iter().filter(|d| !d.is_self).collect();
    let (device, device_id) = match &command {
        Command::Devices => return Ok(list(&devices).await),
        Command::Send { device, .. } | Command::Fetch { device, .. } => {
            let d = find(&devices, device)?;
            if !d.usable() {
                bail!(
                    "{} cannot be reached now: it must be online, with “Share this machine” on in its Settings → Devices",
                    d.name
                );
            }
            (d.name.clone(), d.id.clone())
        }
    };
    // The person's say, through the app.
    let (tx, rx) = oneshot::channel();
    let asks = ASKS.get().ok_or_else(|| anyhow!("CodeAny is starting"))?;
    asks.unbounded_send(Ask { thread, command: command.clone(), device: device.clone(), reply: tx })
        .map_err(|_| anyhow!("CodeAny is closing"))?;
    rx.await.map_err(|_| anyhow!("CodeAny is closing"))?.map_err(|e| anyhow!(e))?;

    let backend = agl_bridge::backend(Some(&device_id));
    match command {
        Command::Send { local, remote, .. } => {
            let bytes = tokio::fs::read(&local).await.with_context(|| format!("cannot read {}", local.display()))?;
            let size = bytes.len();
            let call = backend.call(
                "fs.write",
                json!({ "path": remote, "content_base64": base64::engine::general_purpose::STANDARD.encode(bytes) }),
            );
            tokio::time::timeout(TRANSFER_TIMEOUT, call).await.map_err(|_| anyhow!("timed out"))??;
            Ok(format!("Sent {} ({}) to {device}: {remote}", local.display(), human_size(size)))
        }
        Command::Fetch { remote, local, .. } => {
            let call = backend.call("fs.read", json!({ "path": remote }));
            let reply = tokio::time::timeout(TRANSFER_TIMEOUT, call).await.map_err(|_| anyhow!("timed out"))??;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(reply["content_base64"].as_str().unwrap_or_default())
                .context("the device sent a broken file")?;
            if let Some(dir) = local.parent() {
                tokio::fs::create_dir_all(dir).await?;
            }
            tokio::fs::write(&local, &bytes).await.with_context(|| format!("cannot write {}", local.display()))?;
            Ok(format!("Fetched {remote} from {device} ({}) to {}", human_size(bytes.len()), local.display()))
        }
        Command::Devices => unreachable!(),
    }
}

/// A device by name (any case, or a unique start of it) or id.
fn find<'a>(devices: &'a [remote::Device], query: &str) -> Result<&'a remote::Device> {
    let q = query.trim().to_lowercase();
    if let Some(d) = devices.iter().find(|d| d.name.to_lowercase() == q || d.id == query) {
        return Ok(d);
    }
    let starts: Vec<_> = devices.iter().filter(|d| d.name.to_lowercase().starts_with(&q)).collect();
    match starts.as_slice() {
        [d] => Ok(d),
        [] => {
            let names: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
            if names.is_empty() {
                bail!("no other devices are connected (Settings → Devices)")
            }
            bail!("no device called “{query}”; connected: {}", names.join(", "))
        }
        many => bail!("“{query}” could be {}", many.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(" or ")),
    }
}

async fn list(devices: &[remote::Device]) -> String {
    if devices.is_empty() {
        return "No other devices are connected (Settings → Devices in CodeAny).".into();
    }
    let mut out = format!("This device: {}\n", remote::this_name());
    for d in devices {
        let state = if d.usable() {
            "online, files can be sent and fetched"
        } else if d.online() {
            "online, not sharing (turn on “Share this machine” there)"
        } else {
            "offline"
        };
        out.push_str(&format!("\n{} ({state})\n", d.name));
        if !d.usable() {
            continue;
        }
        let workspaces = tokio::time::timeout(Duration::from_secs(8), crate::sync::device_workspaces(d.id.clone())).await;
        match workspaces {
            Ok(Ok(list)) if !list.is_empty() => {
                for w in list {
                    out.push_str(&format!("  workspace “{}”\n", w.title));
                    for p in &w.projects {
                        out.push_str(&format!("    {}\n", p.display()));
                    }
                }
            }
            Ok(Ok(_)) => out.push_str("  no workspaces\n"),
            _ => out.push_str("  workspaces unavailable\n"),
        }
    }
    out.trim_end().to_string()
}

fn human_size(bytes: usize) -> String {
    match bytes {
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / 1024.),
        b => format!("{b} B"),
    }
}

/// Allow a move by the thread's access, asking when it says to.
fn authorize(ask: Ask, cx: &mut App) {
    let thread = ask.thread.and_then(|id| {
        let hub = crate::hub::hub(cx);
        let shared = hub.read(cx).shared();
        let s = shared.borrow();
        s.thread(id).map(|t| (id, t.access.clone(), t.title.clone()))
    });
    let access = thread.as_ref().map_or("ask", |(_, a, _)| a.as_str());
    match access {
        "full" | "edit" => {
            let _ = ask.reply.send(Ok(()));
            return;
        }
        "read_only" => {
            let _ = ask.reply.send(Err(
                "this thread is Read only; the person can change its access under the input to let files move".into(),
            ));
            return;
        }
        _ => {}
    }
    let id = thread.as_ref().map(|(id, _, _)| *id);
    if id.is_some_and(|id| ALWAYS.lock().unwrap().as_ref().is_some_and(|s| s.contains(&id))) {
        let _ = ask.reply.send(Ok(()));
        return;
    }
    let (message, detail) = match &ask.command {
        Command::Send { local, remote, .. } => (
            tf("Send “{file}” to {device}?", &[("file", file_name(local).unwrap_or_default()), ("device", ask.device.clone())]),
            tf("An agent asks to copy {local} to {remote} on {device}.", &[
                ("local", local.display().to_string()),
                ("remote", remote.clone()),
                ("device", ask.device.clone()),
            ]),
        ),
        Command::Fetch { local, remote, .. } => (
            tf("Fetch “{file}” from {device}?", &[
                ("file", remote.rsplit('/').next().unwrap_or(remote).to_string()),
                ("device", ask.device.clone()),
            ]),
            tf("An agent asks to copy {remote} on {device} to {local}.", &[
                ("local", local.display().to_string()),
                ("remote", remote.clone()),
                ("device", ask.device.clone()),
            ]),
        ),
        Command::Devices => unreachable!(),
    };
    let detail = match &thread {
        Some((_, _, title)) => format!("{detail}\n{}", tf("Thread: {title}", &[("title", crate::threads::display_title(title))])),
        None => detail,
    };
    let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) else {
        let _ = ask.reply.send(Err("no CodeAny window is open to ask the person".into()));
        return;
    };
    let mut buttons = vec![PromptButton::ok(t("Allow"))];
    if id.is_some() {
        buttons.push(PromptButton::new(t("Always allow in this thread")));
    }
    buttons.push(PromptButton::cancel(t("Deny")));
    let always_ix = if id.is_some() { Some(1) } else { None };
    let deny_ix = buttons.len() - 1;
    cx.activate(true);
    let answer = window.update(cx, |_, window, cx| window.prompt(PromptLevel::Info, &message, Some(&detail), &buttons, cx));
    let Ok(answer) = answer else {
        let _ = ask.reply.send(Err("no CodeAny window is open to ask the person".into()));
        return;
    };
    cx.spawn(async move |_| {
        let result = match answer.await {
            Ok(ix) if ix == deny_ix => Err("the person declined".to_string()),
            Ok(ix) => {
                if Some(ix) == always_ix
                    && let Some(id) = id
                {
                    ALWAYS.lock().unwrap().get_or_insert_with(HashSet::new).insert(id);
                }
                Ok(())
            }
            Err(_) => Err("the person declined".to_string()),
        };
        let _ = ask.reply.send(result);
    })
    .detach();
}

/// What a new session of thread `id` on this machine is told, when other
/// devices are connected: their names and the command.
pub fn briefing(id: u64, devices: &[String]) -> Option<String> {
    if devices.is_empty() {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let exe = shell_quote(&exe.to_string_lossy());
    Some(format!(
        "<codeany-devices>\n\
         CodeAny on this computer is connected to other devices: {names}. \
         Use this command for them (not scp, ssh or Tailscale):\n\
         - {exe} devices --thread {id}   (lists them, whether they are reachable, and their workspaces and project folders)\n\
         - {exe} send --thread {id} <device> <file> [destination]   (destination absolute or ~/…, default ~/Downloads/; end a folder with /)\n\
         - {exe} fetch --thread {id} <device> <path there> [local destination]\n\
         One file of up to 100 MB at a time (zip a folder first). Moving a file may wait for the person to allow it.\n\
         </codeany-devices>\n\n",
        names = devices.join(", "),
    ))
}

fn shell_quote(s: &str) -> String {
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c)) {
        s.to_string()
    } else if cfg!(windows) {
        format!("& \"{s}\"")
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::remote_destination;

    #[test]
    fn sent_files_land_in_folders() {
        assert_eq!(remote_destination(None, "a.apk").unwrap(), "~/Downloads/a.apk");
        assert_eq!(remote_destination(Some("~/Desktop"), "a.apk").unwrap(), "~/Desktop/a.apk");
        assert_eq!(remote_destination(Some("/tmp/x/"), "a.apk").unwrap(), "/tmp/x/a.apk");
        assert_eq!(remote_destination(Some("~"), "a.apk").unwrap(), "~/a.apk");
        assert_eq!(remote_destination(Some("~/b.apk"), "a.apk").unwrap(), "~/b.apk");
        assert!(remote_destination(Some("rel/path"), "a.apk").is_err());
    }
}
