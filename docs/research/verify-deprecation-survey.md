# --verify Deprecation Survey

**Date**: 2026-04-21  
**Task**: research-verify-deprecation  
**Purpose**: Factual basis for a deprecation design. Not a design document.

---

## 1. --verify Surface Area

### 1a. CLI Flags

| Command | Flag | File:Line | Summary |
|---------|------|-----------|---------|
| `wg add` | `--verify <cmd>` | `src/cli.rs:232` | Attaches a shell verify command to a new task |
| `wg add` | `--verify-timeout <dur>` | `src/cli.rs:235-236` | Per-task timeout override (e.g., "15m", "900s") |
| `wg done` | `--skip-verify` | `src/cli.rs:454` | Human escape hatch; agents are blocked from using this |
| `wg edit` | `--verify <cmd>` | `src/commands/edit.rs:37` | Update verify command on existing task |

No other commands accept `--verify` directly. The `wg reset --also-strip-meta` flag docs
(`src/cli.rs:71,88`) mention cleaning up `.verify-*` and `.verify-deferred-*` system tasks.

### 1b. Graph Model Fields

| Field | Type | File:Line | Summary |
|-------|------|-----------|---------|
| `Task.verify` | `Option<String>` | `src/graph.rs:305` | The shell command to run as gate |
| `Task.verify_timeout` | `Option<String>` | `src/graph.rs:309` | Per-task timeout override |
| `Task.verify_failures` | `u32` | `src/graph.rs:390` | Circuit-breaker consecutive-failure counter |

These are all serialized to JSONL via the deserialization helper at `src/graph.rs:949-1002`.

### 1c. Gate Logic (Where verify is executed)

All gate execution lives in `src/commands/done.rs`.

| Function | Lines | Summary |
|----------|-------|---------|
| `run_inner` | `721-714` | Main `wg done` entry: decides inline / separate / skip |
| Verify autospawn block | `776-860` | Deprecated: defers verify to `.verify-deferred-<id>` shadow task when task has children; gated by `verify_autospawn_enabled` (default false) |
| Inline gate | `891-927` | Checks `verify_mode`: if "separate" → set `PendingValidation`; else run inline |
| `run_verify_command_with_retry` | `200-330` | Retry wrapper for lock contention |
| `run_verify_command` | `486-712` | Actual shell execution: scoped-verify, smart-verify routing, timeout handling, triage |
| `generate_scoped_verify_command` | `333-369` | Scopes `cargo test` to modified files when `scoped_verify_enabled=true` |
| `is_free_text_verify_command` | `371-442` | Detects prose commands and routes to LLM evaluation fallback |
| `run_llm_verify_evaluation` | `444-481` | Smart-verify fallback: calls `wg evaluate` when command looks like prose |
| `resolve_verify_timeout` | `24-49` | Priority: task.verify_timeout → WG_VERIFY_TIMEOUT env → coordinator default |
| External validation block | `1275-1321` | Sets `PendingValidation` when `task.validation = "external"` (independent of `task.verify`) |

Coordinator side (`src/commands/service/coordinator.rs`):

| Function | Lines | Summary |
|----------|-------|---------|
| `build_separate_verify_tasks` | `1974-2169` | Finds `PendingValidation` tasks with `verify` + matching log entry, spawns `.sep-verify-<id>` agent tasks |
| FLIP verify injection | `1692-1957` | When FLIP score is below threshold, injects `.verify-<source_id>` tasks that inherit the source's `verify` command and run it independently |

### 1d. Status Machinery (pending-validation transitions)

| Location | File:Line | Direction |
|----------|-----------|-----------|
| `wg done` (inline mode) | `src/commands/done.rs:918` | → PendingValidation (verify_mode=separate) |
| `wg done` (external validation) | `src/commands/done.rs:1287` | → PendingValidation (validation="external") |
| `wg approve` | `src/commands/approve.rs:30` | PendingValidation → Done |
| `wg reject` | `src/commands/reject.rs:34` | PendingValidation → Open (or Failed if max_rejections exceeded) |
| `query.rs` | `src/query.rs:99` | PendingValidation counted same as Failed/Abandoned/Waiting (not "open") |
| `compactor.rs` | `src/service/compactor.rs:267` | PendingValidation counted as "waiting" in status stats |
| `coordinator.rs` dispatch | `src/commands/service/coordinator.rs:1984` | `build_separate_verify_tasks` scans for PendingValidation |
| `is_terminal()` | `src/graph.rs:2048` | PendingValidation is NOT terminal |

