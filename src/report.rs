use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::results::{
    harness_path, load_harness_results, load_run_records, load_variants_manifest,
    write_atomic_json, HarnessResult, REPORT_FILE, RUN_RECORDS_FILE, SCHEMA_VERSION,
    V1_HARNESS_A_FILE, V1_HARNESS_B_FILE, VARIANTS_FILE,
};
use crate::runner::RunRecord;
use crate::swebench::SMOKE_INSTANCE_IDS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRow {
    pub instance_id: String,
    pub resolved: Vec<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantSummary {
    pub label: String,
    pub model: String,
    pub resolved: usize,
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
    pub a_wins: usize,
    pub b_wins: usize,
    pub b_count: usize,
    pub c_count: usize,
    pub p_value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub total_tasks: usize,
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

    let mut harnesses = Vec::with_capacity(variants.len());
    for variant in &variants {
        harnesses.push((
            variant.label.clone(),
            load_harness_results(&harness_path(out, &variant.label))?,
        ));
    }

    let records = load_run_records(&out.join(RUN_RECORDS_FILE)).unwrap_or_default();
    Ok(aggregate_report(
        &variants
            .iter()
            .map(|v| (v.label.clone(), v.model.clone()))
            .collect::<Vec<_>>(),
        &harnesses,
        &records,
    ))
}

#[allow(clippy::module_name_repetitions)]
pub fn aggregate_report(
    variants: &[(String, String)],
    harnesses: &[(String, HarnessResult)],
    records: &[RunRecord],
) -> Report {
    let total_tasks = SMOKE_INSTANCE_IDS.len();
    let mut per_task = Vec::with_capacity(total_tasks);

    let mut resolved_sets = Vec::with_capacity(harnesses.len());
    for (_, h) in harnesses {
        let set: HashSet<&str> = h.resolved_ids.iter().map(String::as_str).collect();
        resolved_sets.push(set);
    }

    for instance_id in SMOKE_INSTANCE_IDS {
        let mut row = Vec::with_capacity(resolved_sets.len());
        for set in &resolved_sets {
            row.push(set.contains(instance_id));
        }
        per_task.push(TaskRow {
            instance_id: instance_id.to_string(),
            resolved: row,
        });
    }

    let mut summaries = Vec::with_capacity(variants.len());
    for (label, model) in variants {
        let resolved = per_task
            .iter()
            .filter(|row| {
                row.resolved
                    .iter()
                    .enumerate()
                    .any(|(idx, val)| variants[idx].0 == *label && *val)
            })
            .count();
        let resolve_rate = usize_to_f64(resolved) / usize_to_f64(total_tasks);

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
            resolve_rate,
            // Phase B replaces placeholders with Wilson CI.
            ci_low: resolve_rate,
            ci_high: resolve_rate,
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
            .map(|idx| row.resolved[*idx])
            .collect::<Vec<_>>();
        row.resolved = reordered;
    }

    Report {
        schema_version: SCHEMA_VERSION,
        total_tasks,
        variants: summaries,
        per_task,
        pairwise: Vec::new(),
    }
}

fn usize_to_f64(value: usize) -> f64 {
    f64::from(u32::try_from(value).expect("value should fit into u32"))
}

pub fn render_terminal_table(report: &Report) {
    println!("clawmark leaderboard");
    println!("--------------------");
    println!("total tasks: {}", report.total_tasks);
    println!();
    println!("rank  variant  resolved  rate    cost/resolve");
    for (idx, variant) in report.variants.iter().enumerate() {
        let cost_per_resolve = variant
            .cost_per_resolve
            .map_or_else(|| "n/a".to_string(), |v| format!("{v:.4}"));
        println!(
            "{:>4}  {:<7}  {:>3}/{:<3}   {:>5.1}%  {}",
            idx + 1,
            variant.label,
            variant.resolved,
            report.total_tasks,
            variant.resolve_rate * 100.0,
            cost_per_resolve
        );
    }

    println!();
    println!("per-task matrix (variant order above):");
    for row in &report.per_task {
        let cells = row
            .resolved
            .iter()
            .map(|v| if *v { "1" } else { "0" })
            .collect::<Vec<_>>()
            .join(" ");
        println!("{}: {}", row.instance_id, cells);
    }
}

pub fn write_report_json(out: &Path, report: &Report) -> Result<(), String> {
    write_atomic_json(&out.join(REPORT_FILE), report)
}

pub fn render_patches(out: &Path) -> Result<(), String> {
    let records = load_run_records(&out.join(RUN_RECORDS_FILE))?;
    let variants = load_variants_manifest(&out.join(VARIANTS_FILE))?;

    println!();
    println!("Patches:");
    println!("---------");
    for instance_id in SMOKE_INSTANCE_IDS {
        println!("{instance_id}");
        for variant in &variants {
            println!("  [{}]:", variant.label);
            let patch = records
                .iter()
                .find(|r| r.key.variant.label == variant.label && r.key.instance_id == instance_id)
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

    fn make_record(label: &str, elapsed_secs: f64, usage: Option<ClaudeUsage>) -> RunRecord {
        RunRecord {
            schema_version: SCHEMA_VERSION,
            key: RunKey {
                variant: VariantId {
                    index: 0,
                    label: label.to_string(),
                },
                variant_hash: "abc".to_string(),
                instance_id: "astropy__astropy-12907".to_string(),
            },
            prediction: Prediction {
                instance_id: "astropy__astropy-12907".to_string(),
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
            ("a".to_string(), harness(&["astropy__astropy-12907"])),
            (
                "b".to_string(),
                harness(&["astropy__astropy-12907", "astropy__astropy-14182"]),
            ),
            ("c".to_string(), harness(&[])),
        ];
        let report = aggregate_report(&variants, &harnesses, &[]);
        assert_eq!(report.total_tasks, 5);
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
            ("a".to_string(), harness(&["astropy__astropy-12907"])),
            ("b".to_string(), harness(&["astropy__astropy-12907"])),
        ];
        let records = vec![
            make_record(
                "a",
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
        let report = aggregate_report(&variants, &harnesses, &records);
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
}
