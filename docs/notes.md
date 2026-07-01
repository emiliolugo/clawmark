# Notes

- **Scope:** 5-task bundled smoke set by default, with `--dataset`/`--instances` for custom SWE-bench Lite test-split subsets, `--trials` for repeated runs, and `--resume` for interrupted runs (see [the CLI reference](./cli.md)). Still no config file or cloud execution.
- **Timeouts:** `--timeout-secs` is per agent invocation, not a token budget. Use `600+` for first runs.
- **Doctor:** Missing Claude/Cursor CLI is a warning — you only need the backend you select.
- **Security:** Agents run on the host with full permissions. SWE-bench tests run in Docker.
