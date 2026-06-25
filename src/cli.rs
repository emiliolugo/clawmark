use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

pub const MIN_TIMEOUT_SECS: u64 = 1;
pub const MAX_TIMEOUT_SECS: u64 = 86_400;

#[derive(Debug, Parser)]
#[command(
    name = "clawmark",
    about = "A/B test two or more CLAUDE.md files against SWE-bench Lite"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Validate local prerequisites before a run
    Doctor,
    /// Run variants against the bundled smoke set
    Run(RunArgs),
    /// Read existing output and print the leaderboard summary
    Report(ReportArgs),
}

#[derive(Debug, Clone, Parser)]
pub struct RunArgs {
    /// Alias form for two-variant runs only.
    #[arg(long)]
    pub a: Option<PathBuf>,
    /// Alias form for two-variant runs only.
    #[arg(long)]
    pub b: Option<PathBuf>,
    /// Alias form shared model for two-variant runs only.
    #[arg(long)]
    pub model: Option<String>,
    /// Alias form override for variant A.
    #[arg(long)]
    pub model_a: Option<String>,
    /// Alias form override for variant B.
    #[arg(long)]
    pub model_b: Option<String>,
    /// Variant definition in the form <label>=<path>. Repeatable.
    #[arg(long = "variant")]
    pub variants: Vec<String>,
    /// Variant model override in the form <label>=<model>. Repeatable.
    #[arg(long = "variant-model")]
    pub variant_models: Vec<String>,
    #[arg(long)]
    pub timeout_secs: u64,
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long)]
    pub parallel: Option<u16>,
}

#[derive(Debug, Clone, Parser)]
pub struct ReportArgs {
    #[arg(long)]
    pub out: PathBuf,
    /// Print each task's model patch (truncated to 20 lines) after the summary table
    #[arg(long, default_value_t = false)]
    pub show_patches: bool,
}

#[derive(Debug, Clone)]
pub struct ValidatedRunArgs {
    pub variants: Vec<ValidatedVariant>,
    pub timeout_secs: u64,
    pub out: PathBuf,
    pub parallel: usize,
}

#[derive(Debug, Clone)]
pub struct ValidatedVariant {
    pub index: usize,
    pub label: String,
    pub canonical_path: PathBuf,
    pub model: String,
    pub hash: String,
}

impl RunArgs {
    pub fn validate(&self) -> Result<ValidatedRunArgs, String> {
        let cwd =
            env::current_dir().map_err(|e| format!("failed to read current directory: {e}"))?;
        self.validate_with_cwd(&cwd)
    }

    pub fn validate_with_cwd(&self, cwd: &Path) -> Result<ValidatedRunArgs, String> {
        let using_ab_form = self.a.is_some()
            || self.b.is_some()
            || self.model.is_some()
            || self.model_a.is_some()
            || self.model_b.is_some();
        let using_variant_form = !self.variants.is_empty() || !self.variant_models.is_empty();

        if using_ab_form == using_variant_form {
            return Err(
                "choose exactly one input form: either --a/--b/--model (with optional --model-a/--model-b) or repeated --variant/--variant-model"
                    .to_string(),
            );
        }

        if !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&self.timeout_secs) {
            return Err(format!(
                "--timeout-secs must be between {MIN_TIMEOUT_SECS} and {MAX_TIMEOUT_SECS}"
            ));
        }

        validate_run_out_dir(&self.out)?;

        let parallel = match self.parallel {
            None => 1,
            Some(0) => return Err("--parallel must be at least 1".to_string()),
            Some(n) => usize::from(n),
        };

        let variants = if using_ab_form {
            self.validate_ab_form(cwd)?
        } else {
            self.validate_variant_form(cwd)?
        };

