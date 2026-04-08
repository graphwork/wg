# Design: Graceful Completion and Overwork Prevention for TB Agents

**Date:** 2026-04-08
**Task:** design-graceful-completion
**Status:** Design complete
**Depends on:** [research-condition-g-timeout.md](research-condition-g-timeout.md) — Root cause analysis

---

## 1. Executive Summary

TB agents consistently hit the 30-minute trial timeout rather than completing cleanly. The [root cause analysis](research-condition-g-timeout.md) identified with HIGH confidence that **time-blind agents + no convergence signal** is the primary cause: agents do useful work but cannot tell when to stop iterating and commit partial progress.

This design addresses the problem through a **three-layer approach**:

1. **Time budget injection** — agents know their deadline from the moment they spawn
2. **Coordinator wind-down phase** — the heartbeat shifts from "dispatch" to "wrap up" when time runs low
3. **Soft deadline via `wg msg`** — coordinator sends explicit "commit now" messages to running agents

This approach was chosen because it builds entirely on existing mechanisms (env vars, heartbeat prompt, `wg msg`), requires no signal handling changes, and is backward compatible with conditions A–F.

---

## 2. Mechanisms Evaluated

### 2.1 Time Budget Injection via Env Vars (CHOSEN — Layer 1)

**Mechanism:** Inject `WG_TASK_TIMEOUT` and `WG_TASK_START_EPOCH` environment variables into every spawned agent process. The agent prompt builder reads these and adds a time-awareness section to the agent's prompt.

**Pros:**
- Zero runtime overhead — set once at spawn, available for entire agent lifetime
- Works with all executor types (claude, native, shell, amplifier)
- Already have the plumbing: `execution.rs:477-505` sets multiple `WG_*` env vars
- Agent prompt can include static guidance ("if past 80% of budget, commit and stop")
- No new IPC or signal handling needed

**Cons:**
- Agent only knows budget at spawn time — no dynamic updates
- Relies on the agent (LLM) to actually check and respect the time guidance
- Clock skew between spawn time and prompt rendering is negligible but nonzero

**Verdict:** ✅ Chosen as Layer 1. Simplest possible mechanism with maximum compatibility.

### 2.2 Soft Timeout Signal (SIGUSR1 or SIGUSR2) Before Hard Timeout

**Mechanism:** Replace the `timeout --signal=TERM` wrapper with a two-stage approach: send SIGUSR1 at T-5min as a "wrap up" signal, then SIGTERM at deadline.

**Pros:**
- OS-level guarantee of delivery — doesn't depend on LLM behavior
- Clean separation: SIGUSR1 = soft, SIGTERM = hard

**Cons:**
- Claude CLI has no SIGUSR1 handler — signal would be silently ignored or kill the process (default SIGUSR1 action is terminate)
- Native executor (our agent loop) would need a signal handler added — significant new code
- Signal handling in multi-threaded Rust requires careful `sigaction` setup
- Doesn't help the agent know *how much* time it has — only that time is almost up
- Shell executor and amplifier executor would need separate signal handling
- Testing signal-based behavior is fragile and platform-dependent

**Verdict:** ❌ Rejected. High implementation complexity, fragile across executor types, and Claude CLI can't handle it. The LLM-prompting approach (Layer 1 + Layer 3) achieves the same goal without OS signal machinery.

### 2.3 Periodic "Are You Done Yet?" Messages from Coordinator (CHOSEN — Layer 3)

**Mechanism:** During the wind-down phase (Layer 2), the coordinator sends `wg msg send <task> "TIME CRITICAL: commit your work now"` to all in-progress task agents. Agents already check `wg msg read` as part of their REQUIRED_WORKFLOW.

**Pros:**
- Uses existing `wg msg` infrastructure — no new mechanism needed
- Message content is flexible — coordinator can tailor urgency to remaining time
- Agent sees the message at its next natural breakpoint (between tool calls)
- Works with all executor types that support `wg msg read`
- Coordinator can also assess progress before deciding whether to message

