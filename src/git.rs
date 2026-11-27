//! Thin wrappers around the `git` CLI. Everything here blocks, so callers run
//! it on the background executor.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
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

/// Run git in `cwd` on `machine` (`None`: here); its stdout.
fn git(machine: Option<&str>, cwd: &Path, args: &[&str]) -> Option<String> {
    let argv: Vec<&str> = std::iter::once("git").chain(args.iter().copied()).collect();
    let output = crate::exec::run(machine, cwd, &argv, &[("GIT_OPTIONAL_LOCKS", "0")]).ok()?;
    // `git diff --no-index` exits with 1 when files differ.
    if !matches!(output.code, Some(0 | 1)) {
        return None;
    }
    Some(output.stdout)
}

/// The work tree `root` is in, on `machine` (`None`: here).
pub fn toplevel(machine: Option<&str>, root: &Path) -> Option<PathBuf> {
    git(machine, root, &["rev-parse", "--show-toplevel"])
        .map(|s| PathBuf::from(s.trim()))
        .filter(|top| !top.as_os_str().is_empty())
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectGit {
    pub repo: PathBuf,
    pub worktrees: PathBuf,
    pub path_separator: char,
    pub branch: Option<String>,
    pub branches: Vec<String>,
}

impl ProjectGit {
    pub fn read(project: &Path) -> Option<Self> {
        let repo = toplevel(None, project)?;
        let worktrees = crate::paths::worktree_dir(&repo, 0).parent()?.to_path_buf();
        let branch = git_checked(project, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .ok()
            .map(|branch| branch.trim().to_string());
        let branches = git_checked(project, &["for-each-ref", "--format=%(refname:strip=2)", "refs/heads"])
            .ok()?
            .lines()
            .map(str::to_owned)
            .collect();
        Some(Self {
            repo,
            worktrees,
            path_separator: std::path::MAIN_SEPARATOR,
            branch,
            branches,
        })
    }
}

pub fn switch_branch(project: &Path, branch: &str) -> Result<(), String> {
    let reference = format!("refs/heads/{branch}");
    git_checked(project, &["show-ref", "--verify", "--", &reference])?;
    git_checked(project, &["switch", "--no-guess", "--", branch]).map(|_| ())
}

pub fn snapshot(machine: Option<&str>, root: &Path) -> Option<RepoSnapshot> {
    let top = toplevel(machine, root)?;
    let branch = git(machine, root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let raw = git(
        machine,
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
/// including untracked files, on `machine` (`None`: here).
pub fn working_tree_diff(machine: Option<&str>, root: &Path) -> Option<String> {
    let top = toplevel(machine, root)?;
    let mut patch = git(machine, &top, &["diff", "HEAD", "--no-color", "--no-ext-diff"])
        // A repository without commits has no HEAD to compare against.
        .or_else(|| git(machine, &top, &["diff", "--no-color", "--no-ext-diff"]))
        .unwrap_or_default();

    let untracked = git(machine, &top, &["ls-files", "--others", "--exclude-standard", "-z"])
        .unwrap_or_default();
    let untracked: Vec<&str> = untracked
        .split('\0')
        .filter(|s| !s.is_empty())
        .take(MAX_UNTRACKED_DIFF_FILES)
        .collect();
    if machine.is_some() {
        // One round trip for all of them: sizes are checked over there.
        if !untracked.is_empty() {
            let script = format!(
                "for f; do [ \"$(wc -c <\"$f\")\" -le {MAX_UNTRACKED_DIFF_BYTES} ] && \
                 git diff --no-color --no-index -- /dev/null \"$f\"; done; true"
            );
            let argv: Vec<&str> = ["sh", "-c", &script, "sh"].into_iter().chain(untracked).collect();
            if let Ok(output) = crate::exec::run(machine, &top, &argv, &[("GIT_OPTIONAL_LOCKS", "0")]) {
                patch.push_str(&output.stdout);
            }
        }
        return Some(patch);
    }
    for path in untracked {
        let size = std::fs::metadata(top.join(path)).map(|m| m.len()).unwrap_or(0);
        if size > MAX_UNTRACKED_DIFF_BYTES {
            continue;
        }
        if let Some(file_patch) = git(
            None,
            &top,
            &["diff", "--no-color", "--no-index", "--", "/dev/null", path],
        ) {
            patch.push_str(&file_patch);
        }
    }

    Some(patch)
}

/// Run git and return its stdout, or its stderr as the error.
fn git_checked(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let output = crate::command("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|err| format!("cannot run git: {err}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Check out `branch` of `repo` at `path`, creating it from `base` or `HEAD`
/// if it does not already exist.
pub fn add_worktree(repo: &Path, path: &Path, branch: &str, base: Option<&str>) -> Result<(), String> {
    let base = base.map(|branch| format!("refs/heads/{branch}"));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
    }
    // Forget worktrees whose folders are gone, so their branches free up.
    let _ = git_checked(repo, &["worktree", "prune"]);
    let path = path.to_string_lossy();
    let exists = git_checked(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_ok();
    let args: Vec<&str> = if exists {
        vec!["worktree", "add", &path, branch]
    } else {
        vec!["worktree", "add", "-b", branch, &path, base.as_deref().unwrap_or("HEAD")]
    };
    git_checked(repo, &args).map(|_| ())
}

/// Remove a worktree and its branch, unless either holds work that would be
/// lost: a worktree with uncommitted changes stays, and so does a branch
/// with commits not merged anywhere.
pub fn remove_worktree(repo: &Path, path: &Path, branch: &str) -> Result<(), String> {
    if path.exists() {
        git_checked(repo, &["worktree", "remove", &path.to_string_lossy()])?;
    } else {
        let _ = git_checked(repo, &["worktree", "prune"]);
    }
    git_checked(repo, &["branch", "-d", branch]).map(|_| ())
}

#[cfg(test)]
mod worktree_tests {
    use super::*;

    #[test]
    fn select_branch_for_project_and_worktree() {
        let dir = std::env::temp_dir().join(format!("codeany-branch-{}", uuid::Uuid::new_v4()));
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            &["init", "-b", "main"][..],
            &["config", "user.email", "test@example.com"],
            &["config", "user.name", "Test"],
            &["commit", "--allow-empty", "-m", "main"],
            &["switch", "-c", "feature"],
            &["commit", "--allow-empty", "-m", "feature"],
        ] {
            git_checked(&repo, args).unwrap();
        }
        let selected = git_checked(&repo, &["rev-parse", "HEAD"]).unwrap();
        switch_branch(&repo, "main").unwrap();

        let git = ProjectGit::read(&repo).unwrap();
        assert_eq!(git.branch.as_deref(), Some("main"));
        assert_eq!(git.branches, ["feature", "main"]);

        let worktree = dir.join("worktree");
        add_worktree(&repo, &worktree, "codeany/test", Some("feature")).unwrap();
        assert_eq!(git_checked(&worktree, &["rev-parse", "HEAD"]).unwrap(), selected);
        assert_eq!(ProjectGit::read(&repo).unwrap().branch.as_deref(), Some("main"));
        assert!(switch_branch(&repo, "missing").is_err());
        switch_branch(&repo, "feature").unwrap();
        assert_eq!(git_checked(&repo, &["rev-parse", "HEAD"]).unwrap(), selected);
        remove_worktree(&repo, &worktree, "codeany/test").unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn add_and_remove_worktree() {
        let dir = std::env::temp_dir().join(format!("codeany-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            &["init", "-q"][..],
            &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "init"],
        ] {
            git_checked(&repo, args).unwrap();
        }
        let wt = dir.join("worktrees/repo/1");
        add_worktree(&repo, &wt, "codeany/test", None).unwrap();
        assert!(wt.join(".git").exists());
        assert_eq!(toplevel(None, &wt).unwrap().canonicalize().unwrap(), wt.canonicalize().unwrap());

        // A removed folder comes back on the same branch.
        std::fs::remove_dir_all(&wt).unwrap();
        add_worktree(&repo, &wt, "codeany/test", None).unwrap();
        assert!(wt.join(".git").exists());

        // Uncommitted work keeps the worktree.
        std::fs::write(wt.join("a.txt"), "x").unwrap();
        assert!(remove_worktree(&repo, &wt, "codeany/test").is_err());
        assert!(wt.exists());
        std::fs::remove_file(wt.join("a.txt")).unwrap();
        remove_worktree(&repo, &wt, "codeany/test").unwrap();
        assert!(!wt.exists());
        assert!(git_checked(&repo, &["rev-parse", "--verify", "--quiet", "refs/heads/codeany/test"]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
