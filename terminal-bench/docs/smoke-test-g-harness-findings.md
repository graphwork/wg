# Smoke Test: G-Harness Findings

**Task:** smoke-test-g-harness
**Date:** 2026-04-12
**Model tested:** GPT-OSS-120B (paid + :free variants via OpenRouter)

---

## Summary

The g-harness has **multiple critical bugs** that prevent it from functioning as intended. None of the 4 smoke test checks passed fully. The downstream 89-task batch runs (tb-gpt-oss-a, tb-gpt-oss-g) **must not start** until these are fixed.

---

## Check 1: Decomposition — FAIL

### Problem 1a: `autopoietic: False` prevents decomposition

`CONDITION_CONFIG["G"]` has `"autopoietic": False` (adapter.py:142). This means the autopoietic meta-prompt (`CONDITION_G_META_PROMPT`) is **never injected** into the task instruction. The agent receives the raw task text with no instructions to decompose.

**Code path:** `_run_native_executor()` at adapter.py:653:
```python
if cfg.get("autopoietic"):  # False → meta-prompt not injected
    meta = CONDITION_G_META_PROMPT.replace("{seed_task_id}", task_id)
    full_instruction = meta + task_instruction
else:
    full_instruction = task_instruction  # ← this path taken
```

**Result:** Task dispatched directly to a single agent. Zero subtasks created.

### Problem 1b: GPT-OSS-120B:free ignores architect meta-prompt

Even when the meta-prompt is manually injected (tested in smoke test 2), GPT-OSS-120B:free completely ignores the "You are a graph architect. You do NOT implement solutions yourself" instruction. It directly solves the task using `write_file` instead of creating subtasks with `wg_add`.

**Evidence:** Agent output from manual trial at `/tmp/g-harness-smoke2-GWOY`:
- Turn 1: `write_file` → wrote `/tmp/wordfreq.py` directly
- Turn 2: `wg_done` → marked task done
- Zero `wg_add` calls, zero subtasks created

### Problem 1c: Architect bundle not applied

When `autopoietic=True`, the adapter writes an architect bundle (`ARCHITECT_BUNDLE_TOML`) that restricts tools to `bash, read_file, glob, grep, wg_*`. But the current config (`autopoietic=False`) skips this, and the agent gets `exec_mode="full"` which includes `write_file` and `edit_file`.

---

## Check 2: Gating and validation — CANNOT TEST

No decomposition means no subtask dependencies to test gating on. Cannot verify blocked→unblocked transitions.

---

## Check 3: Model purity — PASS (config level)

### Config.toml is correct

`_build_config_toml_content()` writes the trial model to both `[coordinator].model` and `[agent].model`:

```toml
[coordinator]
model = "openrouter:openai/gpt-oss-120b"
...
[agent]
model = "openrouter:openai/gpt-oss-120b"
```

### `coordinator_model = "sonnet"` is dead config

`CONDITION_CONFIG["G"]["coordinator_model"] = "sonnet"` at adapter.py:145 is **never used** by `_build_config_toml_content()` (confirmed by code audit + doc at `audit-ulivo-g-state.md:94`). The coordinator uses the trial model, not "sonnet".

### Native executor routes correctly

- Provider parsing: `openrouter:openai/gpt-oss-120b` → provider=openrouter, model=`openai/gpt-oss-120b`
- API calls routed to `https://openrouter.ai/api/v1`
- stream.jsonl confirms: `"model":"openai/gpt-oss-120b:free"`
- Zero claude/opus/sonnet/anthropic references in any log or config

### Caveat: Paid variant requires credits

`openai/gpt-oss-120b` (without `:free`) returns HTTP 402 "Insufficient credits" from OpenRouter. Only the `:free` variant works with the current account.

---

## Check 4: Results capture — PARTIAL FAIL

### What works
- `stream.jsonl` captures token counts, model info, tool calls
- `output.log` captures full agent interaction trace
- `graph.jsonl` captures task state
- Harbor creates results directory structure (`config.json`, `result.json`, per-trial dirs)

### What's broken: Completion detection (2 bugs)

**Bug 1 — Invalid status syntax (adapter.py:740):**
```python
# This command ALWAYS fails because wg doesn't support comma-separated statuses
list_result = await environment.exec(
    command=f"cd {trial_workdir} && wg list --status open,in-progress 2>/dev/null"
)
```
`wg list --status open,in-progress` returns error: "Unknown status: 'open,in-progress'". The `2>/dev/null` swallows the error, `return_code != 0`, so the polling loop never enters the completion check branch.

**Bug 2 — Internal daemon tasks prevent "all terminal" (adapter.py:738-753):**
Even if the status syntax is fixed, `wg list --status open` will always return internal daemon tasks (`.coordinator-0`, `.compact-0`, `.user-unknown-0`, `.registry-refresh-0`) that are perpetually open. The polling logic considers these as "not terminal" and never detects completion.

**Combined effect:** Every Condition G trial will time out at 1800 seconds (30 minutes), regardless of whether the actual tasks completed. Results are captured but the trial is always recorded as "timeout".

---

## Bugs Summary (for fix-g-harness)

| # | Severity | Location | Description |
|---|----------|----------|-------------|
| 1 | **Critical** | adapter.py:142 | `autopoietic: False` prevents decomposition meta-prompt |
| 2 | **Critical** | adapter.py:740 | `wg list --status open,in-progress` is invalid syntax |
| 3 | **Critical** | adapter.py:738-753 | Internal daemon tasks prevent completion detection |
| 4 | **High** | Model behavior | GPT-OSS-120B:free ignores architect meta-prompt |
| 5 | **Medium** | adapter.py:145 | `coordinator_model: "sonnet"` is dead config (not wired) — cosmetic but confusing |
| 6 | **Medium** | OpenRouter credits | Paid `openai/gpt-oss-120b` returns 402 |

### Recommended fixes

1. **Set `autopoietic: True`** in `CONDITION_CONFIG["G"]` to enable the meta-prompt
2. **Fix polling command:** Replace `wg list --status open,in-progress` with two separate commands or use `wg list` and parse output
3. **Filter internal tasks:** Exclude `.coordinator-*`, `.compact-*`, `.archive-*`, `.user-*`, `.registry-*` from completion check
4. **Consider model fallback:** GPT-OSS-120B may need stronger prompting or a different decomposition strategy than the meta-prompt approach
5. **Use `:free` variant** or add OpenRouter credits for paid variant
6. **Remove dead `coordinator_model` field** from CONDITION_CONFIG (or wire it into config.toml)
