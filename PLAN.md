# clawmark — Minimal v1 Implementation Plan

> v1 has one job: compare two local `CLAUDE.md` files against SWE-bench Lite and show which one performs better. Anything that does not directly support that A/B workflow is out of scope.

---

## v1 Contract

**Ships in v1:**
- `clawmark doctor` — validate local prerequisites before a run
- `clawmark run --a <path> --b <path> --model <model> [--model-a <model>] [--model-b <model>] --timeout-secs <seconds> --out <dir>` — run exactly two `CLAUDE.md` variants against the bundled 5-task SWE-bench Lite smoke set. `--model` is the shared default; `--model-a`/`--model-b` optionally override the model for variant A or B respectively.
- `clawmark report --out <dir>` — read existing output and print a simple A vs B summary, including per-variant wall-clock time, input tokens, output tokens, and estimated USD cost

**Intentional scope expansion (recorded here per AGENTS.md policy):**
- Per-variant model overrides (`--model-a`/`--model-b`, both optional, defaulting to `--model`) are in scope. No new dependencies are required.
- Per-variant time/token/cost reporting is in scope. Token usage and USD cost are captured from the `claude -p --output-format json` output and aggregated per variant in the report. Cost is shown as `n/a` when the CLI does not provide it. Time was already recorded as `elapsed_secs` in `run_records.jsonl`; tokens and cost are newly captured fields. `schema_version` is bumped from 1 to 2 in both `run_records.jsonl` records and `report.json`. No new dependencies are added; parsing uses the existing `serde_json` crate. Budget enforcement, per-task cost caps, and whole-run accounting beyond reporting remain out of scope.

**Does not ship in v1:**

Some items below shipped after v1 (see `docs/cli.md`): trials > 1, Wilson confidence intervals, McNemar's test, partial SWE-bench Lite subsets via custom datasets, resume.

- More than two variants per run
- Trials greater than 1
- `pass@k`
- McNemar's test
- Wilson confidence intervals
- Full or partial 300-task SWE-bench Lite execution
- Benchmark adapters other than SWE-bench Lite
- Remote or cloud execution
- Multi-user service mode
- Web UI
- Stable plugin API
- Automatic dependency installation
- Distributed execution
- HumanEval or any other benchmark
- Budget enforcement, per-task cost caps, or aggregate cost accounting beyond per-variant reporting
- Persistent clone or result cache beyond the current output directory
- Progress bars or rich terminal UI
- Release automation
- `clawmark.toml` or any config file
- Typed internal failure taxonomy
- Running variant files outside the process current working directory
- Reusing, appending to, or resuming from a previous output directory
- Parsing model text for patches when `git diff HEAD` is empty
- SWE-bench harness output formats other than the current PyPI `swebench` release

**v1 is complete when:**
1. `doctor` validates all prerequisites
2. `clawmark run --a variants/a.md --b variants/b.md --model sonnet --timeout-secs 300 --out out` executes the bundled 5-task SWE-bench Lite smoke subset end-to-end (optional `--model-a`/`--model-b` override the per-variant model)
3. The run evaluates both variants on the same tasks with one trial per variant/task
4. Only doctor-check failures and Claude auth failure abort the entire run; all other per-task Claude/git failures record an error and continue
5. Output includes per-task A/B outcomes, total resolved counts, win/loss/tie counts, per-variant time/token/cost totals, raw prediction files, raw harness results, and `out/report.json`
6. CI passes for non-Docker tests (`cargo fmt --check`, `cargo clippy`, `cargo test`)

**Required acceptance tests:**
- `cargo test` covers CLI validation for exactly two variant paths, different canonical A/B files, variant paths inside the current working directory, and `run --out` failing when the output directory already exists.
- `cargo test` covers report aggregation for A wins, B wins, both resolved, and both failed.
- `cargo run -- doctor` prints a prerequisite table and exits non-zero if a required tool is missing.
- `cargo run -- run --a variants/a.md --b variants/b.md --model sonnet --timeout-secs 300 --out out` writes `out/predictions/a.jsonl`, `out/predictions/b.jsonl`, `out/harness/a.json`, `out/harness/b.json`, and `out/report.json`.
- `cargo run -- report --out out` reads existing output without invoking Claude or Docker.

