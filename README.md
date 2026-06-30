# clawmark

`clawmark` is a local Rust CLI for answering one focused question:

> Which of these `CLAUDE.md` files performs better on a small SWE-bench Lite smoke set?

v1 runs two or more local variant files against five bundled SWE-bench Lite tasks. It runs a local coding agent (`claude` or `cursor`) per variant, evaluates the generated patches with the official SWE-bench harness in Docker, and writes a leaderboard report.

## What Ships In v1

- `clawmark doctor` checks local prerequisites.
- `clawmark run` evaluates one or more variants on the same five tasks.
- `clawmark report` reads an existing output directory and prints the leaderboard summary again.

There is no config file, web UI, remote execution, retries, resume, progress UI, repeated trials, or full 300-task SWE-bench run in v1.

## Prerequisites

Install these yourself before running `clawmark`:

| Dependency | Required version | Notes |
|---|---:|---|
| Rust | stable, MSRV 1.79 | Used to build this CLI |
| Claude CLI | >= 1.0.0 | Required for the `claude` backend: on `PATH` and authenticated |
| Cursor CLI | latest | Required for the `cursor` backend: `cursor-agent` on `PATH`, authenticated via `cursor-agent login` or `CURSOR_API_KEY` |
| Docker | >= 24.0 | Required by the SWE-bench harness |
| Python | 3.11+ | Required by `swebench` |
| swebench | latest | Install into the `python3` on your `PATH` with `python3 -m pip install --upgrade swebench` |
| git | >= 2.39 | Used to clone task repos and collect diffs |

Check your machine:

```sh
cargo run -- doctor
```

`doctor` prints a status table and exits non-zero if a required check fails. Missing or unauthenticated Claude/Cursor CLIs are warnings only — you need the backend selected for your run, not both. A missing SWE-bench Docker image is also a warning; the first evaluation may pull it.

## Quickstart

Create two or more variant files somewhere inside the current working directory:

```sh
mkdir -p variants
$EDITOR variants/a.md
$EDITOR variants/b.md
```

### Two-variant alias form

Run a two-variant smoke benchmark using the `--a/--b/--model` shorthand:

```sh
cargo run -- run \
  --a variants/a.md \
  --b variants/b.md \
  --model sonnet \
  --timeout-secs 300 \
  --out out
```

To run variant A on one model and variant B on another, use `--model-a` or `--model-b` to override the shared default for that variant:

```sh
cargo run -- run \
  --a variants/a.md \
  --b variants/b.md \
  --model sonnet \
  --model-b haiku \
  --timeout-secs 300 \
  --out out
```

In this example, A uses `sonnet` (the shared default) and B uses `haiku`. Both `--model-a` and `--model-b` are optional; omitting either causes that variant to use `--model`.

To run variant A with Claude and variant B with Cursor:

```sh
cargo run -- run \
  --a variants/a.md \
  --b variants/b.md \
  --model sonnet \
  --agent-b cursor \
  --timeout-secs 300 \
  --out out
```

Use `--agent` as the shared default backend, or `--agent-a` / `--agent-b` to override per variant. Omit all agent flags to default to `claude`.

### N-variant form

To compare three or more variants, use repeated `--variant label=path` and `--variant-model label=model` flags instead:

```sh
cargo run -- run \
  --variant alpha=variants/a.md \
  --variant beta=variants/b.md \
  --variant gamma=variants/c.md \
  --variant-model alpha=sonnet \
  --variant-model beta=haiku \
  --variant-model gamma=sonnet \
  --timeout-secs 300 \
  --out out
```

Every label must be unique, match `^[a-z0-9][a-z0-9_-]*$`, and each label must have a corresponding `--variant-model` entry. Variant paths must also be unique (no two labels pointing to the same file).

Set backends per label with repeated `--variant-agent <label>=<backend>` (defaults to `claude` when omitted).

The two forms are mutually exclusive — do not mix `--a/--b/--model` with `--variant/--variant-model` in the same invocation.

This performs:

```text
N variants x 5 tasks x 1 trial = N×5 agent invocations
```

`run` creates a fresh output directory. It fails if `--out` already exists, so use a new directory for each run.

To run up to N agent invocations concurrently within each variant pass, add `--parallel N`:

```sh
cargo run -- run \
  --a variants/a.md \
  --b variants/b.md \
  --model sonnet \
  --timeout-secs 300 \
  --parallel 5 \
  --out out
```

After building a release binary, print the report from existing output:

```sh
cargo run -- report --out out
```

To also display each task's model patch (truncated to 20 lines):

```sh
cargo run -- report --out out --show-patches
```

