# codex:gpt-5.5 Lazy-Completion Fix Proposal

**Task:** `codex-design-fix`
**Date:** 2026-05-06
**Inputs synthesized:**
- `docs/codex-gpt55-investigation/flags.md` (codex CLI / config.toml levers)
- `docs/codex-gpt55-investigation/handler.md` (workgraph spawn + wrapper divergences)
- `docs/codex-gpt55-investigation/skills-injection.md` (knowledge-tier gap)

---

## 1. Root cause synthesis

The bug — `codex:gpt-5.5` "declares done" with no files, no commits, no `wg log`,
~1.6 k output tokens — is **not a single bug**. Three independent layers each
push the system toward the same failure mode, and any one of them in isolation
can produce the observed bail. Fixing only one will leave a residual failure
rate.

### Layer A — Knowledge tier gap (most surprising)

`classify_model_tier` (`src/commands/spawn/context.rs:614-641`) maps model
strings to tiers by substring match. `gpt-5.5` matches **none** of the
substrings (`claude-sonnet`, `claude-opus`, `llama-3.1`, `qwen-2.5`,
`deepseek`, `claude-haiku`, `minimax`) and falls through to the conservative
default `KnowledgeTier::Essential` (~8 KB).

`Essential` does not contain:
- The smoke-gate contract (`wg done` refuses on failing scenarios)
- The full `## Validation` section convention with examples
- The "no built-in Task / TaskCreate" warning
- The detailed completion contract (artifacts, `wg log` breadcrumbs)

`claude:opus` lands in `Full` (~40 KB) because `claude-opus-4-7` matches
`"claude-opus"`. So the two executors are running with **different rulebooks**
on the same task description, and codex is missing the explicit "do work
before declaring done" doctrine.

### Layer B — Codex CLI lazy defaults (highest-conviction trigger)

The codex executor config (`src/service/executor.rs:1571-1588`) invokes:

```
codex exec --json --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox
```

