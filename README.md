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

### Trials and statistics

Run each variant/task pair multiple times with `--trials`:

```sh
cargo run -- run --a variants/a.md --b variants/b.md \
  --model sonnet --trials 3 --timeout-secs 600 --out out
```

The leaderboard reports a Wilson 95% confidence interval on each variant's
resolve rate, and a pairwise exact McNemar p-value for every pair of
variants (paired by task + trial). With the default 5-task bundled set,
differences between variants are rarely statistically significant — for a
credible verdict, use `--trials 3` or more and/or a larger task set via
`--instances`/`--dataset` below.

### Custom task sets

By default `run` uses the bundled 5-task smoke set. Use `--dataset` to
supply a custom JSONL file of SWE-bench Lite instances, and/or
`--instances` to filter to specific instance IDs:

```sh
cargo run -- run --a variants/a.md --b variants/b.md \
  --model sonnet --dataset tasks.jsonl --timeout-secs 600 --out out

cargo run -- run --a variants/a.md --b variants/b.md \
  --model sonnet --instances astropy__astropy-12907,astropy__astropy-6938 \
  --timeout-secs 600 --out out
```

**Contract:** instances must belong to SWE-bench Lite's **test split** —
this is not validated by clawmark, and the harness step will fail (or
silently misscore) if it isn't true.

### Resuming

If a run is interrupted, re-invoke it with the same arguments plus
`--resume` and the same `--out` directory:

```sh
cargo run -- run --a variants/a.md --b variants/b.md \
  --model sonnet --timeout-secs 600 --out out --resume
```

`--resume` requires `--out` to already exist (the opposite of a fresh run)
and requires the invocation to match the original run (same variants,
trials, and task set). It skips every (variant, task, trial) invocation
already completed successfully, retries any that previously errored, and
produces the same report shape as a fresh run.

## Documentation

- [What it does](./docs/what-it-does.md)
- [CLI reference](./docs/cli.md)
- [Output layout](./docs/output.md)
- [Notes](./docs/notes.md)

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md). Open an issue before large changes.

## License

[MIT](./LICENSE)