The report prints a leaderboard ranked by resolve rate, with per-variant wall-clock time, input tokens, output tokens, estimated USD cost (shown as `n/a` when unavailable, always for the `cursor` backend), and cost-per-resolve. After the leaderboard it prints the per-task resolution matrix.

```sh
cargo build --release
./target/release/clawmark doctor
./target/release/clawmark run --a variants/a.md --b variants/b.md --model sonnet --timeout-secs 300 --out out
./target/release/clawmark report --out out --show-patches
```

## Runtime And Budget Warnings

v1 is intentionally minimal and does not enforce a turn limit, token budget, retry policy, or per-task cost cap. `--timeout-secs` is only a wall-clock timeout around each agent invocation. A broad variant file can spend the full timeout exploring the repo, installing dependencies, or running tests without producing a patch.

`clawmark report` shows a leaderboard with per-variant totals for wall-clock time, input tokens, output tokens, estimated USD cost, and cost-per-resolve. This is reporting only — clawmark does not enforce or cap any of these values. Cursor runs never report token or cost data.

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
- Use `--timeout-secs 1800` only when you want to give the agent enough time to solve harder tasks.
- Use a fresh `--out` directory for every attempt.
- Run `cargo run -- doctor` first so failures happen before any agent calls.
- Watch the first task before walking away; if it reaches the timeout with an empty patch, tighten your variant instructions before running the full set.

Budget expectation varies heavily by model behavior. Each variant contributes 5 agent invocations, so open-ended variants can consume materially more time and usage quota than short, patch-focused variants.

## How Runs Work

For each task and variant, `clawmark`:

1. Clones the SWE-bench target repository into a temporary workspace.
2. Checks out the task's base commit.
3. Writes the selected variant file as both `CLAUDE.md` and `AGENTS.md` at the repo root (same contents; Claude reads the former, Cursor reads the latter).
4. Invokes the selected agent backend with the task problem statement.
5. Captures `git diff HEAD` as the model patch.

Claude is invoked locally with:

```sh
claude -p --output-format json --dangerously-skip-permissions --model <model> --add-dir <workspace> -- <problem_statement>
```

Cursor is invoked locally with:

```sh
cursor-agent -p --output-format json --force --trust --model <model> <problem_statement>
```

The Cursor command runs with the cloned repo as its working directory.

After all predictions are written for a variant, `clawmark` invokes the SWE-bench harness once per variant. The report treats the harness `resolved_ids` arrays as the source of truth.

When any (variant, task) pair fails to resolve, `clawmark run` prints a failure summary at the end of the run listing each unresolved variant/instance pair and the error message (if any). When all tasks resolve, no summary is printed.

## CLI Reference

```sh
clawmark doctor
```

Checks Docker, Python, `swebench`, the SWE-bench harness CLI, git, Docker Hub registry reachability, and whether the SWE-bench Docker image is already present. Also checks Claude and Cursor CLIs (present and authenticated); those are warnings, not failures, since only the backend you select for a run is required.

**Two-variant alias form:**

```sh
clawmark run --a <path> --b <path> --model <model> [--model-a <model>] [--model-b <model>] [--agent <backend>] [--agent-a <backend>] [--agent-b <backend>] --timeout-secs <seconds> --out <dir> [--parallel N]
```

Runs the fixed five-task benchmark. `--model` is the shared default model for both variants. `--model-a` and `--model-b` are optional per-variant overrides. `--agent` is the shared default agent backend (`claude` or `cursor`); `--agent-a` and `--agent-b` override it per variant (default: `claude`). `--timeout-secs` must be between `1` and `86400`; it applies to each agent invocation and is also passed to the SWE-bench harness. `--parallel N` (default: `1`) allows up to N agent invocations to run concurrently within each variant pass. The SWE-bench harness is always invoked serially, once per variant.

**N-variant form:**

```sh
clawmark run --variant <label>=<path> [--variant <label>=<path> ...] \
             --variant-model <label>=<model> [--variant-model <label>=<model> ...] \
             [--variant-agent <label>=<backend> ...] \
             --timeout-secs <seconds> --out <dir> [--parallel N]
```

Runs the fixed five-task benchmark for all specified variants. Every label must have its own `--variant-model` entry. `--variant-agent` is optional per label (defaults to `claude`). `--timeout-secs` must be between `1` and `86400`; it applies to each agent invocation and is also passed to the SWE-bench harness. `--parallel N` (default: `1`) allows up to N agent invocations to run concurrently within each variant pass. The SWE-bench harness is always invoked serially, once per variant.

The two forms are mutually exclusive.

```sh
clawmark report --out <dir> [--show-patches]
```

