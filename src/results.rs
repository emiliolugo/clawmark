#![allow(dead_code)]

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::runner::RunRecord;
use crate::swebench::{Prediction, SMOKE_INSTANCE_IDS};

pub const SCHEMA_VERSION: u32 = 4;

pub const RUN_RECORDS_FILE: &str = "run_records.jsonl";
pub const REPORT_FILE: &str = "report.json";
pub const VARIANTS_FILE: &str = "variants.json";
pub const RUN_META_FILE: &str = "run_meta.json";

pub const V1_HARNESS_A_FILE: &str = "harness/a.json";
pub const V1_HARNESS_B_FILE: &str = "harness/b.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwebenchPrediction {
    pub instance_id: String,
    pub model_patch: String,
    pub model_name_or_path: String,
}

impl From<&Prediction> for SwebenchPrediction {
    fn from(prediction: &Prediction) -> Self {
        Self {
            instance_id: prediction.instance_id.clone(),
            model_patch: prediction.model_patch.clone(),
            model_name_or_path: prediction.model_name_or_path.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HarnessResult {
    pub resolved_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantManifestEntry {
    pub index: usize,
    pub label: String,
    pub path: String,
    pub hash: String,
    pub model: String,
    #[serde(default)]
    pub agent: crate::cli::AgentBackend,
}

pub fn variant_hash(contents: &[u8]) -> String {
    let digest = Sha256::digest(contents);
    hex::encode(digest)
}

pub fn write_atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create parent directory {}: {e}",
                parent.display()
            )
        })?;
    }

    let dir = path
        .parent()
        .ok_or_else(|| format!("output path {} has no parent", path.display()))?;
    let mut temp = NamedTempFile::new_in(dir)
        .map_err(|e| format!("failed to create temp file in {}: {e}", dir.display()))?;
    serde_json::to_writer_pretty(&mut temp, value)
        .map_err(|e| format!("failed to serialize JSON for {}: {e}", path.display()))?;
    temp.write_all(b"\n")
        .map_err(|e| format!("failed to finalize JSON for {}: {e}", path.display()))?;
    temp.persist(path)
        .map_err(|e| format!("failed to write {}: {}", path.display(), e.error))?;
    Ok(())
}

pub fn append_jsonl_line(path: &Path, line: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create parent directory {}: {e}",
                parent.display()
            )
        })?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    writeln!(file, "{line}").map_err(|e| format!("failed to append to {}: {e}", path.display()))?;
    Ok(())
}

pub fn append_run_record(path: &Path, record: &RunRecord) -> Result<(), String> {
    let line = serde_json::to_string(record)
        .map_err(|e| format!("failed to serialize run record: {e}"))?;
    append_jsonl_line(path, &line)
}

pub fn write_predictions_jsonl(
    path: &Path,
    predictions: &[SwebenchPrediction],
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create parent directory {}: {e}",
                parent.display()
            )
        })?;
    }

    let dir = path
        .parent()
        .ok_or_else(|| format!("predictions path {} has no parent", path.display()))?;
    let mut temp = NamedTempFile::new_in(dir)
        .map_err(|e| format!("failed to create temp file in {}: {e}", dir.display()))?;

    for prediction in predictions {
        let line = serde_json::to_string(prediction)
            .map_err(|e| format!("failed to serialize prediction: {e}"))?;
        writeln!(temp, "{line}")
            .map_err(|e| format!("failed to write predictions to temp file: {e}"))?;
    }

    temp.persist(path)
        .map_err(|e| format!("failed to write {}: {}", path.display(), e.error))?;
    Ok(())
}