---

## Prerequisites and Supported Versions

| Dependency | Minimum version | Notes |
|---|---|---|
| Rust | stable, MSRV 1.79 | pin in `rust-version` field in `Cargo.toml` |
| Claude CLI | >= 1.0.0 | `claude --version` must exit 0 |
| Docker | >= 24.0 | `docker info` must exit 0 |
| Python | 3.11+ | required by SWE-bench harness |
| swebench | latest | install with `python3 -m pip install --upgrade swebench`; harness CLI changes between versions |
| git | >= 2.39 | `git --version` must exit 0 |

Users install all of these manually. clawmark does not install them. `clawmark doctor` validates each one before any run proceeds.

---

## Bundled Smoke Set

v1 runs exactly these five SWE-bench Lite test instances, in this order:

1. `astropy__astropy-12907`
2. `astropy__astropy-14182`
3. `astropy__astropy-14365`
4. `astropy__astropy-14995`
5. `astropy__astropy-6938`

`data/swebench_lite_v1_subset.jsonl` is the source of task data used by `clawmark run`. Each line must deserialize to `TaskInstance`, the file must contain exactly five unique `instance_id` values, and the IDs must exactly match the list above. Add a unit test that fails if the file has missing, extra, duplicate, or reordered IDs.

---

## Run Limits

| Parameter | Default | Max |
|---|---|---|
| variants per run | 2 | 2 |
| tasks (SWE-bench Lite instances) | 5 | 5 |
| trials per (variant, task) | 1 | 1 |
| concurrency (parallel claude invocations) | 1 | 1 |

v1 is intentionally a reliable 5-task smoke runner. Do not add partial or full 300-task execution until the 5-task A/B workflow is correct and observable.

---

## CLI Validation

`run` validates all inputs before cloning repos or invoking Claude:

| Input | Rule |
|---|---|
| `--a` / `--b` existence | Each path must exist and be a regular file after symlink resolution |
| `--a` / `--b` location | Each canonical path must be inside the process current working directory |
| `--a` / `--b` identity | A and B must not resolve to the same canonical file |
| `--a` / `--b` filename | The filename may be anything; clawmark injects the file contents as `CLAUDE.md` inside each temp workspace |
| `--model` | Non-empty string; shared default model passed to `claude --model` for both variants |
| `--model-a` | Optional non-empty string; overrides `--model` for variant A only |
| `--model-b` | Optional non-empty string; overrides `--model` for variant B only |
| `--timeout-secs` | Integer from 1 to 86_400 |
| `--out` for `run` | Parent directory must exist; `--out` itself must not already exist; clawmark creates it |
| `--out` for `report` | Directory must already exist and contain the expected v1 output files |

v1 never appends to, partially reuses, or clears an existing output directory. If a run is interrupted, choose a new `--out` value.

---

## Security Model

**Claude runs locally on the host, not in a container.** The temp workspace is a normal directory on the host filesystem. v1 is not a sandbox for untrusted variants, benchmark tasks, prompts, or model behavior.

Security boundaries:

| Access during `claude` execution | Status |
|---|---|
| Workspace TempDir (the cloned repo) | Allowed — explicitly via `--add-dir` |
| Files outside the workspace | Not OS-sandboxed; do not rely on v1 for hard isolation |
| Network access by claude | Allowed (claude may call APIs) |
| Shell command execution by claude (Bash tool) | Intended to operate in the workspace, but not OS-isolated |
| Host filesystem outside workspace | Not blocked at OS level |
| Docker socket | Not intentionally passed, but not OS-blocked by clawmark |
| User credentials / env vars | Available unless the user runs clawmark from a scrubbed environment |

**SWE-bench test execution runs inside Docker** via the official harness. The model-generated patch is applied inside a container; test results come from the container's exit code. The patch itself is never `exec`'d directly.

**Threat model — what clawmark mitigates:**
- Shell injection via user-controlled CLI values (mitigated by Command argv arrays and input validation)
- Path traversal via variant file paths (mitigated by `canonicalize()` + bounds check)
- Partial write corruption (mitigated by `NamedTempFile` atomic rename)

