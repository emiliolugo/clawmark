#![allow(dead_code)]

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::cli::ValidatedRunArgs;
use crate::report;
use crate::results::{
    append_run_record, harness_path, harness_raw_path, load_run_records, predictions_path,
    variant_hash, write_predictions_jsonl, SwebenchPrediction, RUN_RECORDS_FILE, SCHEMA_VERSION,
};
use crate::sandbox;
use crate::swebench::{self, Prediction, TaskInstance, SMOKE_INSTANCE_IDS};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VariantSlot {
    A,
    B,
}

impl VariantSlot {
    pub fn label(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }

    pub fn model_name_or_path(self) -> &'static str {
        match self {
            Self::A => "clawmark/a",
            Self::B => "clawmark/b",
        }
    }

    fn run_id(self) -> &'static str {
        match self {
            Self::A => "clawmark-a",
            Self::B => "clawmark-b",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunKey {
    pub variant: VariantSlot,
    pub variant_hash: String,
    pub instance_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClaudeUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub schema_version: u32,
    pub key: RunKey,
    pub prediction: Prediction,
    pub elapsed_secs: f64,
    pub error: Option<String>,
    #[serde(default)]
    pub usage: Option<ClaudeUsage>,
}

/// Outcome of a single `claude` invocation.
///
/// `Auth` aborts the entire run. `Other` is a recoverable per-task error that is
/// stored on the `RunRecord` so the run can continue with the next task.
enum ClaudeOutcome {
    Ok(ClaudeUsage),
    Auth(String),
    Other(String),
}

/// Orchestrate the full A/B run: variant A then variant B, serially.
pub async fn run_ab(args: &ValidatedRunArgs) -> Result<(), String> {
    let tasks = swebench::load_bundled_smoke_set()?;

    let a_contents = fs::read(&args.a_canonical).map_err(|e| {
        format!(
            "failed to read variant A {}: {e}",
            args.a_canonical.display()
        )
    })?;
    let b_contents = fs::read(&args.b_canonical).map_err(|e| {
        format!(
            "failed to read variant B {}: {e}",
            args.b_canonical.display()
        )
    })?;
    let a_hash = variant_hash(&a_contents);
    let b_hash = variant_hash(&b_contents);

    let invocations = 2 * tasks.len();
    println!(
        "2 variants x {} tasks x 1 trial = {} Claude invocations",
        tasks.len(),
        invocations
    );

    // Create the output directory only after validation has passed. `create_dir`
    // fails if it already exists, guarding against a race after validation.
    fs::create_dir(&args.out).map_err(|e| {
        format!(
            "failed to create output directory {}: {e}",
            args.out.display()
        )
    })?;
    fs::create_dir(args.out.join("predictions"))
        .map_err(|e| format!("failed to create predictions directory: {e}"))?;
    fs::create_dir(args.out.join("harness"))
        .map_err(|e| format!("failed to create harness directory: {e}"))?;

    let run_records = args.out.join(RUN_RECORDS_FILE);

    run_variant(
        VariantSlot::A,
        &a_hash,
        &a_contents,
        &tasks,
        args,
        &run_records,
    )
    .await?;
    run_variant(
        VariantSlot::B,
        &b_hash,
        &b_contents,
        &tasks,
        args,
        &run_records,
    )
    .await?;

    invoke_harness(VariantSlot::A, &args.out, args.timeout_secs)?;
    invoke_harness(VariantSlot::B, &args.out, args.timeout_secs)?;

    let report = report::compute_report(&args.out)?;
    report::render_terminal_table(&report);
    print_failure_summary(&report, &args.out);
    report::write_report_json(&args.out, &report)?;

    Ok(())
}

async fn run_variant(
    variant: VariantSlot,
    variant_hash: &str,
    variant_contents: &[u8],
    tasks: &[TaskInstance],
    args: &ValidatedRunArgs,
    run_records: &Path,
) -> Result<(), String> {
    println!("== variant {} ==", variant.label());
    let parallel = args.parallel;

    let semaphore = Arc::new(tokio::sync::Semaphore::new(parallel));
    let write_lock = Arc::new(tokio::sync::Mutex::new(()));
    let predictions = Arc::new(tokio::sync::Mutex::new(Vec::with_capacity(tasks.len())));
    let variant_contents = Arc::new(variant_contents.to_vec());
    let variant_hash = variant_hash.to_string();
    let model = match variant {
        VariantSlot::A => args.model_a.clone(),
        VariantSlot::B => args.model_b.clone(),
    };
    let timeout_secs = args.timeout_secs;
    let run_records = run_records.to_path_buf();

    let mut handles = Vec::with_capacity(tasks.len());

    for task in tasks {
        let task = task.clone();
        let sem = Arc::clone(&semaphore);
        let write_lock = Arc::clone(&write_lock);
        let predictions = Arc::clone(&predictions);
        let vc = Arc::clone(&variant_contents);
        let vh = variant_hash.clone();
        let model = model.clone();
        let run_records = run_records.clone();

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let instance_id = task.instance_id.clone();
            println!("[{}] {}", variant.label(), instance_id);

            let record_result = tokio::task::spawn_blocking(move || {
                run_single(variant, &vh, vc.as_slice(), &task, &model, timeout_secs)
            })
            .await
            .map_err(|e| format!("task panicked: {e}"))?;
            let record = record_result?;

            if let Some(err) = &record.error {
                println!("[{}] {}: error: {err}", variant.label(), instance_id);
            }

            {
                let _guard = write_lock.lock().await;
                append_run_record(&run_records, &record)?;
            }

            predictions
                .lock()
                .await
                .push(SwebenchPrediction::from(&record.prediction));

            Ok::<(), String>(())
        });

        handles.push(handle);
    }

    let mut first_err: Option<String> = None;
    for handle in handles {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(format!("task panicked: {e}"));
                }
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }

    let preds = Arc::try_unwrap(predictions)
        .expect("predictions Arc should have no other owners")
        .into_inner();
    write_predictions_jsonl(&predictions_path(&args.out, variant), &preds)?;
    Ok(())
}

