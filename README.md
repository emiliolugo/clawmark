# clawmark

`clawmark` is a local Rust CLI for answering one focused question:

> Which of these two `CLAUDE.md` files performs better on a small SWE-bench Lite smoke set?

v1 compares exactly two local variant files against five bundled SWE-bench Lite tasks. It runs Claude locally, evaluates the generated patches with the official SWE-bench harness in Docker, and writes a simple A/B report.

## What Ships In v1

- `clawmark doctor` checks local prerequisites.
- `clawmark run` evaluates variant A and variant B on the same five tasks.
- `clawmark report` reads an existing output directory and prints the A/B summary again.

There is no config file, web UI, remote execution, retries, resume, progress UI, repeated trials, or full 300-task SWE-bench run in v1.

## Prerequisites

Install these yourself before running `clawmark`:

| Dependency | Required version | Notes |
|---|---:|---|
| Rust | stable, MSRV 1.79 | Used to build this CLI |
| Claude CLI | >= 1.0.0 | Must be installed, on `PATH`, and authenticated |
| Docker | >= 24.0 | Required by the SWE-bench harness |
| Python | 3.11+ | Required by `swebench` |
| swebench | latest | Install into the `python3` on your `PATH` with `python3 -m pip install --upgrade swebench` |
| git | >= 2.39 | Used to clone task repos and collect diffs |

Check your machine:

```sh
cargo run -- doctor
```

`doctor` prints a status table and exits non-zero if a required check fails. A missing SWE-bench Docker image is only a warning; the first evaluation may pull it.

## Quickstart

Create two variant files somewhere inside the current working directory:

```sh
mkdir -p variants
$EDITOR variants/a.md
$EDITOR variants/b.md
```

Run the A/B smoke benchmark:

```sh
cargo run -- run \
  --a variants/a.md \
  --b variants/b.md \
  --model sonnet \
  --timeout-secs 300 \
  --out out
```

This performs:

```text
2 variants x 5 tasks x 1 trial = 10 Claude invocations
```

`run` creates a fresh output directory. It fails if `--out` already exists, so use a new directory for each run.

Print the report from existing output:

```sh
cargo run -- report --out out
```

After building a release binary, the same commands can be run as:

```sh
cargo build --release
./target/release/clawmark doctor
./target/release/clawmark run --a variants/a.md --b variants/b.md --model sonnet --timeout-secs 300 --out out
./target/release/clawmark report --out out
```

## Runtime And Budget Warnings

v1 is intentionally minimal and does not enforce a turn limit, token budget, retry policy, or per-task cost cap. `--timeout-secs` is only a wall-clock timeout around each Claude invocation. A broad `CLAUDE.md` can spend the full timeout exploring the repo, installing dependencies, or running tests without producing a patch.

For first e2e runs, use strict benchmark-oriented variants:

```md
You are running inside an automated benchmark. Make the smallest code change that addresses the issue.

Rules:
- Do not run the full test suite.
- Only inspect files needed for the issue.
- If you run tests, run at most one targeted test command.
- Do not spend time on formatting, docs, or unrelated cleanup.
- When a plausible minimal patch is made, stop.
```

Recommended starting settings:

- Use `--timeout-secs 600` for a bounded smoke test.
- Use `--timeout-secs 1800` only when you want to give Claude enough time to solve harder tasks.
- Use a fresh `--out` directory for every attempt.
- Run `cargo run -- doctor` first so failures happen before any Claude calls.
- Watch the first task before walking away; if it reaches the timeout with an empty patch, tighten your variant instructions before running all 10 invocations.

Budget expectation varies heavily by model behavior. The v1 smoke run performs 10 Claude invocations, so open-ended variants can consume materially more time and usage quota than short, patch-focused variants.

## How Runs Work

For each task and variant, `clawmark`:

1. Clones the SWE-bench target repository into a temporary workspace.
2. Checks out the task's base commit.
3. Writes the selected variant file as `CLAUDE.md` at the repo root.
4. Invokes Claude with the task problem statement.
5. Captures `git diff HEAD` as the model patch.

Claude is invoked locally with:

