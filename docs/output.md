# Output

```text
out/
  variants.json               # manifest (label, path, hash, model, agent)
  run_meta.json                # schema_version, trials, dataset_source, instance_ids
  run_records.jsonl            # per (variant, task, trial) attempt
  predictions/
    <label>-t<N>.jsonl          # harness input, one file per variant per trial
  harness/
    <label>-t<N>.json           # harness output, one file per variant per trial
  report.json
```

`schema_version` is `4`. `report --out <dir>` rejects directories produced
by older clawmark builds (schema < 4) with a clear error instead of
misreporting; there is no backwards-compatible reading of schema-3 output.

## `run_meta.json`

```json
{
  "schema_version": 4,
  "trials": 3,
  "dataset_source": "bundled",
  "instance_ids": ["astropy__astropy-12907", "..."]
}
```

`dataset_source` is `"bundled"` for the default smoke set, or the
canonicalized `--dataset` path otherwise. `instance_ids` is the resolved,
ordered task list actually used for the run (after `--instances` filtering).

## Per-trial paths

Predictions and harness output are named `<label>-t<trial>.jsonl` /
`<label>-t<trial>.json` (1-indexed trials), e.g. `predictions/a-t1.jsonl`,
`harness/a-t2.json`. The harness run id is `clawmark-<label>-t<trial>`.

## `report.json` fields

```json
{
  "schema_version": 4,
  "total_tasks": 5,
  "trials": 3,
  "variants": [
    {
      "label": "a",
      "model": "sonnet",
      "resolved": 9,
      "n_invocations": 15,
      "resolve_rate": 0.6,
      "ci_low": 0.357,
      "ci_high": 0.802,
      "elapsed_secs": 123.4,
      "input_tokens": 1000,
      "output_tokens": 500,
      "cost_usd": 1.23,
      "cost_per_resolve": 0.1367
    }
  ],
  "per_task": [
    { "instance_id": "astropy__astropy-12907", "resolved_counts": [3, 1] }
  ],
  "pairwise": [
    {
      "a_label": "a",
      "b_label": "b",
      "a_only": 6,
      "b_only": 1,
      "both": 3,
      "neither": 5,
      "p_value": 0.125
    }
  ]
}
```

- `n_invocations` = `total_tasks * trials` for that variant.
- `ci_low`/`ci_high` are a 95% Wilson score confidence interval on
  `resolved / n_invocations`.
- `per_task[].resolved_counts` is the number of resolved trials per variant
  (aligned with the `variants` array order), out of `trials`.
- `pairwise` covers every pair of variants (in leaderboard order); `a_only`/
  `b_only`/`both`/`neither` count (task, trial) pairs, and `p_value` is the
  two-sided exact McNemar p-value (`null` when there are no discordant
  pairs, i.e. `a_only + b_only == 0`).