`wg reset` reopens `PendingValidation` tasks to `Open` (`src/commands/reset.rs:23`).  
`wg reject` reopens to `InProgress` (or `Open`) after clearing the assignment.

### 1e. Config Fields

All in `CoordinatorConfig` (`src/config.rs`):

| Field | Default | Line | Summary |
|-------|---------|------|---------|
| `verify_mode` | `"inline"` | 2657 | "inline" or "separate" |
| `verify_autospawn_enabled` | `false` | 2671 | Deprecated .verify-deferred-* autospawn |
| `max_verify_failures` | 7 (see fn at 2864) | 2678 | Circuit-breaker threshold |
| `verify_default_timeout` | None | 2682 | Default timeout if not set per-task or via env |
| `verify_triage_enabled` | true | 2690 | Triage on timeout to detect hang vs wait |
| `verify_progress_timeout` | None | 2694 | Max time without output before triage kicks in |
| `scoped_verify_enabled` | true | 2726 | Auto-scope `cargo test` to modified files |
| `auto_test_discovery` | false | 2718 | Auto-populate `--verify` from discovered test files |

### 1f. Agent Prompt Injection

The `task.verify` content is injected into the agent prompt as a `## Verification Required` section:

- `src/service/executor.rs:934-939` — appends verify block for all scope levels
- `src/service/executor.rs:1103,1177,1180,1313` — `TemplateVars.task_verify` → `{{task_verify}}` in template

### 1g. Tests Exercising Verify

In `src/commands/done.rs` (inline unit tests):

| Test Name | Line | What it covers |
|-----------|------|----------------|
| `test_done_separate_verify_transitions_to_pending_validation` | 2723 | separate mode → PendingValidation |
| `test_done_inline_verify_still_works` | 2759 | inline mode → Done on pass |
| `test_verify_circuit_breaker_distinguishes_from_agent_failures` | 2660 | actor tagging on failures |
| `test_verify_circuit_breaker_configurable_threshold` | 2693 | config controls threshold |
| `test_is_free_text_verify_command_*` | 2854-2898 | prose vs command detection |
| `test_smart_verify_routes_free_text_to_evaluation` | 2901 | smart-verify routing |
| `test_done_defers_verify_when_task_has_children` | 2968 | deprecated autospawn path |
| `test_done_skip_verify_bypasses_gate` | 2234 | --skip-verify (human) |
| `test_done_skip_verify_blocked_for_agents` | 2253 | --skip-verify blocked for agents |
| `test_done_external_validation_transitions_to_pending` | 2319 | external validation path |

In `src/commands/service/coordinator.rs`:

| Test Name | Line | What it covers |
|-----------|------|----------------|
| `test_separate_verify_task_created_for_pending_validation` | 6027 | coordinator spawns .sep-verify-* |

Integration test files:

| File | What it covers |
|------|----------------|
| `tests/integration_verify_first.rs` | verify-first eval pipeline, FLIP ordering |
| `tests/test_verify_lint_integration.rs` | verify command lint/auto-correct |
| `tests/test_verify_timeout_basic.rs` | timeout parsing |
| `tests/test_verify_timeout_functionality.rs` | timeout enforcement |
| `tests/test_prompt_logging_debug.rs:137,228` | verify injected into agent prompt |

In `src/verify_lint.rs`:

Full module of lint helpers: `print_warnings`, `auto_correct_verify_command`, `is_free_text_verify_command`. These run at `wg add` and `wg edit` time.

### 1h. Documentation