**Cons:**
- Delivery is asynchronous — agent only sees message when it checks
- Agent might be mid-generation and not check messages for minutes
- Relies on REQUIRED_WORKFLOW including periodic message checks (already specified)
- Only works when coordinator agent is enabled (Condition G only)

**Verdict:** ✅ Chosen as Layer 3. Natural fit with existing message infrastructure. Asynchronous delivery is acceptable because Layer 1 already gives the agent static time awareness.

### 2.4 Coordinator-Side Completion Detection (Log Reading)

**Mechanism:** Coordinator periodically reads agent output logs (stream.jsonl) to determine if the agent has effectively completed its work (tests passing, code committed) even if the agent hasn't called `wg done` yet.

**Pros:**
- Can detect "agent is done but still polishing" — the overwork case
- Coordinator could force-complete the task (`wg done <task>`) on the agent's behalf
- Provides ground truth about agent progress beyond self-reporting

**Cons:**
- Parsing agent logs for semantic completion is unreliable — what constitutes "done"?
- stream.jsonl format varies by executor type (claude JSONL vs native events vs shell output)
- Log files can be large (megabytes) — reading them every 30s is expensive
- Force-completing a task while the agent is mid-write could leave files in inconsistent state
- Requires building a completion heuristic that generalizes across task types
- The zero-output detector (`zero_output.rs`) already handles the "agent is dead" case

**Verdict:** ❌ Rejected. Too complex and unreliable for the benefit. The simpler approach (tell agents their budget, ask them to stop) is more robust than trying to infer completion from logs.

### 2.5 Progress Checkpoints (Agent-Logged Velocity Tracking)

**Mechanism:** Require agents to call `wg log <task> "CHECKPOINT: <description>"` at regular intervals. Coordinator reads log entries to assess velocity (checkpoints per minute).

**Pros:**
- Gives coordinator quantitative progress data
- Can detect "agent is stuck" (no checkpoints) vs "agent is making progress" (regular checkpoints)
- Low overhead — `wg log` is a simple append operation

**Cons:**
- Adds cognitive burden to the agent prompt — "remember to log checkpoints"
- LLMs log checkpoints inconsistently — some will spam, others will forget entirely
- Velocity metric is hard to calibrate: 1 checkpoint/min could mean fast progress or thrashing
- Doesn't directly solve the timeout problem — only improves diagnosis
- Zero-output detection already covers the "completely stuck" case

