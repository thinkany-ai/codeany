//! Thin wrappers around the `git` CLI. Everything here blocks, so callers run
//! it on the background executor.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

/// Untracked files larger than this are listed but not diffed.
const MAX_UNTRACKED_DIFF_BYTES: u64 = 512 * 1024;
const MAX_UNTRACKED_DIFF_FILES: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl FileStatus {
    pub fn letter(self) -> &'static str {
        match self {
            FileStatus::Modified => "M",
            FileStatus::Added => "A",
            FileStatus::Deleted => "D",
            FileStatus::Renamed => "R",
            FileStatus::Untracked => "U",
            FileStatus::Conflicted => "!",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RepoSnapshot {
    pub branch: Option<String>,
    /// Absolute path -> status, for every changed file.
    pub statuses: HashMap<PathBuf, FileStatus>,
}

impl RepoSnapshot {
    /// The status of a directory is that of any changed file beneath it.
    pub fn dir_has_changes(&self, dir: &Path) -> Option<FileStatus> {
        self.statuses
            .iter()
            .find(|(path, _)| path.starts_with(dir))
            .map(|(_, status)| *status)
    }
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    // `git diff --no-index` exits with 1 when files differ.
    if !output.status.success() && output.status.code() != Some(1) {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn toplevel(root: &Path) -> Option<PathBuf> {
    git(root, &["rev-parse", "--show-toplevel"]).map(|s| PathBuf::from(s.trim()))
}

pub fn snapshot(root: &Path) -> Option<RepoSnapshot> {
    let top = toplevel(root)?;
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let raw = git(
        root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut statuses = HashMap::new();
    let mut entries = raw.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let (code, path) = entry.split_at(3);
        let mut chars = code.chars();
        let (x, y) = (chars.next().unwrap_or(' '), chars.next().unwrap_or(' '));
        let status = match (x, y) {
            ('?', '?') => FileStatus::Untracked,
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => FileStatus::Conflicted,
            ('R', _) | (_, 'R') => FileStatus::Renamed,
            ('A', _) => FileStatus::Added,
            ('D', _) | (_, 'D') => FileStatus::Deleted,
            _ => FileStatus::Modified,
        };
        if x == 'R' || x == 'C' {
            // Renames carry the original path as the next NUL-separated field.
            entries.next();
        }
        statuses.insert(top.join(path), status);
    }

    Some(RepoSnapshot { branch, statuses })
}

/// A unified patch of every change in the working tree against `HEAD`,
/// including untracked files.
pub fn working_tree_diff(root: &Path) -> Option<String> {
    let top = toplevel(root)?;
    let mut patch = git(&top, &["diff", "HEAD", "--no-color", "--no-ext-diff"])
        // A repository without commits has no HEAD to compare against.
        .or_else(|| git(&top, &["diff", "--no-color", "--no-ext-diff"]))
        .unwrap_or_default();

    let untracked = git(&top, &["ls-files", "--others", "--exclude-standard", "-z"])
        .unwrap_or_default();
    for path in untracked
        .split('\0')
        .filter(|s| !s.is_empty())
        .take(MAX_UNTRACKED_DIFF_FILES)
    {
        let size = std::fs::metadata(top.join(path)).map(|m| m.len()).unwrap_or(0);
        if size > MAX_UNTRACKED_DIFF_BYTES {
            continue;
        }
        if let Some(file_patch) = git(
            &top,
            &["diff", "--no-color", "--no-index", "--", "/dev/null", path],
        ) {
            patch.push_str(&file_patch);
        }
    }

    Some(patch)
}