| File | Line(s) | Summary |
|------|---------|---------|
| `CLAUDE.md` | 55, 60 | Instructs agents to use `--verify` for hard gates |
| `docs/AGENT-LIFECYCLE.md` | 24, 36, 54, 73 | Lists `verify` field, pending-validation status, approve/reject commands |
| `src/executor/native/tools/wg.rs` (quickstart) | 720-767 | Explains `--verify` and pending-validation in agent guide |
| `src/service/executor.rs` (decomp templates) | 383-409 | Agent templates include `--verify` in pipeline/fan-out examples |
| `docs/research/verify-cycle-interaction.md` | — | Research doc on verify + cycle interaction |

---

## 2. .evaluate Deputization Status

### Can `.evaluate` tasks invoke `wg rescue` (or equivalent) to inject tasks?

**YES — three paths exist.**

#### Path 1: Auto-rescue in `evaluate run` (programmatic)

`src/commands/evaluate.rs:1444-1484`

When `config.agency.auto_rescue_on_eval_fail = true` and the evaluation score is below
threshold, `evaluate_run_inner` calls `super::rescue::run()` directly. The evaluator's
notes become the rescue task's description. The rescue task is injected parallel to the
failed target with its downstream edges rewired (`insert::Position::Parallel, replace_edges=true`).

This is a **programmatic path** — the `.evaluate` agent does not need to do anything;
it happens automatically based on the score.

#### Path 2: Native tool `wg_rescue`

`src/executor/native/tools/wg.rs:723-853`

All native-executor agents (including `.evaluate` agents) receive the `wg_rescue` tool
(`register_wg_tools` at line 34). The tool calls `wg rescue <target> --description "..."` as a subprocess.

`wg rescue` internally calls `insert::run(Position::Parallel, ..., replace_edges=true)`
(`src/commands/rescue.rs`), injecting a new task at the same graph slot with edges
rewired to the rescue task.

So: evaluate agents **can inject corrective tasks parallel to a reference task** via `wg_rescue`.

#### Path 3: Native tool `wg_add`

`src/executor/native/tools/wg.rs:259-540`

All native-executor agents can create tasks with `after` dependencies via `wg_add`. This
creates tasks **serial to** (downstream of) a reference task. The `after` parameter accepts
comma-separated task IDs.

So: evaluate agents **can inject tasks serial to a reference task** via `wg_add`.

### Can context from evaluation be passed into the injected task?

**YES** (via rescue path): `evaluate.rs:1446-1452` shows the rescue description includes
the evaluation score, threshold, and evaluator notes verbatim. Evaluate agents using
`wg_rescue` via the native tool also control the `description` parameter.

### What's missing for evaluate to REPLACE --verify?

The current evaluate mechanism can inject corrective tasks when evaluation fails — but
it does NOT replace the verify gate role. Specifically:

1. **No `wg_approve` / `wg_reject` native tools**: The `register_wg_tools` function
   (`src/executor/native/tools/wg.rs:18-41`) provides: `wg_show`, `wg_list`, `wg_add`,
   `wg_done`, `wg_fail`, `wg_rescue`, `wg_log`, `wg_artifact`. There is no `wg_approve`
   or `wg_reject` tool. So a `.evaluate` agent running as native executor cannot formally
   approve/reject a pending-validation task without invoking the CLI as a subprocess.

2. **The sep-verify agent uses `wg approve`/`wg reject` via CLI**: The separate verify
   path (`coordinator.rs:2080-2094`) instructs the `.sep-verify-<id>` agent to call
   `wg approve <task_id>` or `wg reject <task_id>` as shell commands. The claude executor
   agents can do this as shell commands.

3. **What would need to be built for evaluate to fully replace verify**:
   - Add `wg_approve` and `wg_reject` as native tools (or teach evaluate agents to call
     them as shell commands via an existing generic shell tool)
   - OR: define the evaluate agent's failure path as calling `wg rescue` (already exists)
     and success path as calling `wg done` on itself (already exists), and ensure the
     coordinator wires the evaluate result back to approve/reject the source task.
   - The `auto_rescue_on_eval_fail` mechanism is close but doesn't handle the case where
     the source task is in `PendingValidation` (it acts on `failed` tasks, not
     `pending-validation` ones).

