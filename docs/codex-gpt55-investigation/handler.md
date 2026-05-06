# Codex vs Claude Handler Investigation

**Bug:** `codex:gpt-5.5` declares done without doing the work; `claude:opus` on the
same harness with the same task descriptions completes 15/15.

---

## Methodology

All findings are from reading source code directly. No runtime observations.

Primary files read:

| File | Purpose |
|---|---|
| `src/dispatch/handler_for_model.rs` | Model-prefix → executor routing |
| `src/dispatch/plan.rs` | SpawnPlan assembly |
| `src/commands/spawn/execution.rs` | `spawn_agent_inner`, `build_inner_command`, `write_wrapper_script` |
| `src/commands/spawn/context.rs` | `build_tiered_guide`, `classify_model_tier`, `resolve_task_scope` |
| `src/service/executor.rs` | Executor configs, `build_prompt`, `REQUIRED_WORKFLOW_SECTION` |
| `src/commands/claude_handler.rs` | Claude chat-session handler |
| `src/commands/codex_handler.rs` | Codex chat-session handler |

---

## 1. Handler Routing

`handler_for_model` (`src/dispatch/handler_for_model.rs:71`) maps the model-spec prefix:

```
codex:gpt-5.5  →  ExecutorKind::Codex
claude:opus    →  ExecutorKind::Claude
```

Both CLIs handle their own auth; no endpoint needed.

The `build_inner_command` function (`src/commands/spawn/execution.rs:919`) assembles the
actual spawn argv. This is where the real divergences begin.

---

## 2. Spawn Args Side-by-Side

### Claude (full mode, `execution.rs:1047-1075`)

Default executor config args (`service/executor.rs:1552`):
```
--print --verbose --permission-mode bypassPermissions --output-format stream-json
```

Added by `build_inner_command`:
```
--disallowedTools Agent,EnterWorktree,ExitWorktree
--disable-slash-commands
[--model <effective_model>]
```

Full command piped:
```bash
cat prompt.txt | claude --print --verbose --permission-mode bypassPermissions \
  --output-format stream-json \
  --disallowedTools Agent,EnterWorktree,ExitWorktree \
  --disable-slash-commands \
  [--model gpt-5.5-equivalent]
```

### Codex (single code path, `execution.rs:1076-1098`)

Default executor config args (`service/executor.rs:1575`):
```
exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox
```

Added by `build_inner_command`: **nothing extra**.

Full command piped:
```bash
cat prompt.txt | codex exec --json --skip-git-repo-check \
  --dangerously-bypass-approvals-and-sandbox \
  [--model gpt-5.5]
```

### Diff Table

| Feature | Claude | Codex |
|---|---|---|
| Output format flag | `--output-format stream-json` | `--json` |
| Permission bypass | `--permission-mode bypassPermissions` | `--dangerously-bypass-approvals-and-sandbox` |
| Sub-agent block | `--disallowedTools Agent,EnterWorktree,ExitWorktree` | **missing** |
| Slash-command block | `--disable-slash-commands` | **missing** |
| Model arg | `--model <effective_model>` | `--model <effective_model>` (same) |
| exec_mode branching | `bare` / `light` / `full` / `resume` | **single path, ignores exec_mode** |

---

## 3. Prompt Injection: Is `wg agent-guide` Content Included for Codex?

**Yes.** Both executors go through the same injection path (`execution.rs:396-407`):

```rust
let model_str = settings.model.as_deref().unwrap_or("");
let model_tier = super::context::classify_model_tier(model_str);
scope_ctx.wg_guide_content = super::context::build_tiered_guide(dir, model_tier, model_str);
```

`executor_uses_auto_prompt` (`execution.rs:909`) returns `true` for both `"claude"` and
`"codex"`. The assembled prompt (`build_prompt`, `service/executor.rs:987`) includes:

- `REQUIRED_WORKFLOW_SECTION` (step 0 through 7, inc. `wg log`, `wg done`)
- `GIT_HYGIENE_SECTION`
- `wg_guide_content` (the tiered guide)

