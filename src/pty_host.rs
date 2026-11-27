//! This device's shells, for the terminal panel of another device whose
//! window shows a workspace here.
//!
//! A local WebSocket server, reached like the agl daemon and the thread
//! service: through a paired device's encrypted link or the relay, and only
//! while this device shares its agents. One connection is one shell:
//!
//! ```text
//!   → {"open":{"cwd":"/path","cols":80,"rows":24}}     (text, first)
//!   ← {"ok":true} | {"error":"…"}
//!   → keys (binary) · {"resize":{"cols":…,"rows":…}}  (text)
//!   ← output (binary) · {"exit":true} when the shell ends
//! ```
//!
//! Closing the connection ends the shell.

use anyhow::Result;
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

/// The service another device asks for.
pub const SERVICE: &str = "codeany.pty";

static URL: tokio::sync::Mutex<Option<String>> = tokio::sync::Mutex::const_new(None);

/// The local URL of the shell service, started on first use.
pub async fn server_url() -> Result<String> {
    let mut started = URL.lock().await;
    if let Some(url) = started.clone() {
        return Ok(url);
    }
    // The cached listener must outlive whichever caller first opens it.
    let runtime = crate::agl_bridge::runtime();
    let listener = runtime.spawn(TcpListener::bind(("127.0.0.1", 0))).await??;
    let port = listener.local_addr()?.port();
    let token = uuid::Uuid::new_v4().simple().to_string();
    let url = format!("ws://127.0.0.1:{port}/pty?token={token}");
    runtime.spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let token = token.clone();
            tokio::spawn(async move {
                use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
                let check = |req: &Request, resp: Response| {
                    let ok = req.uri().query().is_some_and(|q| q.split('&').any(|kv| kv == format!("token={token}")));
                    if ok {
                        Ok(resp)
                    } else {
                        let mut refused = ErrorResponse::new(None);
                        *refused.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
                        Err(refused)
                    }
                };
                if let Ok(ws) = tokio_tungstenite::accept_hdr_async(socket, check).await {
                    serve(ws).await;
                }
            });
        }
    });
    *started = Some(url.clone());
    Ok(url)
}

async fn serve(mut ws: WebSocketStream<TcpStream>) {
    // The first message says where the shell opens and how big it is.
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await;
    let Ok(Some(Ok(Message::Text(text)))) = first else { return };
    let request: Value = serde_json::from_str(&text).unwrap_or_default();
    let open = &request["open"];
    if !open.is_object() {
        let _ = ws.send(Message::Text(json!({ "error": "unknown request" }).to_string().into())).await;
        return;
    }
    let cwd = open["cwd"].as_str().map(std::path::PathBuf::from);
    let size = (cells(&open["cols"], 80), cells(&open["rows"], 24));
    shell::run(ws, cwd, size).await;
}

/// A terminal dimension from the wire, within what a PTY takes.
fn cells(value: &Value, default: u16) -> u16 {
    value.as_u64().map(|n| n.clamp(2, 1000) as u16).unwrap_or(default)
}

#[cfg(not(unix))]
mod shell {
    use super::*;

    pub async fn run(mut ws: WebSocketStream<TcpStream>, _cwd: Option<std::path::PathBuf>, _size: (u16, u16)) {
        let error = "that device cannot share its terminal yet (Windows)";
        let _ = ws.send(Message::Text(json!({ "error": error }).to_string().into())).await;
    }
}

#[cfg(unix)]
mod shell {
    use std::{
        fs::File,
        io::{Read as _, Write as _},
        os::fd::AsRawFd as _,
        path::PathBuf,
    };

    use alacritty_terminal::{
        event::{OnResize as _, WindowSize},
        tty,
    };

    use super::*;

    fn window_size((cols, rows): (u16, u16)) -> WindowSize {
        WindowSize { num_lines: rows, num_cols: cols, cell_width: 8, cell_height: 16 }
    }