---

## 3. pending-validation Status Usage

### Is `pending-validation` used for anything other than --verify gating?

**YES — two independent use cases share the status.**

#### Use Case 1: verify shell command awaiting separate agent

Trigger: `verify_mode = "separate"` AND task has `task.verify` set AND `wg done` is called.

- `done.rs:902-927`: sets status to `PendingValidation`
- `coordinator.rs:1974-2169`: `build_separate_verify_tasks` spawns `.sep-verify-<id>` agent
- `.sep-verify` agent runs the verify command and calls `wg approve` or `wg reject`

#### Use Case 2: external/manual hold

Trigger: `task.validation = "external"` (a distinct field from `task.verify`).

- `done.rs:1275-1321`: sets status to `PendingValidation` when called on a task with `validation="external"`
- No automatic agent is spawned; requires human `wg approve` or `wg reject`

#### Use Case 3: inline verify mode (NOT PendingValidation)

In the default `verify_mode = "inline"`, the verify command runs synchronously inside
`wg done`. If it passes, status goes directly to `Done`. If it fails, the task stays
`InProgress` (or gets `verify_failures` incremented toward circuit-breaker `Failed`).
**PendingValidation is never set in inline mode.**

#### FLIP-triggered verification tasks

The coordinator's FLIP injection (`coordinator.rs:1692-1957`) creates `.verify-<id>` tasks
with `Status::Open` — they are normal open tasks that happen to inherit the verify command.
They do NOT use `PendingValidation`.

#### Summary

| Trigger | PendingValidation? | Notes |
|---------|-------------------|-------|
| `verify_mode=separate` + `task.verify` | YES | Separate agent runs verify |
| `task.validation=external` | YES | Manual human approval |
| `verify_mode=inline` + `task.verify` | NO | Runs inline, goes direct to Done/Failed |
| FLIP-triggered `.verify-<id>` task | NO | Regular Open task |
| `verify_autospawn_enabled` (deprecated) | NO | Creates `.verify-deferred-<id>` open tasks |

---

## 4. Existing Tasks with --verify Set

From graph.jsonl scan (2026-04-21):

| Task ID | Status | Verify Command |
|---------|--------|----------------|
| `run-5-task-smoke` | **in-progress** | `true` (trivial always-pass) |
| `write-harbor-config` | done | `true` |
| `fix-e-url` | done | `env -u WG_ENDPOINT_URL -u WG_LLM_PROVIDER wg nex -m qwen3:4b -e http://localhost` |
| `sanity-check-wg` | done | `wg nex --eval-mode -m qwen3-coder-30b -e lambda01 'list files in cwd' 2>/dev/null` |
| `implement-nexevalagent-class` | done | `python -c 'from wg.adapter import NexEvalAgent; a = NexEvalAgent(); print(a.name)'` |
| `confirm-docker-access` | done | `docker ps exits 0; target/bookworm-out/wg --version runs inside a docker run ...` |
| `spark-wg-nex` | done | `true` |

**Migration story needed for**: `run-5-task-smoke` (in-progress, non-terminal). Its verify
is `true` (trivial), so the migration impact is minimal — removing `verify` from it would
cause no behavioral change. All other tasks are already done.

---

## Appendix: Deprecated Paths

1. **`.verify-deferred-*` shadow tasks** (`done.rs:776-860`, `config.rs:2671`):
   When `verify_autospawn_enabled=true` (default: false), `wg done` on a task with children
   strips the `verify` field from the parent, creates a `.verify-deferred-<id>` task that
   runs after all children complete, and carries the original verify command. This is
   **opt-in, deprecated as of 2026-04-17**. The replacement is "single-leaf evaluate +
   wg rescue proxy-insert on FAIL" (from `done.rs:783-785`).

2. **`.verify-*` FLIP tasks** are NOT deprecated — they are active but named `.verify-*`
   coincidentally. They are regular tasks (not shadow tasks) that independently verify
   FLIP-flagged work.