        Ok(ValidatedRunArgs {
            variants,
            timeout_secs: self.timeout_secs,
            out: self.out.clone(),
            parallel,
        })
    }

    fn validate_ab_form(&self, cwd: &Path) -> Result<Vec<ValidatedVariant>, String> {
        let a = self
            .a
            .as_ref()
            .ok_or_else(|| "--a is required when using the two-variant alias form".to_string())?;
        let b = self
            .b
            .as_ref()
            .ok_or_else(|| "--b is required when using the two-variant alias form".to_string())?;
        let shared_model = self.model.as_ref().ok_or_else(|| {
            "--model is required when using the two-variant alias form".to_string()
        })?;

        let a_canonical = validate_variant_path(a, cwd)?;
        let b_canonical = validate_variant_path(b, cwd)?;
        if a_canonical == b_canonical {
            return Err("variant paths --a and --b must resolve to different files".to_string());
        }

        let model_a = self.model_a.as_deref().unwrap_or(shared_model).trim();
        let model_b = self.model_b.as_deref().unwrap_or(shared_model).trim();
        if model_a.is_empty() || model_b.is_empty() {
            return Err("--model must be a non-empty string".to_string());
        }

        Ok(vec![
            ValidatedVariant {
                index: 0,
                label: "a".to_string(),
                canonical_path: a_canonical.clone(),
                model: model_a.to_string(),
                hash: compute_variant_hash(&a_canonical)?,
            },
            ValidatedVariant {
                index: 1,
                label: "b".to_string(),
                canonical_path: b_canonical.clone(),
                model: model_b.to_string(),
                hash: compute_variant_hash(&b_canonical)?,
            },
        ])
    }

    fn validate_variant_form(&self, cwd: &Path) -> Result<Vec<ValidatedVariant>, String> {
        if self.a.is_some()
            || self.b.is_some()
            || self.model.is_some()
            || self.model_a.is_some()
            || self.model_b.is_some()
        {
            return Err(
                "--a/--b/--model/--model-a/--model-b cannot be combined with --variant/--variant-model"
                    .to_string(),
            );
        }
        if self.variants.len() < 2 {
            return Err("at least two --variant entries are required".to_string());
        }

        let model_map = parse_variant_models(&self.variant_models)?;
        let mut seen_labels = HashSet::new();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();
        let mut variants = Vec::with_capacity(self.variants.len());

        for (index, raw) in self.variants.iter().enumerate() {
            let (label, path) = parse_label_equals_value(raw, "--variant", "path")?;
            validate_variant_label(&label)?;
            if !seen_labels.insert(label.clone()) {
                return Err(format!("duplicate variant label: {label}"));
            }

            let canonical_path = validate_variant_path(Path::new(&path), cwd)?;
            if !seen_paths.insert(canonical_path.clone()) {
                return Err(format!(
                    "variant path {} resolves to a duplicate canonical file",
                    canonical_path.display()
                ));
            }

            let model = model_map.get(&label).ok_or_else(|| {
                format!(
                    "missing --variant-model for label {label}; provide --variant-model {label}=<model>"
                )
            })?;
            if model.trim().is_empty() {
                return Err(format!("model for variant {label} must be non-empty"));
            }

            variants.push(ValidatedVariant {
                index,
                label,
                canonical_path: canonical_path.clone(),
                model: model.trim().to_string(),
                hash: compute_variant_hash(&canonical_path)?,
            });
        }

        for label in model_map.keys() {
            if !variants.iter().any(|v| &v.label == label) {
                return Err(format!(
                    "--variant-model provided for unknown variant label: {label}"
                ));
            }
        }

        Ok(variants)
    }
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
    Ok(())
}

fn parse_label_equals_value(
    input: &str,
    flag: &str,
    value_name: &str,
) -> Result<(String, String), String> {
    let Some((label, value)) = input.split_once('=') else {
        return Err(format!("{flag} must be in the form <label>={value_name}"));
    };
    let label = label.trim().to_string();
    let value = value.trim().to_string();
    if label.is_empty() || value.is_empty() {
        return Err(format!("{flag} must be in the form <label>={value_name}"));
    }
    Ok((label, value))
}

fn parse_variant_models(raw: &[String]) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::new();
    for item in raw {
        let (label, model) = parse_label_equals_value(item, "--variant-model", "model")?;
        validate_variant_label(&label)?;
        if map.insert(label.clone(), model).is_some() {
            return Err(format!(
                "duplicate --variant-model entry for label: {label}"
            ));
        }
    }
    Ok(map)
}

