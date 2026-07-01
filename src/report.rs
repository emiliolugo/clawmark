use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::results::{
    harness_path, load_harness_results, load_run_meta, load_run_records, load_variants_manifest,
    write_atomic_json, HarnessResult, REPORT_FILE, RUN_META_FILE, RUN_RECORDS_FILE, SCHEMA_VERSION,
    V1_HARNESS_A_FILE, V1_HARNESS_B_FILE, VARIANTS_FILE,
};
use crate::runner::RunRecord;
use crate::stats::{mcnemar_exact_p, wilson_interval, Z_95};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRow {
    pub instance_id: String,
    /// resolved trial count per variant, aligned with `Report.variants` order
    pub resolved_counts: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantSummary {
    pub label: String,
    pub model: String,
    pub resolved: usize,
    pub n_invocations: usize,
    pub resolve_rate: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    pub elapsed_secs: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
    pub cost_per_resolve: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairStat {
    pub a_label: String,
    pub b_label: String,
    pub a_only: usize,
    pub b_only: usize,
    pub both: usize,
    pub neither: usize,
    pub p_value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub total_tasks: usize,
    pub trials: u32,
    pub variants: Vec<VariantSummary>,
    pub per_task: Vec<TaskRow>,
    pub pairwise: Vec<PairStat>,
}

#[allow(clippy::module_name_repetitions)]
pub fn compute_report(out: &Path) -> Result<Report, String> {
    let manifest_path = out.join(VARIANTS_FILE);
    if !manifest_path.is_file() {
        if out.join(V1_HARNESS_A_FILE).is_file() && out.join(V1_HARNESS_B_FILE).is_file() {
            return Err("run produced with clawmark v1; re-run to get a v1.1 report".to_string());
        }
        return Err(format!(
            "expected output file missing: {}",
            manifest_path.display()
        ));
    }

    let variants = load_variants_manifest(&manifest_path)?;
    if variants.len() < 2 {
        return Err("variants.json must contain at least two variants".to_string());
    }

    let meta_path = out.join(RUN_META_FILE);
    if !meta_path.is_file() {
        return Err(
            "run produced with an older clawmark (schema < 4); re-run to get a current report"
                .to_string(),
        );
    }
    let meta = load_run_meta(&meta_path)?;
    if meta.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported run schema_version {} (expected {SCHEMA_VERSION}); re-run",
            meta.schema_version
        ));
    }

    let mut harnesses = Vec::with_capacity(variants.len());
    for variant in &variants {
        let mut trial_results = Vec::with_capacity(meta.trials as usize);
        for trial in 1..=meta.trials {
            trial_results.push(load_harness_results(&harness_path(
                out,
                &variant.label,
                trial,
            ))?);
        }
        harnesses.push((variant.label.clone(), trial_results));
    }

    let records = load_run_records(&out.join(RUN_RECORDS_FILE)).unwrap_or_default();
    Ok(aggregate_report(
        &variants
            .iter()
            .map(|v| (v.label.clone(), v.model.clone()))
            .collect::<Vec<_>>(),
        &harnesses,
        &records,
        &meta.instance_ids,
        meta.trials,
    ))
}

