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
  --timeout-secs <secs> --out <dir> [--parallel N] \
  [--dataset <path>] [--instances <id1,id2,...>] \
  [--trials N] [--resume]
```

## N-variant form

Mutually exclusive with `--a/--b`:

```sh
clawmark run \
  --variant <label>=<path> ... \
  --variant-model <label>=<model> ... \
  [--variant-agent <label>=claude|cursor> ...] \
  --timeout-secs <secs> --out <dir> [--parallel N] \
  [--dataset <path>] [--instances <id1,id2,...>] \
  [--trials N] [--resume]
```

Variant paths must be regular files inside the current working directory. Agent backend defaults to `claude`.

## Task set flags

`--dataset`/`--instances`/`--trials`/`--resume` are orthogonal to the variant
input form above (two-variant or N-variant) — they can be combined with
either.

| Flag | Rule |
|---|---|
| `--dataset <path>` | Path to a JSONL file of SWE-bench Lite test-split `TaskInstance` rows. Must be a regular file. Each row is validated (`repo` slug, 40-hex `base_commit`); duplicate `instance_id`s are rejected. Defaults to the bundled 5-task smoke set when omitted. |
| `--instances <id1,id2,...>` | Comma-separated instance IDs to run, filtered from the (bundled or `--dataset`) task set. Order follows the dataset, not the flag. Empty entries (`"a,,b"`), duplicate IDs, and IDs not present in the dataset are rejected. The final selected task list must be non-empty. |
| `--trials N` | Number of trials per (variant, task) pair. Integer from 1 to 10 (default 1). |
| `--resume` | Resume an interrupted run. Requires `--out` to already point at an existing directory produced by a prior invocation with the same variants, trials, dataset, and task list (the opposite of the normal "`--out` must not exist" rule). Completed (variant, task, trial) invocations are skipped; ones that previously errored are retried. |

## Report

```sh
clawmark report --out <dir> [--show-patches]
```

Rejects directories produced by clawmark builds older than schema version 4
(pre-trials/stats) with a clear error instead of a wrong report.
