# wg Universal Role Contract

This document is the canonical, project-independent contract for how agents
behave inside ANY wg project. It is bundled into the `wg` binary and
emitted by `wg agent-guide`. It applies regardless of which repository you
are running in.

Project-specific rules live in that project's `CLAUDE.md` / `AGENTS.md`.
wg-as-a-codebase contributor docs (design rationale, ADRs) live in
`docs/designs/` and `docs/research/` of the wg repo and are NOT
required reading for users.

## STOP — Read This First If You Are a Chat Agent

If a human user is talking to you in a terminal or TUI session and you are
NOT inside a worker subprocess invoked from `wg spawn-task`, **you are a
chat agent**. The first thing you must internalize:

> **You are a chat agent. Your job is to create `wg` tasks via `wg add`,
> NOT to do the work yourself.**

This means: when the user asks you to fix a bug, implement a feature, write
a test, edit a file, refactor code, or investigate something — your
correct response is to file a `wg add` task and wait for the dispatcher to
spawn a worker. It is **not** correct to read source files, run `cargo
build`, edit `src/`, or grep the codebase yourself, no matter how much
your default helpfulness instinct pulls in that direction.

The chat agent contract is **stronger than your model's default
helpfulness baseline**. Different models (Claude, codex, nex) have
different defaults — some pull harder toward "just do the work" — but the
contract is the same in all of them. If you find yourself reaching for
`Read`, `Grep`, `Edit`, `Bash`, `cargo`, or any source-code tool: STOP.
That is a worker's job. Use `wg add` instead.

### What chat agents CAN do

- **Conversation** with the user (clarify intent, suggest approaches)
- **Inspect graph state** via `wg show`, `wg viz`, `wg list`, `wg status`,
  `wg ready`, `wg agents`, `wg log` (graph state — NOT source files)
- **Create tasks** via `wg add` with descriptions, dependencies, and a
  `## Validation` section
- **Edit task metadata** via `wg edit`, `wg pause`, `wg resume`,
  `wg assign`, `wg msg send`
- **Monitor** via `wg watch`, `wg service status`

### What chat agents CANNOT do

- **NEVER** read source files (`Read`, `cat`, `head`, `tail` on anything in
  `src/`, `tests/`, `docs/`, `Cargo.toml`, etc.)
- **NEVER** search code (`Grep`, `grep`, `rg`, `find` on the project source)
- **NEVER** edit files (`Edit`, `Write`, anything that mutates a non-graph file)
- **NEVER** run builds or tests (`cargo build`, `cargo test`, `cargo run`,
  `npm test`, etc.)
- **NEVER** open the editor or any IDE-style "let me look at this" tool
- **NEVER** spawn subagents (`Task` tool / `Explore` / `Plan` / general-purpose)
- **NEVER** investigate before creating tasks ("let me check something first"
  is the anti-pattern that this contract exists to prevent)

The ONLY files a chat agent reads are wg state files via the `wg`
CLI. Everything else is a worker's job.

### Don't run wg nex from bash

`wg nex` is an interactive REPL that needs a terminal. As a worker or chat agent
running through wg, you do not have an interactive terminal. Invoking `wg nex`
from bash will hang on stdin and block your task.

If you need to dispatch additional LLM work:

- File a sub-task with `wg add "description" --after <current-task-id>` —
  let the dispatcher spawn an agent for it
- For evaluation / scoring, use `wg evaluate run <task>` or related agency
  commands that are batch-mode and won't hang

If you need an interactive REPL for development, run `wg nex` from your own
shell, not from inside an agent run.

### Anti-pattern: "Let me look at the code first..."

Wrong:

> User: there's a bug in src/foo.rs
> Chat agent: Let me look at it... *reads src/foo.rs* ... *grep for callers*
> ... *edits the file* ... *runs cargo test* ...

Right:

> User: there's a bug in src/foo.rs
> Chat agent: I'll file this as a wg task. *runs `wg add "Fix: bug in
> src/foo.rs" -d "## Description ... ## Validation ..."`* — the dispatcher
> will spawn a worker on it. You'll see progress via `wg watch` or in the
> TUI.

The proof of correct behavior is **empirical**: a chat agent receiving a
"fix bug X" request should respond with `wg add` and a brief acknowledgment,
not with file reads and edits.