**Threat model — what clawmark does NOT mitigate in v1:**
- A malicious CLAUDE.md variant that instructs the model to exfiltrate host files
- A malicious SWE-bench task that causes the model to execute harmful code in the workspace
- Exposure of host environment variables to local subprocesses
- Network access by Claude

This is acceptable only because v1 is a local developer tool for user-controlled inputs. Document this explicitly in the README.

---

## Architecture Overview

```
CLI args only
    ↓
[cli.rs] parse + validate exactly two variant paths and required run parameters
    ↓
[swebench.rs] load SWE-bench Lite TaskInstance[] from JSONL
    ↓
[runner.rs] A/B × selected tasks × 1 trial (serial)
    ↓  per work item:
    ├── [sandbox.rs] git clone at base_commit → inject CLAUDE.md → TempDir workspace
    ├── claude -p --output-format json --dangerously-skip-permissions --bare
    │         --add-dir <workspace> "<problem_statement>"
    ├── sandbox.collect_patch() → git diff HEAD  (ground-truth patch)
    └── [results.rs] append RunRecord to out/run_records.jsonl
    ↓
[runner.rs] invoke_harness() → python3 -m swebench.harness.run_evaluation
    (once for A and once for B)
    ↓
[report.rs] compare A vs B resolved outcomes → out/report.json + terminal table
```

---

## Output File Layout

```
out/
  run_records.jsonl              # one record per variant/task attempt
  predictions/
    a.jsonl                      # SWE-bench harness input for variant A
    b.jsonl                      # SWE-bench harness input for variant B
  harness/
    a.json                       # SWE-bench harness output for variant A
    b.json                       # SWE-bench harness output for variant B
  report.json                    # final A/B aggregate report
```

Every `run_records.jsonl` line includes `"schema_version": 2`. `report.json` includes top-level `"schema_version": 2`. (Schema version was bumped from 1 to 2 when per-variant token/cost fields were added.)

---

## SWE-bench Harness Invocation

Invoke the official harness once per variant after writing predictions. Use blocking `std::process::Command`, argv arrays only, and set the subprocess working directory to `<out>/harness`.

For variant A:

```sh
python3 -m swebench.harness.run_evaluation \
  --dataset_name princeton-nlp/SWE-bench_Lite \
  --split test \
  --predictions_path <absolute out>/predictions/a.jsonl \
  --instance_ids astropy__astropy-12907 astropy__astropy-14182 astropy__astropy-14365 astropy__astropy-14995 astropy__astropy-6938 \
  --max_workers 1 \
  --run_id clawmark-a \
  --timeout <timeout_secs>
```

For variant B, use `predictions/b.jsonl` and `--run_id clawmark-b`.

With current PyPI `swebench`, the harness writes a summary report in its current working directory named from `model_name_or_path.replace("/", "__") + "." + run_id + ".json"`. Because clawmark sets `model_name_or_path` to `clawmark/a` and `clawmark/b`, the expected raw summary files are:

- `<out>/harness/clawmark__a.clawmark-a.json`
- `<out>/harness/clawmark__b.clawmark-b.json`

After each harness invocation succeeds, copy those files to the stable clawmark paths:

- `<out>/harness/a.json`
- `<out>/harness/b.json`

If either expected raw summary file is missing, treat harness invocation as failed and abort before writing `out/report.json`.

---

## Crate Layout

```
clawmark/
  src/
    main.rs           — clap entry and subcommand dispatch
    cli.rs            — clap derive types only (Cli, Commands, RunArgs, ReportArgs)
    swebench.rs       — TaskInstance loading and SWE-bench prediction schema
    runner.rs         — run_ab(), run_single(), invoke_harness(), RunKey, RunRecord
    sandbox.rs        — Workspace (TempDir wrapper), create(), inject_claude_md(), collect_patch()
    results.rs        — write_predictions_jsonl(), load_run_records(), variant_hash()
    report.rs         — compute_report(), render_terminal_table(), write_json()
    doctor.rs         — run_doctor() with prerequisite checks
  data/
    swebench_lite_v1_subset.jsonl   — 5 hardcoded instances for smoke tests (include_str!)
```

---

## Cargo.toml