fn validate_variant_label(label: &str) -> Result<(), String> {
    let mut chars = label.chars();
    let Some(first) = chars.next() else {
        return Err("variant label must not be empty".to_string());
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(format!(
            "invalid variant label {label}: must match ^[a-z0-9][a-z0-9_-]*$"
        ));
    }
    if chars.any(|ch| !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '_' && ch != '-') {
        return Err(format!(
            "invalid variant label {label}: must match ^[a-z0-9][a-z0-9_-]*$"
        ));
    }
    Ok(())
}

fn compute_variant_hash(path: &Path) -> Result<String, String> {
    let contents =
        fs::read(path).map_err(|e| format!("failed to read variant {}: {e}", path.display()))?;
    Ok(crate::results::variant_hash(&contents))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, contents).expect("write test file");
        path
    }

    fn valid_alias_run_args(dir: &Path, a: PathBuf, b: PathBuf, out_name: &str) -> RunArgs {
        RunArgs {
            a: Some(a),
            b: Some(b),
            model: Some("sonnet".to_string()),
            model_a: None,
            model_b: None,
            variants: Vec::new(),
            variant_models: Vec::new(),
            timeout_secs: 300,
            out: dir.join(out_name),
            parallel: None,
        }
    }

    #[test]
    fn run_rejects_identical_variant_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let variant = write_file(dir.path(), "a.md", "variant a");
        let args = valid_alias_run_args(dir.path(), variant.clone(), variant, "out");
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
        let args = valid_alias_run_args(dir.path(), a, b, "out");
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
        let args = valid_alias_run_args(dir.path(), a, b, "out");
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
        let args = valid_alias_run_args(dir.path(), a, b, "out");
        args.validate_with_cwd(dir.path())
            .expect("validation should succeed");
    }

    #[test]
    fn run_rejects_empty_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let args = RunArgs {
            a: Some(a),
            b: Some(b),
            model: Some("   ".to_string()),
            model_a: None,
            model_b: None,
            variants: Vec::new(),
            variant_models: Vec::new(),
            timeout_secs: 300,
            out: dir.path().join("out"),
            parallel: None,
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
            a: Some(a),
            b: Some(b),
            model: Some("sonnet".to_string()),
            model_a: None,
            model_b: None,
            variants: Vec::new(),
            variant_models: Vec::new(),
            timeout_secs: 0,
            out: dir.path().join("out"),
            parallel: None,
        };
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("timeout-secs"));
    }

    #[test]
    fn report_requires_existing_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = ReportArgs {
            out: dir.path().join("missing"),
            show_patches: false,
        };
        let err = args.validate().expect_err("expected validation error");
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn report_accepts_existing_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let args = ReportArgs {
            out: dir.path().to_path_buf(),
            show_patches: false,
        };
        args.validate().expect("validation should succeed");
    }

    #[test]
    fn run_rejects_parallel_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let args = RunArgs {
            a: Some(a),
            b: Some(b),
            model: Some("sonnet".to_string()),
            model_a: None,
            model_b: None,
            variants: Vec::new(),
            variant_models: Vec::new(),
            timeout_secs: 300,
            out: dir.path().join("out"),
            parallel: Some(0),
        };
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("parallel"));
    }

    #[test]
    fn run_accepts_parallel_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let args = RunArgs {
            a: Some(a),
            b: Some(b),
            model: Some("sonnet".to_string()),
            model_a: None,
            model_b: None,
            variants: Vec::new(),
            variant_models: Vec::new(),
            timeout_secs: 300,
            out: dir.path().join("out"),
            parallel: Some(1),
        };
        let validated = args.validate_with_cwd(dir.path()).expect("should accept 1");
        assert_eq!(validated.parallel, 1);
    }

    #[test]
    fn run_defaults_per_variant_models_to_shared() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let args = RunArgs {
            a: Some(a),
            b: Some(b),
            model: Some("sonnet".to_string()),
            model_a: None,
            model_b: None,
            variants: Vec::new(),
            variant_models: Vec::new(),
            timeout_secs: 300,
            out: dir.path().join("out"),
            parallel: None,
        };
        let validated = args
            .validate_with_cwd(dir.path())
            .expect("validation should succeed");
        assert_eq!(validated.variants[0].model, "sonnet");
        assert_eq!(validated.variants[1].model, "sonnet");
    }

    #[test]
    fn run_overrides_model_b() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let args = RunArgs {
            a: Some(a),
            b: Some(b),
            model: Some("sonnet".to_string()),
            model_a: None,
            model_b: Some("haiku".to_string()),
            variants: Vec::new(),
            variant_models: Vec::new(),
            timeout_secs: 300,
            out: dir.path().join("out"),
            parallel: None,
        };
        let validated = args
            .validate_with_cwd(dir.path())
            .expect("validation should succeed");
        assert_eq!(validated.variants[0].model, "sonnet");
        assert_eq!(validated.variants[1].model, "haiku");
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
        let args = valid_alias_run_args(dir.path(), link, b, "out");
        args.validate_with_cwd(dir.path())
            .expect("symlinked regular file should validate");
    }

    #[test]
    fn variant_form_requires_two_entries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let args = RunArgs {
            a: None,
            b: None,
            model: None,
            model_a: None,
            model_b: None,
            variants: vec![format!("alpha={}", a.display())],
            variant_models: vec!["alpha=sonnet".to_string()],
            timeout_secs: 300,
            out: dir.path().join("out"),
            parallel: None,
        };
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("at least two --variant"));
    }

    #[test]
    fn variant_form_rejects_duplicate_labels() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let args = RunArgs {
            a: None,
            b: None,
            model: None,
            model_a: None,
            model_b: None,
            variants: vec![
                format!("alpha={}", a.display()),
                format!("alpha={}", b.display()),
            ],
            variant_models: vec!["alpha=sonnet".to_string()],
            timeout_secs: 300,
            out: dir.path().join("out"),
            parallel: None,
        };
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("duplicate variant label"));
    }

    #[test]
    fn variant_form_rejects_duplicate_canonical_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let args = RunArgs {
            a: None,
            b: None,
            model: None,
            model_a: None,
            model_b: None,
            variants: vec![
                format!("alpha={}", a.display()),
                format!("beta={}", a.display()),
            ],
            variant_models: vec!["alpha=sonnet".to_string(), "beta=haiku".to_string()],
            timeout_secs: 300,
            out: dir.path().join("out"),
            parallel: None,
        };
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("duplicate canonical file"));
    }

    #[test]
    fn variant_form_rejects_invalid_label_slug() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let args = RunArgs {
            a: None,
            b: None,
            model: None,
            model_a: None,
            model_b: None,
            variants: vec![
                format!("Alpha={}", a.display()),
                format!("beta={}", b.display()),
            ],
            variant_models: vec!["Alpha=sonnet".to_string(), "beta=haiku".to_string()],
            timeout_secs: 300,
            out: dir.path().join("out"),
            parallel: None,
        };
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("must match"));
    }

    #[test]
    fn variant_form_requires_model_for_each_label() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let args = RunArgs {
            a: None,
            b: None,
            model: None,
            model_a: None,
            model_b: None,
            variants: vec![
                format!("alpha={}", a.display()),
                format!("beta={}", b.display()),
            ],
            variant_models: vec!["alpha=sonnet".to_string()],
            timeout_secs: 300,
            out: dir.path().join("out"),
            parallel: None,
        };
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("missing --variant-model for label beta"));
    }

    #[test]
    fn forms_are_mutually_exclusive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let a = write_file(dir.path(), "a.md", "a");
        let b = write_file(dir.path(), "b.md", "b");
        let args = RunArgs {
            a: Some(a.clone()),
            b: Some(b.clone()),
            model: Some("sonnet".to_string()),
            model_a: None,
            model_b: None,
            variants: vec![
                format!("alpha={}", a.display()),
                format!("beta={}", b.display()),
            ],
            variant_models: vec!["alpha=sonnet".to_string(), "beta=haiku".to_string()],
            timeout_secs: 300,
            out: dir.path().join("out"),
            parallel: None,
        };
        let err = args
            .validate_with_cwd(dir.path())
            .expect_err("expected validation error");
        assert!(err.contains("exactly one input form"));
    }
}