when scope ≥ `Task` (the default).

### Knowledge Tier

`classify_model_tier` (`context.rs:615`) matches model strings:

| Pattern | Tier | Guide size |
|---|---|---|
| `claude-sonnet`, `claude-opus` | Full | ~40 KB |
| `deepseek`, `claude-haiku` | Core | ~16 KB |
| `minimax`, `qwen-2.5` | Essential | ~8 KB |
| **anything else** | Essential | ~8 KB |

`settings.model` is `None` for both default executor configs (`service/executor.rs:1568`,
`1586`). Therefore `model_str = ""` and **both claude and codex default to the 8 KB
Essential tier**. There is no tier difference for default configurations.

If a user configures a `.wg/executors/codex.toml` with `model = "gpt-5.5"` the result is
the same: `"gpt-5.5"` matches no tier pattern → Essential.

---

## 4. Exec Mode Branching

**This is divergence #2 (see §6).**

Claude's `build_inner_command` has four branches:

| exec_mode | What claude gets |
|---|---|
| `bare` | `--system-prompt <full-prompt>` + stdin user-msg; `--tools Bash(wg:*)` |
| `light` | `--allowedTools Bash(wg:*),Read,Glob,Grep,WebFetch,WebSearch`; `--disallowedTools Edit,Write,...` |
| `full` (default) | no extra tool flags (all tools allowed) |
| `resume+not-bare` | `--resume <session_id>` + checkpoint follow-up |

**Codex has a single code path. It does not branch on exec_mode.** The value of
`resolved_exec_mode` is computed (`context.rs:resolve_task_exec_mode`) but never consulted
in the codex arm of `build_inner_command`.

Practical consequence for research/analysis tasks (which typically use `light` exec mode):
- Claude in `light` mode gets explicit `--allowedTools` that enable `Read`, `Glob`, `Grep`
- Codex in `light` mode gets only `--dangerously-bypass-approvals-and-sandbox`

---

## 5. Completion Gate

The wrapper script (`execution.rs:1367-1420`, shared by **both** claude and codex) detects
completion like this:

```bash
TASK_STATUS=$(wg show "$TASK_ID" --json ... | ...)

if [ "$TASK_STATUS" = "in-progress" ]; then
    if [ $EXIT_CODE -eq 124 ]; then
        wg fail "$TASK_ID" ...   # hard timeout
    elif [ $EXIT_CODE -eq 0 ]; then
        wg done "$TASK_ID"       # <-- auto-marks done
    else
        wg fail "$TASK_ID" ...   # non-zero exit
    fi
fi
```

**There is no minimum-work gate.** There is no check for:
- "wg log was called at least once"
- "at least one file was written"
- "wg artifact was recorded"
- "output.log has more than N bytes"

If an executor exits 0 while the task is still `in-progress`, the wrapper marks it done
unconditionally. This is identical for both executors.

### How the two executors interact with this gate differently

| Executor | Typical exit-0 path |
|---|---|
| Claude:opus | Model uses shell tools → calls `wg done` → exits; OR exits 0 after full turn with tool-heavy output. Either way, `TASK_STATUS` is `done` before wrapper fires, or real work was done. |
| Codex:gpt-5.5 | Single-shot exit; if model produces text without tool calls → exits 0 → wrapper auto-marks done |

This asymmetry is structural: claude's `--print` mode stays alive through all tool calls in
one turn; codex's `--json` mode also runs one turn, but gpt-5.5 may produce only text output
if it interprets the task as "describe what I would do" rather than "execute using shell
tools." Either way, both land at `EXIT_CODE=0 → wg done`.

---

## 6. Top-2 Most Suspicious Divergences

### Divergence A (most likely): Completion gate auto-fires on exit 0 with no work verification

**File:** `src/commands/spawn/execution.rs:1404-1413` (wrapper script body)