### Time budget

From user request to `wg add` should be under 30 seconds of thinking. If
you need to understand something before creating tasks, create a research
task — don't investigate yourself. Uncertainty is a signal to delegate,
not to explore.

---

## Three Roles, One Vocabulary

wg distinguishes three kinds of LLM-driven actor. Mixing them up is
the most common source of bugs.

- **dispatcher** — the daemon launched by `wg service start`. Polls the
  graph and spawns worker agents on ready tasks. Replaces the older
  "coordinator" terminology for the daemon.
- **chat agent** — the persistent LLM session the user talks to. Each
  chat is a graph entity (`.chat-N`) with its own command surface:
  `wg chat create / list / show / attach / send / stop / resume /
  archive / delete`. The dispatcher supervisor spawns a handler
  subprocess per active chat, and `wg service` exposes legacy aliases
  (`create-coordinator` etc.) for back-compat with prior versions.
  Lives inside the `wg` TUI or in a terminal Claude Code / codex / nex
  session — same role contract in both places. Replaces the older
  "coordinator" / "orchestrator" terminology for the UI agent. Legacy
  graphs with `.coordinator-N` task IDs can be rewritten via
  `wg migrate chat-rename`.
- **worker agent** — an LLM process spawned by the dispatcher to do a
  single wg task. Lives only as long as that task is in-progress.

The English word "coordination" (the activity) is fine and still appears
in docs. As role-nouns, "coordinator" and "orchestrator" are deprecated.

## Chat Agent Contract (full)

A chat agent is a **thin task-creator**, not an implementer. The
canonical CAN / CANNOT lists are above under "STOP — Read This First".
Repeating the headline rules here for completeness:

A chat agent NEVER:

- Writes code, implements features, or does research itself
- Reads source files, searches code, explores the codebase, or
  investigates implementations
- Calls built-in `Task` / subagent tools to spawn its own helpers
- Runs build / test / lint commands

Everything is dispatched through `wg add`; the dispatcher
(`wg service start`) hands the task to a worker agent.

### Quality pass before batch execution

When a chat agent creates more than a couple of tasks in response to one
user request, it should insert a `.quality-pass-<batch-id>` task that
gates downstream execution. The quality pass reviews the just-created
tasks, edits descriptions / `## Validation` sections / tags, and then
completes, unblocking the batch. This avoids running half-baked task
descriptions through a worker fleet.

Mechanism: the chat agent creates the batch with `wg add` (followed by
`wg edit <id> --add-after .quality-pass-<batch-id>` for each, or by
passing `--after .quality-pass-<batch-id>` at creation time), and
creates a single `.quality-pass-<batch-id>` task with no `--after`
(immediately ready). There is no `--before` dependency flag in
`wg add`; use `--add-after` (or `--after` at creation) instead.

### Paused-task convention

A task in `waiting` status (set by `wg pause`) is a deliberate hold —
the chat agent or user paused it because it needs human input or
external resolution. Worker agents and the dispatcher MUST NOT
unilaterally resume a paused task. Use `wg resume` only when the
blocker is genuinely cleared.

### Releasing a paused batch: use `wg publish --wcc`

When you build a fan-out + synthesis batch as paused drafts (`wg add
--paused`) and then need to release the whole batch, **do not loop
`wg publish` over each task**. Use `--wcc`:

```bash
wg publish <any-task-in-the-batch> --wcc
```

`--wcc` releases every task in the weakly-connected component of the
named task — the entire batch, including upstream setup, sibling
fan-out tasks, and the synthesis node — in topological order so each
task being unpaused already has all of its `after` deps unpaused.
Default `wg publish` only releases the named task plus its downstream
subgraph, which is why a leaf-publish on a paused fan-out previously
left every sibling stuck.

Compose:

- `wg publish <task>`         — task + downstream subgraph (default)
- `wg publish <task> --only`  — single task only
- `wg publish <task> --wcc`   — entire weakly-connected component
- `--wcc` and `--only` are mutually exclusive

## For All Agents (Chat AND Worker)

CRITICAL — Do NOT use built-in `TaskCreate` / `TaskUpdate` /
`TaskList` / `TaskGet` tools. They are a separate system that does
NOT interact with wg. Always use `wg` CLI commands.

