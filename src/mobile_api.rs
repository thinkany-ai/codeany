//! CodeAny's own threads, served to the user's other devices.
//!
//! The window is the one place threads live (`Store`, SQLite). A phone shows
//! the same threads by talking to this API through a nex stream (service
//! `codeany`, see [`crate::remote`]) instead of keeping a copy: it reads the
//! thread list and a thread's items, is told as they change, and sends
//! prompts, approvals and stops that the window carries out exactly as if
//! they were typed here. So both sides always show one conversation.
//!
//! Wire format, JSON text frames on a local WebSocket:
//! - `{"id", "method", "params"}`            phone -> CodeAny
//! - `{"id", "result"}` / `{"id", "error"}`   CodeAny -> phone
//! - `{"event": "state", "state": {..}}`      the thread list, on connect and when it changes
//! - `{"event": "items", "thread", "rev", "from", "len", "items": [..]}`
//!   a watched thread's items from index `from` on; it now has `len` items
//!
//! The server runs on the tokio runtime; the threads belong to the
//! `AgentPanel` on GPUI's thread, which takes requests from [`attach`] and
//! reports changes with [`publish_state`] and [`publish_items`].

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

use anyhow::{Context as _, Result};
use futures::{
    SinkExt as _, StreamExt as _,
    channel::{mpsc, oneshot},
};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::{
    Message,
    handshake::server::{ErrorResponse, Request as Handshake, Response},
};

use crate::{
    agl_bridge,
    threads::{ChatItem, ToolStatus},
};

/// The nex service name this API is exposed under.
pub const SERVICE: &str = "codeany";

/// A request for the window to carry out.
pub struct Request {
    pub method: String,
    pub params: Value,
    pub reply: oneshot::Sender<Result<Value, String>>,
}

#[derive(Clone)]
enum Push {
    State(Arc<str>),
    Items { thread: u64, frame: Arc<str> },
}

/// A watched thread's items as last published, serialized one by one.
#[derive(Default)]
struct Published {
    items: Vec<String>,
    rev: u64,
}

struct Hub {
    /// To the window; `None` while no window runs (`codeany serve`).
    requests: Mutex<Option<mpsc::UnboundedSender<Request>>>,
    pushes: broadcast::Sender<Push>,
    state: Mutex<Option<Arc<str>>>,
    threads: Mutex<HashMap<u64, Published>>,
    /// How many connections watch each thread.
    watched: Mutex<HashMap<u64, usize>>,
}

fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(|| Hub {
        requests: Mutex::new(None),
        pushes: broadcast::channel(1024).0,
        state: Mutex::new(None),
        threads: Mutex::new(HashMap::new()),
        watched: Mutex::new(HashMap::new()),
    })
}

// ---------------------------------------------------------------------------
// The window's side
// ---------------------------------------------------------------------------

/// The window starts taking requests. Call once, from the agent panel.
pub fn attach() -> mpsc::UnboundedReceiver<Request> {
    let (tx, rx) = mpsc::unbounded();
    *hub().requests.lock().unwrap() = Some(tx);
    rx
}

/// The threads some device is looking at: only their items are published.
pub fn watched() -> Vec<u64> {
    hub().watched.lock().unwrap().keys().copied().collect()
}

/// The thread list changed (or may have): tell the devices if it did.
pub fn publish_state(state: Value) {
    let text: Arc<str> = json!({ "event": "state", "state": state }).to_string().into();
    let mut current = hub().state.lock().unwrap();
    if current.as_deref() == Some(&*text) {
        return;
    }
    *current = Some(text.clone());
    let _ = hub().pushes.send(Push::State(text));
}

