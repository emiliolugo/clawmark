# Notes

- **Scope:** 5 bundled smoke tasks only; no config file, resume, or cloud execution.
- **Timeouts:** `--timeout-secs` is per agent invocation, not a token budget. Use `600+` for first runs.
- **Doctor:** Missing Claude/Cursor CLI is a warning — you only need the backend you select.
- **Security:** Agents run on the host with full permissions. SWE-bench tests run in Docker.
