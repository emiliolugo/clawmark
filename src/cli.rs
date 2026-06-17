use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

pub const MIN_TIMEOUT_SECS: u64 = 1;
pub const MAX_TIMEOUT_SECS: u64 = 86_400;

#[derive(Debug, Parser)]
#[command(
    name = "clawmark",
    about = "A/B test two CLAUDE.md files against SWE-bench Lite"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Validate local prerequisites before a run
    Doctor,
    /// Run two CLAUDE.md variants against the bundled smoke set
    Run(RunArgs),
    /// Read existing output and print an A vs B summary
    Report(ReportArgs),
}

#[derive(Debug, Clone, Parser)]
pub struct RunArgs {
    #[arg(long)]
    pub a: PathBuf,
    #[arg(long)]
    pub b: PathBuf,
    #[arg(long)]
    pub model: String,
    #[arg(long)]
    pub timeout_secs: u64,
    #[arg(long)]
    pub out: PathBuf,
}

#[derive(Debug, Clone, Parser)]
pub struct ReportArgs {
    #[arg(long)]
    pub out: PathBuf,
}

impl RunArgs {
    pub fn validate(&self) -> Result<ValidatedRunArgs, String> {
        let cwd =
            env::current_dir().map_err(|e| format!("failed to read current directory: {e}"))?;
        self.validate_with_cwd(&cwd)
    }

    pub fn validate_with_cwd(&self, cwd: &Path) -> Result<ValidatedRunArgs, String> {
        let a_canonical = validate_variant_path(&self.a, cwd)?;
        let b_canonical = validate_variant_path(&self.b, cwd)?;

        if a_canonical == b_canonical {
            return Err("variant paths --a and --b must resolve to different files".to_string());
        }

        let model = self.model.trim();
        if model.is_empty() {
            return Err("--model must be a non-empty string".to_string());
        }

        if !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&self.timeout_secs) {
            return Err(format!(
                "--timeout-secs must be between {MIN_TIMEOUT_SECS} and {MAX_TIMEOUT_SECS}"
            ));
        }

        validate_run_out_dir(&self.out)?;

        Ok(ValidatedRunArgs {
            a_canonical,
            b_canonical,
            model: model.to_string(),
            timeout_secs: self.timeout_secs,
            out: self.out.clone(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ValidatedRunArgs {
    pub a_canonical: PathBuf,
    pub b_canonical: PathBuf,
    pub model: String,
    pub timeout_secs: u64,
    pub out: PathBuf,
}

impl ReportArgs {
    pub fn validate(&self) -> Result<(), String> {
        validate_report_out_dir(&self.out)
    }
}

fn validate_variant_path(path: &Path, cwd: &Path) -> Result<PathBuf, String> {
    let metadata =
        fs::metadata(path).map_err(|e| format!("variant path {}: {e}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "variant path {} must be a regular file",
            path.display()
        ));
    }

    let canonical = path
        .canonicalize()
        .map_err(|e| format!("variant path {}: {e}", path.display()))?;
    let cwd_canonical = cwd
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize current directory: {e}"))?;

    if !canonical.starts_with(cwd_canonical) {
        return Err(format!(
            "variant path {} must be inside the current working directory",
            path.display()
        ));
    }

    Ok(canonical)
}

fn validate_run_out_dir(out: &Path) -> Result<(), String> {
    if out.exists() {
        return Err(format!(
            "output directory {} already exists; choose a new --out path",
            out.display()
        ));
    }

    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| format!("output directory {} has no parent", out.display()))?;

    if !parent.exists() {
        return Err(format!(
            "parent directory {} does not exist",
            parent.display()
        ));
    }

    Ok(())
}

fn validate_report_out_dir(out: &Path) -> Result<(), String> {
    if !out.is_dir() {
        return Err(format!("output directory {} does not exist", out.display()));
    }

    for relative in crate::results::REPORT_REQUIRED_FILES {
        let path = out.join(relative);
        if !path.is_file() {
            return Err(format!("expected output file missing: {}", path.display()));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).expect("write test file");
        path
    }

    fn valid_run_args(dir: &Path, a: PathBuf, b: PathBuf, out_name: &str) -> RunArgs {
        RunArgs {
            a,
            b,
            model: "sonnet".to_string(),
            timeout_secs: 300,
            out: dir.join(out_name),
        }
    }

    #[test]
    fn run_rejects_identical_variant_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let variant = write_file(dir.path(), "a.md", "variant a");
        let args = valid_run_args(dir.path(), variant.clone(), variant, "out");
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("different files"));
    }

    #[test]
    fn run_rejects_variant_outside_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(outside.path(), "b.md", "b");
        let args = valid_run_args(dir.path(), a, b, "out");
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("inside the current working directory"));
    }

    #[test]
    fn run_rejects_existing_output_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let out = dir.path().join("out");
        fs::create_dir(out).expect("create out");
        let args = valid_run_args(dir.path(), a, b, "out");
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("already exists"));
    }

    #[test]
    fn run_accepts_two_distinct_variant_paths_inside_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let args = valid_run_args(dir.path(), a, b, "out");
        args.validate_with_cwd(dir.path())
            .expect("validation should succeed");
    }

    #[test]
    fn run_rejects_empty_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let args = RunArgs {
            a,
            b,
            model: "   ".to_string(),
            timeout_secs: 300,
            out: dir.path().join("out"),
        };
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("non-empty"));
    }

    #[test]
    fn run_rejects_invalid_timeout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let args = RunArgs {
            a,
            b,
            model: "sonnet".to_string(),
            timeout_secs: 0,
            out: dir.path().join("out"),
        };
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("timeout-secs"));
    }

    #[test]
    fn report_requires_existing_harness_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = ReportArgs {
            out: dir.path().to_path_buf(),
        };
        let err = args.validate().expect_err("expected validation error");
        assert!(err.contains("missing"));
    }

    #[test]
    fn report_accepts_directory_with_required_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("harness")).expect("harness dir");
        write_file(dir.path(), "harness/a.json", r#"{"resolved_ids":[]}"#);
        write_file(dir.path(), "harness/b.json", r#"{"resolved_ids":[]}"#);
        let args = ReportArgs {
            out: dir.path().to_path_buf(),
        };
        args.validate().expect("validation should succeed");
    }

    #[test]
    fn run_follows_symlinks_to_regular_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = write_file(dir.path(), "real.md", "content");
        let link = dir.path().join("link.md");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(real, &link).expect("symlink");
        }
        #[cfg(not(unix))]
        {
            fs::copy(&real, &link).expect("copy fallback");
        }
        let b = write_file(dir.path(), "b.md", "b");
        let args = valid_run_args(dir.path(), link, b, "out");
        args.validate_with_cwd(dir.path())
            .expect("symlinked regular file should validate");
    }
}
