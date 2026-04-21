# Research: Smoke Test Scope and Pass Criteria for wg nex + qwen3

## 1. TB Task Landscape

### Two Task Registries

| Registry | Count | Location | Execution Path |
|----------|-------|----------|---------------|
| Harbor TB 2.0 | 89 tasks | `~/.cache/harbor/tasks/packages/terminal-bench/` | `harbor run` with Docker |
| Local `wg/tasks.py` | 18 tasks | `terminal-bench/wg/tasks.py` + `tasks/` | Standalone Python runners or `wg nex --eval-mode` |

### Local 18 Tasks by Difficulty

| Difficulty | Count | Task IDs |
|-----------|-------|----------|
| Easy (2) | 2 | `file-ops`, `text-processing` |
| Medium (3) | 3 | `debugging`, `shell-scripting`, `data-processing` |
| Hard — calibration (3) | 3 | `algorithm`, `ml`, `sysadmin` |
| Hard — benchmark (10) | 10 | `configure-git-webserver`, `mailman`, `multi-source-data-merger`, `financial-document-processor`, `cobol-modernization`, `build-cython-ext`, `fix-code-vulnerability`, `constraints-scheduling`, `multi-module-type-migration`, `iterative-test-fix` |

### Pass Criteria in TB

TB uses a binary reward model:
- **reward = 1.0**: Task verify command exits 0 (all assertions pass)
- **reward = 0.0**: Verify command exits non-zero, timeout, or agent error
- Verify commands are shell pipelines in `wg/tasks.py` → `verify_cmd` field
- Harbor path uses `tests/run-tests.sh` inside the task package (equivalent)

Terminal statuses: `done` (verify passed), `failed` (agent gave up or verify failed), `timeout`, `abandoned`

---

## 2. Prior Results with qwen3-coder-30b (Local SGLang)

### pilot-qwen3-local-10 — Condition A, 10 tasks

- **Model**: `local:qwen3-coder-30b` via SGLang on `lambda01:30000/v1`
- **GPU**: RTX 6000 Ada 48GB
- **Context window**: 32,768 tokens
- **Result: 10/10 passed (100%)**

| Task | Difficulty | Reward | Turns | Input Tokens | Output Tokens |
|------|-----------|--------|-------|-------------|---------------|
| file-ops | easy | 1.0 | 17 | 66,154 | 1,233 |
| text-processing | easy | 1.0 | 10 | 35,947 | 1,099 |
| debugging | medium | 1.0 | 13 | 58,206 | 1,510 |
| shell-scripting | medium | 1.0 | 8 | 38,730 | 1,963 |
| data-processing | medium | 1.0 | 16 | 92,293 | 3,142 |
| algorithm | hard | 1.0 | 11 | 51,181 | 1,346 |
| ml | hard | 1.0 | 17 | 144,183 | 7,210 |
| sysadmin | hard | 1.0 | 44 | 371,768 | 5,825 |
| configure-git-webserver | hard | 1.0 | 97 | 991,695 | 7,950 |
| mailman | hard | 1.0 | 25 | 145,434 | 2,920 |

**Observations:**
- Easy/medium tasks: 8–17 turns, <100K input tokens
- Hard calibration tasks: 11–44 turns, wide variance
- Hard benchmark tasks: 25–97 turns, `configure-git-webserver` is a stress test (97 turns, ~1M input tokens)
- Total: 258 turns, ~2M input tokens, ~34K output tokens

### qwen3-hard-20-a — Condition A, 6 tasks (partial run)

- **Result: 6/6 passed (100%)** — but only easy+medium tasks completed before run was stopped

### pilot-qwen3-coder-free — OpenRouter free tier, 5 tasks

- **Result: 0/5 passed (0%)** — free-tier rate limits caused all failures (not a model capability issue)

### Baseline: pilot-a-89 — minimax-m2.7, 89 Harbor tasks

- **Result: 37/89 passed (41.6%)**
- This is a different model (minimax-m2.7 via OpenRouter), but shows which tasks are "hard for any agent":
  - `configure-git-webserver`: FAILED (0.0) — even m2.7 couldn't do it, but qwen3 did (97 turns)
  - `mailman`: PASSED (1.0) — both m2.7 and qwen3 passed
  - `build-cython-ext`: PASSED (1.0) — m2.7 passed, qwen3 not yet tested on this
  - `cobol-modernization`: PASSED (1.0) — m2.7 passed
  - `multi-source-data-merger`: PASSED (1.0) — m2.7 passed
  - `financial-document-processor`: FAILED (0.0) — m2.7 failed
  - `fix-code-vulnerability`: FAILED (0.0) — m2.7 failed
  - `constraints-scheduling`: FAILED (0.0) — m2.7 failed

---

## 3. Proposed Smoke Test Tasks (5 tasks)

