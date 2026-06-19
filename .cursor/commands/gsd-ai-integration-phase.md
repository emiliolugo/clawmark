<objective>
Create an AI design contract (AI-SPEC.md) for a phase involving AI system development.
Orchestrates gsd-framework-selector → gsd-ai-researcher → gsd-domain-researcher → gsd-eval-planner.
Flow: Select Framework → Research Docs → Research Domain → Design Eval Strategy → Done
</objective>

<execution_context>
@/Users/emiliolugo/Code/clawmark/.cursor/gsd-core/workflows/ai-integration-phase.md
@/Users/emiliolugo/Code/clawmark/.cursor/gsd-core/references/ai-frameworks.md
@/Users/emiliolugo/Code/clawmark/.cursor/gsd-core/references/ai-evals.md
</execution_context>

<context>
Phase number: {{GSD_ARGS}} — optional, auto-detects next unplanned phase if omitted.
</context>

<process>
Execute end-to-end.
Preserve all workflow gates.
</process>
