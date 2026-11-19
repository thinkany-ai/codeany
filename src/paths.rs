//! Where CodeAny keeps its data: `~/.codeany` (or `$CODEANY_HOME`).
//!
//! CodeAny was called dscode before and kept its data in `~/.dscode`, a
//! directory shared with the earlier Electron DSCode. [`migrate`] moves
//! the files this app owns from there on first start and leaves the rest.

use std::path::PathBuf;

pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("CODEANY_HOME") {
        return PathBuf::from(dir);
    }
    user_home().join(".codeany")
}

pub fn database() -> PathBuf {
    home().join("codeany.db")
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

fn user_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

/// Move the database, agl's home and the nex link from `~/.dscode` into
/// `~/.codeany` the first time CodeAny starts. Screenshots stay where they
/// are: transcripts refer to them by absolute path. Must run before the
/// database is opened or agl starts.
pub fn migrate() {
    if std::env::var_os("CODEANY_HOME").is_some() {
        return;
    }
    let (old, new) = (user_home().join(".dscode"), home());
    if new.exists() || !old.join("dscode.db").is_file() {
        return;
    }
    if let Err(err) = std::fs::create_dir_all(&new) {
        eprintln!("codeany: cannot create {}: {err}", new.display());
        return;
    }
    // SQLite finds the WAL and shared-memory files by the database's name.
    let moves = [
        ("dscode.db", "codeany.db"),
        ("dscode.db-wal", "codeany.db-wal"),
        ("dscode.db-shm", "codeany.db-shm"),
        ("agl", "agl"),
        ("nex.json", "nex.json"),
    ];
    for (from, to) in moves {
        let from = old.join(from);
        if from.exists() {
            if let Err(err) = std::fs::rename(&from, new.join(to)) {
                eprintln!("codeany: could not move {}: {err}", from.display());
            }
        }
    }
}