**Verdict:** ❌ Rejected as a primary mechanism, but a lightweight version is included in the prompt guidance (Layer 1 tells agents to `wg log` when they're winding down). Not worth building coordinator-side velocity tracking.

### 2.6 Task Scoping Guidance in Prompts (Included in Layer 1)

**Mechanism:** Add estimated time ranges to task descriptions: "this should take ~10 minutes; if you're past 15, stop and report."

**Pros:**
- Simple prompt-level guidance with no infrastructure changes
- Helps calibrate agent expectations before they start

**Cons:**
- Time estimates are unreliable for diverse tasks
- TB tasks vary from 15min to 3.3hr — hard to estimate accurately
- Doesn't adapt to actual elapsed time

**Verdict:** ⚠️ Partially included. Layer 1 provides actual time budget (from config) rather than estimates. The prompt guidance in Layer 1 includes "if past 80%, wrap up" which is a dynamic version of this idea.

---

## 3. Chosen Approach: Three-Layer Graceful Completion

### Layer 1: Time Budget Injection (Agent-Side Awareness)

**What:** Every spawned agent receives environment variables telling it the time budget and spawn timestamp. The agent prompt builder uses these to add a time-awareness section.

**Environment variables (set in `execution.rs`):**

| Variable | Value | Source |
|----------|-------|--------|
| `WG_TASK_TIMEOUT_SECS` | Effective timeout in seconds | Same resolution chain as `effective_timeout_secs` |
| `WG_SPAWN_EPOCH` | Unix epoch seconds at spawn time | `SystemTime::now()` |

**Prompt injection (in `state_injection.rs` or equivalent):**

```
## Time Budget
- Total budget: {timeout}s ({timeout_min}min)
- Spawned at: {spawn_time} UTC
- Hard deadline: {deadline} UTC

**CRITICAL:** When you have used 80% of your time budget ({eighty_pct}min elapsed),
stop iterating and commit your best work:
1. `git add <files> && git commit -m "partial: <description>"`
2. `wg log <task-id> "Partial progress: <what's done, what remains>"`
3. `wg done <task-id>`

Do NOT start new iterations, refactoring, or polish after the 80% mark.
Committed partial progress is infinitely more valuable than uncommitted perfect work.
```

**Code changes:**

1. **`src/commands/spawn/execution.rs`** (~5 lines): After setting `WG_TASK_ID` (line 478), add:
   ```rust
   if let Some(secs) = effective_timeout_secs {
       cmd.env("WG_TASK_TIMEOUT_SECS", secs.to_string());
   }
   cmd.env("WG_SPAWN_EPOCH", std::time::SystemTime::now()
       .duration_since(std::time::UNIX_EPOCH)
       .unwrap_or_default()
       .as_secs()
       .to_string());
   ```

2. **`src/executor/native/state_injection.rs`** (~20 lines): Read `WG_TASK_TIMEOUT_SECS` and `WG_SPAWN_EPOCH` from env. If both are set, compute remaining time and inject the "Time Budget" section into the agent prompt.

### Layer 2: Coordinator Wind-Down Phase (Heartbeat Behavior Shift)

**What:** Wire `budget_secs` into the heartbeat, and change the heartbeat prompt when time is running low.

**Phase transitions based on remaining time:**

| Remaining | Phase | Heartbeat Behavior |
|-----------|-------|-------------------|
| > 5min | **Normal** | Current behavior: review, dispatch, recover |
| 2–5min | **Wind-down** | Stop dispatching new work, send wrap-up messages to agents |
| < 2min | **Emergency** | Force-complete any task with committed code, kill stuck agents |

**Code changes:**

1. **`src/config.rs`** (~3 lines): Add `trial_budget_secs` field to `CoordinatorConfig`:
   ```rust
   /// Total trial time budget in seconds. When set, heartbeat prompts include
   /// remaining time and shift to wind-down behavior near the deadline.
   #[serde(default)]
   pub trial_budget_secs: Option<u64>,
   ```

2. **`src/commands/service/mod.rs`** (~3 lines): Pass `config.coordinator.trial_budget_secs` instead of `None` at line 2823:
   ```rust
   match agent.send_heartbeat(
       heartbeat_tick_number,
       daemon_start_time,
       config.coordinator.trial_budget_secs, // was: None
   ) {
   ```

3. **`src/commands/service/coordinator_agent.rs`** (~40 lines): Modify `send_heartbeat()` to emit different prompts based on remaining time:

   ```rust
   pub fn send_heartbeat(
       &self,
       tick_number: u64,
       start_time: std::time::Instant,
       budget_secs: Option<u64>,
   ) -> Result<()> {
       let elapsed = start_time.elapsed().as_secs();
       let remaining = budget_secs
           .map(|b| b.saturating_sub(elapsed));
       let remaining_display = remaining
           .map(|r| format!("~{}s", r))
           .unwrap_or_else(|| "unlimited".to_string());
       let timestamp = chrono::Utc::now().format("%H:%M:%S").to_string();

       let phase_guidance = match remaining {
           Some(r) if r < 120 => {
               "⚠️ EMERGENCY — <2 minutes remaining!\n\
                1. Do NOT create or dispatch any new tasks\n\
                2. For each in-progress task: if the agent has committed code, \
                   run `wg done <task>` to force-complete it\n\
                3. Kill any agents that appear stuck: `wg kill <id>`\n\
                4. Accept partial progress — it's better than nothing"
           }
           Some(r) if r < 300 => {
               "⚠️ WIND-DOWN — <5 minutes remaining!\n\
                1. Do NOT create new tasks or spawn new agents\n\
                2. Send wrap-up message to ALL in-progress agents:\n\
                   `wg msg send <task> \"TIME CRITICAL: <2min remaining. \
                   Commit your current work NOW, run wg done, stop iterating.\"`\n\
                3. If a task's tests pass, force-complete it with `wg done <task>`\n\
                4. Focus on preserving progress, not perfection"
           }
           _ => {
               "Review the system state and take action:\n\
                1. STUCK AGENTS: Any agent running >5min with no output? → `wg kill <id>` and retry\n\
                2. FAILED TASKS: Any tasks failed? → Analyze cause, create fix-up task or retry\n\
                3. READY WORK: Unblocked tasks waiting? → Ensure they'll be dispatched\n\
                4. PROGRESS CHECK: Is the work converging toward completion?\n\
                5. STRATEGIC: Should any running approach be abandoned?"
           }
       };

       let prompt = format!(
           "[AUTONOMOUS HEARTBEAT] Tick #{tick_number} at {timestamp}\n\
            Time elapsed: {elapsed}s | Budget remaining: {remaining_display}\n\
            \n\
            You are the autonomous coordinator for this project. No human operator.\n\
            {phase_guidance}\n\
            \n\
            If everything is nominal, respond: \"NOOP — all systems nominal.\"\n\
            If you take action, log what and why.",
       );

       let request_id = format!("heartbeat-{}", tick_number);
       self.send_message(request_id, prompt)
   }
   ```

4. **`terminal-bench/wg/adapter.py`** (~2 lines): In `_build_config_toml_content()`, add `trial_budget_secs`:
   ```python
   if cfg.get("coordinator_agent"):
       lines.append("coordinator_agent = true")
       lines.append(f"trial_budget_secs = {timeout_secs}")  # NEW
   ```
   Where `timeout_secs` is passed as a parameter (from `DEFAULT_TRIAL_TIMEOUT` or per-task timeout).

### Layer 3: Wrap-Up Messages via `wg msg` (Coordinator → Agent)

**What:** During wind-down phase, the coordinator sends explicit `wg msg send` commands to in-progress agents. This is not new infrastructure — it's a behavior change in the heartbeat prompt (Layer 2 already handles this via the wind-down guidance).

**How it works:**

1. Heartbeat fires during wind-down phase (< 5min remaining)
2. Coordinator sees the wind-down prompt guidance
3. Coordinator runs `wg list --status in-progress` to find active tasks
4. For each in-progress task, coordinator runs:
   ```
   wg msg send <task-id> "TIME CRITICAL: Less than 5 minutes remaining in the trial. 
   Commit your current work immediately (git add + git commit), then run wg done. 
   Do not start new iterations or refactoring."
   ```
5. Agents see the message at their next `wg msg read` check (part of REQUIRED_WORKFLOW)

**No code changes needed for Layer 3** — it's entirely driven by the coordinator's LLM behavior responding to the wind-down prompt from Layer 2.

---

## 4. Integration with Existing Heartbeat Mechanism

The design extends the heartbeat without changing its core architecture:

```
                         ┌──────────────────────────┐
                         │    Heartbeat Timer        │
                         │    (every 30s)            │
                         └────────────┬─────────────┘
                                      │
                              ┌───────▼────────┐
                              │ budget_secs set?│
                              └───┬────────┬───┘
                                  │ Yes    │ No
                           ┌──────▼──┐  ┌──▼──────────┐
                           │ Compute │  │ "unlimited"  │
                           │remaining│  │ normal prompt│
                           └────┬────┘  └─────────────┘
                                │
                    ┌───────────┼───────────┐
                    │           │           │
               ┌────▼───┐ ┌────▼───┐ ┌────▼────┐
               │ >5min  │ │ 2-5min │ │ <2min   │
               │ NORMAL │ │WINDOWN │ │EMERGENCY│
               └────────┘ └────────┘ └─────────┘