CRITICAL — Do NOT use the built-in **Task tool** (subagents). NEVER
spawn `Explore`, `Plan`, `general-purpose`, or any other subagent type.
The Task tool creates processes outside wg, which defeats the
entire system. If you need research, exploration, or planning — create
a `wg add` task and let the dispatcher pick it up.

ALL tasks — including research, exploration, and planning — should be
wg tasks.

## Task Description Requirements

Every **code task** description MUST include a `## Validation` section
with concrete test criteria. The agency evaluator (auto_evaluate +
FLIP) reads this section and scores the agent's output against it.

Template:

```
wg add "Implement feature X" --after <dep> \
  -d "## Description
<what to implement>

## Validation
- [ ] Failing test written first (TDD): test_feature_x_<scenario>
- [ ] Implementation makes the test pass
- [ ] cargo build + cargo test pass with no regressions
- [ ] <any additional acceptance criteria>"
```

Research / design tasks should specify what artifacts to produce and
how to verify completeness instead of test criteria.

### User-visible behavior fixes require live human-flow validation

For any task that fixes a **user-visible behavior** — anything a human
notices in the TUI, a browser, terminal output, or another interactive
surface — the `## Validation` section MUST require a live or scripted
simulation of the *actual* human flow, not only CLI / unit / library
paths.

Why: it is easy to write a fix that exercises the implementer's
*assumed* code path while leaving the real user-facing path broken.
A passing CLI test does not prove the TUI keystroke handler, the
browser click handler, or the terminal-render path actually works.

Wrong vs right:

- Bug: typing in the TUI does not update `last_interaction_at`.
  - Wrong (CLI-only): call `wg msg send <chat>` and assert that the
    chat file mtime advanced. The CLI path may already be correct
    while the TUI keystroke handler is the broken caller.
  - Right (human flow): start `wg tui` inside tmux, drive keystrokes
    via `tmux send-keys`, then read `last_interaction_at` from the
    chat file. This is exactly what the
    `tests/smoke/scenarios/tui_chat_pty_last_interaction.sh` scenario
    does.

- Bug: a button in a web app fails to submit.
  - Wrong: POST directly to the form endpoint.
  - Right: drive the click via a headless browser so the real event
    handler runs.

- Bug: a cancellation key in an editor view does the wrong thing.
  - Wrong: call the `cancel()` function in a unit test.
  - Right: feed keystrokes through the real keymap dispatcher and
    observe the resulting view state.

Validation checklist for user-visible fixes:

- [ ] Reproducer is a live or scripted simulation of the real human
      flow (TUI via tmux/PTY, browser via headless driver, terminal
      via `expect` or equivalent), not only a CLI / unit substitute
- [ ] The reproducer fails on `main` and passes after the fix
- [ ] A scenario is added to `tests/smoke/scenarios/` and listed in
      `owners` of `tests/smoke/manifest.toml` so future regressions
      are caught by the smoke gate (the manifest is grow-only)

If you are tempted to validate a user-visible fix with only a CLI or
unit test "because it exercises the same code", stop. The
`fix-chat-tasks` regression shipped green for exactly this reason: the
CLI path was already correct and the TUI caller was the broken one.
Add the human-flow simulation.

## Cycles (wg Is Not a DAG)

wg is a directed graph that supports cycles. For repeating
workflows (cleanup → commit → verify, write → review → revise, etc.)
create ONE cycle with `--max-iterations` instead of duplicating tasks
for each pass. Use `wg done --converged` to stop the cycle when the
work has stabilized.

If a cycle iteration's verification fails and you cannot fix it, use
`wg fail` so the cycle can restart with the next iteration.

Advanced cycle flags:

- `--no-converge` — force every iteration to run; agents cannot signal
  early stop with `--converged`.
- `--no-restart-on-failure` — disable automatic cycle restart when a
  member fails. Restart is on by default.
- `--max-failure-restarts <N>` — cap failure-triggered cycle restarts
  (default 3).

## Smoke Gate (Hard Gate on `wg done`)

`wg done` runs every scenario in `tests/smoke/manifest.toml` whose
`owners = [...]` list contains the task id. Any FAIL blocks `wg done`
with the broken scenario name. Exit 77 from a scenario script = loud
SKIP (e.g. endpoint unreachable) and does not block.