/// Run one variant against one task: clone, inject `CLAUDE.md`, invoke Claude,
/// then collect the patch via `git diff HEAD`.
///
/// Returns `Ok(RunRecord)` for both success and per-task failures (the error is
/// stored on the record). Returns `Err` only when the entire run must abort
/// (Claude authentication failure or an output write failure upstream).
pub fn run_single(
    variant: VariantSlot,
    variant_hash: &str,
    variant_contents: &[u8],
    task: &TaskInstance,
    model: &str,
    timeout_secs: u64,
) -> Result<RunRecord, String> {
    let started = Instant::now();
    let mut error: Option<String> = None;
    let mut patch = String::new();
    let mut usage: Option<ClaudeUsage> = None;

    match sandbox::create(task) {
        Err(clone_error) => error = Some(clone_error),
        Ok(workspace) => {
            if let Err(inject_error) = sandbox::inject_claude_md(&workspace, variant_contents) {
                error = Some(inject_error);
            } else {
                match invoke_claude(
                    &workspace.path,
                    model,
                    &task.problem_statement,
                    timeout_secs,
                ) {
                    ClaudeOutcome::Ok(u) => {
                        usage = Some(u);
                    }
                    ClaudeOutcome::Auth(message) => {
                        return Err(format!(
                            "Claude authentication failure; aborting run: {message}"
                        ));
                    }
                    ClaudeOutcome::Other(message) => error = Some(message),
                }

                // Always collect the patch after Claude exits, even on a
                // per-task Claude error. An empty diff is a valid unresolved
                // result; we never parse model text for a patch.
                match sandbox::collect_patch(&workspace) {
                    Ok(collected) => patch = collected,
                    Err(diff_error) => {
                        if error.is_none() {
                            error = Some(diff_error);
                        }
                    }
                }
            }
            // `workspace` (and its TempDir) drops here.
        }
    }

    let elapsed_secs = started.elapsed().as_secs_f64();
    Ok(RunRecord {
        schema_version: SCHEMA_VERSION,
        key: RunKey {
            variant,
            variant_hash: variant_hash.to_string(),
            instance_id: task.instance_id.clone(),
        },
        prediction: Prediction {
            instance_id: task.instance_id.clone(),
            model_patch: patch,
            model_name_or_path: variant.model_name_or_path().to_string(),
        },
        elapsed_secs,
        error,
        usage,
    })
}

