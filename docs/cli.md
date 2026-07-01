# CLI

| Command | Purpose |
|---|---|
| `clawmark doctor` | Check Docker, Python, swebench, git, agent CLIs |
| `clawmark run ...` | Run benchmark; writes `out/` and prints report |
| `clawmark report --out <dir>` | Re-print report from existing output |

## Two-variant form

```sh
clawmark run --a <path> --b <path> --model <model> \
  [--model-a <model>] [--model-b <model>] \
  [--agent claude|cursor] [--agent-a ...] [--agent-b ...] \
  --timeout-secs <secs> --out <dir> [--parallel N]
```

## N-variant form

Mutually exclusive with `--a/--b`:

```sh
clawmark run \
  --variant <label>=<path> ... \
  --variant-model <label>=<model> ... \
  [--variant-agent <label>=claude|cursor> ...] \
  --timeout-secs <secs> --out <dir> [--parallel N]
```

Variant paths must be regular files inside the current working directory. Agent backend defaults to `claude`.
