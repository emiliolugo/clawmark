# clawmark

Local Rust CLI to compare agent instruction files (`CLAUDE.md` / `AGENTS.md` variants) on real code.

`clawmark` runs two or more variant files against a bundled 5-task SWE-bench Lite smoke set, invokes **Claude** or **Cursor** per variant, evaluates patches with the official SWE-bench harness in Docker, and prints a leaderboard.

**Commands:** `doctor` · `run` · `report`

## Quickstart

**1. Check prerequisites**

```sh
cargo run -- doctor
```

You need Rust, Docker, Python 3.11+, `swebench`, and `git`. For agents: `claude` and/or `cursor-agent` on `PATH` (only the backend you use must be installed and authenticated).

**2. Create variant files** (inside your working directory)

```sh
mkdir -p variants
echo "Make the smallest fix. Do not run the full test suite." > variants/a.md
echo "Make the smallest fix. Prefer targeted tests only." > variants/b.md
```

**3. Run the benchmark**

```sh
cargo run -- run \
  --a variants/a.md \
  --b variants/b.md \
  --model sonnet \
  --timeout-secs 600 \
  --out out
```

This runs 2 variants × 5 tasks = **10 agent invocations**. `--out` must not already exist.

**4. Read the report** (also printed at the end of `run`)

```sh
cargo run -- report --out out
cargo run -- report --out out --show-patches   # include patches (20 lines each)
```

### Common variations

Different model per variant:

```sh
cargo run -- run --a variants/a.md --b variants/b.md \
  --model sonnet --model-b haiku --timeout-secs 600 --out out
```

Claude vs Cursor (A=claude, B=cursor):

```sh
cargo run -- run --a variants/a.md --b variants/b.md \
  --model sonnet --agent-b cursor --timeout-secs 600 --out out
```

Three or more variants:

```sh
cargo run -- run \
  --variant alpha=variants/a.md \
  --variant beta=variants/b.md \
  --variant gamma=variants/c.md \
  --variant-model alpha=sonnet \
  --variant-model beta=haiku \
  --variant-model gamma=sonnet \
  --timeout-secs 600 \
  --out out
```

Concurrent tasks within a variant pass:

```sh
cargo run -- run --a variants/a.md --b variants/b.md \
  --model sonnet --timeout-secs 600 --parallel 3 --out out
```

Release binary:

```sh
cargo build --release
./target/release/clawmark run --a variants/a.md --b variants/b.md --model sonnet --timeout-secs 600 --out out
```

## What it does

For each variant and task:

1. Clone the SWE-bench repo at the task's base commit
2. Inject variant contents as both `CLAUDE.md` and `AGENTS.md`
3. Invoke the selected agent (`claude` or `cursor-agent`) with the problem statement
4. Collect `git diff HEAD` as the patch
5. Run the SWE-bench harness once per variant

The report ranks variants by resolve rate (then cost-per-resolve). Token/cost totals are `n/a` when the agent CLI does not provide them (always for Cursor).

## CLI

| Command | Purpose |
|---|---|
| `clawmark doctor` | Check Docker, Python, swebench, git, agent CLIs |
| `clawmark run ...` | Run benchmark; writes `out/` and prints report |
| `clawmark report --out <dir>` | Re-print report from existing output |

**Two-variant form**

```sh
clawmark run --a <path> --b <path> --model <model> \
  [--model-a <model>] [--model-b <model>] \
  [--agent claude|cursor] [--agent-a ...] [--agent-b ...] \
  --timeout-secs <secs> --out <dir> [--parallel N]
```

**N-variant form** (mutually exclusive with `--a/--b`)

```sh
clawmark run \
  --variant <label>=<path> ... \
  --variant-model <label>=<model> ... \
  [--variant-agent <label>=claude|cursor> ...] \
  --timeout-secs <secs> --out <dir> [--parallel N]
```

Variant paths must be regular files inside the current working directory. Agent backend defaults to `claude`.

## Output

```text
out/
  variants.json       # manifest (label, path, hash, model, agent)
  run_records.jsonl   # per task attempt
  predictions/        # harness inputs
  harness/            # harness results
  report.json
```

## Notes

- **Scope:** 5 bundled smoke tasks only; no config file, resume, or cloud execution.
- **Timeouts:** `--timeout-secs` is per agent invocation, not a token budget. Use `600+` for first runs.
- **Doctor:** Missing Claude/Cursor CLI is a warning — you only need the backend you select.
- **Security:** Agents run on the host with full permissions. SWE-bench tests run in Docker.

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md). Open an issue before large changes.

## License

[MIT](./LICENSE)
