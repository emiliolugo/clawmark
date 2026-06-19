---
name: gsd-plan-review-convergence
description: "Cross-AI plan convergence - replan until review concerns are resolved."
---

<cursor_skill_adapter>
## A. Skill Invocation
- This skill is invoked when the user mentions `gsd-plan-review-convergence` or describes a task matching this skill.
- Treat all user text after the skill mention as `{{GSD_ARGS}}`.
- If no arguments are present, treat `{{GSD_ARGS}}` as empty.

## B. User Prompting
When the workflow needs user input, prompt the user conversationally:
- Present options as a numbered list in your response text
- Ask the user to reply with their choice
- For multi-select, ask for comma-separated numbers

## C. Tool Usage
Use these Cursor tools when executing GSD workflows:
- `Shell` for running commands (terminal operations)
- `StrReplace` for editing existing files
- `Read`, `Write`, `Glob`, `Grep`, `Task`, `WebSearch`, `WebFetch`, `TodoWrite` as needed

## D. Subagent Spawning
When the workflow needs to spawn a subagent:
- Use `Task(subagent_type="generalPurpose", ...)`
- The `model` parameter maps to Cursor's model options (e.g., "fast")
</cursor_skill_adapter>

<objective>
Cross-AI plan convergence loop — an outer revision gate around gsd-review and gsd-planner.
Repeatedly: review plans with external AI CLIs → if HIGH or actionable non-HIGH concerns remain → replan with --reviews feedback → re-review. Stops when no unresolved HIGH concerns or actionable MEDIUM/LOW findings remain outside PLAN.md, or when max cycles is reached.

**Flow:** Skill("gsd-plan-phase") → Agent→Skill("gsd-review") → check unresolved HIGH + actionable non-HIGH → Skill("gsd-plan-phase --reviews") → Agent→Skill("gsd-review") → ... → Converge or escalate

Replaces gsd-plan-phase's internal gsd-plan-checker with external AI reviewers (codex, gemini, etc.). Plan-phase runs **inline** (bare Skill at depth 0) so it can spawn gsd-planner/gsd-plan-checker at depth 1. Review runs inside an isolated Agent (gsd-review is a Bash leaf — no sub-agents needed). Orchestrator only does loop control.

**Orchestrator role:** Parse arguments, validate phase, run plan-phase inline (Skill at depth 0), spawn an Agent for gsd-review, check unresolved HIGH and actionable non-HIGH counts, stall detection, escalation gate.
</objective>

<execution_context>
@/Users/emiliolugo/Code/clawmark/.cursor/gsd-core/workflows/plan-review-convergence.md
@/Users/emiliolugo/Code/clawmark/.cursor/gsd-core/references/revision-loop.md
@/Users/emiliolugo/Code/clawmark/.cursor/gsd-core/references/gates.md
@/Users/emiliolugo/Code/clawmark/.cursor/gsd-core/references/agent-contracts.md
</execution_context>

<runtime_note>
**Copilot (VS Code):** Use `vscode_askquestions` wherever this workflow calls `conversational prompting`. They are equivalent — `vscode_askquestions` is the VS Code Copilot implementation of the same interactive question API. Do not skip questioning steps because `conversational prompting` appears unavailable; use `vscode_askquestions` instead.
</runtime_note>

<context>
Phase number: extracted from {{GSD_ARGS}} (required)

**Flags:**
- `--codex` — Use Codex CLI as reviewer (default if no reviewer specified)
- `--gemini` — Use Gemini CLI as reviewer
- `--claude` — Use Claude CLI as reviewer (separate session)
- `--opencode` — Use OpenCode as reviewer
- `--ollama` — Use local Ollama server as reviewer (OpenAI-compatible, default host `http://localhost:11434`; configure model via `review.models.ollama`)
- `--lm-studio` — Use local LM Studio server as reviewer (OpenAI-compatible, default host `http://localhost:1234`; configure model via `review.models.lm_studio`)
- `--llama-cpp` — Use local llama.cpp server as reviewer (OpenAI-compatible, default host `http://localhost:8080`; configure model via `review.models.llama_cpp`)
- `--all` — Use all available CLIs and running local model servers
- `--max-cycles N` — Maximum replan→review cycles (default: 3)

**Feature gate:** This command requires `workflow.plan_review_convergence=true`. Enable with:
`gsd config-set workflow.plan_review_convergence true`
</context>

<process>
Execute end-to-end.
Preserve all workflow gates (pre-flight, revision loop, stall detection, escalation).
</process>