fn invoke_claude(
    workspace_path: &Path,
    model: &str,
    problem_statement: &str,
    timeout_secs: u64,
) -> ClaudeOutcome {
    let argv = claude_argv(model, workspace_path, problem_statement);
    let spawn = Command::new("claude")
        .args(&argv)
        .current_dir(workspace_path)
        .stdin(Stdio::null())
        // The JSON result is small (well under the OS pipe buffer), so draining
        // only after exit is safe.
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match spawn {
        Ok(child) => child,
        Err(e) => {
            return ClaudeOutcome::Other(format!("claude failed: failed to spawn claude: {e}"))
        }
    };

    let timeout = Duration::from_secs(timeout_secs);
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return ClaudeOutcome::Other(format!("claude timed out after {timeout_secs}s"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return ClaudeOutcome::Other(format!(
                    "claude failed: error waiting for process: {e}"
                ))
            }
        }
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(e) => return ClaudeOutcome::Other(format!("claude failed: {e}")),
    };

    if output.status.success() {
        return ClaudeOutcome::Ok(parse_claude_usage(&output.stdout));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if is_auth_failure(&stderr) {
        return ClaudeOutcome::Auth(stderr.trim().to_string());
    }

    let detail = {
        let trimmed = stderr.trim();
        if trimmed.is_empty() {
            format!("{}", output.status)
        } else {
            trimmed.to_string()
        }
    };
    ClaudeOutcome::Other(format!("claude failed: {detail}"))
}

/// Detect a Claude authentication failure from stderr text.
///
/// A match aborts the entire run, since every subsequent invocation would fail
/// the same way.
fn is_auth_failure(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("not authenticated")
        || lower.contains("authentication")
        || lower.contains("login")
        || lower.contains("api key")
}

fn claude_argv(model: &str, workspace_path: &Path, problem_statement: &str) -> Vec<OsString> {
    // `--add-dir` is variadic, so it must not directly precede the positional
    // prompt or it swallows the prompt as another directory. The trailing `--`
    // terminates option parsing so the problem statement is always received as
    // the prompt, even if it starts with `-`.
    //
    // `--bare` is intentionally NOT used: it disables CLAUDE.md auto-discovery
    // (which defeats clawmark's variant injection) and forces ANTHROPIC_API_KEY
    // auth, ignoring the user's OAuth/keychain login.
    vec![
        OsString::from("-p"),
        OsString::from("--output-format"),
        OsString::from("json"),
        OsString::from("--dangerously-skip-permissions"),
        OsString::from("--model"),
        OsString::from(model),
        OsString::from("--add-dir"),
        workspace_path.as_os_str().to_os_string(),
        OsString::from("--"),
        OsString::from(problem_statement),
    ]
}

