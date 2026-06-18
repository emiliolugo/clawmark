#![allow(dead_code)]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

use crate::swebench::{self, TaskInstance};

/// A cloned SWE-bench repo living inside a `TempDir`.
///
/// `path` points at the cloned repository directory. The repository is always
/// nested inside the temp directory, so it is removed when the `Workspace`
/// (and its `TempDir`) is dropped.
pub struct Workspace {
    _temp_dir: TempDir,
    pub path: PathBuf,
}

/// Create a temp workspace by cloning `task.repo` at `task.base_commit`.
///
/// The repo slug and base commit are validated before any URL is constructed or
/// any argument is passed to git. All git invocations use argv arrays — never a
/// shell string. Clone or checkout failures are returned as plain error strings
/// so the caller can record them as per-task errors.
pub fn create(task: &TaskInstance) -> Result<Workspace, String> {
    swebench::validate_repo_slug(&task.repo)?;
    swebench::validate_base_commit(&task.base_commit)?;

    let temp_dir = TempDir::new().map_err(|e| format!("failed to create temp workspace: {e}"))?;
    let repo_path = temp_dir.path().join("repo");

    let url = format!("https://github.com/{}", task.repo);
    git_checked(
        &[OsStr::new("clone"), OsStr::new(&url), repo_path.as_os_str()],
        None,
        "git clone",
    )?;

    git_checked(
        &[OsStr::new("checkout"), OsStr::new(&task.base_commit)],
        Some(&repo_path),
        "git checkout",
    )?;

    Ok(Workspace {
        _temp_dir: temp_dir,
        path: repo_path,
    })
}

/// Write the variant file contents as `CLAUDE.md` at the root of the cloned repo.
///
/// Uses `std::fs::write`, never a subprocess.
pub fn inject_claude_md(workspace: &Workspace, variant_contents: &[u8]) -> Result<(), String> {
    let target = workspace.path.join("CLAUDE.md");
    std::fs::write(&target, variant_contents)
        .map_err(|e| format!("failed to write {}: {e}", target.display()))
}

/// Collect the ground-truth patch via `git diff HEAD`.
///
/// An empty diff is a valid result and means the task is unresolved.
pub fn collect_patch(workspace: &Workspace) -> Result<String, String> {
    git_diff_head(&workspace.path)
}

fn git_diff_head(repo: &Path) -> Result<String, String> {
    let output = git_output(&[OsStr::new("diff"), OsStr::new("HEAD")], Some(repo))?;
    if !output.status.success() {
        return Err(format!(
            "git diff HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_checked(args: &[&OsStr], cwd: Option<&Path>, label: &str) -> Result<(), String> {
    let output = git_output(args, cwd)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{label} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn git_output(args: &[&OsStr], cwd: Option<&Path>) -> Result<Output, String> {
    let mut command = Command::new("git");
    let owned: Vec<OsString> = args.iter().map(|a| (*a).to_os_string()).collect();
    command.args(&owned);
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    command
        .output()
        .map_err(|e| format!("failed to run git: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .expect("run git");
        assert!(status.status.success(), "git {args:?} failed");
    }

    fn init_repo(repo: &Path) {
        run_git(repo, &["init", "-q"]);
        run_git(repo, &["config", "user.email", "test@example.com"]);
        run_git(repo, &["config", "user.name", "test"]);
        std::fs::write(repo.join("file.txt"), "hello\n").expect("write file");
        run_git(repo, &["add", "."]);
        run_git(repo, &["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn collect_patch_is_empty_for_clean_repo() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        init_repo(dir.path());
        let diff = git_diff_head(dir.path()).expect("diff");
        assert!(diff.is_empty(), "clean repo should produce empty diff");
    }

    #[test]
    fn collect_patch_reports_changes() {
        if !git_available() {
            return;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        init_repo(dir.path());
        std::fs::write(dir.path().join("file.txt"), "hello world\n").expect("modify file");
        let diff = git_diff_head(dir.path()).expect("diff");
        assert!(
            diff.contains("diff --git"),
            "modified repo should produce a diff"
        );
        assert!(diff.contains("hello world"));
    }

    #[test]
    fn inject_claude_md_writes_file_in_repo() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let repo = temp_dir.path().join("repo");
        std::fs::create_dir(&repo).expect("repo dir");
        let workspace = Workspace {
            _temp_dir: temp_dir,
            path: repo.clone(),
        };
        inject_claude_md(&workspace, b"variant contents").expect("inject");
        let read = std::fs::read(repo.join("CLAUDE.md")).expect("read");
        assert_eq!(read, b"variant contents");
    }
}
