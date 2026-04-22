# Workgraph

Use workgraph for task management.

**At the start of each session, run `wg quickstart` in your terminal to orient yourself.**
Use `wg service start` to dispatch work — do not manually claim tasks.

## Development

The global `wg` command is installed via `cargo install`. After making changes to the code, run:

```
cargo install --path .
```

to update the global binary. Forgetting this step is a common source of "why isn't this working" issues when testing changes.

## Service Configuration

Configure the coordinator's executor and model with `wg config --coordinator-executor <type>` and `wg config --model <model>`. Supported executors: `claude` (default), `amplifier` (provides bundles and multi-agent delegation). Spawned agents receive `WG_EXECUTOR_TYPE` and `WG_MODEL` env vars indicating their runtime context.

## For All Agents (Including the Orchestrating Agent)

CRITICAL: Do NOT use built-in TaskCreate/TaskUpdate/TaskList/TaskGet tools.
These are a separate system that does NOT interact with workgraph.
Always use `wg` CLI commands for all task management.

CRITICAL: Do NOT use the built-in **Task tool** (subagents). NEVER spawn Explore, Plan,
general-purpose, or any other subagent type. The Task tool creates processes outside
workgraph, which defeats the entire system. If you need research, exploration, or planning
done — create a `wg add` task and let the coordinator dispatch it.

ALL tasks — including research, exploration, and planning — should be workgraph tasks.

### Cycles

Workgraph is a directed graph, NOT a DAG — it supports cycles. For repeating workflows, create ONE cycle with `--max-iterations` instead of duplicating tasks for each pass. Use `wg done --converged` to stop the cycle when the work has converged. See `wg quickstart` for examples.

### Orchestrating agent role

The orchestrating agent (the one the user interacts with directly) is a **thin orchestrator**. It does ONLY:
- **Conversation** with the user
- **Inspection** via `wg show`, `wg viz`, `wg list`, `wg status` (graph state only — NOT source files)
- **Task creation** via `wg add` with descriptions, dependencies, and context
- **Monitoring** via `wg agents`, `wg service status`, `wg watch`

It NEVER writes code, implements features, or does research itself.
It NEVER reads source files, searches code, explores the codebase, or investigates implementations.
Everything gets dispatched through `wg add` and `wg service start`.

**Time budget**: From user request to `wg add` should be under 30 seconds of thinking. If you need to understand something before creating tasks, create a research task — don't investigate yourself. Uncertainty is a signal to delegate, not to explore.

### Task description requirements

Every **code task** description MUST include a `## Validation` section with concrete test criteria.

#### Preferred: LLM verification (`--validation llm`)

Use `--validation llm` for most tasks. When the agent calls `wg done`, the task moves to
`pending-validation` and the coordinator dispatches an independent evaluator that reads the
`## Validation` checklist and the agent's artifacts to issue a pass/fail/uncertain verdict.

```
wg add "Implement feature X" --after <dep> \
  --validation llm \
  -d "## Description
<what to implement>

## Validation
- [ ] Failing test written first (TDD): test_feature_x_<scenario>
- [ ] Implementation makes the test pass
- [ ] cargo build + cargo test pass with no regressions
- [ ] <any additional acceptance criteria>"
```

#### Shell verify (`--verify`) — use only for cheap, reliable shell checks

Reserve `--verify` for cases where a specific shell command provides a fast, deterministic
pass/fail — e.g., a single named test that the task must make pass:

```
wg add "Fix auth bug" --after <dep> \
  --verify "cargo test test_auth_rejects_expired_token" \
  -d "..."
```

Do **not** use `--verify` for prose criteria (use `--validation llm` instead).

#### Human review (`--validation external`)

For tasks that require a human decision before completion:

```
wg add "Security audit" --validation external \
  -d "## Validation\n- [ ] All findings documented with severity ratings"
```

#### Research/design tasks

Specify what artifacts to produce and how to verify completeness instead of test criteria.
No `--validation` flag needed unless you want the output reviewed before the task closes.