/// Parse `ClaudeUsage` from the raw stdout bytes of a `claude -p --output-format json`
/// invocation. Never fails: any parse error returns `ClaudeUsage::default()`.
fn parse_claude_usage(stdout: &[u8]) -> ClaudeUsage {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(stdout) else {
        return ClaudeUsage::default();
    };
    let input_tokens = v
        .get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let output_tokens = v
        .get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let cache_read_input_tokens = v
        .get("usage")
        .and_then(|u| u.get("cache_read_input_tokens"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let cache_creation_input_tokens = v
        .get("usage")
        .and_then(|u| u.get("cache_creation_input_tokens"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let cost_usd = v.get("total_cost_usd").and_then(serde_json::Value::as_f64);
    ClaudeUsage {
        input_tokens,
        output_tokens,
        cache_read_input_tokens,
        cache_creation_input_tokens,
        cost_usd,
    }
}

/// Invoke the SWE-bench harness once for a variant, then copy the raw summary
/// to the stable `harness/a.json` / `harness/b.json` path.
pub fn invoke_harness(variant: VariantSlot, out: &Path, timeout_secs: u64) -> Result<(), String> {
    let harness_dir = out.join("harness");
    let abs_out = out
        .canonicalize()
        .map_err(|e| format!("failed to resolve output directory {}: {e}", out.display()))?;
    let predictions = predictions_path(&abs_out, variant);

    let argv = harness_argv(&predictions, variant.run_id(), timeout_secs);
    let status = Command::new("python3")
        .args(&argv)
        .current_dir(&harness_dir)
        .status()
        .map_err(|e| format!("failed to run swebench harness: {e}"))?;

    if !status.success() {
        return Err(format!(
            "swebench harness failed for variant {} ({status})",
            variant.label()
        ));
    }

    finalize_harness_summary(out, variant)
}

/// Copy the harness raw summary to the stable clawmark path.
///
/// Fails clearly if the raw summary is missing so the caller aborts before
/// writing `report.json`.
fn finalize_harness_summary(out: &Path, variant: VariantSlot) -> Result<(), String> {
    let raw = harness_raw_path(out, variant);
    let stable = harness_path(out, variant);
    if !raw.is_file() {
        return Err(format!(
            "swebench harness raw summary missing: {}",
            raw.display()
        ));
    }
    fs::copy(&raw, &stable).map_err(|e| {
        format!(
            "failed to copy harness summary {} -> {}: {e}",
            raw.display(),
            stable.display()
        )
    })?;
    Ok(())
}

/// Build the list of `(variant_label, instance_id, error)` tuples for every
/// `(variant, task)` pair that the harness did not resolve.
///
/// Extracted as a pure function so it can be unit-tested without I/O.
fn collect_failure_entries(
    report: &report::Report,
    records: &[RunRecord],
) -> Vec<(String, String, Option<String>)> {
    let mut failures = Vec::new();
    for task in &report.tasks {
        for (label, resolved) in [("a", task.a_resolved), ("b", task.b_resolved)] {
            if !resolved {
                let error = records
                    .iter()
                    .find(|r| {
                        r.key.variant.label() == label && r.key.instance_id == task.instance_id
                    })
                    .and_then(|r| r.error.clone());
                failures.push((label.to_string(), task.instance_id.clone(), error));
            }
        }
    }
    failures
}

/// Print a failure summary after a run completes.
///
/// Prints nothing when every (variant, task) pair was resolved. Loads run
/// records best-effort; a missing file silently produces an empty record list.
fn print_failure_summary(report: &report::Report, out: &Path) {
    let records = load_run_records(&out.join(RUN_RECORDS_FILE)).unwrap_or_default();
    let failures = collect_failure_entries(report, &records);

    if failures.is_empty() {
        return;
    }

    println!();
    println!("Failure summary:");
    for (label, instance_id, error) in failures {
        if let Some(err) = error {
            println!("  [{label}] {instance_id}: {err}");
        } else {
            println!("  [{label}] {instance_id}: unresolved (patch did not pass tests)");
        }
    }
}

fn harness_argv(predictions: &Path, run_id: &str, timeout_secs: u64) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec![
        OsString::from("-m"),
        OsString::from("swebench.harness.run_evaluation"),
        OsString::from("--dataset_name"),
        OsString::from("princeton-nlp/SWE-bench_Lite"),
        OsString::from("--split"),
        OsString::from("test"),
        OsString::from("--predictions_path"),
        predictions.as_os_str().to_os_string(),
        OsString::from("--instance_ids"),
    ];
    for id in SMOKE_INSTANCE_IDS {
        argv.push(OsString::from(id));
    }
    argv.push(OsString::from("--max_workers"));
    argv.push(OsString::from("1"));
    argv.push(OsString::from("--run_id"));
    argv.push(OsString::from(run_id));
    argv.push(OsString::from("--timeout"));
    argv.push(OsString::from(timeout_secs.to_string()));
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_failure_detected_case_insensitively() {
        assert!(is_auth_failure("Error: Not Authenticated"));
        assert!(is_auth_failure("please run authentication flow"));
        assert!(is_auth_failure("you must LOGIN first"));
        assert!(is_auth_failure("invalid API Key provided"));
    }

    #[test]
    fn non_auth_errors_not_flagged() {
        assert!(!is_auth_failure("model not available"));
        assert!(!is_auth_failure("rate limit exceeded"));
        assert!(!is_auth_failure("request timed out"));
        assert!(!is_auth_failure(""));
    }

    #[test]
    fn claude_argv_has_expected_order_and_values() {
        let argv = claude_argv("sonnet", Path::new("/tmp/ws"), "fix the bug");
        let expected: Vec<OsString> = [
            "-p",
            "--output-format",
            "json",
            "--dangerously-skip-permissions",
            "--model",
            "sonnet",
            "--add-dir",
            "/tmp/ws",
            "--",
            "fix the bug",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        assert_eq!(argv, expected);
    }

    #[test]
    fn harness_argv_contains_required_flags_and_instances() {
        let argv = harness_argv(Path::new("/abs/out/predictions/a.jsonl"), "clawmark-a", 300);
        let strings: Vec<String> = argv
            .iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect();

        assert_eq!(strings[0], "-m");
        assert_eq!(strings[1], "swebench.harness.run_evaluation");

        let find = |flag: &str| strings.iter().position(|s| s == flag);
        let pred_idx = find("--predictions_path").expect("predictions_path flag");
        assert_eq!(strings[pred_idx + 1], "/abs/out/predictions/a.jsonl");

        let dataset_idx = find("--dataset_name").expect("dataset flag");
        assert_eq!(strings[dataset_idx + 1], "princeton-nlp/SWE-bench_Lite");

        let split_idx = find("--split").expect("split flag");
        assert_eq!(strings[split_idx + 1], "test");

        let run_idx = find("--run_id").expect("run_id flag");
        assert_eq!(strings[run_idx + 1], "clawmark-a");

        let workers_idx = find("--max_workers").expect("max_workers flag");
        assert_eq!(strings[workers_idx + 1], "1");

        let timeout_idx = find("--timeout").expect("timeout flag");
        assert_eq!(strings[timeout_idx + 1], "300");

        // All five smoke instance IDs appear, in order, after --instance_ids.
        let ids_idx = find("--instance_ids").expect("instance_ids flag");
        for (offset, id) in SMOKE_INSTANCE_IDS.iter().enumerate() {
            assert_eq!(&strings[ids_idx + 1 + offset], id);
        }
    }

    #[test]
    fn collect_failure_entries_empty_when_all_resolved() {
        use crate::report::aggregate_report;
        use crate::results::HarnessResult;
        use crate::swebench::SMOKE_INSTANCE_IDS;

        let all_ids: Vec<String> = SMOKE_INSTANCE_IDS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let a = HarnessResult {
            resolved_ids: all_ids.clone(),
        };
        let b = HarnessResult {
            resolved_ids: all_ids,
        };
        let report = aggregate_report(&a, &b);
        assert!(collect_failure_entries(&report, &[]).is_empty());
    }

    #[test]
    fn collect_failure_entries_includes_error_from_record() {
        use crate::report::aggregate_report;
        use crate::results::HarnessResult;
        use crate::swebench::Prediction;

        let a = HarnessResult {
            resolved_ids: vec![],
        };
        let b = HarnessResult {
            resolved_ids: vec![],
        };
        let report = aggregate_report(&a, &b);

        let record = RunRecord {
            schema_version: SCHEMA_VERSION,
            key: RunKey {
                variant: VariantSlot::A,
                variant_hash: "abc".to_string(),
                instance_id: "astropy__astropy-12907".to_string(),
            },
            prediction: Prediction {
                instance_id: "astropy__astropy-12907".to_string(),
                model_patch: String::new(),
                model_name_or_path: "clawmark/a".to_string(),
            },
            elapsed_secs: 1.0,
            error: Some("claude timed out after 300s".to_string()),
            usage: None,
        };

        let failures = collect_failure_entries(&report, &[record]);
        let entry = failures
            .iter()
            .find(|(l, id, _)| l == "a" && id == "astropy__astropy-12907");
        assert!(entry.is_some());
        let (_, _, err) = entry.unwrap();
        assert_eq!(err.as_deref(), Some("claude timed out after 300s"));
    }

    #[test]
    fn collect_failure_entries_marks_no_error_when_record_missing() {
        use crate::report::aggregate_report;
        use crate::results::HarnessResult;

        let a = HarnessResult {
            resolved_ids: vec![],
        };
        let b = HarnessResult {
            resolved_ids: vec![],
        };
        let report = aggregate_report(&a, &b);

        // No records provided — error should be None (unresolved but no claude error)
        let failures = collect_failure_entries(&report, &[]);
        let entry = failures
            .iter()
            .find(|(l, id, _)| l == "a" && id == "astropy__astropy-12907");
        assert!(entry.is_some());
        let (_, _, err) = entry.unwrap();
        assert!(err.is_none());
    }

    #[test]
    fn finalize_harness_summary_copies_raw_to_stable() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("harness")).expect("harness dir");
        let raw = harness_raw_path(dir.path(), VariantSlot::A);
        fs::write(raw, r#"{"resolved_ids":["astropy__astropy-12907"]}"#).expect("write raw");

        finalize_harness_summary(dir.path(), VariantSlot::A).expect("finalize");

        let stable = harness_path(dir.path(), VariantSlot::A);
        let contents = fs::read_to_string(stable).expect("read stable");
        assert!(contents.contains("astropy__astropy-12907"));
    }

    #[test]
    fn finalize_harness_summary_errors_when_raw_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("harness")).expect("harness dir");
        let err = finalize_harness_summary(dir.path(), VariantSlot::B)
            .expect_err("missing raw should error");
        assert!(err.contains("raw summary missing"));
    }

    #[test]
    fn parse_claude_usage_reads_fields() {
        let input = br#"{"total_cost_usd":0.0123,"usage":{"input_tokens":100,"output_tokens":200,"cache_read_input_tokens":5,"cache_creation_input_tokens":7}}"#;
        let usage = parse_claude_usage(input);
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 200);
        assert_eq!(usage.cache_read_input_tokens, 5);
        assert_eq!(usage.cache_creation_input_tokens, 7);
        assert_eq!(usage.cost_usd, Some(0.0123));
    }

    #[test]
    fn parse_claude_usage_defaults_on_garbage() {
        let usage = parse_claude_usage(b"not json");
        assert_eq!(usage, ClaudeUsage::default());
    }

    #[test]
    fn parse_claude_usage_missing_usage_object() {
        let input = br#"{"result":"ok"}"#;
        let usage = parse_claude_usage(input);
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_read_input_tokens, 0);
        assert_eq!(usage.cache_creation_input_tokens, 0);
        assert_eq!(usage.cost_usd, None);
    }
}
