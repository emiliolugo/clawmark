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
| swebench | exactly 0.0.14 | Install with `pip install swebench==0.0.14` |
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

## How Runs Work

For each task and variant, `clawmark`:

1. Clones the SWE-bench target repository into a temporary workspace.
2. Checks out the task's base commit.
3. Writes the selected variant file as `CLAUDE.md` at the repo root.
4. Invokes Claude with the task problem statement.
5. Captures `git diff HEAD` as the model patch.

Claude is invoked locally with:

```sh
claude -p --output-format json --dangerously-skip-permissions --bare --model <model> --add-dir <workspace> <problem_statement>
```

After all predictions are written for a variant, `clawmark` invokes the SWE-bench harness once for A and once for B. The report treats the harness `resolved_ids` arrays as the source of truth.

## CLI Reference

```sh
clawmark doctor
```

Checks Docker, Claude CLI, Claude authentication, Python, `swebench`, the SWE-bench harness CLI, git, and whether the SWE-bench Docker image is already present.

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

## Telemetry

`clawmark` does not send telemetry or usage data. Claude and SWE-bench may perform their own network activity as part of their normal operation.