#[allow(clippy::module_name_repetitions, clippy::too_many_lines)]
pub fn aggregate_report(
    variants: &[(String, String)],
    harnesses: &[(String, Vec<HarnessResult>)],
    records: &[RunRecord],
    instance_ids: &[String],
    trials: u32,
) -> Report {
    let total_tasks = instance_ids.len();
    let n_invocations = total_tasks * trials as usize;

    // resolved_sets[variant_idx][trial_idx] = set of resolved instance ids for that trial
    let resolved_sets: Vec<Vec<HashSet<&str>>> = harnesses
        .iter()
        .map(|(_, trial_results)| {
            trial_results
                .iter()
                .map(|h| h.resolved_ids.iter().map(String::as_str).collect())
                .collect()
        })
        .collect();

    // resolved_counts_matrix[variant_idx][task_idx] = number of resolved trials
    let mut resolved_counts_matrix: Vec<Vec<u32>> = vec![vec![0; total_tasks]; harnesses.len()];
    for (v_idx, trial_sets) in resolved_sets.iter().enumerate() {
        for (task_idx, instance_id) in instance_ids.iter().enumerate() {
            let count = trial_sets
                .iter()
                .filter(|set| set.contains(instance_id.as_str()))
                .count();
            resolved_counts_matrix[v_idx][task_idx] = u32::try_from(count).unwrap_or(u32::MAX);
        }
    }

    let mut per_task = Vec::with_capacity(total_tasks);
    for (task_idx, instance_id) in instance_ids.iter().enumerate() {
        let resolved_counts = resolved_counts_matrix
            .iter()
            .map(|row| row[task_idx])
            .collect::<Vec<_>>();
        per_task.push(TaskRow {
            instance_id: instance_id.clone(),
            resolved_counts,
        });
    }

    let mut summaries = Vec::with_capacity(variants.len());
    for (v_idx, (label, model)) in variants.iter().enumerate() {
        let resolved: usize = resolved_counts_matrix[v_idx]
            .iter()
            .map(|&c| c as usize)
            .sum();
        let resolve_rate = usize_to_f64(resolved) / usize_to_f64(n_invocations);
        let (ci_low, ci_high) =
            wilson_interval(usize_to_u64(resolved), usize_to_u64(n_invocations), Z_95);

        let mut elapsed_secs = 0.0_f64;
        let mut input_tokens = 0_u64;
        let mut output_tokens = 0_u64;
        let mut cost_usd: Option<f64> = None;
        for record in records.iter().filter(|r| r.key.variant.label == *label) {
            elapsed_secs += record.elapsed_secs;
            if let Some(usage) = &record.usage {
                input_tokens += usage.input_tokens;
                output_tokens += usage.output_tokens;
                if let Some(c) = usage.cost_usd {
                    cost_usd = Some(cost_usd.unwrap_or(0.0) + c);
                }
            }
        }
        let cost_per_resolve = match (cost_usd, resolved) {
            (Some(cost), r) if r > 0 => Some(cost / usize_to_f64(r)),
            _ => None,
        };

        summaries.push(VariantSummary {
            label: label.clone(),
            model: model.clone(),
            resolved,
            n_invocations,
            resolve_rate,
            ci_low,
            ci_high,
            elapsed_secs,
            input_tokens,
            output_tokens,
            cost_usd,
            cost_per_resolve,
        });
    }

    summaries.sort_by(|a, b| {
        b.resolve_rate
            .partial_cmp(&a.resolve_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| match (a.cost_per_resolve, b.cost_per_resolve) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.label.cmp(&b.label))
    });

    let old_index_by_label: HashMap<String, usize> = variants
        .iter()
        .enumerate()
        .map(|(idx, (label, _))| (label.clone(), idx))
        .collect();
    let sorted_indices = summaries
        .iter()
        .filter_map(|summary| old_index_by_label.get(&summary.label).copied())
        .collect::<Vec<_>>();
    for row in &mut per_task {
        let reordered = sorted_indices
            .iter()
            .map(|idx| row.resolved_counts[*idx])
            .collect::<Vec<_>>();
        row.resolved_counts = reordered;
    }

    let mut pairwise = Vec::new();
    for i in 0..summaries.len() {
        for j in (i + 1)..summaries.len() {
            let a_label = summaries[i].label.clone();
            let b_label = summaries[j].label.clone();
            let a_orig_idx = old_index_by_label[&a_label];
            let b_orig_idx = old_index_by_label[&b_label];

            let mut a_only = 0_usize;
            let mut b_only = 0_usize;
            let mut both = 0_usize;
            let mut neither = 0_usize;
            for instance_id in instance_ids {
                for trial_idx in 0..trials as usize {
                    let a_resolved = resolved_sets[a_orig_idx]
                        .get(trial_idx)
                        .is_some_and(|set| set.contains(instance_id.as_str()));
                    let b_resolved = resolved_sets[b_orig_idx]
                        .get(trial_idx)
                        .is_some_and(|set| set.contains(instance_id.as_str()));
                    match (a_resolved, b_resolved) {
                        (true, true) => both += 1,
                        (true, false) => a_only += 1,
                        (false, true) => b_only += 1,
                        (false, false) => neither += 1,
                    }
                }
            }

            let p_value = mcnemar_exact_p(usize_to_u64(a_only), usize_to_u64(b_only));
            pairwise.push(PairStat {
                a_label,
                b_label,
                a_only,
                b_only,
                both,
                neither,
                p_value,
            });
        }
    }

    Report {
        schema_version: SCHEMA_VERSION,
        total_tasks,
        trials,
        variants: summaries,
        per_task,
        pairwise,
    }
}

