//! Where CodeAny keeps its data: `~/.codeany` (or `$CODEANY_HOME`).

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

/// The working folder of a thread that has no project.
pub fn thread_dir(id: u64) -> PathBuf {
    home().join("threads").join(id.to_string())
}

/// Where a thread's git worktree of `repo` is checked out.
pub fn worktree_dir(repo: &std::path::Path, id: u64) -> PathBuf {
    let name = repo.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".into());
    home().join("worktrees").join(name).join(id.to_string())
}

/// The agent engine's state: its sessions, history and config.
pub fn agent_home() -> PathBuf {
    home().join("agent")
}

fn user_home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}