```

**What stays the same:**
- Heartbeat interval (30s, configurable)
- `send_heartbeat()` function signature (adds behavior to `budget_secs` param that already exists)
- Coordinator agent architecture (persistent LLM session)
- Event-driven tick loop
- All other daemon functionality

**What changes:**
- `budget_secs` parameter actually gets populated (was always `None`)
- Heartbeat prompt content varies by phase (was always the same 5-point checklist)
- `remaining` display shows real values (was always `~0s`)
- New `trial_budget_secs` config field

---

## 5. Impact on Conditions A–F (Backward Compatibility)

| Condition | Heartbeat? | Coordinator Agent? | Impact |
|-----------|-----------|-------------------|--------|
| A (baseline) | No | No | **None.** No heartbeat = no wind-down. Env vars set but ignored by non-wg prompts. |
| B (wg tools) | No | No | **None.** Same as A. |
| C (graph context) | No | No | **None.** Same as A. |
| D (agency) | No | No | **None.** Same as A. |
| E (multi-agent) | No | No | **None.** Agent timeout still works as before. |
| F (full) | No | No | **None.** Single agent, existing timeout wrapper. |
| G (heartbeat) | Yes (30s) | Yes | **Full benefit.** All three layers active. |

**Why conditions A–F are unaffected:**

1. **Layer 1 (env vars):** `WG_TASK_TIMEOUT_SECS` and `WG_SPAWN_EPOCH` are set for all agents, but the prompt injection only fires in the native executor's state injection. For conditions A–F using single-agent mode, the agent already has a timeout wrapper — the env vars are harmless additional context. For non-native executors, the env vars are ignored.

2. **Layer 2 (heartbeat phase):** Only fires when `heartbeat_interval > 0` AND `coordinator_agent = true`. Conditions A–F have both disabled (default). The `trial_budget_secs` config field defaults to `None` — no behavior change.

3. **Layer 3 (wg msg):** Only triggered by coordinator wind-down behavior. No coordinator agent in A–F = no messages sent.

**Config compatibility:** `trial_budget_secs` is `Option<u64>` with `#[serde(default)]` — existing config files without this field parse correctly as `None`.

