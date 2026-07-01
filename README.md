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

## Documentation

- [What it does](./docs/what-it-does.md)
- [CLI reference](./docs/cli.md)
- [Output layout](./docs/output.md)
- [Notes](./docs/notes.md)

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md). Open an issue before large changes.

## License

[MIT](./LICENSE)
