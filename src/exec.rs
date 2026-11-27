//! Run a command where a workspace lives: on this machine, or on another
//! one through its agl (`exec`), the way the conversation reaches it.
//!
//! Everything here blocks, so callers run it on the background executor.

use std::path::Path;

use serde_json::{Value, json};

use crate::agl_bridge;

const REMOTE_TIMEOUT_MS: u64 = 30_000;

pub struct Output {
    pub code: Option<i64>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// One request to `machine`'s agl; the reply's `result`.
pub fn call(machine: &str, method: &str, params: Value) -> Result<Value, String> {
    let call = agl_bridge::backend(Some(machine)).call(method, params);
    futures::executor::block_on(call).map_err(|err| format!("{err:#}"))
}

/// Run `argv` in `cwd` on `machine` (`None`: here). `Err` when it could not
/// be run at all; a command that ran and failed is an [`Output`].
pub fn run(machine: Option<&str>, cwd: &Path, argv: &[&str], env: &[(&str, &str)]) -> Result<Output, String> {
    let Some(machine) = machine else {
        let output = crate::command(argv[0])
            .args(&argv[1..])
            .current_dir(cwd)
            .envs(env.iter().copied())
            .output()
            .map_err(|err| format!("cannot run {}: {err}", argv[0]))?;
        return Ok(Output {
            code: output.status.code().map(i64::from),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    };
    let env: serde_json::Map<String, Value> = env.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    let out = call(
        machine,
        "exec",
        json!({ "argv": argv, "cwd": cwd, "env": env, "timeout_ms": REMOTE_TIMEOUT_MS }),
    )?;
    // The program was not found, or the command timed out.
    if let Some(err) = out["error"].as_str().filter(|e| !e.is_empty()) {
        return Err(err.to_string());
    }
    if out["timed_out"].as_bool() == Some(true) {
        return Err(format!("{} did not finish in time", argv[0]));
    }
    Ok(Output {
        code: out["exit_code"].as_i64(),
        stdout: out["stdout"].as_str().unwrap_or_default().to_string(),
        stderr: out["stderr"].as_str().unwrap_or_default().to_string(),
    })
}