fn usize_to_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).unwrap_or(u32::MAX))
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

pub fn render_terminal_table(report: &Report) {
    println!("clawmark leaderboard");
    println!("--------------------");
    println!(
        "total tasks: {}, trials: {}, invocations per variant: {}",
        report.total_tasks,
        report.trials,
        report.total_tasks * report.trials as usize
    );
    println!();
    println!("rank  variant  resolved  rate    95% CI            cost/resolve");
    for (idx, variant) in report.variants.iter().enumerate() {
        let cost_per_resolve = variant
            .cost_per_resolve
            .map_or_else(|| "n/a".to_string(), |v| format!("{v:.4}"));
        let ci = format!(
            "[{:.1}%, {:.1}%]",
            variant.ci_low * 100.0,
            variant.ci_high * 100.0
        );
        println!(
            "{:>4}  {:<7}  {:>3}/{:<3}   {:>5.1}%  {:<17} {}",
            idx + 1,
            variant.label,
            variant.resolved,
            variant.n_invocations,
            variant.resolve_rate * 100.0,
            ci,
            cost_per_resolve
        );
    }

    println!();
    println!(
        "per-task matrix (variant order above, resolved trials out of {}):",
        report.trials
    );
    for row in &report.per_task {
        let cells = row
            .resolved_counts
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        println!("{}: {}", row.instance_id, cells);
    }

    if !report.pairwise.is_empty() {
        println!();
        println!("pairwise (McNemar exact, paired by task+trial):");
        for pair in &report.pairwise {
            let p = pair.p_value.map_or_else(
                || "n/a (no discordant pairs)".to_string(),
                |v| format!("{v:.4}"),
            );
            println!(
                "{} vs {}: a-only={} b-only={} both={} neither={} p={}",
                pair.a_label, pair.b_label, pair.a_only, pair.b_only, pair.both, pair.neither, p
            );
        }
    }
}

pub fn write_report_json(out: &Path, report: &Report) -> Result<(), String> {
    write_atomic_json(&out.join(REPORT_FILE), report)
}