The wrapper calls `wg done` whenever `EXIT_CODE=0 && TASK_STATUS=in-progress`. No minimum
work has to have happened. For claude:opus, the model reliably calls shell tools (`wg log`,
file edits, `wg done`) before its process exits — so when the wrapper fires, the task is
already either `done` (agent called `wg done`) or has substantial breadcrumbs. For
codex:gpt-5.5, the model can produce a high-quality text response with no tool calls and
exit 0 — the wrapper then auto-marks done with a clean log and no artifacts.

**Why this causes "declares done without doing work":**
- `gpt-5.5` model reasons about the task and outputs an answer textually (e.g. "Here is my
  analysis…") without issuing a single shell command.
- codex exits 0.
- `TASK_STATUS` is still `in-progress` (agent never called `wg done` itself).
- Wrapper: `exit 0 + in-progress` → `wg done "$TASK_ID"`.
- Task becomes `done`. No logs, no artifacts, no files written.

This gap is the most direct structural cause. Fixing it requires a minimum-work gate before
the wrapper can promote to `done`.

### Divergence B (compounding): No exec_mode branching in codex arm

**File:** `src/commands/spawn/execution.rs:1076-1098`

Claude's `light` mode spawn includes `--allowedTools Bash(wg:*),Read,Glob,Grep,...` which
explicitly signals to the model that shell tools are available and expected. Codex in `light`
mode gets only `--dangerously-bypass-approvals-and-sandbox` with no tool allowlist — the
model has no mode-specific signal that it should be calling tools at all.

This is compounding because:
1. If gpt-5.5 is uncertain whether to use tools, the absence of an explicit tool allowlist
   (which claude gets in `light` mode) may tilt it toward text-only responses.
2. For `bare` mode tasks, codex doesn't receive the `--system-prompt` treatment that
   properly frames the task context.

**Note:** This divergence also means bugs are invisible — a research task in `light` mode
that should be read-only can run arbitrary shell commands via codex because the exec_mode
restriction is never applied.

---

## 7. Specific Checklist from Task Description

| Question | Answer | Evidence |
|---|---|---|
| Is `wg agent-guide` content injected into codex prompt? | **Yes**, for Task+ scope | `execution.rs:396-401`, `executor.rs:1078-1083` |
| Does codex handler enforce "wrote at least one file" or "wg log called" before accepting completion? | **No** | Wrapper script `execution.rs:1404-1413`; no such check exists for either executor |
| What happens if codex exec exits 0 with no tool calls? | Wrapper auto-marks task **done** via `wg done "$TASK_ID"` | `execution.rs:1404-1413` (wrapper body) |
| Any difference in env vars? | Same vars set for both: `WG_TASK_ID`, `WG_AGENT_ID`, `WG_EXECUTOR_TYPE`, `WG_MODEL`, `WG_TIER`, etc. | `execution.rs:575-625` |
| Model-spec stripping? | Both get bare model ID (provider prefix stripped by `resolve_model_via_registry:1622-1630`) | `execution.rs:1622-1630` |

---

## 8. Files and Line References

| Item | File:Line |
|---|---|
| Claude executor default config | `src/service/executor.rs:1548-1570` |
| Codex executor default config | `src/service/executor.rs:1571-1588` |
| `build_inner_command` — claude arms | `src/commands/spawn/execution.rs:933-1075` |
| `build_inner_command` — codex arm | `src/commands/spawn/execution.rs:1076-1098` |
| Wrapper script auto-done logic | `src/commands/spawn/execution.rs:1398-1413` |
| `executor_uses_auto_prompt` | `src/commands/spawn/execution.rs:909-911` |
| `build_tiered_guide` / `classify_model_tier` | `src/commands/spawn/context.rs:615-690` |
| `wg_guide_content` injection | `src/commands/spawn/execution.rs:396-401` |
| `build_prompt` — wg_guide and workflow sections | `src/service/executor.rs:1077-1100` |
| `REQUIRED_WORKFLOW_SECTION` | `src/service/executor.rs:20-124` |
| Handler-for-model routing | `src/dispatch/handler_for_model.rs:71-84` |
| Plan construction / executor floor | `src/dispatch/plan.rs:171-310` |
| `resolve_model_via_registry` — provider prefix stripping | `src/commands/spawn/execution.rs:1620-1630` |