with no `-c` overrides. The `gpt-5.5` model catalog hard-codes
`default_verbosity = "low"` and `truncation_policy.limit = 10 000` tokens per
turn. Combined with no `developer_instructions`, the model has every incentive
to produce a brief polished summary and stop — exactly the observed
~1.6 k-token "I would do X, Y, Z" output. Known upstream issues (#13950,
#7247, #12225, #19215) confirm this is a recurring gpt-5.x failure mode that
RLHF rewards for "concise user-friendly answers" rather than "tool-call
completion."

### Layer C — Wrapper auto-done with no minimum-work gate

`execution.rs:1398-1413` (the wrapper script body, shared by claude and codex)
runs `wg done "$TASK_ID"` whenever `EXIT_CODE=0 && TASK_STATUS=in-progress`.
There is no check for "agent wrote a file," "agent called `wg log`," "agent
recorded an artifact," or even "output.log has more than N bytes."

For `claude:opus` this is fine in practice: claude reliably calls shell tools
and either marks itself done or leaves substantial breadcrumbs first. For
`codex:gpt-5.5` (because of A and B), the model exits 0 with a text-only
response and the wrapper unconditionally promotes the task to `done`. The
agency then evaluates an empty deliverable.

### Causal chain

```
[A: Essential tier — no smoke gate / no validation contract]
            │
            ▼
[B: codex CLI low verbosity, no developer_instructions]
            │
            ▼
   model emits text-only "summary," exits 0
            │
            ▼
[C: wrapper auto-`wg done` on exit-0 with no work check]
            │
            ▼
        Bug: declared done, no work
```

A and B make the bail likely; C makes it indistinguishable from real
completion.

---

## 2. Decision: multi-part fix (defended)

A single-fix design is tempting (most "primary fix" framings would point at
Layer C and stop), but each layer has a different blast radius and a different
rollback story. Picking one and ignoring the others leaves residual failure:

- Fix C alone (wrapper gate) catches the bail symptom but leaves codex doing
  no useful work — the task fails fast instead of falsely succeeding. That's
  better than today, but the underlying lazy behavior is unchanged.
- Fix A alone (tier) gives codex the right doctrine but doesn't change the
  model's verbosity defaults — gpt-5.5 may still ignore the doctrine.
- Fix B alone (CLI flags) raises engagement but provides no defense if it
  fails on a hard task.

The three fixes are also **independent in code** — different files, different
review surfaces, easy to land separately and roll back individually. The
design favors landing them in the leverage order below and gating each on
measured improvement.

**Primary fix (the one to land first if forced to pick one): Fix #1 — tier
classification.** It is one line, has zero risk to other executors, and gives
codex the contract it currently doesn't see. Fixes #2 and #3 are defense in
depth on top of it.

---

## 3. Change set (ordered by leverage, cheapest+highest-impact first)

### Fix #1 — Promote gpt-5 family to `KnowledgeTier::Full`

**File:** `src/commands/spawn/context.rs:630-636`

**Edit:** Add `gpt-5` and `gpt-4` substring matches to the Full tier branch:

```rust
// Tier 3: Full (40KB) - 128K+ context window models
else if model_lower.contains("llama-3.1")
    || model_lower.contains("llama3.1")
    || model_lower.contains("claude-sonnet")
    || model_lower.contains("claude-opus")
    || model_lower.contains("gpt-5")    // NEW
    || model_lower.contains("gpt-4")    // NEW
{
    KnowledgeTier::Full
}
```

**Why this addresses the bail:** Codex workers will now receive the same
~40 KB guide claude workers receive, including the smoke-gate contract,
explicit `## Validation` section requirements, and the "no built-in Task tool"
rules. The model will *see* the doctrine that says "produce committed
deliverables, not summaries."

**Cost:** One line of code; ~32 KB more prompt for codex sessions
(gpt-5.5 has a 272 k context window per the catalog — irrelevant overhead).

**Test:** Add a unit test in `context.rs` asserting
`classify_model_tier("gpt-5.5") == KnowledgeTier::Full`.

---

### Fix #2 — Inject completion-forcing flags into codex executor config

**File:** `src/service/executor.rs:1571-1588`

**Edit:** Extend the default codex args with `-c` overrides that counter the
model-catalog defaults:

```rust
"codex" => Ok(ExecutorConfig {
    executor: ExecutorSettings {
        executor_type: "codex".to_string(),
        command: "codex".to_string(),
        args: vec![
            "exec".to_string(),
            "--json".to_string(),
            "--skip-git-repo-check".to_string(),
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            // NEW: counter gpt-5.x catalog defaults that drive lazy completion
            "-c".to_string(),
            "model_verbosity=\"high\"".to_string(),
            "-c".to_string(),
            "tool_output_token_limit=32000".to_string(),
            "-c".to_string(),
            r#"developer_instructions="You are a non-interactive batch worker. You MUST complete the task by writing files to disk and creating at least one git commit before declaring done. A prose summary without file writes or commits is a task failure. Use shell tools (Read, Write, Edit, Bash) to do real work; do not describe work in the response.""#.to_string(),
        ],
        ...
    },
}),
```

**Why this addresses the bail:** `developer_instructions` is injected at the
system-prompt level (highest instruction-following weight per Codex docs),
making the "must call tools" requirement non-optional from the model's
perspective. `model_verbosity=high` overrides the catalog default `low` that
biases gpt-5.5 toward short summaries. `tool_output_token_limit=32000`
prevents large shell/file output truncation, which can cause the model to
"give up" because it cannot see partial progress.

**Cost:** Three `-c` overrides; no schema migration; existing user configs
in `.wg/executors/codex.toml` continue to override these (per
`src/service/executor.rs` precedence rules). The instruction text is only
applied to the built-in default config — users who customize their codex
executor config keep full control.

**Test:** Smoke test — run `wg config show` and confirm the codex args
include the three new `-c` overrides for a fresh install.

---

### Fix #3 — Add minimum-work gate to wrapper script

**File:** `src/commands/spawn/execution.rs:1404-1413` (the `EXIT_CODE -eq 0`
branch of the wrapper)

**Edit:** Before `{complete_cmd}` (which runs `wg done`), check that the agent
produced at least one signal of real work; if not, fail the task instead.

```bash
elif [ $EXIT_CODE -eq 0 ]; then
    echo "" >> "$OUTPUT_FILE"
    UNREAD=$(wg msg read "$TASK_ID" --agent "$WG_AGENT_ID" 2>/dev/null)
    if [ -n "$UNREAD" ] && ! echo "$UNREAD" | grep -q "No unread messages"; then
        echo "[wrapper] WARNING: Agent finished with unread messages:" >> "$OUTPUT_FILE"
        echo "$UNREAD" >> "$OUTPUT_FILE"
    fi

    # NEW: minimum-work gate — refuse to auto-mark done with no evidence of work
    LOG_COUNT=$(wg show "$TASK_ID" --json 2>/dev/null | grep -c '"event"' || echo 0)
    ARTIFACT_COUNT=$(wg show "$TASK_ID" --json 2>/dev/null | grep -c '"artifact"' || echo 0)
    DIFF_BYTES=$(git -C "$WORKING_DIR" diff --stat HEAD 2>/dev/null | wc -c || echo 0)
    COMMITS_AHEAD=$(git -C "$WORKING_DIR" rev-list --count HEAD ^"$BASE_REF" 2>/dev/null || echo 0)

    if [ "$LOG_COUNT" -lt 1 ] && [ "$ARTIFACT_COUNT" -lt 1 ] && [ "$DIFF_BYTES" -lt 50 ] && [ "$COMMITS_AHEAD" -lt 1 ]; then
        echo "[wrapper] FAIL-GATE: agent exited 0 with no logs, no artifacts, no diff, no commits — refusing to auto-mark done" >> "$OUTPUT_FILE"
        wg fail "$TASK_ID" --class "agent-no-work" --reason "Agent exited 0 without producing any work (no wg log, no artifacts, no diff, no commits)" 2>> "$OUTPUT_FILE" || true
    else
        echo "{complete_msg}" >> "$OUTPUT_FILE"
        {complete_cmd}
    fi
fi
```

**Why this addresses the bail:** Removes the structural cause of "false
done." Even if Fixes #1 and #2 fail to convince gpt-5.5 to call tools, the
wrapper now refuses to promote a task to `done` with zero evidence of work,
producing a `failed` task that the dispatcher will retry / escalate per the
existing failure-class machinery. Crucially, this is **also a benefit for
claude:opus** — if claude ever bails (rate limit silently truncating, etc.),
the gate catches it instead of producing a phantom completion.

**Cost:** Four extra shell commands per wrapper exit (each < 100 ms). The
threshold is conservative — `LOG_COUNT < 1 AND ARTIFACT_COUNT < 1 AND
DIFF_BYTES < 50 AND COMMITS_AHEAD < 1` — a normal claude run produces dozens
of log entries. The 50-byte diff threshold tolerates whitespace-only edits
without flagging them, but flags the no-work case (zero diff bytes).

**Test:** Add a smoke scenario in `tests/smoke/scenarios/` that spawns a fake
agent that exits 0 immediately with no work, asserts the task ends up
`failed` (not `done`), with `class=agent-no-work`. Add the scenario to the
manifest under the `codex-impl-fix` task's `owners`.

---

### Fix #4 — (Optional, lower leverage) Codex exec_mode branching

**File:** `src/commands/spawn/execution.rs:1076-1098`

The codex arm of `build_inner_command` is a single code path; it ignores
`resolved_exec_mode` (`bare` / `light` / `full` / `resume`). For `light`-mode
research tasks, claude gets explicit `--allowedTools Bash(wg:*),Read,Glob,Grep`
which signals tool availability; codex gets nothing.

**Why this is #4 not #1:** Codex CLI does not have a direct `--allowedTools`
equivalent — the equivalent levers (`-c sandbox_mode`, `-c approval_policy`)
shape behavior at a coarser granularity, and the bigger lazy-completion
trigger (Fix #2) already lands at the same surface. Recommend deferring this
to a follow-up task once Fixes #1–#3 stabilize.

---

## 4. Success metric (measurable; runnable by `codex-test-fix`)

**Reproducer pack:** four diverse tasks selected to span the bail surface:

| Task slot | Type | Acceptance |
|---|---|---|
| 1 | Research / writeup (like `codex-research-flags`) | Markdown file at specified path; >= 1 git commit |
| 2 | Mechanical rust edit (single-file feature flag) | File modified; `cargo build` passes; >= 1 commit |
| 3 | Multi-file refactor (rename type across 3 files) | All 3 files modified; `cargo build`+`cargo test` pass; >= 1 commit |
| 4 | Doc + code (add a new CLI flag with help text) | Code change + doc update; `cargo build` passes; >= 1 commit |

**Each task is run with model = `codex:gpt-5.5`, exec_mode default.**

**Baseline (current state):** ~1/4 produce committed deliverables at xhigh
reasoning effort (per task description; concrete number to be measured by
`codex-test-fix` running the pack on `main`).

**Target after fixes #1+#2+#3:** **>= 3/4 produce committed deliverables** with:
- At least one file written or modified in the working tree
- At least one git commit on the worktree branch
- `wg show <task>` final status = `done` (not `failed`)
- `wg log <task>` count >= 3 entries

**Secondary metric:** zero false-`done` outcomes. If the model bails, the
task should end in `failed` (Fix #3 catches it), not `done`. We measure:
`count(status=done AND commits_ahead=0) == 0` across the pack.

`codex-test-fix` runs the pack twice — once on `main` (baseline) and once on
the post-fix branch — and reports both the success ratio and the zero-false-
done check.

---

## 5. Rollback plan

The three fixes are independent in code (one line in `context.rs`, one block
in `executor.rs`, one branch in `execution.rs`). Rollback is per-fix.

- **If `claude:opus` regresses on any task:** Fixes #1 and #3 affect claude as
  well. Fix #1 only changes which tier `gpt-*` strings hit — it does NOT
  re-classify `claude-*`, so claude tier is unchanged. Fix #3 adds a
  minimum-work gate that, in the rare case claude exits 0 with no work, will
  flip a `done` to `failed`; this is the *desired* behavior, not a regression.
  If a real false-fail surfaces (e.g., a no-op task that legitimately makes
  no changes), tighten the gate to allow `LOG_COUNT >= 1` alone as sufficient
  evidence.
- **If `codex` regresses on mechanical tasks:** Fix #2's
  `developer_instructions` may make codex over-verbose on simple tasks.
  Mitigation: revert just Fix #2's `-c developer_instructions=...` while
  keeping `model_verbosity=high` and `tool_output_token_limit=32000` (the
  former two are catalog-level adjustments, not behavioral mandates). One-line
  revert in `executor.rs`.
- **Full revert:** `git revert` each of the three commits independently;
  no schema migrations, no external state changes. Worst case is the three
  layers all return to their pre-fix state and codex behavior reverts to the
  ~1/4 baseline.

---

## 6. Out of scope for this proposal

- **Fix #4 (exec_mode branching for codex)** is documented but deferred. It is
  the right cleanup eventually but is dwarfed in impact by Fixes #1–#3.
- **`AGENTS.md` automatic-read behavior of codex CLI under
  `--skip-git-repo-check`** is not confirmed by source inspection
  (skills-injection.md §"AGENTS.md file potentially read by Codex CLI"). If
  Fix #1 makes the `Full` guide land in the spawn prompt directly, the
  AGENTS.md path becomes belt-and-suspenders — not on the critical path.
- **Codex `auto_review` policy** as a second-line gate is interesting but
  duplicates Fix #3's role at higher cost (a sub-agent review per task);
  prefer the wrapper gate.

---

## 7. Concrete next steps for downstream tasks

- `codex-impl-fix` lands Fixes #1, #2, #3 in that order, with one commit per
  fix so each can be reverted independently.
- `codex-test-fix` builds the four-task reproducer pack, runs it on `main`
  (baseline) and on the post-fix branch, and reports the success ratio plus
  the zero-false-done check.
- A follow-up task `codex-exec-mode-branching` may pick up Fix #4 once the
  primary fixes have stabilized.