```sh
claude -p --output-format json --dangerously-skip-permissions --model <model> --add-dir <workspace> -- <problem_statement>
```

After all predictions are written for a variant, `clawmark` invokes the SWE-bench harness once for A and once for B. The report treats the harness `resolved_ids` arrays as the source of truth.

## CLI Reference

```sh
clawmark doctor
```

Checks Docker, Claude CLI, Claude authentication, Python, `swebench`, the SWE-bench harness CLI, git, Docker Hub registry reachability, and whether the SWE-bench Docker image is already present.

```sh
clawmark run --a <path> --b <path> --model <model> --timeout-secs <seconds> --out <dir>
```

Runs the fixed five-task A/B benchmark. `--timeout-secs` must be between `1` and `86400`; it applies to each Claude invocation and is also passed to the SWE-bench harness.

```sh
clawmark report --out <dir>
```

Reads existing harness output, prints resolved counts, A wins, B wins, both-resolved ties, and both-failed ties, then writes `report.json`.

## Input Rules

- `--a` and `--b` must exist and be regular files after symlink resolution.
- Both variant paths must be inside the process current working directory.
- A and B must resolve to different canonical files.
- `--model` must be a non-empty string and is passed as one argument to `claude --model`.
- `run --out` requires an existing parent directory and a destination that does not already exist.
- `report --out` requires an existing v1 output directory with harness results.

Variant filenames do not need to be `CLAUDE.md`; their contents are injected as `CLAUDE.md` inside each temporary task workspace.

## Output Layout

```text
out/
  run_records.jsonl
  predictions/
    a.jsonl
    b.jsonl
  harness/
    a.json
    b.json
  report.json
```

`run_records.jsonl` stores one record per variant/task attempt. `predictions/a.jsonl` and `predictions/b.jsonl` are the SWE-bench harness inputs. `harness/a.json` and `harness/b.json` are stable copies of the raw SWE-bench summary files. `report.json` stores the final A/B aggregate report.

All clawmark-owned records include `schema_version: 1`.

## Failure Behavior

Most per-task failures are recorded and the run continues with an empty patch for that task:

- git clone or checkout failure
- Claude non-auth failure
- Claude timeout
- model unavailable or rate limit errors
- empty `git diff HEAD`

Claude authentication failures abort the whole run. Harness failures abort before `report.json` is written, but already-written predictions remain in the output directory for inspection.

## Security Model

`clawmark` is a local developer tool for user-controlled inputs.

Claude runs on the host, not in a container. The command uses `--dangerously-skip-permissions`, and variant instructions are not OS-sandboxed. Do not run untrusted `CLAUDE.md` variants, untrusted benchmark data, or untrusted prompts through v1.

SWE-bench test execution runs inside Docker through the official harness. The model-generated patch is evaluated by the harness; `clawmark` does not execute the patch directly on the host.

`clawmark` mitigates shell injection by using subprocess argv arrays, variant path traversal by canonicalizing and checking paths against the current working directory, and partial write corruption with atomic file writes. It does not prevent malicious model behavior from accessing host files, environment variables, network resources, or other local credentials available to the process.

## Troubleshooting

- **`No instances to run.`** — The harness filters out empty predictions. This almost always means Claude produced no patch (often from too small a `--timeout-secs`). Use a generous timeout (e.g. `--timeout-secs 600`) and a fresh `--out` directory, then inspect `run_records.jsonl` to confirm patches are non-empty.
- **`lookup registry-1.docker.io: no such host` / Docker image pull errors** — Docker cannot resolve Docker Hub, so the harness cannot pull SWE-bench images. `clawmark doctor` flags this with the "Docker registry reachable" check. Fix your network/VPN and Docker Desktop DNS settings, then retry. This is an environment issue, not a clawmark bug.
- **Hugging Face `404` lines during dataset load** — These are normal probes the `datasets` library makes for optional files (e.g. `SWE-bench_Lite.py`, `dataset_infos.json`). They are not errors and do not affect the run.

## Telemetry

`clawmark` does not send telemetry or usage data. Claude and SWE-bench may perform their own network activity as part of their normal operation.