```toml
[package]
name = "clawmark"
version = "0.1.0"
edition = "2021"
rust-version = "1.79"

[features]
integration = []   # gates tests that require Docker + swebench installed

[dependencies]
clap        = { version = "4.5", features = ["derive", "env"] }
serde       = { version = "1", features = ["derive"] }
serde_json  = "1"
tempfile    = "3.10"
sha2        = "0.10"
hex         = "0.4"

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
all       = "warn"
pedantic  = "warn"
```

**Deferred:**
- full 300-task SWE-bench Lite support
- `indicatif` — progress bars
- `comfy-table` — formatted tables; use `println!` with manual padding in v1
- statistical crates — not needed until repeated trials or significance testing exists

**Dropped:**
- `chrono` — use `std::time::SystemTime::now().duration_since(UNIX_EPOCH)` as `u64` epoch seconds
- `tokio` — v1 is serial; use blocking `std::process::Command`
- `toml` — all run parameters are required CLI args
- `anyhow` and `thiserror` — v1 stores plain error messages instead of rich internal error types

### Dependency Rationale

| Crate | Purpose |
|---|---|
| `clap` (derive) | CLI subcommands, flags, `--help` |
| `serde` + `serde_json` | Serialize/deserialize `RunRecord`, predictions, harness output |
| `tempfile` | `TempDir` auto-deletes on drop; `NamedTempFile` enables atomic writes |
| `sha2` + `hex` | SHA-256 of variant content for traceability in output |

---

## Key Types

```rust
pub struct TaskInstance {
    pub instance_id: String,
    pub repo: String,
    pub base_commit: String,
    pub problem_statement: String,
    pub hints_text: Option<String>,
    pub version: String,
}

pub struct Prediction {
    pub instance_id: String,
    pub model_patch: String,
    pub model_name_or_path: String,
}

// runner.rs
pub enum VariantSlot {
    A,
    B,
}

pub struct RunKey {
    pub variant: VariantSlot,
    pub variant_hash: String,   // SHA-256 hex of variant file content
    pub instance_id: String,
}

pub struct RunRecord {
    pub schema_version: u32,    // always 2 in v1 (bumped from 1 when token/cost fields added)
    pub key: RunKey,
    pub model: String,          // actual model used for this invocation
    pub prediction: Prediction,
    pub elapsed_secs: f64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost_usd: Option<f64>,  // None when the Claude CLI did not report cost
    pub error: Option<String>,  // None means Claude produced a prediction
}

// results.rs — harness-expected schema (do not add fields)
pub struct SwebenchPrediction {
    pub instance_id: String,
    pub model_patch: String,
    pub model_name_or_path: String,
}
```

---

## Claude Invocation

Use blocking `std::process::Command` with an explicit argv array. No shell strings.

```
claude
  -p
  --output-format json
  --dangerously-skip-permissions
  --bare                           ← disables global CLAUDE.md discovery + hooks
  --model <from CLI>
  --add-dir <workspace_path>       ← scopes tool access to the TempDir
  "<problem_statement>"            ← single argv element, never interpolated
```

After `claude` exits, call `git diff HEAD` in the workspace as the ground-truth patch. If the diff is empty, write an empty `model_patch`; the harness scores it as unresolved. Do not attempt to extract fallback diffs from model text in v1.

---

## Budget Policy

v1 exposes no budget flag. Users manage spend through Claude account settings.

Before starting a run, print the exact number of Claude invocations:

```
2 variants × 5 tasks × 1 trial = 10 Claude invocations
```

The user is responsible for choosing Claude account settings appropriate for their budget.

---

## Error / Retry Policy

| Error condition | Behavior |
|---|---|
| Claude exits non-zero | Record `error = "claude failed: <message>"`, write empty patch, no retry, continue |
| Claude times out | Kill process, record `error = "claude timed out after <seconds>s"`, write empty patch, no retry, continue |
| Claude rate-limited | Record the stderr message as `error`, no retry in v1 |
| Claude auth failure | Abort entire run immediately if stderr contains `not authenticated`, `authentication`, `login`, or `API key` case-insensitively |
| Model unavailable | Record the stderr message as `error`, write empty patch, no retry |
| GitHub clone failure | Record the git error message as `error`, write empty patch, no retry |
| Docker image pull failure | Abort harness invocation with a clear message; do not lose already-collected predictions |

Retries, resume, and failure skipping are not v1 features. If a run is interrupted, start a new output directory.