---

## 6. Addressing the Root Cause

The research identified: *"time-blind agents + no convergence signal"*

| Root Cause Element | How Addressed |
|-------------------|---------------|
| Agents don't know the clock is ticking | Layer 1: env vars + prompt section with budget, elapsed, deadline |
| Coordinator can't warn agents | Layer 2 + 3: wind-down heartbeat → `wg msg` to agents |
| No graceful shutdown — only SIGTERM | Layer 1: agent commits *before* timeout; Layer 2: coordinator force-completes |
| Verification cycles restart from scratch | Layer 1: "don't start new iterations after 80%" guidance |
| Agent timeout = trial timeout | Layer 2: addressed indirectly — coordinator stops dispatching before trial end. Also: R6 from research (set `agent_timeout < trial_timeout`) should be done separately. |
| `budget_secs` always `None` | Layer 2: wired from `trial_budget_secs` config |

---

## 7. Implementation Spec

### Files to modify:

| File | Change | Lines |
|------|--------|-------|
| `src/commands/spawn/execution.rs` | Add `WG_TASK_TIMEOUT_SECS` + `WG_SPAWN_EPOCH` env vars | ~5 |
| `src/executor/native/state_injection.rs` | Read env vars, inject "Time Budget" prompt section | ~25 |
| `src/config.rs` | Add `trial_budget_secs: Option<u64>` to `CoordinatorConfig` | ~5 |
| `src/commands/service/mod.rs` | Pass `trial_budget_secs` to `send_heartbeat()` | ~1 |
| `src/commands/service/coordinator_agent.rs` | Phase-aware heartbeat prompt | ~40 |
| `terminal-bench/wg/adapter.py` | Write `trial_budget_secs` in config.toml | ~3 |

