use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::results::{
    load_harness_results, write_atomic_json, HarnessResult, REPORT_FILE, SCHEMA_VERSION,
};
use crate::swebench::SMOKE_INSTANCE_IDS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskOutcome {
    pub instance_id: String,
    pub a_resolved: bool,
    pub b_resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

#[allow(clippy::module_name_repetitions)]
pub fn compute_report(out: &Path) -> Result<Report, String> {
    let a = load_harness_results(&out.join(crate::results::HARNESS_A_FILE))?;
    let b = load_harness_results(&out.join(crate::results::HARNESS_B_FILE))?;
    Ok(aggregate_report(&a, &b))
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
}

pub fn write_report_json(out: &Path, report: &Report) -> Result<(), String> {
    write_atomic_json(&out.join(REPORT_FILE), report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::HarnessResult;

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
}
