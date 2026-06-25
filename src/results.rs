#![allow(dead_code)]

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::runner::RunRecord;
use crate::swebench::{Prediction, SMOKE_INSTANCE_IDS};

pub const SCHEMA_VERSION: u32 = 3;

pub const RUN_RECORDS_FILE: &str = "run_records.jsonl";
pub const REPORT_FILE: &str = "report.json";
pub const VARIANTS_FILE: &str = "variants.json";

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
}

pub fn variant_hash(contents: &[u8]) -> String {
    let digest = Sha256::digest(contents);
    hex::encode(digest)
}

/// Deterministic hash of a directory's full recursive file contents.
///
/// Files are collected recursively, sorted by their `/`-joined relative path, then fed into a
/// single SHA-256 with length-prefixed contents so the digest is independent of filesystem walk
/// order and unambiguous across path/content boundaries. Only regular files contribute; empty
/// directories and symlinks are ignored.
pub fn hash_variant_dir(root: &Path) -> Result<String, String> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = Sha256::new();
    for (rel, abs) in &files {
        let contents =
            fs::read(abs).map_err(|e| format!("failed to read {}: {e}", abs.display()))?;
        let len = u64::try_from(contents.len())
            .map_err(|_| format!("file {} too large to hash", abs.display()))?;
        hasher.update(rel.as_bytes());
        hasher.update([0u8]);
        hasher.update(len.to_le_bytes());
        hasher.update(&contents);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("failed to read directory {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("failed to read entry in {}: {e}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("failed to stat {}: {e}", path.display()))?;
        if file_type.is_dir() {
            collect_files(root, &path, out)?;
        } else if file_type.is_file() {
            out.push((relative_slash_path(root, &path)?, path));
        }
    }
    Ok(())
}

fn relative_slash_path(root: &Path, path: &Path) -> Result<String, String> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| format!("path {} is not under {}", path.display(), root.display()))?;
    let mut parts = Vec::new();
    for comp in rel.components() {
        match comp {
            std::path::Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            _ => return Err(format!("unexpected path component in {}", rel.display())),
        }
    }
    Ok(parts.join("/"))
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

pub fn predictions_path(out: &Path, label: &str) -> PathBuf {
    out.join("predictions").join(format!("{label}.jsonl"))
}

pub fn harness_path(out: &Path, label: &str) -> PathBuf {
    out.join("harness").join(format!("{label}.json"))
}

pub fn harness_raw_path(out: &Path, label: &str) -> PathBuf {
    out.join("harness")
        .join(format!("clawmark__{label}.clawmark-{label}.json"))
}

/// Write a minimal output directory ready for harness evaluation: empty patches
/// for all bundled smoke tasks, for variants A and B.
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
        write_predictions_jsonl(&predictions_path(out, label), &prediction)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{RunKey, VariantId};
    use crate::swebench::Prediction;

    #[test]
    fn hash_variant_dir_is_deterministic_and_content_sensitive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let claude_dir = dir.path().join(".claude");
        fs::create_dir(&claude_dir).expect("create .claude dir");
        fs::write(dir.path().join("CLAUDE.md"), "variant instructions").expect("write CLAUDE.md");
        fs::write(claude_dir.join("settings.json"), r#"{"model":"sonnet"}"#)
            .expect("write settings.json");

        let hash1 = hash_variant_dir(dir.path()).expect("hash 1");
        let hash2 = hash_variant_dir(dir.path()).expect("hash 2");
        assert_eq!(hash1, hash2, "hash must be deterministic");
        assert_eq!(hash1.len(), 64, "hash must be 64 hex chars");

        fs::write(dir.path().join("CLAUDE.md"), "different content").expect("modify CLAUDE.md");
        let hash3 = hash_variant_dir(dir.path()).expect("hash 3");
        assert_ne!(hash1, hash3, "hash must change when content changes");
    }

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
            let path = predictions_path(dir.path(), label);
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
}
