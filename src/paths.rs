//! Where dscode keeps its data: `~/.dscode` (or `$DSCODE_HOME`).
//!
//! The directory is shared with the earlier Electron DSCode, whose files
//! (`state.sqlite`, `sessions/`, `settings.json`, …) are left alone; dscode
//! only owns `dscode.db`, `images/` and `agl/`.

use std::path::PathBuf;

pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("DSCODE_HOME") {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".dscode")
}

pub fn database() -> PathBuf {
    home().join("dscode.db")
}

pub fn images() -> PathBuf {
    home().join("images")
}

pub fn agl_home() -> PathBuf {
    home().join("agl")
}

/// Where versions before `~/.dscode` kept their data.
pub fn legacy() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/dscode"))
}