/// A thread's items from index `from` on are now `tail`; what is before
/// `from` is as published earlier. Devices get the part that changed.
pub fn publish_items(thread: u64, from: usize, tail: impl Iterator<Item = Value>) {
    let mut threads = hub().threads.lock().unwrap();
    let published = threads.entry(thread).or_default();
    let from = from.min(published.items.len());
    let tail: Vec<String> = tail.map(|item| item.to_string()).collect();
    let len = from + tail.len();
    // The first item that differs from what the devices have.
    let same = published.items[from..]
        .iter()
        .zip(&tail)
        .take_while(|(old, new)| old == new)
        .count();
    if from + same == len && published.items.len() == len {
        return;
    }
    published.items.truncate(from + same);
    published.items.extend(tail.into_iter().skip(same));
    published.rev += 1;
    let frame = items_frame(thread, published, from + same);
    let _ = hub().pushes.send(Push::Items { thread, frame: frame.into() });
}

fn items_frame(thread: u64, published: &Published, from: usize) -> String {
    // The items are JSON already: join them instead of parsing them again.
    format!(
        r#"{{"event":"items","thread":{thread},"rev":{},"from":{from},"len":{},"items":[{}]}}"#,
        published.rev,
        published.items.len(),
        published.items[from..].join(",")
    )
}

/// A thread was deleted.
pub fn forget(thread: u64) {
    hub().threads.lock().unwrap().remove(&thread);
}

/// One transcript item as devices get it.
pub fn item_json(item: &ChatItem) -> Value {
    match item {
        ChatItem::User(text) => json!({ "kind": "user", "text": text }),
        ChatItem::Assistant(text) => json!({ "kind": "assistant", "text": text }),
        ChatItem::Thinking(text) => json!({ "kind": "thinking", "text": text }),
        ChatItem::Tool {
            id,
            title,
            detail,
            output,
            status,
            ..
        } => json!({
            "kind": "tool",
            "id": id,
            "title": title,
            "detail": detail,
            "output": output,
            "status": match status {
                ToolStatus::Running => "running",
                ToolStatus::Done => "done",
                ToolStatus::Failed => "failed",
            },
        }),
        ChatItem::Approval {
            request_id,
            title,
            detail,
            options,
            decision,
        } => json!({
            "kind": "approval",
            "request_id": request_id,
            "title": title,
            "detail": detail,
            "options": options,
            "decision": decision,
        }),
        ChatItem::Plan(entries) => json!({ "kind": "plan", "entries": entries }),
        ChatItem::TurnEnd { ms, at } => json!({ "kind": "turn_end", "ms": ms, "at": at }),
        ChatItem::Handoff { from_label, to_label, .. } => {
            json!({ "kind": "handoff", "from": from_label, "to": to_label })
        }
        ChatItem::Error(text) => json!({ "kind": "error", "text": text }),
    }
}

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

/// The local WebSocket URL of the API, started on first use.
pub async fn endpoint() -> Result<String> {
    static URL: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();
    URL.get_or_try_init(|| async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("start the devices API")?;
        let port = listener.local_addr()?.port();
        let token = uuid::Uuid::new_v4().simple().to_string();
        let url = format!("ws://127.0.0.1:{port}/ws?token={token}");
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let token = token.clone();
                tokio::spawn(async move {
                    let check = |request: &Handshake, response: Response| -> Result<Response, ErrorResponse> {
                        let allowed = request
                            .uri()
                            .query()
                            .is_some_and(|query| query.split('&').any(|pair| pair.strip_prefix("token=") == Some(&token)));
                        if allowed {
                            Ok(response)
                        } else {
                            let mut refused = ErrorResponse::new(None);
                            *refused.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;
                            Err(refused)
                        }
                    };
                    if let Ok(ws) = tokio_tungstenite::accept_hdr_async(stream, check).await {
                        serve(ws).await;
                    }
                });
            }
        });
        Ok(url)
    })
    .await
    .cloned()
}

