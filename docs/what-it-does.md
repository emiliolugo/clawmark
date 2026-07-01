# What it does

For each variant and task:

1. Clone the SWE-bench repo at the task's base commit
2. Inject variant contents as both `CLAUDE.md` and `AGENTS.md`
3. Invoke the selected agent (`claude` or `cursor-agent`) with the problem statement
4. Collect `git diff HEAD` as the patch
5. Run the SWE-bench harness once per variant

The report ranks variants by resolve rate (then cost-per-resolve). Token/cost totals are `n/a` when the agent CLI does not provide them (always for Cursor).