pub fn render_patches(out: &Path) -> Result<(), String> {
    let records = load_run_records(&out.join(RUN_RECORDS_FILE))?;
    let variants = load_variants_manifest(&out.join(VARIANTS_FILE))?;
    let meta = load_run_meta(&out.join(RUN_META_FILE))?;

    println!();
    println!("Patches:");
    println!("---------");
    for instance_id in &meta.instance_ids {
        println!("{instance_id}");
        for variant in &variants {
            for trial in 1..=meta.trials {
                println!("  [{} trial {trial}]:", variant.label);
                let patch = records
                    .iter()
                    .find(|r| {
                        r.key.variant.label == variant.label
                            && &r.key.instance_id == instance_id
                            && r.key.trial == trial
                    })
                    .map_or("", |r| r.prediction.model_patch.as_str());

                if patch.is_empty() {
                    println!("    (empty patch)");
                    continue;
                }
                let mut line_count = 0usize;
                for line in patch.lines().take(20) {
                    println!("    {line}");
                    line_count += 1;
                }
                let total = patch.lines().count();
                if total > line_count {
                    println!("    ... ({} more lines)", total - line_count);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::HarnessResult;
    use crate::runner::{ClaudeUsage, RunKey, RunRecord, VariantId};
    use crate::swebench::Prediction;

    fn harness(resolved: &[&str]) -> HarnessResult {
        HarnessResult {
            resolved_ids: resolved.iter().map(|id| (*id).to_string()).collect(),
        }
    }

    fn bundled_ids() -> Vec<String> {
        crate::swebench::SMOKE_INSTANCE_IDS
            .iter()
            .map(|id| (*id).to_string())
            .collect()
    }

    fn make_record(
        label: &str,
        trial: u32,
        instance_id: &str,
        elapsed_secs: f64,
        usage: Option<ClaudeUsage>,
    ) -> RunRecord {
        RunRecord {
            schema_version: SCHEMA_VERSION,
            key: RunKey {
                variant: VariantId {
                    index: 0,
                    label: label.to_string(),
                },
                variant_hash: "abc".to_string(),
                instance_id: instance_id.to_string(),
                trial,
            },
            prediction: Prediction {
                instance_id: instance_id.to_string(),
                model_patch: String::new(),
                model_name_or_path: format!("clawmark/{label}"),
            },
            elapsed_secs,
            error: None,
            usage,
        }
    }

    #[test]
    fn report_aggregates_three_variants() {
        let variants = vec![
            ("a".to_string(), "sonnet".to_string()),
            ("b".to_string(), "haiku".to_string()),
            ("c".to_string(), "opus".to_string()),
        ];
        let harnesses = vec![
            ("a".to_string(), vec![harness(&["astropy__astropy-12907"])]),
            (
                "b".to_string(),
                vec![harness(&[
                    "astropy__astropy-12907",
                    "astropy__astropy-14182",
                ])],
            ),
            ("c".to_string(), vec![harness(&[])]),
        ];
        let report = aggregate_report(&variants, &harnesses, &[], &bundled_ids(), 1);
        assert_eq!(report.total_tasks, 5);
        assert_eq!(report.trials, 1);
        assert_eq!(report.variants.len(), 3);
        assert_eq!(report.per_task.len(), 5);
        assert!(report.variants[0].resolve_rate >= report.variants[1].resolve_rate);
    }

    #[test]
    fn leaderboard_tiebreak_uses_cost_per_resolve_then_label() {
        let variants = vec![
            ("a".to_string(), "m1".to_string()),
            ("b".to_string(), "m2".to_string()),
        ];
        let harnesses = vec![
            ("a".to_string(), vec![harness(&["astropy__astropy-12907"])]),
            ("b".to_string(), vec![harness(&["astropy__astropy-12907"])]),
        ];
        let records = vec![
            make_record(
                "a",
                1,
                "astropy__astropy-12907",
                1.0,
                Some(ClaudeUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cost_usd: Some(2.0),
                }),
            ),
            make_record(
                "b",
                1,
                "astropy__astropy-12907",
                1.0,
                Some(ClaudeUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cost_usd: Some(1.0),
                }),
            ),
        ];
        let report = aggregate_report(&variants, &harnesses, &records, &bundled_ids(), 1);
        assert_eq!(report.variants[0].label, "b");
    }

    #[test]
    fn v1_directory_rejected_with_clear_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("harness")).expect("harness");
        std::fs::write(
            dir.path().join(V1_HARNESS_A_FILE),
            r#"{"resolved_ids":["astropy__astropy-12907"]}"#,
        )
        .expect("a");
        std::fs::write(
            dir.path().join(V1_HARNESS_B_FILE),
            r#"{"resolved_ids":["astropy__astropy-12907"]}"#,
        )
        .expect("b");
        let err = compute_report(dir.path()).expect_err("expected v1 rejection");
        assert!(err.contains("run produced with clawmark v1"));
    }

    #[test]
    fn report_rejects_missing_run_meta() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manifest = vec![
            crate::results::VariantManifestEntry {
                index: 0,
                label: "a".to_string(),
                path: "a.md".to_string(),
                hash: "hash-a".to_string(),
                model: "sonnet".to_string(),
                agent: crate::cli::AgentBackend::Claude,
            },
            crate::results::VariantManifestEntry {
                index: 1,
                label: "b".to_string(),
                path: "b.md".to_string(),
                hash: "hash-b".to_string(),
                model: "sonnet".to_string(),
                agent: crate::cli::AgentBackend::Claude,
            },
        ];
        write_atomic_json(&dir.path().join(VARIANTS_FILE), &manifest).expect("write manifest");
        let err = compute_report(dir.path()).expect_err("expected missing run_meta rejection");
        assert!(err.contains("older clawmark"));
    }

    #[test]
    fn aggregate_report_counts_trials() {
        // 2 variants x 2 trials x 2 tasks. Variant a resolves task-1 in both
        // trials and task-2 in one trial only.
        let variants = vec![
            ("a".to_string(), "sonnet".to_string()),
            ("b".to_string(), "haiku".to_string()),
        ];
        let instance_ids = vec![
            "astropy__astropy-12907".to_string(),
            "astropy__astropy-14182".to_string(),
        ];
        let harnesses = vec![
            (
                "a".to_string(),
                vec![
                    harness(&["astropy__astropy-12907", "astropy__astropy-14182"]),
                    harness(&["astropy__astropy-12907"]),
                ],
            ),
            ("b".to_string(), vec![harness(&[]), harness(&[])]),
        ];
        let report = aggregate_report(&variants, &harnesses, &[], &instance_ids, 2);
        assert_eq!(report.total_tasks, 2);
        assert_eq!(report.trials, 2);
        let a_summary = report
            .variants
            .iter()
            .find(|v| v.label == "a")
            .expect("variant a");
        assert_eq!(a_summary.resolved, 3);
        assert_eq!(a_summary.n_invocations, 4);
        assert!(a_summary.ci_low < a_summary.resolve_rate);
        assert!(a_summary.resolve_rate < a_summary.ci_high);

        let task1 = report
            .per_task
            .iter()
            .find(|t| t.instance_id == "astropy__astropy-12907")
            .expect("task 1");
        let a_idx = report
            .variants
            .iter()
            .position(|v| v.label == "a")
            .expect("a index");
        let b_idx = report
            .variants
            .iter()
            .position(|v| v.label == "b")
            .expect("b index");
        assert_eq!(task1.resolved_counts[a_idx], 2);
        assert_eq!(task1.resolved_counts[b_idx], 0);

        let task2 = report
            .per_task
            .iter()
            .find(|t| t.instance_id == "astropy__astropy-14182")
            .expect("task 2");
        assert_eq!(task2.resolved_counts[a_idx], 1);
    }

    #[test]
    fn pairwise_counts_and_p_value() {
        // 5 tasks x 1 trial: a_only=1, b_only=0, both=1, neither=3.
        let variants = vec![
            ("a".to_string(), "sonnet".to_string()),
            ("b".to_string(), "haiku".to_string()),
        ];
        let instance_ids = bundled_ids();
        let harnesses = vec![
            (
                "a".to_string(),
                vec![harness(&[
                    "astropy__astropy-12907",
                    "astropy__astropy-14182",
                ])],
            ),
            ("b".to_string(), vec![harness(&["astropy__astropy-14182"])]),
        ];
        let report = aggregate_report(&variants, &harnesses, &[], &instance_ids, 1);
        assert_eq!(report.pairwise.len(), 1);
        let pair = &report.pairwise[0];
        assert_eq!(pair.a_only, 1);
        assert_eq!(pair.b_only, 0);
        assert_eq!(pair.both, 1);
        assert_eq!(pair.neither, 3);
        assert_eq!(pair.p_value, Some(1.0));
    }
}
