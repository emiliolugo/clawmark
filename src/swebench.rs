#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const DATASET: &str = include_str!("../data/swebench_lite_v1_subset.jsonl");

pub const SMOKE_INSTANCE_IDS: [&str; 5] = [
    "astropy__astropy-12907",
    "astropy__astropy-14182",
    "astropy__astropy-14365",
    "astropy__astropy-14995",
    "astropy__astropy-6938",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskInstance {
    pub instance_id: String,
    pub repo: String,
    pub base_commit: String,
    pub problem_statement: String,
    pub hints_text: Option<String>,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prediction {
    pub instance_id: String,
    pub model_patch: String,
    pub model_name_or_path: String,
}

pub fn load_bundled_smoke_set() -> Result<Vec<TaskInstance>, String> {
    load_task_instances_from_str(DATASET)
}

pub fn load_task_instances(path: &Path) -> Result<Vec<TaskInstance>, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read dataset {}: {e}", path.display()))?;
    load_task_instances_from_str(&contents)
}

pub fn load_task_instances_from_str(contents: &str) -> Result<Vec<TaskInstance>, String> {
    let mut instances = Vec::new();
    for (line_no, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let instance: TaskInstance = serde_json::from_str(line)
            .map_err(|e| format!("invalid TaskInstance JSON on line {}: {e}", line_no + 1))?;
        instances.push(instance);
    }
    Ok(instances)
}

pub fn load_task_instances_jsonl(path: &Path) -> Result<Vec<TaskInstance>, String> {
    let file = std::fs::File::open(path)
        .map_err(|e| format!("failed to open dataset {}: {e}", path.display()))?;
    let reader = BufReader::new(file);
    let mut instances = Vec::new();
    for (line_no, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| format!("failed to read line {}: {e}", line_no + 1))?;
        if line.trim().is_empty() {
            continue;
        }
        let instance: TaskInstance = serde_json::from_str(&line)
            .map_err(|e| format!("invalid TaskInstance JSON on line {}: {e}", line_no + 1))?;
        instances.push(instance);
    }
    Ok(instances)
}

pub fn prediction_to_json_line(prediction: &Prediction) -> Result<String, String> {
    serde_json::to_string(prediction).map_err(|e| format!("failed to serialize prediction: {e}"))
}

pub fn validate_repo_slug(repo: &str) -> Result<(), String> {
    let Some((owner, name)) = repo.split_once('/') else {
        return Err(format!("invalid repo slug: {repo}"));
    };

    let valid_part = |part: &str| {
        !part.is_empty()
            // Reject `.`/`..` so a slug can never introduce a path-traversal
            // component into the constructed clone URL.
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
    };

    if valid_part(owner) && valid_part(name) {
        Ok(())
    } else {
        Err(format!("invalid repo slug: {repo}"))
    }
}

pub fn validate_base_commit(commit: &str) -> Result<(), String> {
    if commit.len() == 40
        && commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        Err(format!("invalid base_commit: {commit}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn bundled_smoke_set_has_exact_ids_in_order() {
        let instances = load_bundled_smoke_set().expect("load bundled smoke set");
        assert_eq!(instances.len(), SMOKE_INSTANCE_IDS.len());

        let ids: Vec<&str> = instances
            .iter()
            .map(|instance| instance.instance_id.as_str())
            .collect();
        assert_eq!(ids, SMOKE_INSTANCE_IDS);

        let unique: HashSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), SMOKE_INSTANCE_IDS.len());
    }

    #[test]
    fn bundled_instances_deserialize_required_fields() {
        let instances = load_bundled_smoke_set().expect("load bundled smoke set");
        for instance in instances {
            assert!(!instance.instance_id.is_empty());
            assert!(!instance.repo.is_empty());
            assert_eq!(instance.repo, "astropy/astropy");
            assert_eq!(instance.base_commit.len(), 40);
            assert!(!instance.problem_statement.is_empty());
            assert!(!instance.version.is_empty());
            validate_repo_slug(&instance.repo).expect("repo slug");
            validate_base_commit(&instance.base_commit).expect("base commit");
        }
    }

    #[test]
    fn validate_repo_slug_accepts_well_formed_slugs() {
        for slug in ["astropy/astropy", "owner_1/repo.name", "a-b/c-d"] {
            validate_repo_slug(slug).unwrap_or_else(|e| panic!("{slug} should be valid: {e}"));
        }
    }

    #[test]
    fn validate_repo_slug_rejects_malformed_and_traversal() {
        // No slash, empty parts, path traversal, and injection-flavored inputs.
        for slug in [
            "noslash",
            "",
            "/repo",
            "owner/",
            "../repo",
            "owner/..",
            ".././..",
            "owner/re po",
            "owner/repo\nrm -rf",
            "owner/repo;whoami",
            "own er/repo",
        ] {
            assert!(
                validate_repo_slug(slug).is_err(),
                "{slug:?} should be rejected"
            );
        }
    }

    #[test]
    fn validate_base_commit_accepts_full_sha1() {
        validate_base_commit("d16bfe05a744909de4b27f5875fe0d4ed41ce607").expect("valid sha");
    }

    #[test]
    fn validate_base_commit_rejects_bad_shapes() {
        for commit in [
            "",
            "abc123",                                    // too short
            "d16bfe05a744909de4b27f5875fe0d4ed41ce60",   // 39 chars
            "d16bfe05a744909de4b27f5875fe0d4ed41ce6077", // 41 chars
            "D16BFE05A744909DE4B27F5875FE0D4ED41CE607",  // uppercase not allowed
            "g16bfe05a744909de4b27f5875fe0d4ed41ce607",  // non-hex char
            "d16bfe05a744909de4b27f5875fe0d4ed41ce60 ",  // trailing space
        ] {
            assert!(
                validate_base_commit(commit).is_err(),
                "{commit:?} should be rejected"
            );
        }
    }

    #[test]
    fn prediction_serializes_to_harness_schema() {
        let prediction = Prediction {
            instance_id: "astropy__astropy-12907".to_string(),
            model_patch: "diff --git a/foo b/foo".to_string(),
            model_name_or_path: "clawmark/a".to_string(),
        };
        let line = prediction_to_json_line(&prediction).expect("serialize");
        let parsed: Prediction = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(parsed, prediction);
    }
}