---

## Cleanup Behavior

| Event | Cleanup |
|---|---|
| Normal exit | All `TempDir` workspaces deleted automatically on drop |
| Ctrl-C (SIGINT) | No custom signal handling in v1; already-written output remains for inspection |
| Timeout | The timed-out claude process is killed by the blocking timeout loop; workspace is dropped |
| Panic | TempDir drop runs via Rust's panic unwinding unless the process aborts |
| Harness failure | Already-written predictions JSONL is preserved; harness output dir may be partially written |

Cloned repos live inside `TempDir` and are deleted with the workspace. No persistent clones remain after a run.

---

## Data Retention

| Data | Persisted | Location |
|---|---|---|
| Variant file contents | No (read at run time) | user-managed |
| Problem statements | No | embedded in dataset |
| Model output (raw JSON) | No | discarded after patch and usage extraction |
| Patches (git diff) | Yes | `out/run_records.jsonl` |
| Harness results | Yes | `out/harness/a.json` and `out/harness/b.json` |
| Aggregate report | Yes | `out/report.json` |
| Cloned repos | No | deleted with TempDir |
| Docker containers | Managed by harness | harness cleans up after each instance |

---

## Security Rules (input validation)

| Vector | Mitigation |
|---|---|
| Path traversal via variant paths | `canonicalize()` + assert path is within working dir |
| Variant output names | Fixed to `a` and `b`; no user-controlled filesystem name |
| `repo` in git clone URL | Validate `^[a-zA-Z0-9_.-]+/[a-zA-Z0-9_.-]+$`; construct `https://github.com/{repo}` only after validation |
| `base_commit` in git checkout | Validate 40-char lowercase hex; pass as argv element, never interpolated |
| CLAUDE.md write | `std::fs::write`, not a subprocess |
| Partial write corruption | `NamedTempFile` + atomic `persist()` rename |
| JSONL writes | Serial append only; write complete lines |
| Shell injection | `Command` argv arrays everywhere — `sh -c` is prohibited |

---

## Doctor Checks

Each check has an exact command, pass condition, and failure message.

| Check | Command | Pass condition | Failure message |
|---|---|---|---|
| Docker running | `docker info` | exits 0 within 10s | "Docker is not running. Start Docker Desktop or the Docker daemon." |
| Claude CLI present | `claude --version` | exits 0 within 5s, stdout contains version string | "Claude CLI not found. Install from https://claude.ai/download and ensure it is on PATH." |
| Claude authenticated | `claude -p --output-format text "ping"` | exits 0 within 15s, non-empty stdout | "Claude CLI is not authenticated. Run `claude` interactively to log in." |
| Python 3.11+ | `python3 --version` | exits 0, version >= 3.11 | "Python 3.11+ required. Found: <version>." |
| swebench installed | `python3 -c "import swebench; print(swebench.__version__)"` | exits 0, prints a non-empty version string | "swebench required. Install: python3 -m pip install --upgrade swebench" |
| Harness CLI reachable | `python3 -m swebench.harness.run_evaluation --help` | exits 0 within 10s | "SWE-bench harness CLI not reachable. Reinstall swebench." |
| git | `git --version` | exits 0 | "git not found. Install git." |
| SWE-bench Docker image | `docker images -q swebench/sweb.eval.x86_64` | exits 0, non-empty stdout | WARN only: "SWE-bench Docker image not pulled. First run will pull it (may take time)." |

All checks run in sequence. Print a status table. Exit 1 if any check fails (not warns).

---

## Report Implementation

`report.rs` computes only direct A/B comparison metrics in v1:

| Metric | Definition |
|---|---|
| `a_resolved` | Count of tasks resolved by variant A |
| `b_resolved` | Count of tasks resolved by variant B |
| `a_wins` | Count of tasks where A resolved and B did not |
| `b_wins` | Count of tasks where B resolved and A did not |
| `ties_both_resolved` | Count of tasks both variants resolved |
| `ties_both_failed` | Count of tasks neither variant resolved |
| `a_elapsed_secs` | Total wall-clock seconds across all variant A invocations |
| `b_elapsed_secs` | Total wall-clock seconds across all variant B invocations |
| `a_input_tokens` | Total input tokens across all variant A invocations (`null` if unavailable) |
| `b_input_tokens` | Total input tokens across all variant B invocations (`null` if unavailable) |
| `a_output_tokens` | Total output tokens across all variant A invocations (`null` if unavailable) |
| `b_output_tokens` | Total output tokens across all variant B invocations (`null` if unavailable) |
| `a_cost_usd` | Estimated total USD cost for variant A (`null` when the Claude CLI did not provide it) |
| `b_cost_usd` | Estimated total USD cost for variant B (`null` when the Claude CLI did not provide it) |