async fn serve(ws: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) {
    let (mut sink, mut stream) = ws.split();
    let mut pushes = hub().pushes.subscribe();
    let mut watching: Vec<u64> = Vec::new();

    let state = hub().state.lock().unwrap().clone();
    if let Some(state) = state {
        if sink.send(Message::Text(state.to_string().into())).await.is_err() {
            return;
        }
    }

    loop {
        tokio::select! {
            push = pushes.recv() => {
                let text = match push {
                    Ok(Push::State(text)) => text,
                    Ok(Push::Items { thread, frame }) if watching.contains(&thread) => frame,
                    Ok(_) => continue,
                    // Fell behind: the device loads everything again.
                    Err(broadcast::error::RecvError::Lagged(_)) => break,
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                if sink.send(Message::Text(text.to_string().into())).await.is_err() {
                    break;
                }
            }
            msg = stream.next() => {
                let text = match msg {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => continue,
                };
                let Ok(request) = serde_json::from_str::<Value>(&text) else { continue };
                let Some(id) = request["id"].as_str().map(String::from) else { continue };
                let method = request["method"].as_str().unwrap_or_default().to_string();
                let params = request["params"].clone();
                // Answered in order with the pushes, so a thread's items and
                // the changes after them arrive in the order they happened.
                let reply = match handle(&method, params, &mut watching).await {
                    Ok(result) => format!(r#"{{"id":{},"result":{result}}}"#, Value::String(id)),
                    Err(error) => json!({ "id": id, "error": error }).to_string(),
                };
                if sink.send(Message::Text(reply.into())).await.is_err() {
                    break;
                }
            }
        }
    }
    for thread in watching {
        unwatch(thread);
    }
}

fn unwatch(thread: u64) {
    let mut watched = hub().watched.lock().unwrap();
    if let Some(count) = watched.get_mut(&thread) {
        *count -= 1;
        if *count == 0 {
            watched.remove(&thread);
            // Nobody keeps it current any more.
            hub().threads.lock().unwrap().remove(&thread);
        }
    }
}

/// One request; the result as JSON text.
async fn handle(method: &str, params: Value, watching: &mut Vec<u64>) -> Result<String, String> {
    let thread = || params["thread"].as_u64().ok_or("missing `thread`");
    let done = |value: Value| Ok(value.to_string());
    match method {
        // The thread's items, and their changes from now on.
        "watch" => {
            let thread = thread()?;
            if !watching.contains(&thread) {
                watching.push(thread);
                *hub().watched.lock().unwrap().entry(thread).or_insert(0) += 1;
            }
            // The window publishes all of it, which fills the cache below.
            window("items", json!({ "thread": thread })).await?;
            let threads = hub().threads.lock().unwrap();
            let published = threads.get(&thread).ok_or("the thread is gone")?;
            Ok(format!(
                r#"{{"rev":{},"items":[{}]}}"#,
                published.rev,
                published.items.join(",")
            ))
        }
        "unwatch" => {
            let thread = thread()?;
            if let Some(at) = watching.iter().position(|t| *t == thread) {
                watching.remove(at);
                unwatch(thread);
            }
            done(Value::Null)
        }
        "ping" => done(json!({ "pong": true })),
        // What the machine itself answers, without the window.
        "models" => {
            let agent = params["agent"].as_str().ok_or("missing `agent`")?;
            let models = agl_bridge::backend(None)
                .call("agents.models", json!({ "agent": agent }))
                .await
                .map_err(|err| format!("{err:#}"))?;
            done(models["models"].clone())
        }
        "home" => {
            let home = agl_bridge::backend(None).home().await.map_err(|err| format!("{err:#}"))?;
            done(json!(home))
        }
        "folders" => {
            let dir = params["dir"].as_str().ok_or("missing `dir`")?;
            let dirs = agl_bridge::backend(None)
                .list_dirs(dir.into())
                .await
                .map_err(|err| format!("{err:#}"))?;
            done(json!(dirs))
        }
        _ => window(method, params).await.map(|value| value.to_string()),
    }
}

/// Has the window carry out a request.
async fn window(method: &str, params: Value) -> Result<Value, String> {
    let (reply, answer) = oneshot::channel();
    let sent = match &*hub().requests.lock().unwrap() {
        Some(requests) => requests
            .unbounded_send(Request {
                method: method.to_string(),
                params,
                reply,
            })
            .is_ok(),
        None => false,
    };
    if !sent {
        return Err("CodeAny is not open on the computer".into());
    }
    answer.await.map_err(|_| "CodeAny closed".to_string())?
}
