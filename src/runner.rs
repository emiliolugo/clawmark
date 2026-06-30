#![allow(dead_code)]

use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::cli::{AgentBackend, ValidatedRunArgs, ValidatedVariant};
use crate::report;
use crate::results::{
    append_run_record, harness_path, harness_raw_path, load_run_records, predictions_path,
    write_atomic_json, write_predictions_jsonl, SwebenchPrediction, VariantManifestEntry,
    RUN_RECORDS_FILE, SCHEMA_VERSION, VARIANTS_FILE,
};
use crate::sandbox;
use crate::swebench::{self, Prediction, TaskInstance, SMOKE_INSTANCE_IDS};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VariantId {
    pub index: usize,
    pub label: String,
}

impl VariantId {
    pub fn model_name_or_path(&self) -> String {
        format!("clawmark/{}", self.label)
    }

    pub fn run_id(&self) -> String {
        format!("clawmark-{}", self.label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunKey {
    pub variant: VariantId,
    pub variant_hash: String,
    pub instance_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

enum AgentOutcome {
    Ok(Option<ClaudeUsage>),
    Auth(String),
    Other(String),
}

pub async fn run_all(args: &ValidatedRunArgs) -> Result<(), String> {
    let tasks = swebench::load_bundled_smoke_set()?;
    let invocations = args.variants.len() * tasks.len();
    println!(
        "{} variants x {} tasks x 1 trial = {} agent invocations",
        args.variants.len(),
        tasks.len(),
        invocations
    );

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

    let manifest: Vec<VariantManifestEntry> = args
        .variants
        .iter()
        .map(|v| VariantManifestEntry {
            index: v.index,
            label: v.label.clone(),
            path: v.canonical_path.display().to_string(),
            hash: v.hash.clone(),
            model: v.model.clone(),
            agent: v.agent,
        })
        .collect();
    write_atomic_json(&args.out.join(VARIANTS_FILE), &manifest)?;

    let run_records = args.out.join(RUN_RECORDS_FILE);
    for variant in &args.variants {
        run_variant(variant, &tasks, args, &run_records).await?;
    }
    for variant in &args.variants {
        invoke_harness(&variant.label, &args.out, args.timeout_secs)?;
    }

    let computed = report::compute_report(&args.out)?;
    report::render_terminal_table(&computed);
    print_failure_summary(&computed, &args.out);
    report::write_report_json(&args.out, &computed)?;
    Ok(())
}

async fn run_variant(
    variant: &ValidatedVariant,
    tasks: &[TaskInstance],
    args: &ValidatedRunArgs,
    run_records: &Path,
) -> Result<(), String> {
    println!("== variant {} ==", variant.label);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(args.parallel));
    let write_lock = Arc::new(tokio::sync::Mutex::new(()));
    let predictions = Arc::new(tokio::sync::Mutex::new(Vec::with_capacity(tasks.len())));
    let variant_contents = Arc::new(
        fs::read(&variant.canonical_path)
            .map_err(|e| format!("failed to read variant {}: {e}", variant.label))?,
    );

    let variant_id = VariantId {
        index: variant.index,
        label: variant.label.clone(),
    };
    let variant_hash = variant.hash.clone();
    let model = variant.model.clone();
    let agent = variant.agent;
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
        let variant_id = variant_id.clone();
        let variant_label = variant_id.label.clone();

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore closed");
            let instance_id = task.instance_id.clone();
            println!("[{variant_label}] {instance_id}");

            let record_result = tokio::task::spawn_blocking(move || {
                run_single(&variant_id, &vh, vc.as_slice(), &task, &model, agent, timeout_secs)
            })
            .await
            .map_err(|e| format!("task panicked: {e}"))?;
            let record = record_result?;

            if let Some(err) = &record.error {
                println!("[{variant_label}] {instance_id}: error: {err}");
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
    write_predictions_jsonl(&predictions_path(&args.out, &variant.label), &preds)?;
    Ok(())
}

pub fn run_single(
    variant: &VariantId,
    variant_hash: &str,
    variant_contents: &[u8],
    task: &TaskInstance,
    model: &str,
    agent: AgentBackend,
    timeout_secs: u64,
) -> Result<RunRecord, String> {
    let started = Instant::now();
    let mut error: Option<String> = None;
    let mut patch = String::new();
    let mut usage: Option<ClaudeUsage> = None;

    match sandbox::create(task) {
        Err(clone_error) => error = Some(clone_error),
        Ok(workspace) => {
            if let Err(inject_error) = sandbox::inject_variant(&workspace, variant_contents) {
                error = Some(inject_error);
            } else {
                let outcome = match agent {
                    AgentBackend::Claude => invoke_claude(
                        &workspace.path,
                        model,
                        &task.problem_statement,
                        timeout_secs,
                    ),
                    AgentBackend::Cursor => invoke_cursor(
                        &workspace.path,
                        model,
                        &task.problem_statement,
                        timeout_secs,
                    ),
                };
                match outcome {
                    AgentOutcome::Ok(u) => {
                        usage = u;
                    }
                    AgentOutcome::Auth(message) => {
                        return Err(format!(
                            "{} authentication failure; aborting run: {message}",
                            agent.as_str()
                        ));
                    }
                    AgentOutcome::Other(message) => error = Some(message),
                }

                match sandbox::collect_patch(&workspace) {
                    Ok(collected) => patch = collected,
                    Err(diff_error) => {
                        if error.is_none() {
                            error = Some(diff_error);
                        }
                    }
                }
            }
        }
    }

    let elapsed_secs = started.elapsed().as_secs_f64();
    Ok(RunRecord {
        schema_version: SCHEMA_VERSION,
        key: RunKey {
            variant: variant.clone(),
            variant_hash: variant_hash.to_string(),
            instance_id: task.instance_id.clone(),
        },
        prediction: Prediction {
            instance_id: task.instance_id.clone(),
            model_patch: patch,
            model_name_or_path: variant.model_name_or_path(),
        },
        elapsed_secs,
        error,
        usage,
    })
}

/// Spawn `program` in `cwd` with the given argv, enforcing a wall-clock timeout.
///
/// Returns the collected output on completion, or an error string describing a
/// spawn failure, wait failure, or timeout. The error string is prefixed with
/// `<program> failed: ...` / `<program> timed out ...` so callers can surface it
/// directly as a per-task error.
fn run_process_with_timeout(
    program: &str,
    argv: &[OsString],
    cwd: &Path,
    timeout_secs: u64,
) -> Result<std::process::Output, String> {
    let mut child = Command::new(program)
        .args(argv)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{program} failed: failed to spawn {program}: {e}"))?;

    let timeout = Duration::from_secs(timeout_secs);
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{program} timed out after {timeout_secs}s"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("{program} failed: error waiting for process: {e}")),
        }
    }

    child
        .wait_with_output()
        .map_err(|e| format!("{program} failed: {e}"))
}

fn invoke_claude(
    workspace_path: &Path,
    model: &str,
    problem_statement: &str,
    timeout_secs: u64,
) -> AgentOutcome {
    let argv = claude_argv(model, workspace_path, problem_statement);
    let output = match run_process_with_timeout("claude", &argv, workspace_path, timeout_secs) {
        Ok(output) => output,
        Err(e) => return AgentOutcome::Other(e),
    };

    if output.status.success() {
        return AgentOutcome::Ok(Some(parse_claude_usage(&output.stdout)));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if is_auth_failure(&stderr) {
        return AgentOutcome::Auth(stderr.trim().to_string());
    }
    AgentOutcome::Other(format!(
        "claude failed: {}",
        nonempty_detail(&stderr, output.status)
    ))
}

fn invoke_cursor(
    workspace_path: &Path,
    model: &str,
    problem_statement: &str,
    timeout_secs: u64,
) -> AgentOutcome {
    let argv = cursor_argv(model, problem_statement);
    let output = match run_process_with_timeout("cursor-agent", &argv, workspace_path, timeout_secs)
    {
        Ok(output) => output,
        Err(e) => return AgentOutcome::Other(e),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if is_auth_failure(&stderr) {
            return AgentOutcome::Auth(stderr.trim().to_string());
        }
        return AgentOutcome::Other(format!(
            "cursor-agent failed: {}",
            nonempty_detail(&stderr, output.status)
        ));
    }

    // cursor-agent exits 0 even when the task itself errored; the failure is
    // reported inside the JSON result object. It does not expose token usage.
    if let Some(message) = cursor_error_message(&output.stdout) {
        if is_auth_failure(&message) {
            return AgentOutcome::Auth(message);
        }
        return AgentOutcome::Other(format!("cursor-agent failed: {message}"));
    }
    AgentOutcome::Ok(None)
}

fn nonempty_detail(stderr: &str, status: std::process::ExitStatus) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        format!("{status}")
    } else {
        trimmed.to_string()
    }
}

/// Extract an error message from a cursor-agent JSON result, if the run failed.
///
/// Returns `None` when the JSON is absent, unparseable, or reports success.
fn cursor_error_message(stdout: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<serde_json::Value>(stdout).ok()?;
    let is_error = value
        .get("is_error")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let subtype_error =
        value.get("subtype").and_then(serde_json::Value::as_str) == Some("error");
    if !is_error && !subtype_error {
        return None;
    }
    let message = value
        .get("result")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("cursor-agent reported an error");
    Some(message.to_string())
}

fn is_auth_failure(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("not authenticated")
        || lower.contains("authentication")
        || lower.contains("login")
        || lower.contains("api key")
}

fn claude_argv(model: &str, workspace_path: &Path, problem_statement: &str) -> Vec<OsString> {
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

// cursor-agent operates on the process working directory (set to the cloned
// workspace by the caller), so there is no `--add-dir` equivalent here.
// `--force` bypasses command/permission prompts and `--trust` trusts the freshly
// cloned workspace without prompting, both required for unattended headless runs.
// cursor-agent's JSON output does not include token usage, so cost is reported
// as `n/a` for this backend.
fn cursor_argv(model: &str, problem_statement: &str) -> Vec<OsString> {
    vec![
        OsString::from("-p"),
        OsString::from("--output-format"),
        OsString::from("json"),
        OsString::from("--force"),
        OsString::from("--trust"),
        OsString::from("--model"),
        OsString::from(model),
        OsString::from(problem_statement),
    ]
}

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

pub fn invoke_harness(label: &str, out: &Path, timeout_secs: u64) -> Result<(), String> {
    let harness_dir = out.join("harness");
    let abs_out = out
        .canonicalize()
        .map_err(|e| format!("failed to resolve output directory {}: {e}", out.display()))?;
    let predictions = predictions_path(&abs_out, label);
    let run_id = format!("clawmark-{label}");
    let argv = harness_argv(&predictions, &run_id, timeout_secs);
    let status = Command::new("python3")
        .args(&argv)
        .current_dir(&harness_dir)
        .status()
        .map_err(|e| format!("failed to run swebench harness: {e}"))?;
    if !status.success() {
        return Err(format!(
            "swebench harness failed for variant {label} ({status})"
        ));
    }
    finalize_harness_summary(out, label)
}

fn finalize_harness_summary(out: &Path, label: &str) -> Result<(), String> {
    let raw = harness_raw_path(out, label);
    let stable = harness_path(out, label);
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

fn print_failure_summary(report: &report::Report, out: &Path) {
    let records = load_run_records(&out.join(RUN_RECORDS_FILE)).unwrap_or_default();
    let mut failures = Vec::new();
    for task in &report.per_task {
        for (idx, resolved) in task.resolved.iter().enumerate() {
            if *resolved {
                continue;
            }
            let Some(variant) = report.variants.get(idx) else {
                continue;
            };
            let error = records
                .iter()
                .find(|r| {
                    r.key.variant.label == variant.label && r.key.instance_id == task.instance_id
                })
                .and_then(|r| r.error.clone());
            failures.push((variant.label.clone(), task.instance_id.clone(), error));
        }
    }
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
    fn cursor_argv_has_expected_order_and_values() {
        let argv = cursor_argv("gpt-5", "fix the bug");
        let expected: Vec<OsString> = [
            "-p",
            "--output-format",
            "json",
            "--force",
            "--trust",
            "--model",
            "gpt-5",
            "fix the bug",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        assert_eq!(argv, expected);
    }

    #[test]
    fn cursor_error_message_reads_json_failure() {
        let stdout = br#"{"type":"result","subtype":"error","is_error":true,"result":"Authentication failed"}"#;
        assert_eq!(
            cursor_error_message(stdout),
            Some("Authentication failed".to_string())
        );
    }

    #[test]
    fn cursor_error_message_ignores_success() {
        let stdout = br#"{"type":"result","subtype":"success","is_error":false,"result":"done"}"#;
        assert_eq!(cursor_error_message(stdout), None);
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
        let run_idx = find("--run_id").expect("run_id flag");
        assert_eq!(strings[run_idx + 1], "clawmark-a");
        let timeout_idx = find("--timeout").expect("timeout flag");
        assert_eq!(strings[timeout_idx + 1], "300");
    }

    #[test]
    fn finalize_harness_summary_copies_raw_to_stable() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("harness")).expect("harness dir");
        let raw = harness_raw_path(dir.path(), "a");
        fs::write(raw, r#"{"resolved_ids":["astropy__astropy-12907"]}"#).expect("write raw");
        finalize_harness_summary(dir.path(), "a").expect("finalize");
        let stable = harness_path(dir.path(), "a");
        let contents = fs::read_to_string(stable).expect("read stable");
        assert!(contents.contains("astropy__astropy-12907"));
    }

    #[test]
    fn parse_claude_usage_defaults_on_garbage() {
        let usage = parse_claude_usage(b"not json");
        assert_eq!(usage, ClaudeUsage::default());
    }

    #[test]
    fn variant_id_model_name_or_path_uses_label() {
        let variant = VariantId {
            index: 2,
            label: "gamma".to_string(),
        };
        assert_eq!(variant.model_name_or_path(), "clawmark/gamma");
        assert_eq!(variant.run_id(), "clawmark-gamma");
        let hash = crate::results::variant_hash(b"test");
        assert_eq!(hash.len(), 64);
    }
}