    /// The login shell on a new PTY, and the PTY's end to read and write.
    fn spawn(cwd: Option<PathBuf>, size: (u16, u16)) -> Result<(tty::Pty, File)> {
        let mut env = std::collections::HashMap::new();
        env.insert("TERM".to_string(), "xterm-256color".to_string());
        env.insert("COLORTERM".to_string(), "truecolor".to_string());
        env.insert("TERM_PROGRAM".to_string(), "codeany".to_string());
        // A folder that is gone must not keep the shell from opening.
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let options = tty::Options {
            shell: None,
            working_directory: cwd.filter(|dir| dir.is_dir()).or(home),
            drain_on_exit: false,
            env,
        };
        let pty = tty::new(&options, window_size(size), 0)?;
        let file = pty.file().try_clone()?;
        // alacritty polls it; the threads below block on it instead. The
        // flag belongs to the open file, so it holds for every clone.
        // SAFETY: plain fcntl calls on a descriptor this function owns.
        unsafe {
            let fd = file.as_raw_fd();
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        Ok((pty, file))
    }

    pub async fn run(ws: WebSocketStream<TcpStream>, cwd: Option<PathBuf>, size: (u16, u16)) {
        let (mut sink, mut stream) = ws.split();
        let spawned = spawn(cwd, size).and_then(|(pty, file)| Ok((pty, file.try_clone()?, file)));
        let (mut pty, mut reader, mut writer) = match spawned {
            Ok(spawned) => spawned,
            Err(err) => {
                let _ = sink.send(Message::Text(json!({ "error": format!("{err:#}") }).to_string().into())).await;
                return;
            }
        };
        if sink.send(Message::Text(json!({ "ok": true }).to_string().into())).await.is_err() {
            tokio::task::spawn_blocking(move || drop(pty));
            return;
        }

        // Output: a thread reads the PTY; a full channel holds it back when
        // the other device reads slower than the shell writes.
        let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    // The shell and everything it started let go of the PTY.
                    Ok(0) => break,
                    Ok(n) => {
                        if out_tx.blocking_send(buffer[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        });
        // Input: a thread too, so a shell that is not reading cannot stall
        // the connection.
        let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::spawn(move || {
            while let Ok(bytes) = in_rx.recv() {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
            }
        });

        loop {
            tokio::select! {
                chunk = out_rx.recv() => match chunk {
                    Some(bytes) => {
                        if sink.send(Message::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        let _ = sink.send(Message::Text(json!({ "exit": true }).to_string().into())).await;
                        let _ = sink.close().await;
                        break;
                    }
                },
                message = stream.next() => match message {
                    Some(Ok(Message::Binary(bytes))) => {
                        if in_tx.send(bytes.to_vec()).is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        let request: Value = serde_json::from_str(&text).unwrap_or_default();
                        let resize = &request["resize"];
                        if resize.is_object() {
                            pty.on_resize(window_size((cells(&resize["cols"], 80), cells(&resize["rows"], 24))));
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => {}
                },
            }
        }
        // Hangs the shell up and waits for it: off the runtime's threads.
        tokio::task::spawn_blocking(move || drop(pty));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Read output until `needle` shows up in it.
    async fn read_until<S>(ws: &mut S, needle: &str) -> String
    where
        S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
    {
        let mut seen = String::new();
        while !seen.contains(needle) {
            match ws.next().await {
                Some(Ok(Message::Binary(bytes))) => seen.push_str(&String::from_utf8_lossy(&bytes)),
                Some(Ok(Message::Text(text))) => seen.push_str(&text),
                other => panic!("the shell stopped before `{needle}`: {other:?}\n{seen}"),
            }
        }
        seen
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_shell_opens_in_the_folder_runs_and_exits() {
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let url = server_url().await.unwrap();
        // Without the token there is no shell.
        assert!(tokio_tungstenite::connect_async(url.split('?').next().unwrap()).await.is_err());

        let (mut ws, _) = tokio_tungstenite::connect_async(url.as_str()).await.unwrap();
        let open = json!({ "open": { "cwd": dir, "cols": 100, "rows": 30 } });
        ws.send(Message::Text(open.to_string().into())).await.unwrap();
        let all = async {
            assert!(read_until(&mut ws, "ok").await.contains(r#""ok":true"#));
            // The arithmetic keeps the typed command's echo from matching.
            ws.send(Message::Binary(b"echo pty-$((40+2)); pwd; stty size\r".to_vec().into())).await.unwrap();
            let out = read_until(&mut ws, "30 100").await;
            assert!(out.contains("pty-42"), "{out}");
            assert!(out.contains(dir.to_str().unwrap()), "{out}");
            ws.send(Message::Text(json!({ "resize": { "cols": 90, "rows": 20 } }).to_string().into())).await.unwrap();
            ws.send(Message::Binary(b"stty size\r".to_vec().into())).await.unwrap();
            read_until(&mut ws, "20 90").await;
            ws.send(Message::Binary(b"exit\r".to_vec().into())).await.unwrap();
            read_until(&mut ws, r#""exit":true"#).await;
        };
        tokio::time::timeout(std::time::Duration::from_secs(30), all).await.expect("the shell answers in time");
    }
}