The terminal report prints the win/loss/tie counts, total task count, and per-variant time/token/cost summary (cost displayed as `n/a` when absent). `report.json` stores the same values plus per-task outcomes.

Resolved parsing rule:

1. Load `<out>/harness/a.json` and `<out>/harness/b.json`.
2. Treat `resolved_ids` as the only source of truth for successful task resolution.
3. For each bundled smoke `instance_id`, `resolved = resolved_ids.contains(instance_id)`.
4. If a harness JSON file is missing `resolved_ids` or if `resolved_ids` is not an array of strings, fail `report` with a clear error.
5. Empty patches are unresolved because they will not appear in `resolved_ids`.

---

## Parallel Implementation Plan

This section is about parallelizing implementation work across agents. It does not change v1 runtime behavior: `clawmark run` still invokes Claude serially with concurrency fixed at 1.

### Sequential Prerequisites

These tasks must happen first because later work depends on their APIs, file layout, or test fixtures:

1. **Create the Rust scaffold and module boundaries.** Add `Cargo.toml`, `src/main.rs`, `src/cli.rs`, and empty module files matching the crate layout. This establishes names, imports, lint settings, and the public function signatures other agents should target.
2. **Define shared v1 data types.** Add `TaskInstance`, `Prediction`, `SwebenchPrediction`, `VariantSlot`, `RunKey`, `RunRecord`, and the report structs before agents write serializers, runners, or report code.
3. **Implement CLI parsing and validation contracts.** `doctor`, `run`, and `report` args should exist early so downstream work can compile against `RunArgs` and `ReportArgs`.
4. **Add the bundled smoke dataset fixture.** Create `data/swebench_lite_v1_subset.jsonl` with exactly the five approved IDs and a unit test that locks the order and uniqueness.
5. **Agree on output path constants and schema version.** Fix `predictions/a.jsonl`, `predictions/b.jsonl`, `harness/a.json`, `harness/b.json`, `run_records.jsonl`, `report.json`, and `schema_version = 1` before parallel writers/readers begin.

After these prerequisites compile, most remaining work should be assigned in parallel.

### Parallel Workstreams

| Workstream | Depends on | Can run in parallel with | Output |
|---|---|---|---|
| `doctor.rs` prerequisite checks | CLI scaffold | All non-CLI work | `cargo run -- doctor` status table |
| `swebench.rs` dataset loader + prediction schema | Shared types, dataset fixture | `doctor`, `report`, `sandbox` | Parsed `TaskInstance` list and JSONL-ready predictions |
| `results.rs` writers/readers + variant hashes | Shared types, output constants | `doctor`, `report`, `sandbox` | Atomic JSON/JSONL output helpers |
| `sandbox.rs` workspace clone/inject/diff helpers | Shared types | `doctor`, `report`, pure `results` tests | Temp workspace lifecycle and `git diff HEAD` extraction |
| `report.rs` pure aggregation and terminal rendering | Shared types, output constants | `doctor`, `sandbox`, `runner` skeleton | `report --out` logic from fixture harness JSON |
| `runner.rs` orchestration | CLI, `swebench`, `sandbox`, `results` | Mostly after helpers exist | `run_ab()`, `run_single()`, harness invocation |
| README + security notes | Final CLI names and security model | Implementation work after CLI stabilizes | User-facing quickstart and caveats |
| CI workflow | Cargo scaffold | Most implementation work | `fmt`, `clippy`, `test` checks |

`runner.rs` is the main integration point and should be owned by the strongest agent. Other agents should expose small, tested helpers and avoid changing runner-facing APIs after handoff.

### Difficulty-Based Delegation

Assign lower-risk, self-contained tasks to less powerful agents:

| Difficulty | Suitable tasks | Guardrails |
|---|---|---|
| Low | README quickstart, CLI examples, prerequisite table, no-telemetry note, CI YAML, formatting fixes | Do not add features, flags, dependencies, or new scope |
| Low | Dataset fixture test for the five fixed IDs | IDs and order must exactly match the Bundled Smoke Set |
| Medium | `doctor.rs` checks | Use argv arrays, fixed timeouts, clear messages, no auto-install |
| Medium | `swebench.rs` JSONL parsing and prediction serialization | Keep schema narrow; do not add alternate benchmark support |
| Medium | `results.rs` atomic writes, JSONL helpers, SHA-256 hashing | Preserve fixed output paths and `schema_version = 1` |
| Medium | `report.rs` aggregation from synthetic `a.json`/`b.json` fixtures | Only read `resolved_ids`; do not infer results from other fields |
| High | `sandbox.rs` clone, checkout, CLAUDE.md injection, timeout-safe command execution, patch collection | Use `Command` argv arrays only; empty diff means unresolved |
| High | `runner.rs` A/B orchestration and per-task error handling | Preserve serial execution and abort only on doctor/auth failures |
| Highest | SWE-bench harness integration and end-to-end run debugging | Use current PyPI `swebench`; copy raw summaries to stable paths |

Use the lowest capable agent for low and medium tasks, but require the strongest agent to review API boundaries, subprocess behavior, filesystem safety, and final end-to-end integration.

---

## Phased Milestones

### Phase 1 — Scaffold
**Rust concepts:** ownership, structs, modules, `Result`, `?`

Create: `Cargo.toml`, `src/main.rs`, `src/cli.rs`, `src/doctor.rs`, `data/swebench_lite_v1_subset.jsonl`

**Verify:** `cargo run -- doctor` compiles and prints a check table. `cargo test` passes.

---

### Phase 2 — Single Variant Execution
**Rust concepts:** `std::process::Command`, `serde`, plain error messages

Create: `src/swebench.rs`, `src/sandbox.rs`, `src/results.rs`, `run_single()` in `src/runner.rs`

Wire internal `run_single()` for one task × one variant.

**Verify:** unit tests cover empty diff as unresolved, prediction serialization, and `RunRecord` serialization for both `VariantSlot::A` and `VariantSlot::B`.

---

### Phase 3 — A/B Run + Harness Integration
**Rust concepts:** subprocesses, integration tests

Add `run_ab()` and `invoke_harness()` in `runner.rs`. Add `load_harness_results()` in `results.rs`. Add integration tests gated on `#[cfg(feature = "integration")]` and `#[ignore]`.

Run integration tests: `cargo test --features integration -- --ignored`

**Verify:** `cargo run -- run --a variants/a.md --b variants/b.md --model sonnet --timeout-secs 300 --out out` writes predictions for A and B, invokes the Python harness for both, and produces parseable harness output for the bundled 5 tasks.

---

### Phase 4 — Reporting
**Rust concepts:** iterators, `Display`

Implement `src/report.rs`.

**Verify:** `cargo run -- report --out out` prints A/B resolved counts, win/loss/tie counts, and writes `out/report.json`.

---

### Phase 5 — Minimal Project Hygiene

- `.github/workflows/ci.yml`: `cargo check`, `cargo clippy -D warnings`, `cargo fmt --check`, `cargo test` on ubuntu-latest
- `README.md`: quickstart, prerequisites table, CLI reference, security model note, no-telemetry statement
- `[profile.release]` with `strip = true` and `lto = true`

---

## CLI Examples

```sh
cargo run -- doctor
cargo run -- run --a variants/a.md --b variants/b.md --model sonnet --timeout-secs 300 --out out
cargo run -- report --out out
```

---

## Predictions JSONL Format

One JSON object per line, UTF-8, no BOM. Written to `out/predictions/a.jsonl` and `out/predictions/b.jsonl`.

```json
{"instance_id": "astropy__astropy-12907", "model_patch": "diff --git ...", "model_name_or_path": "clawmark/a"}
```

`model_name_or_path` is either `"clawmark/a"` or `"clawmark/b"`. Empty `model_patch` (`""`) is valid — the harness scores it as unresolved.