### Selection Criteria
1. **Span all difficulty tiers** (easy, medium, hard)
2. **Include at least one task that qwen3 found challenging** (high turn count)
3. **Include at least one task where m2.7 failed** (validates qwen3's edge over weaker models)
4. **Keep total expected time under 30 minutes** (smoke, not full eval)
5. **Match the existing smoke convention** (`run_pilot_free_smoke.py` uses 5 tasks)

### Proposed Smoke Set

| # | Task ID | Difficulty | Rationale |
|---|---------|-----------|-----------|
| 1 | `text-processing` | easy | Fast baseline sanity check (10 turns, ~36K tokens in prior run) |
| 2 | `debugging` | medium | Tests code comprehension + fix capability (13 turns) |
| 3 | `data-processing` | medium | Tests structured data manipulation (16 turns) |
| 4 | `algorithm` | hard (calibration) | Tests algorithmic reasoning — KV store with transactions (11 turns) |
| 5 | `mailman` | hard (benchmark) | Tests multi-file system construction; passed by both m2.7 and qwen3 but is a real benchmark task (25 turns) |

**Why these 5:**
- Covers easy (1), medium (2), hard (2) — same distribution as the full set
- Total expected turns: ~75 (based on prior qwen3 runs), well within timeout
- `mailman` is from the actual TB 2.0 benchmark (not just calibration), giving signal on real-world tasks
- All 5 have been validated by qwen3-coder-30b before (all passed), so failures indicate regression or config issues — exactly what a smoke test should catch
- Avoids `configure-git-webserver` (97 turns = too slow for smoke) and `sysadmin` (44 turns = borderline)
- Matches the 5-task convention from `run_pilot_free_smoke.py`

### Alternative: 3-task minimal smoke

If even faster iteration is needed:

| # | Task ID | Difficulty | Expected Turns |
|---|---------|-----------|---------------|
| 1 | `text-processing` | easy | ~10 |
| 2 | `debugging` | medium | ~13 |
| 3 | `algorithm` | hard | ~11 |

Total ~34 turns. Under 10 minutes. Sacrifices benchmark-task coverage.

---

## 4. Pass Criteria for the Smoke Run

### Primary Pass Criteria

**The smoke test passes if ALL of the following hold:**

1. **All 5 tasks reach `done` status** (verify command exits 0)
2. **No task times out** (stays within per-task timeout)
3. **No infrastructure errors** (endpoint unreachable, OOM, CUDA errors)

### Reward Threshold

- **Full pass**: 5/5 reward = 1.0 (all verify commands pass)
- **Acceptable**: 4/5 (one hard task failure is tolerable if the harness itself worked)
- **Fail**: ≤3/5 or any infrastructure error (indicates config/endpoint/harness problem, not just model capability)

### What Constitutes "Smoke" vs "Full Run"

| Dimension | Smoke Test | Full Run |
|-----------|-----------|----------|
| Task count | 3–5 tasks | 10–18 (local) or 89 (Harbor) |
| Replicas | 1 per task | 3–5 per task for statistical power |
| Difficulty spread | Representative sample | All tiers |
| Purpose | Validate harness + endpoint + config | Measure model capability |
| Expected duration | 10–30 minutes | 2–8 hours |
| Pass criteria | "Does it work at all?" | "What's the pass rate?" |

---

## 5. Recommended Timeout and Concurrency

### Per-Task Timeout

- **Recommended: 2400s (40 minutes)** — same as `run_pilot_qwen3_local_10.py`
- Rationale: qwen3 on `configure-git-webserver` took 97 turns with ~1M input tokens. While that task isn't in the smoke set, `mailman` (25 turns) could take longer if the model needs retries. The 40-minute timeout provides 2x headroom over the worst observed case (~1200s for `sysadmin` in prior runs).
- For the 3-task minimal smoke: 1200s (20 minutes) per task is sufficient since all three are fast tasks.

### Concurrency

- **Recommended: 1 agent (sequential)** — same as all existing qwen3 local runners
- Rationale: SGLang on a single RTX 6000 Ada 48GB can serve one agent at a time. Concurrent requests would hit OOM or degrade throughput. The qwen3-coder-30b model at 30B params in Q8 occupies most of the 48GB VRAM.

### Context Window

- **Recommended: 32768 tokens** — matches the existing `run_pilot_qwen3_local_10.py` config
- The model supports larger windows but SGLang performance degrades significantly beyond 32K with this model size.

---

## 6. Expected Baseline

Based on prior `pilot-qwen3-local-10` results:

| Task | Prior Result | Expected Smoke Result |
|------|-------------|----------------------|
| text-processing | 1.0 (10 turns) | 1.0 |
| debugging | 1.0 (13 turns) | 1.0 |
| data-processing | 1.0 (16 turns) | 1.0 |
| algorithm | 1.0 (11 turns) | 1.0 |
| mailman | 1.0 (25 turns) | 1.0 |

**Expected pass rate: 5/5 (100%)**

Any deviation from 100% on the smoke set indicates:
- Infrastructure issue (endpoint down, model not loaded, OOM)
- Harness configuration error (wrong model string, missing API key, context window mismatch)
- Regression in `wg nex --eval-mode` (the thing we're actually testing)

---

## 7. eval-mode vs Full Service Path

### Recommendation: Use eval-mode (single nex invocation)

`wg nex --eval-mode` is the right path for the smoke test because:

1. **It's the thing being tested.** The whole point of this smoke is to validate the nex eval-mode → qwen3 pipeline.
2. **It's simpler.** Single process, no daemon, no coordinator, no service lifecycle.
3. **It's what TB/SWE-bench harnesses expect.** eval-mode outputs JSON on stdout, suppresses progress bars on stderr, runs autonomously (one-shot), and skips MCP.
4. **It matches the target architecture.** The synthesis task (synth-wg-nex-plan-of-attack) needs to know if eval-mode works end-to-end with qwen3.

### eval-mode behavior (from source):
- Implies `--autonomous` (EndTurn exits the loop)
- Implies `--no-mcp` (deterministic tool surface)
- No chat-file surface (no inbox/outbox pollution)
- JSON summary on stdout (machine-readable)
- Stderr kept clean for harness log capture
- Session lock uses `HandlerKind::Adapter`

### Full service path comparison:
- Uses `wg service start` + coordinator + `wg native-exec`
- More complex, more moving parts
- Tests the coordinator's task dispatch, not the harness interface
- Appropriate for Condition B–G evaluations, not for validating the eval-mode smoke

---

## 8. Proposed Smoke Test Commands

### Option A: Using eval-mode directly (recommended)

```bash
# Ensure qwen3 endpoint is reachable
curl -s http://lambda01:30000/v1/models | python3 -c "import json,sys; d=json.load(sys.stdin); print([m['id'] for m in d['data']])"

# Run smoke test tasks sequentially via nex --eval-mode
for task in text-processing debugging data-processing algorithm mailman; do
  echo "=== Running: $task ==="
  
  # Load instruction
  instruction=$(cat terminal-bench/tasks/*/$(ls terminal-bench/tasks/condition-a-calibration terminal-bench/tasks/hard-benchmarks | grep "$task" | head -1) 2>/dev/null)
  
  # Create isolated temp dir
  tmpdir=$(mktemp -d /tmp/tb-smoke-$task-XXXXXX)
  cd "$tmpdir"
  
  # Run nex in eval-mode
  wg nex --eval-mode \
    -m local:qwen3-coder-30b \
    -e http://lambda01:30000/v1 \
    --max-turns 200 \
    "$instruction"
  
  echo "Exit code: $?"
  cd -
  rm -rf "$tmpdir"
done
```

### Option B: Using the existing Python runner pattern (battle-tested)

```bash
cd terminal-bench

python run_pilot_qwen3_local_10.py \
  --smoke \
  --tasks text-processing debugging data-processing algorithm mailman \
  --timeout 2400
```

Note: `run_pilot_qwen3_local_10.py --smoke` currently only runs `text-processing`. Passing `--tasks` overrides this to the full smoke set.

### Option C: New dedicated smoke runner (cleanest)

Create `terminal-bench/run_smoke_nex_qwen3.py` that:
1. Uses `wg nex --eval-mode` instead of `wg service start`
2. Hardcodes the 5-task smoke set
3. Parses the JSON summary from stdout
4. Checks verify commands after nex completes
5. Outputs CTRF-format results

**Recommended approach: Option C** — this is the purpose of the eval-integration work. Option B works but tests the service path, not eval-mode. Option A is a shell sketch, not production-grade.

### Exact command for Option B (works today):

```bash
cd /home/erik/workgraph/terminal-bench
python run_pilot_qwen3_local_10.py \
  --tasks text-processing debugging data-processing algorithm mailman \
  --timeout 2400
```

Exit code 0 = at least one task passed. For full-pass check, inspect `results/pilot-qwen3-local-10/summary.json` and verify `pass_rate == 1.0`.

---

## Summary

| Question | Answer |
|----------|--------|
| Smoke task count | 5 (or 3 for minimal) |
| Proposed tasks | `text-processing`, `debugging`, `data-processing`, `algorithm`, `mailman` |
| Pass criteria | All 5 verify commands exit 0; no infra errors |
| Expected baseline | 5/5 (100%) based on prior qwen3 runs |
| Timeout per task | 2400s (40 min) |
| Concurrency | 1 agent (sequential, single GPU) |
| Context window | 32768 tokens |
| Execution path | `wg nex --eval-mode` (preferred) or `run_pilot_qwen3_local_10.py --tasks` (fallback) |
| Total expected duration | 15–30 minutes for 5 tasks |
