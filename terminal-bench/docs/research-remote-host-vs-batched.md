# Research: Remote Host vs Batched Trials for Long-Running TB Experiments

## Problem Statement

The full-scale TB 2.0 leaderboard submission requires **89 tasks x 5 trials per condition**.
Current state:
- **Condition A**: 89 tasks x 1 trial (need 4 more trials per task)
- **Condition F**: 18 tasks x 5 trials (need 71 more tasks x 5 trials)
- pilot-f-89 took 9+ hours; agent timed out at 59/90 trials before DNS rerun

The user's laptop must sleep when carried. We need a strategy that survives
interruptions and completes ~445+ trials per condition.

---

## 1. Disk Space Analysis

### Existing Data Sizes

| Run | Trials | Total Size | Per Trial |
|-----|--------|-----------|-----------|
| pilot-f-89 (wg-native) | 90 | 303 MB | 3.3 MB |
| full-condition-b (Harbor) | 270 | 304 MB | 1.1 MB |
| full-condition-c (Harbor) | 165 | 406 MB | 2.5 MB |
| full-condition-a (wg-native) | varies | 18 MB | ~1-2 MB |
| pilot-a-5x1 | varies | 15 MB | ~1-2 MB |

### Estimated Disk for Full Runs

**Per condition (89 tasks x 5 trials = 445 trials):**

| Format | Per Trial | 445 Trials | Notes |
|--------|-----------|-----------|-------|
| wg-native (Condition F) | ~3.3 MB | ~1.5 GB | Includes wg state per trial |
| Harbor native (Condition A) | ~1.1 MB | ~0.5 GB | Lighter, just result.json + config |

**All conditions combined:** ~2-4 GB of result data. **Negligible** on both local
(481 GB free) and most remote hosts.

**Docker images:** The 71 pre-pulled TB images total ~75 GB. This is the real disk
constraint for remote hosts — a fresh VM would need to pull all of these.

### Verdict: Disk is not a constraint locally (481 GB free). Remote hosts need
~80 GB for Docker images + results.

---

## 2. Remote Host Options

### What's Available

No remote host configuration exists in the codebase. No SSH configs, VM references,
or cloud provider setup were found. The project runs entirely on the local laptop.

### Remote Options (if pursued)

| Option | Disk | Cost | Setup Time | Notes |
|--------|------|------|-----------|-------|
| Cloud VM (e.g., Hetzner CX41) | 160 GB | ~$15/mo | 30-60 min | Need Docker + pre-pull images |
| Cloud VM (larger, e.g., CX51) | 320 GB | ~$30/mo | 30-60 min | Comfortable for images |
| Self-hosted server | varies | electricity | varies | If available |
| GitHub Codespace (16-core) | 64 GB | ~$0.36/hr | 10 min | Tight on disk for 75GB images |

### Remote Host Challenges

1. **Docker image pull**: 75 GB of images takes significant time and bandwidth
2. **API keys**: Need OPENROUTER_API_KEY on the remote host
3. **wg binary**: Must be compiled or copied to remote
4. **Monitoring**: Need SSH session or tmux/screen for long runs
5. **No existing infrastructure**: Everything must be set up from scratch

---

## 3. Batched Approach Design

### Core Insight

`run_scale_experiment.py` already has **crash-safe resume**: it writes per-trial
results to disk and can reload a manifest to skip completed trials. This is the
foundation for a batched approach.

### Design: Batch Runner with Resume

```
┌─────────────────────────────────────────────┐
│  Batch Controller (shell script or Python)  │
│                                             │
│  for batch in 1..N:                         │
│    run_scale_experiment.py --resume <dir>    │
│    → picks up where last batch stopped      │
│    → runs up to BATCH_SIZE trials           │
│    → writes results to disk atomically      │
│    → exits cleanly when batch complete       │
│                                             │
│  Progress persists in:                      │
│    results/<run-id>/manifest.json           │
│    results/<run-id>/<trial-id>/             │
└─────────────────────────────────────────────┘
```

### Implementation: Simple Wrapper

The simplest approach adds a `--max-trials` flag to `run_scale_experiment.py`
(or a wrapper that kills/restarts it):

```bash
#!/usr/bin/env bash
# batch-runner.sh — Run TB in resumable batches
set -euo pipefail

RESULTS_DIR="terminal-bench/results/full-submission-a"
BATCH_SIZE=30    # trials per batch (30 trials ≈ 1-1.5 hours)
CONDITION="A"

while true; do
    # Count remaining trials
    REMAINING=$(python3 -c "
import json, sys
m = json.load(open('$RESULTS_DIR/manifest.json'))
pending = [t for t in m['trial_order'] if m['trials'][t]['status'] not in ('done','failed_permanent')]
print(len(pending))
" 2>/dev/null || echo "445")

    if [ "$REMAINING" = "0" ]; then
        echo "All trials complete!"
        break
    fi

    echo "=== Starting batch: $REMAINING trials remaining ==="

    # Run with timeout (batch size * ~5 min per trial + buffer)
    timeout $((BATCH_SIZE * 300 + 600)) \
        python3 terminal-bench/run_scale_experiment.py \
            --resume "$RESULTS_DIR" \
            --conditions "$CONDITION" \
            --max-concurrent 4 \
        || true  # Don't fail on timeout/interruption

    echo "=== Batch complete, sleeping 10s before next ==="
    sleep 10
done
```