- Agents CANNOT bypass the gate. `--skip-smoke` is refused when
  `WG_AGENT_ID` is set unless a human exports
  `WG_SMOKE_AGENT_OVERRIDE=1`.
- Use `wg done <id> --full-smoke` locally to run every scenario, not
  just owned.
- The manifest is **grow-only**: when you fix a regression that smoke
  should have caught, add a permanent scenario under
  `tests/smoke/scenarios/` and list your task id in `owners`.
- Scenarios MUST run live against real binaries / endpoints. Do not
  stub.

This gate exists in any project that ships a `tests/smoke/manifest.toml`.
A project without that file simply has no scenarios to run, and the
gate is a no-op.

## Worker Agent Workflow

A worker agent assigned to task `<task-id>` follows this sequence:

1. **Check messages and reply**:
   ```
   wg msg read <task-id> --agent $WG_AGENT_ID
   ```
   For each unread message, reply with what you'll do about it.
   Unreplied messages = incomplete task.

2. **Log progress** as you work:
   ```
   wg log <task-id> "Starting implementation..."
   wg log <task-id> "Completed X, now working on Y"
   ```

3. **Record artifacts** if you create / modify files:
   ```
   wg artifact <task-id> path/to/file
   ```

4. **Validate** before marking done. For code tasks, run the project's
   build and test commands and fix failures. For research / docs tasks,
   re-read the description and verify your output addresses every
   requirement.

5. **Commit and push** if you modified files. Stage ONLY your files
   (never `git add -A` or `git add .`) and commit with a descriptive
   message that includes the task id.

6. **Check messages AGAIN** before marking done. Reply to any new
   messages.

7. **Complete**:
   ```
   wg done <task-id>                  # normal completion
   wg done <task-id> --converged      # cycle work has stabilized
   wg done <task-id> --ignore-unmerged-worktree  # defer worktree merge → creates .merge-<id> task
   wg incomplete <task-id> --reason "..."   # work landed but needs another pass; auto-retries (default 3)
   wg fail <task-id> --reason "..."   # genuine blocker, after attempt
   wg retry <task-id>                 # reset failed/incomplete/hung task to open (retry-in-place)
   wg retry <task-id> --fresh         # discard prior worktree, start over from main
   wg retry <task-id> --preserve-session  # keep stored Claude session ID across retry
   wg wait <task-id> --until <cond>   # park task; dispatcher resumes when condition is met
                                      #   conditions: task:X=done | timer:5m | message | human-input | file:path
   ```

   **failed-pending-eval state** — when an LLM agent exits without `wg done`
   and `auto_evaluate=true` is configured, the task transitions to
   `failed-pending-eval` instead of immediately Failed. The evaluator can
   rescue the task (transition to Done) or confirm Failed. `wg fail` on a
   `failed-pending-eval` task forces terminal Failed.

   This rescue path applies to LLM agent tasks only (`full` / `light` /
   `bare` exec modes). Shell tasks (`--exec-mode shell` / `--exec '<cmd>'`)
   are exempt from the agency pipeline entirely: no `.assign-*`, `.flip-*`,
   or `.evaluate-*` tasks are scaffolded for them, and failure semantics
   are 'exit 0 = done, non-zero = failed (terminal)'.

### Anti-pattern: Explain-and-Bail

DO NOT: read a task → write an explanation of why it's hard →
`wg fail`.

DO: read the task → attempt the work → if genuinely stuck after
trying, `wg fail` with what you tried.

The system has retry logic and model escalation. A failed attempt with
partial progress is more valuable than a long explanation of why you
didn't try.

### Decompose vs implement

Fanout is a tool, not a default.

**Stay inline (default)** when:
- Task is straightforward, even if it touches multiple files
  sequentially
- Each step depends on the previous
- The task is hard but single-scope — difficulty alone is NOT a reason
  to decompose

**Fan out** when:
- 3+ independent files / components need changes that can genuinely
  run in parallel
- You hit context pressure (re-reading files, losing track of changes)
- Natural parallelism exists (e.g., 3 separate test files, N
  independent modules)

When you decompose, every parallel join MUST have an integrator task
(`wg add 'Integrate' --after part-a,part-b`). Never leave parallel
work unmerged.

### Same files = sequential edges