pub fn load_run_records(path: &Path) -> Result<Vec<RunRecord>, String> {
    if !path.is_file() {
        return Err(format!("run records file not found: {}", path.display()));
    }

    let file = File::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();

    for (line_no, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("failed to read line {}: {e}", line_no + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let record: RunRecord = serde_json::from_str(&line)
            .map_err(|e| format!("invalid RunRecord JSON on line {}: {e}", line_no + 1))?;
        records.push(record);
    }

    Ok(records)
}

#[allow(clippy::module_name_repetitions)]
pub fn load_harness_results(path: &Path) -> Result<HarnessResult, String> {
    let file = File::open(path)
        .map_err(|e| format!("failed to open harness result {}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_reader(file)
        .map_err(|e| format!("failed to parse harness result {}: {e}", path.display()))?;

    let resolved_ids = value
        .get("resolved_ids")
        .ok_or_else(|| format!("harness result {} is missing resolved_ids", path.display()))?
        .as_array()
        .ok_or_else(|| {
            format!(
                "harness result {} has non-array resolved_ids",
                path.display()
            )
        })?
        .iter()
        .map(|item| {
            item.as_str()
                .ok_or_else(|| {
                    format!(
                        "harness result {} has non-string resolved_ids entry",
                        path.display()
                    )
                })
                .map(str::to_string)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(HarnessResult { resolved_ids })
}

pub fn load_variants_manifest(path: &Path) -> Result<Vec<VariantManifestEntry>, String> {
    let file = File::open(path)
        .map_err(|e| format!("failed to open variants manifest {}: {e}", path.display()))?;
    serde_json::from_reader(file)
        .map_err(|e| format!("failed to parse variants manifest {}: {e}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMeta {
    pub schema_version: u32,
    pub trials: u32,
    pub dataset_source: String,
    pub instance_ids: Vec<String>,
}

pub fn load_run_meta(path: &Path) -> Result<RunMeta, String> {
    let file =
        File::open(path).map_err(|e| format!("failed to open run meta {}: {e}", path.display()))?;
    serde_json::from_reader(file)
        .map_err(|e| format!("failed to parse run meta {}: {e}", path.display()))
}

/// Verify that `out` was produced by a compatible prior invocation of clawmark
/// and can be safely resumed with the given (freshly validated) `variants` and
/// `meta`. Checks are ordered so the earliest, most fundamental mismatch is
/// reported first.
pub fn verify_resume_dir(
    out: &Path,
    variants: &[VariantManifestEntry],
    meta: &RunMeta,
) -> Result<(), String> {
    let meta_path = out.join(RUN_META_FILE);
    if !meta_path.is_file() {
        return Err(format!(
            "cannot resume: {} not found (was this directory produced by clawmark >= schema 4?)",
            meta_path.display()
        ));
    }

    let stored_meta = load_run_meta(&meta_path).map_err(|e| format!("cannot resume: {e}"))?;

    if stored_meta.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "cannot resume: run has schema_version {}, this build writes {SCHEMA_VERSION}",
            stored_meta.schema_version
        ));
    }

    if stored_meta.trials != meta.trials {
        return Err(format!(
            "cannot resume: run used --trials {}, current invocation uses {}",
            stored_meta.trials, meta.trials
        ));
    }

    if stored_meta.dataset_source != meta.dataset_source {
        return Err(format!(
            "cannot resume: run used dataset {}, current invocation uses {}",
            stored_meta.dataset_source, meta.dataset_source
        ));
    }

    if stored_meta.instance_ids != meta.instance_ids {
        return Err("cannot resume: task list differs from the original run".to_string());
    }

    let manifest_path = out.join(VARIANTS_FILE);
    let stored_variants =
        load_variants_manifest(&manifest_path).map_err(|e| format!("cannot resume: {e}"))?;

    if stored_variants.len() != variants.len() {
        return Err(
            "cannot resume: variant list differs from the original run (label/model/agent)"
                .to_string(),
        );
    }

    for (stored, current) in stored_variants.iter().zip(variants.iter()) {
        let core_matches = stored.label == current.label
            && stored.model == current.model
            && stored.agent == current.agent;

        if core_matches && stored.hash != current.hash {
            return Err(format!(
                "cannot resume: variant '{}' contents changed since the original run (hash mismatch)",
                stored.label
            ));
        }
        if !core_matches {
            return Err(
                "cannot resume: variant list differs from the original run (label/model/agent)"
                    .to_string(),
            );
        }
    }

    Ok(())
}

/// Rebuild `predictions/<label>-t<trial>.jsonl` from `run_records.jsonl`,
/// taking the last record per `instance_id` (so retried instances use the
/// newest attempt) and ordering rows by `instance_ids`. Used for both fresh
/// and resumed runs so there is a single code path for predictions output.
pub fn rebuild_predictions(
    out: &Path,
    label: &str,
    trial: u32,
    instance_ids: &[String],
) -> Result<(), String> {
    let records = load_run_records(&out.join(RUN_RECORDS_FILE))?;

    let mut last_by_instance: std::collections::HashMap<&str, &RunRecord> =
        std::collections::HashMap::new();
    for record in &records {
        if record.key.variant.label == label && record.key.trial == trial {
            last_by_instance.insert(record.key.instance_id.as_str(), record);
        }
    }

    let mut predictions = Vec::with_capacity(instance_ids.len());
    for instance_id in instance_ids {
        let record = last_by_instance.get(instance_id.as_str()).ok_or_else(|| {
            format!("internal error: no run record for {label} t{trial} {instance_id}")
        })?;
        predictions.push(SwebenchPrediction::from(&record.prediction));
    }

    write_predictions_jsonl(&predictions_path(out, label, trial), &predictions)
}

pub fn predictions_path(out: &Path, label: &str, trial: u32) -> PathBuf {
    out.join("predictions")
        .join(format!("{label}-t{trial}.jsonl"))
}

pub fn harness_path(out: &Path, label: &str, trial: u32) -> PathBuf {
    out.join("harness").join(format!("{label}-t{trial}.json"))
}

pub fn harness_raw_path(out: &Path, label: &str, trial: u32) -> PathBuf {
    out.join("harness")
        .join(format!("clawmark__{label}.clawmark-{label}-t{trial}.json"))
}

/// Write a minimal output directory ready for harness evaluation: empty patches
/// for all bundled smoke tasks, for variants A and B, trial 1.
pub fn write_minimum_valid_dir(out: &Path) -> Result<(), String> {
    fs::create_dir_all(out.join("predictions"))
        .map_err(|e| format!("failed to create predictions directory: {e}"))?;
    fs::create_dir_all(out.join("harness"))
        .map_err(|e| format!("failed to create harness directory: {e}"))?;

    for label in ["a", "b"] {
        let prediction = SMOKE_INSTANCE_IDS
            .iter()
            .map(|instance_id| SwebenchPrediction {
                instance_id: (*instance_id).to_string(),
                model_patch: String::new(),
                model_name_or_path: format!("clawmark/{label}"),
            })
            .collect::<Vec<_>>();
        write_predictions_jsonl(&predictions_path(out, label, 1), &prediction)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{RunKey, VariantId};
    use crate::swebench::Prediction;

    #[test]
    fn variant_hash_is_stable_sha256_hex() {
        let hash = variant_hash(b"hello");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn write_predictions_jsonl_round_trips() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("predictions/a.jsonl");
        let predictions = vec![SwebenchPrediction {
            instance_id: "astropy__astropy-12907".to_string(),
            model_patch: String::new(),
            model_name_or_path: "clawmark/a".to_string(),
        }];
        write_predictions_jsonl(&path, &predictions).expect("write predictions");
        let contents = fs::read_to_string(&path).expect("read predictions");
        let parsed: SwebenchPrediction =
            serde_json::from_str(contents.trim()).expect("parse prediction");
        assert_eq!(parsed, predictions[0]);
    }

    #[test]
    fn load_harness_results_reads_resolved_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("a.json");
        fs::write(&path, r#"{"resolved_ids":["astropy__astropy-12907"]}"#)
            .expect("write harness json");
        let result = load_harness_results(&path).expect("load harness");
        assert_eq!(result.resolved_ids, vec!["astropy__astropy-12907"]);
    }

    #[test]
    fn write_minimum_valid_out_dir_writes_ab_predictions() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimum_valid_dir(dir.path()).expect("write fixture");

        assert!(dir.path().join("harness").is_dir());

        for label in ["a", "b"] {
            let path = predictions_path(dir.path(), label, 1);
            let contents = fs::read_to_string(&path).expect("read predictions");
            let lines: Vec<&str> = contents
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect();
            assert_eq!(lines.len(), SMOKE_INSTANCE_IDS.len());
            for line in lines {
                let parsed: SwebenchPrediction =
                    serde_json::from_str(line).expect("parse prediction");
                assert!(parsed.model_patch.is_empty());
                assert_eq!(parsed.model_name_or_path, format!("clawmark/{label}"));
            }
        }
    }

    #[test]
    fn append_run_record_preserves_schema_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(RUN_RECORDS_FILE);
        let record = RunRecord {
            schema_version: SCHEMA_VERSION,
            key: RunKey {
                variant: VariantId {
                    index: 0,
                    label: "a".to_string(),
                },
                variant_hash: variant_hash(b"a"),
                instance_id: "astropy__astropy-12907".to_string(),
                trial: 1,
            },
            prediction: Prediction {
                instance_id: "astropy__astropy-12907".to_string(),
                model_patch: String::new(),
                model_name_or_path: "clawmark/a".to_string(),
            },
            elapsed_secs: 1.0,
            error: None,
            usage: None,
        };
        append_run_record(&path, &record).expect("append");
        let records = load_run_records(&path).expect("load");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].schema_version, SCHEMA_VERSION);
    }

    fn sample_meta(trials: u32, dataset_source: &str, instance_ids: &[&str]) -> RunMeta {
        RunMeta {
            schema_version: SCHEMA_VERSION,
            trials,
            dataset_source: dataset_source.to_string(),
            instance_ids: instance_ids.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    fn sample_variant_manifest(label: &str, hash: &str) -> VariantManifestEntry {
        VariantManifestEntry {
            index: 0,
            label: label.to_string(),
            path: format!("{label}.md"),
            hash: hash.to_string(),
            model: "sonnet".to_string(),
            agent: crate::cli::AgentBackend::Claude,
        }
    }

    fn write_resume_fixture(
        dir: &Path,
        trials: u32,
        dataset_source: &str,
        instance_ids: &[&str],
        variants: &[VariantManifestEntry],
    ) {
        let meta = sample_meta(trials, dataset_source, instance_ids);
        write_atomic_json(&dir.join(RUN_META_FILE), &meta).expect("write run meta");
        write_atomic_json(&dir.join(VARIANTS_FILE), &variants.to_vec()).expect("write variants");
    }

    #[test]
    fn verify_resume_accepts_matching_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let variants = vec![
            sample_variant_manifest("a", "hash-a"),
            sample_variant_manifest("b", "hash-b"),
        ];
        write_resume_fixture(
            dir.path(),
            2,
            "bundled",
            &["astropy__astropy-12907"],
            &variants,
        );

        let meta = sample_meta(2, "bundled", &["astropy__astropy-12907"]);
        verify_resume_dir(dir.path(), &variants, &meta).expect("resume should be accepted");
    }

    #[test]
    fn verify_resume_rejects_hash_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stored_variants = vec![
            sample_variant_manifest("a", "hash-a"),
            sample_variant_manifest("b", "hash-b"),
        ];
        write_resume_fixture(
            dir.path(),
            1,
            "bundled",
            &["astropy__astropy-12907"],
            &stored_variants,
        );

        let current_variants = vec![
            sample_variant_manifest("a", "hash-a-changed"),
            sample_variant_manifest("b", "hash-b"),
        ];
        let meta = sample_meta(1, "bundled", &["astropy__astropy-12907"]);
        let err = verify_resume_dir(dir.path(), &current_variants, &meta)
            .expect_err("expected hash mismatch rejection");
        assert!(err.contains("hash mismatch"));
        assert!(err.contains('a'));
    }

    #[test]
    fn verify_resume_rejects_trials_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let variants = vec![
            sample_variant_manifest("a", "hash-a"),
            sample_variant_manifest("b", "hash-b"),
        ];
        write_resume_fixture(
            dir.path(),
            1,
            "bundled",
            &["astropy__astropy-12907"],
            &variants,
        );

        let meta = sample_meta(2, "bundled", &["astropy__astropy-12907"]);
        let err = verify_resume_dir(dir.path(), &variants, &meta)
            .expect_err("expected trials mismatch rejection");
        assert!(err.contains("--trials"));
    }

    #[test]
    fn verify_resume_rejects_task_list_mismatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let variants = vec![
            sample_variant_manifest("a", "hash-a"),
            sample_variant_manifest("b", "hash-b"),
        ];
        write_resume_fixture(
            dir.path(),
            1,
            "bundled",
            &["astropy__astropy-12907"],
            &variants,
        );

        let meta = sample_meta(1, "bundled", &["astropy__astropy-14182"]);
        let err = verify_resume_dir(dir.path(), &variants, &meta)
            .expect_err("expected task list mismatch rejection");
        assert!(err.contains("task list differs"));
    }

    #[test]
    fn rebuild_predictions_orders_and_dedupes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let run_records = dir.path().join(RUN_RECORDS_FILE);

        let make = |instance_id: &str, patch: &str, error: Option<&str>| RunRecord {
            schema_version: SCHEMA_VERSION,
            key: RunKey {
                variant: VariantId {
                    index: 0,
                    label: "a".to_string(),
                },
                variant_hash: "hash-a".to_string(),
                instance_id: instance_id.to_string(),
                trial: 1,
            },
            prediction: Prediction {
                instance_id: instance_id.to_string(),
                model_patch: patch.to_string(),
                model_name_or_path: "clawmark/a".to_string(),
            },
            elapsed_secs: 1.0,
            error: error.map(str::to_string),
            usage: None,
        };

        append_run_record(&run_records, &make("X", "", Some("boom"))).expect("append 1");
        append_run_record(&run_records, &make("X", "final-patch", None)).expect("append 2");
        append_run_record(&run_records, &make("Y", "y-patch", None)).expect("append 3");

        let instance_ids = vec!["Y".to_string(), "X".to_string()];
        rebuild_predictions(dir.path(), "a", 1, &instance_ids).expect("rebuild predictions");

        let contents =
            fs::read_to_string(predictions_path(dir.path(), "a", 1)).expect("read predictions");
        let lines: Vec<&str> = contents.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 2);

        let first: SwebenchPrediction = serde_json::from_str(lines[0]).expect("parse first");
        let second: SwebenchPrediction = serde_json::from_str(lines[1]).expect("parse second");
        assert_eq!(first.instance_id, "Y");
        assert_eq!(second.instance_id, "X");
        assert_eq!(second.model_patch, "final-patch");
    }
}
