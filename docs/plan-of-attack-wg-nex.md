# Plan of Attack: wg nex + TerminalBench + Harbor

**Date:** 2026-04-21
**Sources:** research-tb-harness-wiring, research-qwen3-nex-config, research-wg-in-harbor, research-agent-wg-awareness, research-smoke-scope-criteria

---

## 1. What We Can Smoke-Test Today vs What Blocks Us

### Can we run `wg nex --eval-mode` against TB tasks right now?

**Partially.** The eval-mode machinery works end-to-end — JSON contract on stdout, clean stderr, autonomous one-shot execution. But there is no integrated runner that combines eval-mode invocation with TB task setup (instruction loading, temp directory creation, verify command execution, result collection). The existing Python runners (`run_pilot_qwen3_local_10.py`) use the `wg service start` path, not eval-mode.

**What works today:**
- `wg nex --eval-mode -m qwen3-coder-30b -e lambda01 'instruction'` produces a JSON summary on stdout and exits cleanly.
- The SGLang endpoint on lambda01 responds in ~2s with correct completions.
- A manual bash loop can run eval-mode against individual tasks (see Option A in smoke-scope-criteria research).

**What doesn't work yet:**
- No Python runner for eval-mode (the `NexEvalAgent` class doesn't exist — only a sketch).
- The `-e <url>` flag has a `/v1`-stripping bug (`provider.rs:71`) — must use named endpoints instead.
- Local Ollama is blocked from `wg nex` by two bugs (the `-e` URL bug + default endpoint override).

### Is qwen3 reachable? What's the fallback?

| Endpoint | Status | How to use |
|----------|--------|-----------|
| SGLang on lambda01 (Tailscale FQDN) | **WORKING** | `wg nex -m qwen3-coder-30b -e lambda01` |
| Local Ollama (:11434) | Reachable via curl, **blocked from nex** by 2 bugs | Fix bugs first, or add `ollama-local` named endpoint |
| Local Ollama (:11435) | Not running | `bash terminal-bench/start-local-model.sh` |
| OpenRouter | Available but free tier rate-limits cause 0% pass | Paid tier needed |

**Fallback chain:** lambda01 (primary) → fix bugs + use local Ollama → start alt Ollama on :11435 → OpenRouter paid tier.

### What adapter changes are needed?

For a **standalone smoke test** (no Harbor): None. A bash script or thin Python wrapper calling `wg nex --eval-mode` directly works today.

For **Harbor integration** (Condition A): A new `NexEvalAgent` class in `adapter.py` that calls `wg nex --eval-mode` instead of `wg service start`. The TB harness research produced an 80-line sketch. Key differences from the existing `WorkgraphAgent`: single process (no daemon/polling), JSON summary parsed from stdout, verify gate is external (harness runs test after agent exits).

---

## 2. Minimum Viable Path to First Green TerminalBench Run with wg nex

### Exact sequence of steps

**Phase A: Standalone smoke (no Harbor, no adapter changes)**

| Step | What | Effort | Blocks on |
|------|------|--------|-----------|
| A1 | Verify lambda01 is up: `curl -s https://lambda01.tail334fe6.ts.net:30000/v1/models` | 1 min | Network access |
| A2 | Write `run_smoke_nex_qwen3.sh` — a bash script that: (1) loads task instruction from `terminal-bench/tasks/`, (2) creates isolated temp dir with git init, (3) copies task setup files, (4) runs `wg nex --eval-mode -m qwen3-coder-30b -e lambda01 --max-turns 200`, (5) runs verify command, (6) parses JSON summary, (7) reports pass/fail | 2–3 hours | Nothing |
| A3 | Run the 5-task smoke set: `text-processing`, `debugging`, `data-processing`, `algorithm`, `mailman` | 15–30 min | A1, A2 |
| A4 | Confirm 5/5 pass. Any failure = investigate (config issue, endpoint issue, or eval-mode regression) | 30 min | A3 |

**Phase B: Python runner for eval-mode (reusable, CTRF output)**

| Step | What | Effort | Blocks on |
|------|------|--------|-----------|
| B1 | Create `terminal-bench/run_smoke_nex_qwen3.py` — Python equivalent of A2 using `subprocess.run()` for `wg nex --eval-mode`, CTRF JSON output, per-task verify, summary statistics | 3–4 hours | Nothing |
| B2 | Run the 5-task smoke with the Python runner | 15–30 min | B1 |
| B3 | Compare results with prior `pilot-qwen3-local-10` baseline (expect identical pass/fail, similar turn counts) | 30 min | B2 |

### Which condition(s) to start with

**Condition A** — bare agent, no wg tools. This is the natural fit for eval-mode: the agent gets bash + file tools, no wg mutation tools, no coordinator. Eval-mode's default tool surface matches exactly.

Conditions B/C/F could partially use eval-mode if wg tools are restored (via `--role`), but add complexity. Conditions D/E/G require the coordinator path. Start simple.

### Which model(s) to start with

**qwen3-coder-30b on lambda01 SGLang** — already proven at 10/10 on the local task set. Known working with `wg nex -e lambda01`. 32768 context window, ~2s latency.

Expand to other models after green smoke:
- Local Ollama qwen3:32b (after fixing the two bugs)
- OpenRouter models (paid tier)
- Claude models (as a capability ceiling reference)

---

## 3. Minimum Viable Path to wg-in-Harbor with Agent Self-Discovery

### Docker image requirements

The wg binary must be glibc ≤ 2.36 compatible (Debian bookworm). A cross-built binary already exists at `target/bookworm-out/wg`. TB task containers use bookworm-based images.

**Three delivery options** (in order of recommendation):

1. **`mounts_json` bind mount** (quickest, no Docker build): Mount `target/bookworm-out/wg` read-only into the container via Harbor config:
   ```json
   {"environment": {"mounts_json": "[\"target/bookworm-out/wg:/usr/local/bin/wg:ro\"]"}}
   ```

2. **Binary upload** (current approach, works): `adapter.setup()` uploads via `environment.upload_file()`. 5–8s overhead per trial.

3. **Pre-baked wg Docker image** (cleanest long-term): `ghcr.io/ekg/wg:latest` image containing just the binary, used as a multi-stage copy source. Requires CI pipeline.

### Agent bootstrap mechanism

The adapter's `setup()` method handles all bootstrap. For a `NexEvalAgent` (Condition A eval-mode):

1. Upload/mount wg binary → `/usr/local/bin/wg`
2. Create isolated trial dir: `/var/tmp/tb-trial-<uuid>`
3. `wg init --no-agency` in trial dir
4. Write `config.toml` with model + endpoint config
5. Write task instruction to file (base64-encoded to avoid shell quoting)
6. `wg nex --eval-mode` from the task repo CWD, with `WG_DIR` pointing to trial dir

For full coordinator path (Conditions B–G), the existing adapter already handles this: bundle writing, agency setup, condition-specific meta-prompts, `wg service start`, polling.

### Config/env contract

**Environment variables the agent needs:**

| Variable | Required for eval-mode? | Required for coordinator? | Source |
|----------|------------------------|--------------------------|--------|
| `WG_DIR` | Yes (points to trial .workgraph/) | Yes | Set in exec command |
| `OPENROUTER_API_KEY` | If using OpenRouter | If using OpenRouter | Inherited from host |
| `WG_TASK_ID` | No (eval-mode is taskless) | Yes | Coordinator spawn |
| `WG_AGENT_ID` | No | Yes | Coordinator spawn |
| `WG_MODEL` | No (passed via `-m`) | Yes | Coordinator spawn |
| `WG_EXECUTOR_TYPE` | No | Yes | Coordinator config |

**Config file (`config.toml`) minimum for eval-mode:**
```toml
[agent]
model = "qwen3-coder-30b"

[[llm_endpoints.endpoints]]
name = "lambda01"
provider = "oai-compat"
url = "https://lambda01.tail334fe6.ts.net:30000/v1"
api_key = "none"
is_default = true
context_window = 32768
```

**Agent self-discovery layers** (from agent-wg-awareness research):

| Layer | What | When needed |
|-------|------|------------|
| 1. Binary + init | wg on PATH, `.workgraph/` initialized | All conditions |
| 2. Env vars | `WG_TASK_ID`, `WG_AGENT_ID`, etc. | Conditions B+ |
| 3. Tool surface | wg_* tools in tool registry (or excluded) | Conditions B+ (excluded for A) |
| 4. Prompt injection | Tiered guide (8KB essential → 40KB full) | Conditions C+ |

For Condition A eval-mode, only Layer 1 is strictly needed (and even that only for model/endpoint config resolution).

---

## 4. Recommended Sequencing

### Dependency graph

```
                    fix-e-url (Bug 1)
                         │
                         ▼
 ┌──────────────────────────────────────────┐
 │  A1: Verify lambda01                     │
 │  A2: Write bash smoke script             │──┐
 └──────────────────────────────────────────┘  │
                                                ▼
                                    A3: Run 5-task smoke
                                          │
                                          ▼
                              A4: Confirm 5/5 green
                                    │         │
            ┌───────────────────────┘         └──────────────┐
            ▼                                                 ▼
  B1: Python eval-mode runner              C1: NexEvalAgent class
  B2: Run smoke via Python                 C2: Harbor config JSON
  B3: Compare with baseline                C3: Run in Docker container
            │                                        │
            └──────────────┬─────────────────────────┘
                           ▼
              D1: Full TB 2.0 run (89 tasks)
              D2: Multi-condition comparison
```

### What to build first

**Track 1 (do first): Standalone smoke — validate eval-mode + qwen3 pipeline**

| Priority | Task | Effort | Parallelizable? |
|----------|------|--------|----------------|
| P0 | A1: Verify lambda01 reachability | 1 min | — |
| P0 | A2: Bash smoke script (`run_smoke_nex_qwen3.sh`) | 2–3 hours | Yes, with A1 |
| P0 | A3+A4: Run 5-task smoke, confirm green | 30–60 min | After A1+A2 |

**Track 2 (do second): Bug fixes — unblock local model fallback**

| Priority | Task | Effort | Parallelizable? |
|----------|------|--------|----------------|
| P1 | Fix `-e <url>` /v1 stripping bug (provider.rs:63-86) | 1–2 hours | Yes, with Track 1 |
| P1 | Fix default endpoint override bug (provider.rs:288-291) | 1–2 hours | Yes, with above |
| P1 | Add `ollama-local` named endpoint to config | 15 min | After bug fixes |

**Track 3 (do third): Harbor integration — Docker + NexEvalAgent**

| Priority | Task | Effort | Parallelizable? |
|----------|------|--------|----------------|
| P2 | C1: Implement `NexEvalAgent` class in adapter.py | 3–4 hours | After Track 1 green |
| P2 | C2: Write Harbor config JSON for Condition A eval-mode | 30 min | Yes, with C1 |
| P2 | C3: Test NexEvalAgent in Docker container | 1–2 hours | After C1+C2 |

**Track 4 (do last): Python runner + full eval**

| Priority | Task | Effort | Parallelizable? |
|----------|------|--------|----------------|
| P3 | B1: Python eval-mode runner with CTRF output | 3–4 hours | After Track 1 green |
| P3 | D1: Full TB 2.0 run (89 tasks, 3 replicas) | 4–8 hours runtime | After Track 3 |
| P3 | D2: Multi-condition comparison (A vs B vs C) | 2–4 hours runtime | After D1 |

### Estimated total effort

| Track | Implementation time | Wall-clock time (including runs) |
|-------|-------------------|--------------------------------|
| Track 1 (standalone smoke) | 3 hours | 4 hours |
| Track 2 (bug fixes) | 3 hours | 3 hours |
| Track 3 (Harbor integration) | 5 hours | 7 hours |
| Track 4 (Python runner + full eval) | 4 hours | 12+ hours |
| **Total (sequential)** | **15 hours** | **26 hours** |
| **Total (parallelized Tracks 1+2)** | **12 hours** | **20 hours** |

---

## 5. Open Questions for User Resolution

### Design forks that need human decision

1. **NexEvalAgent as new class vs mode in WorkgraphAgent?**
   The research suggests a separate `NexEvalAgent` class for clarity, but a `use_eval_mode=True` flag on the existing `WorkgraphAgent` would avoid code duplication. Which approach?

2. **Verify gate: external-only or add `--verify-cmd` to eval-mode?**
   Currently, eval-mode has no built-in verify loop. The harness checks the test after the agent exits. Should we add a `--verify-cmd` flag to eval-mode so the agent retries until tests pass? This would bridge eval-mode and the coordinator path. Not needed for Condition A but useful for B/C/F.

3. **Condition A focus or multi-condition from day one?**
   The research shows Condition A is the natural eval-mode fit. Should we ship A-only first and add B/C/F later? Or wire all single-agent conditions into the eval-mode path from the start?

4. **Agent self-discovery component: create a "wg-aware" agency primitive?**
   No such primitive exists today. The agent-wg-awareness research identified this gap. Worth creating, or is prompt injection sufficient?

### Resource/infra questions

5. **Is lambda01 stable and expected to stay up?**
   The SGLang endpoint at `lambda01.tail334fe6.ts.net:30000` is the primary qwen3 path. Is it a persistent service or an ad-hoc GPU session? If it goes down, we fall back to local Ollama (after bug fixes) or OpenRouter (paid tier).

6. **Docker access on the build machine?**
   Harbor integration needs Docker. Is Docker available and configured on the machine where we'll run the Harbor benchmarks? The `setup-docker.sh` script exists but may not have been run.

7. **OpenRouter paid tier available?**
   The free tier caused 0/5 failures due to rate limiting. For model comparison runs (qwen3 vs other models via OpenRouter), do we have a paid API key?

### Priority question

8. **TB smoke first or Harbor integration first?**

   **Recommendation: TB smoke first (Track 1).**

   Rationale: The standalone smoke test validates the eval-mode + qwen3 pipeline with zero adapter changes. If it passes, we know the core machinery works and can confidently build the Harbor adapter on top of it. If it fails, we debug locally without Docker complexity. Harbor integration adds Docker networking, binary compatibility, and container filesystem concerns — all of which are easier to debug once the core path is proven.

   If you want to move faster, Tracks 1 and 2 can run in parallel — the bug fixes unblock local Ollama as a fallback while the smoke test validates the lambda01 path.

---

## Appendix: Key File Locations

| Artifact | Path |
|----------|------|
| Eval-mode implementation | `src/commands/nex.rs:36-47, 249-262, 341, 361, 389-422` |
| Provider/endpoint resolution | `src/executor/native/provider.rs:63-86, 288-291` |
| Harbor adapter | `terminal-bench/wg/adapter.py` |
| Existing Python runner | `terminal-bench/run_pilot_qwen3_local_10.py` |
| Smoke harness script | `scripts/eval-harness-smoke.sh` |
| Lambda01 endpoint config | `.workgraph.1/config.toml` (llm_endpoints section) |
| Task definitions | `terminal-bench/wg/tasks.py` |
| Task setup files | `terminal-bench/tasks/` |
| Tiered guide system | `src/commands/spawn/context.rs:644-664` |
| Agent spawn env vars | `src/commands/spawn/execution.rs:547-565` |
| Bundle/tool filtering | `src/executor/native/bundle.rs` |
| Agent wg tools | `src/executor/native/tools/wg.rs` |

## Appendix: Proposed Implementation Tasks

These are concrete `wg add` candidates for the next phase:

```
# Track 1: Standalone smoke
wg add "Write bash smoke runner for wg nex eval-mode" \
  --verify "bash terminal-bench/run_smoke_nex_qwen3.sh --dry-run exits 0"
wg add "Run 5-task smoke with qwen3-coder-30b on lambda01" \
  --after write-bash-smoke-runner \
  --verify "5/5 tasks pass"

# Track 2: Bug fixes (can run parallel with Track 1)
wg add "Fix -e URL /v1 stripping bug in provider.rs" \
  --verify "wg nex -e http://localhost:11434 -m qwen3:4b exits without 404"
wg add "Fix default endpoint override for provider-prefixed models" \
  --after fix-e-url \
  --verify "wg nex -m ollama:qwen3:4b uses localhost not lambda01"

# Track 3: Harbor integration (after Track 1 green)
wg add "Implement NexEvalAgent class in adapter.py" \
  --after run-5-task-smoke \
  --verify "python -c 'from wg.adapter import NexEvalAgent; print(NexEvalAgent.name())'"
wg add "Write Harbor config JSON for Condition A eval-mode" \
  --after implement-nexevalagent
wg add "Test NexEvalAgent in Docker with 5-task smoke" \
  --after write-harbor-config \
  --verify "harbor run --config ... completes with 5/5 pass"

# Track 4: Python runner + full eval (after Track 3)
wg add "Write Python eval-mode runner with CTRF output" \
  --after run-5-task-smoke
wg add "Full TB 2.0 run: 89 tasks × 3 replicas" \
  --after test-nexevalagent-docker
```