Reads existing harness output, prints a leaderboard ranked by resolve rate (then by cost-per-resolve), per-variant totals for wall-clock time, input tokens, output tokens, estimated USD cost (shown as `n/a` when unavailable, including all Cursor runs) and cost-per-resolve, and a per-task resolution matrix, then writes `report.json`. With `--show-patches`, also prints each task's model patch truncated to 20 lines.

## Input Rules

**Alias form (`--a/--b/--model`):**
- `--a` and `--b` must exist and be regular files after symlink resolution.
- Both variant paths must be inside the process current working directory.
- A and B must resolve to different canonical files.
- `--model` must be a non-empty string and is passed to the selected agent CLI. It is the shared default for both variants.
- `--model-a` and `--model-b` are optional. When given, each must be a non-empty string and overrides `--model` for that variant only.
- `--agent`, `--agent-a`, and `--agent-b` are optional. Values must be `claude` or `cursor`. When omitted, the backend defaults to `claude`.

**N-variant form (`--variant/--variant-model`):**
- At least two `--variant label=path` entries are required.
- Each label must match `^[a-z0-9][a-z0-9_-]*$` and be unique within the run.
- Each variant path must exist as a regular file, be inside the current working directory, and resolve to a unique canonical path.
- Every label must have a corresponding `--variant-model label=model` entry with a non-empty model string.
- `--variant-agent label=backend` is optional per label (defaults to `claude`; values must be `claude` or `cursor`).
- The two forms are mutually exclusive — mixing `--a/--b/--model` with `--variant/--variant-model` in the same invocation is an error.

**Common:**
- `run --out` requires an existing parent directory and a destination that does not already exist.
- `report --out` requires an existing output directory.

Variant filenames do not need to be `CLAUDE.md`; their contents are injected as both `CLAUDE.md` and `AGENTS.md` inside each temporary task workspace.

## Output Layout

```text
out/
  variants.json
  run_records.jsonl
  predictions/
    <label>.jsonl
    ...
  harness/
    <label>.json
    ...
  report.json
```

`variants.json` is the variant manifest written at the start of the run; it records each variant's label, file path, SHA-256 hash, model, and agent backend. `run_records.jsonl` stores one record per variant/task attempt. `predictions/<label>.jsonl` files are the SWE-bench harness inputs for each variant. `harness/<label>.json` files are stable copies of the raw SWE-bench summary files. `report.json` stores the final leaderboard report.

All clawmark-owned records include `schema_version: 3`.

## Failure Behavior

Most per-task failures are recorded and the run continues with an empty patch for that task:

- git clone or checkout failure
- agent non-auth failure (other errors are recorded per task)
- agent timeout
- model unavailable or rate limit errors
- empty `git diff HEAD`

Agent authentication failures abort the whole run. Harness failures abort before `report.json` is written, but already-written predictions remain in the output directory for inspection.

## Security Model

`clawmark` is a local developer tool for user-controlled inputs.

Agents run on the host, not in a container. Claude uses `--dangerously-skip-permissions`; Cursor uses `--force` and `--trust`. Variant instructions are not OS-sandboxed. Do not run untrusted variant files, untrusted benchmark data, or untrusted prompts through v1.

SWE-bench test execution runs inside Docker through the official harness. The model-generated patch is evaluated by the harness; `clawmark` does not execute the patch directly on the host.

`clawmark` mitigates shell injection by using subprocess argv arrays, variant path traversal by canonicalizing and checking paths against the current working directory, and partial write corruption with atomic file writes. It does not prevent malicious model behavior from accessing host files, environment variables, network resources, or other local credentials available to the process.

## Troubleshooting

- **`No instances to run.`** — The harness filters out empty predictions. This almost always means the agent produced no patch (often from too small a `--timeout-secs`). Use a generous timeout (e.g. `--timeout-secs 600`) and a fresh `--out` directory, then inspect `run_records.jsonl` to confirm patches are non-empty.
- **`lookup registry-1.docker.io: no such host` / Docker image pull errors** — Docker cannot resolve Docker Hub, so the harness cannot pull SWE-bench images. `clawmark doctor` flags this with the "Docker registry reachable" check. Fix your network/VPN and Docker Desktop DNS settings, then retry. This is an environment issue, not a clawmark bug.
- **Hugging Face `404` lines during dataset load** — These are normal probes the `datasets` library makes for optional files (e.g. `SWE-bench_Lite.py`, `dataset_infos.json`). They are not errors and do not affect the run.

## Telemetry

`clawmark` does not send telemetry or usage data. Agent CLIs and SWE-bench may perform their own network activity as part of their normal operation.

## Contributing

Pull requests are welcome. For major changes or new features, open an issue
first to discuss the change — see [CONTRIBUTING.md](./CONTRIBUTING.md).

## License

Licensed under the [MIT License](./LICENSE).