### Key Properties

1. **Survives laptop sleep**: Each batch is independent. When the laptop wakes,
   the next batch picks up from the manifest.
2. **Fresh agent per batch**: No turn budget exhaustion — each batch spawns new
   agents for remaining trials.
3. **Progress persists**: `manifest.json` tracks completed trials on disk.
4. **Configurable batch size**: 15-30 trials per batch (30-90 min each).
5. **No wg cycles needed**: The existing `--resume` infrastructure in
   `run_scale_experiment.py` handles this natively.

### Why Not wg Cycles?

Using wg cycles would add coordination overhead without benefit. The trial runner
already has:
- Crash-safe manifest persistence
- Resume from interruption
- Per-trial result isolation
- Adaptive concurrency

A wg cycle would wrap this in another layer of task management that doesn't add
value here. The runner IS the batch controller.

---

## 4. Time and Cost Estimates

### Per-Trial Timing (from pilot-f-89 data)

| Metric | Value |
|--------|-------|
| Mean trial time | 304 s (~5 min) |
| Total wall clock (90 trials, parallel) | 27,399 s (7.6 hrs) |
| Effective parallelism | ~1.0x (sequential due to resource contention) |

### Full Run Estimates

**Per condition (445 trials):**

| Scenario | Parallelism | Wall Clock | Notes |
|----------|------------|-----------|-------|
| Sequential (1 at a time) | 1 | ~37 hours | Conservative |
| 4 concurrent | 4 | ~9-10 hours | Pilot-f pattern |
| 8 concurrent | 8 | ~5-6 hours | Scale experiment default |
| Batched (30/batch, 4 conc) | 4 | ~37 hours total, ~1.5 hr/batch | Sleep-friendly |

**Two conditions (A + F) = ~890 trials:**

| Scenario | Wall Clock | Batches (30/batch) |
|----------|-----------|-------------------|
| Sequential per condition | ~74 hours | ~30 batches |
| Parallel conditions, 4 conc each | ~20 hours | ~15 batches |
| 8 concurrent, one condition at a time | ~12 hours | ~15 batches |

### API Cost

From pilot-f-89 token stats:
- ~710K tokens/trial (input-heavy due to context injection)
- At Minimax M2.7 via OpenRouter: effectively $0/trial (M2.7 is free-tier on OpenRouter)
- **Cost: $0** (model inference is free)

Condition A uses less context, so token usage will be lower (~200-400K/trial).

---

## 5. Recommendation: Batched Local

### Decision: **Batched local execution using `run_scale_experiment.py --resume`**

### Rationale

| Factor | Local Batched | Remote Host | Hybrid |
|--------|--------------|-------------|--------|
| Setup time | 0 (ready now) | 1-2 hours | 1-2 hours |
| Docker images | Already pulled (75 GB) | Must pull 75 GB | Must pull 75 GB |
| Disk space | 481 GB free | Need 80+ GB VM | Split |
| Sleep tolerance | Yes (resume between batches) | N/A (always on) | Partial |
| Monitoring | Local terminal | SSH/tmux | Both |
| API keys | Already configured | Must transfer | Both |
| Cost | $0 | $15-30/mo VM | $15-30/mo |
| Reliability | Proven (pilot runs worked) | Untested | Untested |

### Why Not Remote?

1. **No existing infrastructure**: Zero remote setup exists. Building it is 1-2
   hours of work for marginal benefit.
2. **Docker images are 75 GB**: Transferring or re-pulling these on a remote host
   is slow and may hit Docker Hub rate limits.
3. **Free model**: No cost savings from running remotely (model is free-tier).
4. **Resume already works**: `run_scale_experiment.py` has crash-safe resume built
   in — the hard problem is already solved.

### Why Not Hybrid?

A hybrid approach (start locally, finish remotely) adds complexity without clear
benefit given the resume capability. If the laptop can't sustain even batched runs
(e.g., multi-day travel), then a remote host becomes necessary — but that's a
contingency, not the default plan.

### Execution Plan

1. **Use `run_scale_experiment.py`** with `--resume` for both conditions
2. **Batch via `batch-runner.sh`** wrapper (30 trials per batch, ~1.5 hours each)
3. **Run Condition A first** (simpler, faster trials, validates the pipeline)
4. **Then Condition F** (heavier trials with wg context + surveillance loops)
5. **~15 batches per condition**, runnable across multiple laptop sessions
6. **Total calendar time**: 2-3 days of intermittent running

### Contingency: When to Switch to Remote

Switch to a remote host if:
- Laptop availability drops below 2 hours/day for running batches
- Network issues (DNS failures) are frequent enough to waste >30% of trials
- A deadline requires completing both conditions within 24 hours

In that case: spin up a Hetzner CX51 (320 GB, ~$30/mo), rsync the Docker images
via `docker save`/`docker load`, transfer API keys, and run continuously.