**Total: ~79 lines of new/modified code across 6 files.**

### Implementation order:

1. **Config** (`config.rs`): Add field — minimal, no behavior change
2. **Env vars** (`execution.rs`): Set env vars at spawn — no behavior change yet
3. **Prompt injection** (`state_injection.rs`): Read env vars, add prompt section
4. **Heartbeat wiring** (`mod.rs`): Pass `trial_budget_secs` — enables real budget display
5. **Phase-aware prompt** (`coordinator_agent.rs`): Wind-down behavior
6. **Adapter** (`adapter.py`): Wire trial timeout into config

Steps 1–3 can proceed independently from steps 4–6.

---

## 8. Test Plan

### Unit tests:

1. **Heartbeat phase selection** (`coordinator_agent.rs`):
   - `budget_secs = None` → normal prompt (backward compat)
   - `budget_secs = Some(1800)`, elapsed = 100s → normal prompt
   - `budget_secs = Some(1800)`, elapsed = 1600s → wind-down prompt (contains "WIND-DOWN")
   - `budget_secs = Some(1800)`, elapsed = 1750s → emergency prompt (contains "EMERGENCY")

2. **Config parsing** (`config.rs`):
   - Config without `trial_budget_secs` → parses as `None`
   - Config with `trial_budget_secs = 1800` → parses as `Some(1800)`

3. **Env var injection** (`execution.rs`):
   - Agent spawned with timeout → `WG_TASK_TIMEOUT_SECS` set
   - Agent spawned without timeout → `WG_TASK_TIMEOUT_SECS` not set
   - `WG_SPAWN_EPOCH` always set and is within 1s of current time

### Integration tests:

4. **Prompt injection** (`state_injection.rs`):
   - With `WG_TASK_TIMEOUT_SECS` set → prompt contains "Time Budget" section
   - Without `WG_TASK_TIMEOUT_SECS` → prompt does not contain "Time Budget" section

5. **End-to-end heartbeat** (existing `test_coordinator_special_agents.rs`):
   - Verify heartbeat with `trial_budget_secs` shows real remaining time (not `~0s`)

### Manual/TB validation:

6. **Condition G run:** Run a TB trial with all three layers active. Success criteria:
   - Agent logs show "Time Budget" section in prompt
   - Heartbeat logs show transition from NORMAL → WIND-DOWN → EMERGENCY
   - At least some agents commit partial work before timeout
   - Pass rate ≥ 64% (not worse than run 4 baseline)

### Existing test suite:

7. **Regression:** `cargo test` must pass with no new failures. The 5 discovered test files should continue to pass:
   - `tests/test_context_pressure_agent.rs`
   - `tests/test_coordinator_special_agents.rs`
   - `tests/test_prompt_from_components.rs`
   - `tests/test_shell_retry_loop.rs`
   - `tests/test_streaming_agent_loop.rs`

---

## 9. Future Work (Out of Scope)

These items from the research recommendations are valuable but not part of this design:

- **R4: Worktree isolation** (`worktree_isolation = true` for Condition G) — orthogonal; should be a separate task
- **R5: Verification preserves progress** — requires changes to cycle iteration semantics; separate design
- **R6: `agent_timeout < trial_timeout`** — simple config change; can be done in adapter without new code
- **Coordinator model wiring** — `coordinator_model` not used in config.toml; separate bug fix

---

## 10. Open Questions

1. **80% threshold:** Is 80% the right wind-down trigger for agents? Could be configurable, but starting with a fixed value avoids complexity.
2. **Force-complete semantics:** When the coordinator runs `wg done <task>` on behalf of an agent, the agent process continues running (it doesn't know it was completed). The timeout wrapper will eventually kill it. Is this acceptable? (Yes — the alternative is killing the agent, which is what happens today anyway.)
3. **Non-TB callers:** Should `trial_budget_secs` be exposed via CLI (`wg service start --budget 1800`)? Useful for non-TB autonomous runs but out of scope for this design.
