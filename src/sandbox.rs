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

/// What to overlay into a cloned workspace for one variant.
///
/// `File` is a single CLAUDE.md written at the repo root (legacy single-file variant). `Dir` is
/// a directory whose entire contents are copied into the repo root, allowing a full `.claude/`
/// config (settings.json, commands, agents, skills, hooks) plus an optional CLAUDE.md.
#[derive(Debug)]
pub enum VariantSource {
    File(Vec<u8>),
    Dir(PathBuf),
}

/// Overlay a variant onto the cloned repo before invoking Claude.
///
/// For `File`, the bytes are written verbatim to `<repo>/CLAUDE.md`. For `Dir`, the directory's
/// contents are copied recursively into the repo root. A top-level `.git` entry is skipped so the
/// clone history is never clobbered, and symlinks are ignored. Injected files are untracked in the
/// clone, so they never appear in `git diff HEAD`.
pub fn inject_variant(workspace: &Workspace, source: &VariantSource) -> Result<(), String> {
    match source {
        VariantSource::File(bytes) => {
            let target = workspace.path.join("CLAUDE.md");
            std::fs::write(&target, bytes)
                .map_err(|e| format!("failed to write {}: {e}", target.display()))
        }
        VariantSource::Dir(src) => copy_dir_recursive(src, &workspace.path, true),
    }
}

/// Recursively copy the *contents* of `src` into `dst` (not `src` itself). Creates intermediate
/// directories as needed. At the root level, a `.git` entry is skipped. Symlinks and other special
/// files are ignored. Existing files in `dst` with the same relative path are overwritten.
fn copy_dir_recursive(src: &Path, dst: &Path, is_root: bool) -> Result<(), String> {
    let entries = std::fs::read_dir(src)
        .map_err(|e| format!("failed to read variant directory {}: {e}", src.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read entry in {}: {e}", src.display()))?;
        let file_name = entry.file_name();
        if is_root && file_name.to_str() == Some(".git") {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&file_name);
        let file_type = entry
            .file_type()
            .map_err(|e| format!("failed to stat {}: {e}", from.display()))?;
        if file_type.is_dir() {
            std::fs::create_dir_all(&to)
                .map_err(|e| format!("failed to create {}: {e}", to.display()))?;
            copy_dir_recursive(&from, &to, false)?;
        } else if file_type.is_file() {
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
            }
            std::fs::copy(&from, &to).map_err(|e| {
                format!("failed to copy {} -> {}: {e}", from.display(), to.display())
            })?;
        }
    }
    Ok(())
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
    fn inject_variant_file_writes_claude_md() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let repo = temp_dir.path().join("repo");
        std::fs::create_dir(&repo).expect("repo dir");
        let workspace = Workspace {
            _temp_dir: temp_dir,
            path: repo.clone(),
        };
        inject_variant(
            &workspace,
            &VariantSource::File(b"variant contents".to_vec()),
        )
        .expect("inject");
        let read = std::fs::read(repo.join("CLAUDE.md")).expect("read");
        assert_eq!(read, b"variant contents");
    }

    #[test]
    fn inject_variant_dir_copies_tree_and_skips_git() {
        let src_dir = tempfile::tempdir().expect("src tempdir");
        let src = src_dir.path();

        // Create variant directory structure
        std::fs::write(src.join("CLAUDE.md"), b"variant dir contents").expect("write CLAUDE.md");
        std::fs::create_dir(src.join(".claude")).expect(".claude dir");
        std::fs::write(src.join(".claude").join("settings.json"), b"{}")
            .expect("write settings.json");
        std::fs::create_dir(src.join(".claude").join("commands")).expect("commands dir");
        std::fs::write(
            src.join(".claude").join("commands").join("foo.md"),
            b"foo command",
        )
        .expect("write foo.md");
        // This should be skipped
        std::fs::create_dir(src.join(".git")).expect(".git dir");
        std::fs::write(src.join(".git").join("config"), b"[core]").expect("write .git/config");

        let temp_dir = tempfile::tempdir().expect("repo tempdir");
        let repo = temp_dir.path().join("repo");
        std::fs::create_dir(&repo).expect("repo dir");
        let workspace = Workspace {
            _temp_dir: temp_dir,
            path: repo.clone(),
        };

        inject_variant(&workspace, &VariantSource::Dir(src.to_path_buf())).expect("inject dir");

        // These should exist
        let claude_md = std::fs::read(repo.join("CLAUDE.md")).expect("read CLAUDE.md");
        assert_eq!(claude_md, b"variant dir contents");
        let settings =
            std::fs::read(repo.join(".claude").join("settings.json")).expect("read settings.json");
        assert_eq!(settings, b"{}");
        let foo_cmd = std::fs::read(repo.join(".claude").join("commands").join("foo.md"))
            .expect("read foo.md");
        assert_eq!(foo_cmd, b"foo command");

        // .git must NOT have been copied
        assert!(
            !repo.join(".git").exists(),
            ".git must not be copied into repo"
        );
    }
}
