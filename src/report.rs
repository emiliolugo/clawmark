use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::results::{
    load_harness_results, load_run_records, write_atomic_json, HarnessResult, REPORT_FILE,
    RUN_RECORDS_FILE, SCHEMA_VERSION,
};
use crate::runner::{RunRecord, VariantSlot};
use crate::swebench::SMOKE_INSTANCE_IDS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskOutcome {
    pub instance_id: String,
    pub a_resolved: bool,
    pub b_resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantTotals {
    pub resolved: usize,
    pub elapsed_secs: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub total_tasks: usize,
    pub a_resolved: usize,
    pub b_resolved: usize,
    pub a_wins: usize,
    pub b_wins: usize,
    pub ties_both_resolved: usize,
    pub ties_both_failed: usize,
    pub tasks: Vec<TaskOutcome>,
    pub a_totals: VariantTotals,
    pub b_totals: VariantTotals,
}

#[allow(clippy::module_name_repetitions)]
pub fn compute_report(out: &Path) -> Result<Report, String> {
    let a = load_harness_results(&out.join(crate::results::HARNESS_A_FILE))?;
    let b = load_harness_results(&out.join(crate::results::HARNESS_B_FILE))?;
    let mut report = aggregate_report(&a, &b);
    let records = load_run_records(&out.join(RUN_RECORDS_FILE)).unwrap_or_default();
    report.a_totals = variant_totals(&records, VariantSlot::A, report.a_resolved);
    report.b_totals = variant_totals(&records, VariantSlot::B, report.b_resolved);
    Ok(report)
}

#[allow(clippy::module_name_repetitions)]
pub fn aggregate_report(a: &HarnessResult, b: &HarnessResult) -> Report {
    let a_resolved_set: HashSet<&str> = a.resolved_ids.iter().map(String::as_str).collect();
    let b_resolved_set: HashSet<&str> = b.resolved_ids.iter().map(String::as_str).collect();

    let mut a_resolved = 0;
    let mut b_resolved = 0;
    let mut a_wins = 0;
    let mut b_wins = 0;
    let mut ties_both_resolved = 0;
    let mut ties_both_failed = 0;
    let mut tasks = Vec::new();

    for instance_id in SMOKE_INSTANCE_IDS {
        let a_ok = a_resolved_set.contains(instance_id);
        let b_ok = b_resolved_set.contains(instance_id);

        if a_ok {
            a_resolved += 1;
        }
        if b_ok {
            b_resolved += 1;
        }

        match (a_ok, b_ok) {
            (true, false) => a_wins += 1,
            (false, true) => b_wins += 1,
            (true, true) => ties_both_resolved += 1,
            (false, false) => ties_both_failed += 1,
        }

        tasks.push(TaskOutcome {
            instance_id: instance_id.to_string(),
            a_resolved: a_ok,
            b_resolved: b_ok,
        });
    }

    Report {
        schema_version: SCHEMA_VERSION,
        total_tasks: SMOKE_INSTANCE_IDS.len(),
        a_resolved,
        b_resolved,
        a_wins,
        b_wins,
        ties_both_resolved,
        ties_both_failed,
        tasks,
        a_totals: VariantTotals {
            resolved: a_resolved,
            elapsed_secs: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: None,
        },
        b_totals: VariantTotals {
            resolved: b_resolved,
            elapsed_secs: 0.0,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: None,
        },
    }
}

/// Sum elapsed time, tokens, and cost for records belonging to `variant`.
///
/// If any record has a `Some` cost, the sum is `Some`; if none do, it is `None`.
#[must_use]
pub fn variant_totals(records: &[RunRecord], variant: VariantSlot, resolved: usize) -> VariantTotals {
    let mut elapsed_secs = 0.0_f64;
    let mut input_tokens = 0_u64;
    let mut output_tokens = 0_u64;
    let mut cost_usd: Option<f64> = None;

    for r in records.iter().filter(|r| r.key.variant == variant) {
        elapsed_secs += r.elapsed_secs;
        if let Some(u) = &r.usage {
            input_tokens += u.input_tokens;
            output_tokens += u.output_tokens;
            if let Some(c) = u.cost_usd {
                cost_usd = Some(cost_usd.unwrap_or(0.0) + c);
            }
        }
    }

    VariantTotals {
        resolved,
        elapsed_secs,
        input_tokens,
        output_tokens,
        cost_usd,
    }
}

pub fn render_terminal_table(report: &Report) {
    println!("clawmark A/B report");
    println!("-------------------");
    println!("total tasks:          {}", report.total_tasks);
    println!("A resolved:           {}", report.a_resolved);
    println!("B resolved:           {}", report.b_resolved);
    println!("A wins:               {}", report.a_wins);
    println!("B wins:               {}", report.b_wins);
    println!("ties (both resolved): {}", report.ties_both_resolved);
    println!("ties (both failed):   {}", report.ties_both_failed);

    let fmt_cost = |c: Option<f64>| c.map_or_else(|| "n/a".to_string(), |v| format!("{v:.4}"));

    println!();
    println!("metrics            A            B");
    println!("time (s):    {:>10.1}  {:>10.1}", report.a_totals.elapsed_secs, report.b_totals.elapsed_secs);
    println!("input tokens:{:>10}  {:>10}", report.a_totals.input_tokens, report.b_totals.input_tokens);
    println!("output tokens:{:>9}  {:>10}", report.a_totals.output_tokens, report.b_totals.output_tokens);
    println!("cost (USD):  {:>10}  {:>10}", fmt_cost(report.a_totals.cost_usd), fmt_cost(report.b_totals.cost_usd));
}

pub fn write_report_json(out: &Path, report: &Report) -> Result<(), String> {
    write_atomic_json(&out.join(REPORT_FILE), report)
}

/// Print each task's model patch for both variants, truncated to 20 lines each.
///
/// Reads `run_records.jsonl` from `out`. Returns an error if the file is missing or unreadable.
pub fn render_patches(out: &Path) -> Result<(), String> {
    let records = load_run_records(&out.join(RUN_RECORDS_FILE))?;

    println!();
    println!("Patches:");
    println!("---------");

    for instance_id in SMOKE_INSTANCE_IDS {
        println!("{instance_id}");
        for label in ["a", "b"] {
            println!("  [{label}]:");
            let patch = records
                .iter()
                .find(|r| r.key.variant.label() == label && r.key.instance_id == instance_id)
                .map_or("", |r| r.prediction.model_patch.as_str());

            if patch.is_empty() {
                println!("    (empty patch)");
            } else {
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
    use crate::runner::{ClaudeUsage, RunKey, RunRecord, VariantSlot};
    use crate::swebench::Prediction;
    use crate::results::SCHEMA_VERSION;

    fn harness(resolved: &[&str]) -> HarnessResult {
        HarnessResult {
            resolved_ids: resolved.iter().map(|id| (*id).to_string()).collect(),
        }
    }

    #[test]
    fn report_counts_a_win() {
        let report = aggregate_report(
            &harness(&["astropy__astropy-12907"]),
            &HarnessResult {
                resolved_ids: Vec::new(),
            },
        );
        assert_eq!(report.a_wins, 1);
        assert_eq!(report.b_wins, 0);
        assert_eq!(report.ties_both_resolved, 0);
        assert_eq!(report.ties_both_failed, 4);
        assert!(report.tasks[0].a_resolved);
        assert!(!report.tasks[0].b_resolved);
    }

    #[test]
    fn report_counts_b_win() {
        let report = aggregate_report(
            &HarnessResult {
                resolved_ids: Vec::new(),
            },
            &harness(&["astropy__astropy-14182"]),
        );
        assert_eq!(report.a_wins, 0);
        assert_eq!(report.b_wins, 1);
        assert_eq!(report.ties_both_resolved, 0);
        assert_eq!(report.ties_both_failed, 4);
        assert!(!report.tasks[1].a_resolved);
        assert!(report.tasks[1].b_resolved);
    }

    #[test]
    fn report_counts_both_resolved_tie() {
        let report = aggregate_report(
            &harness(&["astropy__astropy-14365"]),
            &harness(&["astropy__astropy-14365"]),
        );
        assert_eq!(report.ties_both_resolved, 1);
        assert_eq!(report.a_wins, 0);
        assert_eq!(report.b_wins, 0);
        assert!(report.tasks[2].a_resolved);
        assert!(report.tasks[2].b_resolved);
    }

    #[test]
    fn report_counts_both_failed_tie() {
        let report = aggregate_report(
            &HarnessResult {
                resolved_ids: Vec::new(),
            },
            &HarnessResult {
                resolved_ids: Vec::new(),
            },
        );
        assert_eq!(report.ties_both_failed, 5);
        assert_eq!(report.ties_both_resolved, 0);
        assert_eq!(report.a_resolved, 0);
        assert_eq!(report.b_resolved, 0);
    }

    fn make_record(variant: VariantSlot, elapsed_secs: f64, usage: Option<ClaudeUsage>) -> RunRecord {
        RunRecord {
            schema_version: SCHEMA_VERSION,
            key: RunKey {
                variant,
                variant_hash: "abc".to_string(),
                instance_id: "astropy__astropy-12907".to_string(),
            },
            prediction: Prediction {
                instance_id: "astropy__astropy-12907".to_string(),
                model_patch: String::new(),
                model_name_or_path: variant.model_name_or_path().to_string(),
            },
            elapsed_secs,
            error: None,
            usage,
        }
    }

    #[test]
    fn variant_totals_sums_usage_and_time() {
        let records = vec![
            make_record(
                VariantSlot::A,
                10.0,
                Some(ClaudeUsage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_read_input_tokens: 5,
                    cache_creation_input_tokens: 3,
                    cost_usd: Some(0.01),
                }),
            ),
            make_record(
                VariantSlot::A,
                5.0,
                Some(ClaudeUsage {
                    input_tokens: 200,
                    output_tokens: 80,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cost_usd: Some(0.02),
                }),
            ),
            // usage: None should contribute zero tokens and not force cost to Some
            make_record(VariantSlot::A, 2.0, None),
            // variant B record should be ignored
            make_record(
                VariantSlot::B,
                99.0,
                Some(ClaudeUsage {
                    input_tokens: 999,
                    output_tokens: 999,
                    cache_read_input_tokens: 0,
                    cache_creation_input_tokens: 0,
                    cost_usd: Some(9.99),
                }),
            ),
        ];

        let totals = variant_totals(&records, VariantSlot::A, 1);
        assert_eq!(totals.resolved, 1);
        assert!((totals.elapsed_secs - 17.0).abs() < f64::EPSILON);
        assert_eq!(totals.input_tokens, 300);
        assert_eq!(totals.output_tokens, 130);
        assert!((totals.cost_usd.unwrap() - 0.03).abs() < 1e-9);

        // Records with no cost usage should not affect a variant whose other
        // records also have no cost — cost_usd should remain None.
        let none_records = vec![make_record(VariantSlot::A, 1.0, None)];
        let no_cost = variant_totals(&none_records, VariantSlot::A, 0);
        assert_eq!(no_cost.cost_usd, None);
    }
}