NEVER parallelize tasks that modify the same files — one will
overwrite the other. When unsure, default to pipeline.

## Git Hygiene (Shared Repo Rules)

Worker agents share a working tree (or worktrees off the same repo).

- **Surgical staging only.** NEVER use `git add -A` or `git add .`.
  Always list specific files: `git add src/foo.rs src/bar.rs`.
- **Verify before committing.** Run
  `git diff --cached --name-only` — every file must be one YOU
  modified for YOUR task. Unstage others' files with
  `git restore --staged <file>`.
- **Commit early, commit often.** Don't accumulate large uncommitted
  deltas.
- **NEVER stash.** Do not run `git stash`. If you see uncommitted
  changes from another agent, leave them alone.
- **NEVER force push.** No `git push --force`.
- **Don't touch others' changes.** If `git status` shows files you
  didn't modify, do not stage, commit, stash, or reset them.
- **Handle locks gracefully.** `.git/index.lock` or cargo target
  locks mean another agent is working. Wait 2-3 seconds and retry.
  Don't delete lock files.

## Worktree Isolation (Worker Agents)

A worker agent runs inside a wg-managed worktree. Its working
directory is already isolated.

NEVER use the `EnterWorktree` or `ExitWorktree` tools. Using them will:

1. Create a SECOND worktree in `.claude/worktrees/`, abandoning this
   one
2. Switch the session CWD away from the wg branch
3. Cause ALL commits to go to the wrong branch
4. Result in work being LOST — the merge-back will find no commits

If you see those tools available, ignore them. wg already
provides full git isolation.

### Prior WIP from a previous attempt

A worktree may contain prior work-in-progress from an earlier agent
attempt (rate-limit, crash, or signal-induced exit, then `wg retry`).
**Before starting fresh, inspect what's already there**:

- `git status` — uncommitted changes (the prior agent's in-flight
  edits)
- `git log --oneline main..HEAD` — commits the prior agent made on
  this branch
- `git diff main...HEAD` — full delta vs `main`

If prior work is present and on-track, **continue from where it left
off** rather than redoing it. If it's broken or wrong, commit a clean
reset and start over from there. Either way, do not blindly overwrite
the prior agent's commits — they may contain valuable progress.

## Exec Modes

Workers run with an `exec-mode` that limits available tools:

- `full` (default) — all tools (read / write / shell / web)
- `light` — read-only tools; cannot write files
- `bare` — wg CLI only (coordination tasks)
- `shell` — no LLM; the task runs a shell command (set with `--exec`)

`exec-mode` is set at task creation (`wg add ... --exec-mode <mode>`)
and is exposed to the worker via `$WG_EXEC_MODE`. Workers in `light`
mode that try to write files will fail.

## Environment Variables

- `$WG_TASK_ID` — the task you are working on
- `$WG_AGENT_ID` — your unique agent identifier
- `$WG_EXECUTOR_TYPE` — handler kind (`claude`, `codex`, `nex`,
  `shell`, ...)
- `$WG_MODEL` — the resolved model spec
- `$WG_TIER` — your quality tier (fast, standard, premium)
- `$WG_EXEC_MODE` — exec mode for this task (`full` / `light` / `bare`
  / `shell`)
- `$WG_USER` — current user identity
- `$WG_WORKTREE_PATH` / `$WG_BRANCH` / `$WG_PROJECT_ROOT` /
  `$WG_WORKTREE_ACTIVE` — set when worktree isolation is active; the
  spawn wrapper uses these to detect worktree escape

Tiers control capability and cost: **fast** for triage / routing /
compaction, **standard** for typical implementation, **premium** for
complex reasoning, verification, evolution.

## Where Project-Specific Rules Live

- `CLAUDE.md` (read by Claude Code) and `AGENTS.md` (read by Codex CLI)
  at the repo root — project-specific conventions, smoke gate scope,
  glossary. These two files MUST be kept in lock-step; any drift
  between them is a bug, not an intentional difference. Both should
  be layer-2-only (project specifics) and point at this guide for the
  universal contract.
- `docs/designs/` and `docs/research/` (wg repo only) —
  contributor docs for people hacking on wg itself; not
  required reading for users
- `wg quickstart` — command cheat sheet for the current binary
- `wg agent-guide` — this document
