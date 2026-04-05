# Design: TB Trial Federation to Base Project

**Task:** design-tb-federation
**Date:** 2026-04-05
**Status:** Proposed
**Depends on:** [DESIGN-native-wg-integration.md](DESIGN-native-wg-integration.md), implement-native-wg

---

## 1. Problem Statement

Each Terminal Bench trial creates a temporary workgraph in `/tmp/tb-wg-XXXX/` that gets destroyed after the trial completes. This means:

1. **Evaluation data is lost** — FLIP scores, agent performance, task outcomes disappear with `shutil.rmtree`
2. **Agency primitives are not shared** — each trial bootstraps from starter roles/tradeoffs via `wg agency init`, never learning from prior trials
3. **Evolution cannot run** — `wg evolve` needs accumulated evaluation data across many tasks; a single-trial graph has at most one evaluation
4. **No cross-trial analysis** — performance trends, role fitness, tradeoff effectiveness are invisible

The goal: make TB trials federate with a persistent base project so that evaluation data accumulates, agency primitives improve over time via evolution, and benchmark results compound.

## 2. Federation Topology

### 2.1 Hub-and-Spoke Model

```
                    ┌──────────────────────────────┐
                    │  Hub: tb-evaluations/         │
                    │  .workgraph/                  │
                    │    agency/                    │
                    │      cache/roles/             │  ← accumulated roles
                    │      primitives/tradeoffs/    │  ← accumulated tradeoffs
                    │      cache/agents/            │  ← accumulated agents
                    │      evaluations/             │  ← ALL trial evaluations
                    │    federation.yaml            │  ← (optional: upstream remote)
                    │    config.toml                │
                    └──────────┬───────────────────┘
                               │
              ┌────────────────┼────────────────┐
              │                │                │
         pull ↓ push ↑   pull ↓ push ↑   pull ↓ push ↑
              │                │                │
    ┌─────────┴──┐   ┌────────┴───┐   ┌────────┴───┐
    │ Trial 1    │   │ Trial 2    │   │ Trial N    │
    │ /tmp/tb-wg │   │ /tmp/tb-wg │   │ /tmp/tb-wg │
    │ (ephemeral)│   │ (ephemeral)│   │ (ephemeral)│
    └────────────┘   └────────────┘   └────────────┘
```

### 2.2 Hub Project: Dedicated `tb-evaluations/`

**Decision: Use a dedicated project, NOT the main workgraph repo.**

Rationale:
- The main workgraph project's agency pool serves production use — polluting it with benchmark-model evaluations (e.g., minimax-m2.7 results) would distort evolution for production agents
- A dedicated hub keeps benchmark data isolated while still using the full federation machinery
- The hub can optionally federate *upstream* to the main project for roles/tradeoffs that prove universally good, but this is a deliberate manual step, not automatic

Location: `terminal-bench/tb-evaluations/` (checked into the repo, .gitignored for evaluation data)

```
terminal-bench/tb-evaluations/
├── .workgraph/
│   ├── config.toml          # hub config (no coordinator needed)
│   ├── federation.yaml      # optional upstream remote to main project
│   └── agency/
│       ├── cache/
│       │   ├── roles/       # evolved roles
│       │   └── agents/      # agent identities used in trials
│       ├── primitives/
│       │   ├── components/  # skill components
│       │   ├── outcomes/    # desired outcomes
│       │   └── tradeoffs/   # evolved tradeoffs
│       └── evaluations/     # ALL trial evaluations accumulate here
└── .gitignore               # ignore evaluations/ (large, generated)
```

### 2.3 Why Not Global `~/.workgraph/agency/`?

The global agency store (`~/.workgraph/agency/`) is another candidate hub, but:
- It mixes benchmark results with production agent data
- It's user-specific, not reproducible across machines
- It can't be checked into the repo or shared with collaborators

The `--global` flag on `wg agency pull/push` exists for production federation, not benchmarking.

## 3. Data Flow

### 3.1 What Flows Where

| Direction | What | Why |
|-----------|------|-----|
| Hub → Trial (pull) | Roles, tradeoffs, agents | Trial uses the current evolved primitive pool instead of starters |
| Trial → Hub (push) | Evaluations, performance records | Evaluation data accumulates for evolution |
| Hub → Hub (evolve) | Roles, tradeoffs mutated/crossed | `wg evolve` improves the primitive pool |
| Hub → Main (manual push) | Proven roles/tradeoffs | Manually promote benchmark-validated primitives to production |

### 3.2 What Does NOT Flow

| Data | Reason |
|------|--------|
| Full task graphs (`graph.jsonl`) | Trial-specific, too large, already archived by adapter |
| Agent stream logs (`stream.jsonl`) | Trial-specific, already archived by adapter |
| Task descriptions/instructions | Trial-specific, comes from Harbor task definitions |
| Config (`config.toml`) | Trial-specific, condition-dependent |

### 3.3 Federation Transfer Options

Based on the existing `TransferOptions` in `federation.rs`:

**Pull (hub → trial):**
```rust
TransferOptions {
    dry_run: false,
    no_performance: false,   // trial WANTS performance data to inform assignment
    no_evaluations: true,    // trial doesn't need 1000s of evaluation JSONs
    force: false,
    entity_ids: vec![],      // pull everything
    entity_filter: EntityFilter::All,
}
```

**Push (trial → hub):**
```rust
TransferOptions {
    dry_run: false,
    no_performance: false,   // push updated performance records
    no_evaluations: false,   // push new evaluations (the whole point)
    force: false,
    entity_ids: vec![],      // push everything that changed
    entity_filter: EntityFilter::All,
}
```

## 4. Content-Hash ID Consistency

Federation already handles content-hash IDs correctly:

1. **Same primitive, same hash**: The role "programmer" with the same component_ids and outcome_id produces the same content hash regardless of where it was created
2. **Performance record merging**: `transfer()` in `federation.rs` already unions evaluation lists by `task_id` — duplicate evaluations are deduplicated
3. **Cross-trial dedup**: If Trial 1 and Trial 2 both use role `r-abc123`, their evaluations accumulate under the same role in the hub
4. **Evolved entities get new hashes**: When `wg evolve` mutates a role, it gets a new hash (new `id`) with lineage pointing to the parent — no collision with the original

**Edge case — starter roles**: `wg agency init` creates roles with deterministic IDs from the starter CSV. All trials calling `wg agency init` independently would produce identical IDs. This is *correct* — they're the same roles. But if a trial has already pulled evolved roles from the hub, the starters become redundant. Solution: skip `wg agency init` when pulling from a hub that already has roles.

## 5. Trial Adapter Modifications

### 5.1 New Parameters

```python
class WorkgraphAgent(BaseAgent):
    def __init__(
        self,
        ...
        federation_hub: str | None = None,  # path to tb-evaluations/.workgraph
        evolve_after_n: int = 0,            # run evolve every N trials (0 = never)
        pull_primitives: bool = True,       # pull roles/tradeoffs from hub before trial
        push_evaluations: bool = True,      # push evaluations to hub after trial
        ...
    ):
```

### 5.2 Modified `setup()` — Add Federation Pull

Current flow:
```
1. Create temp dir
2. wg init
3. Write config
4. wg agency init (for D/E conditions)
5. Create root task
```

New flow:
```
1. Create temp dir
2. wg init
3. Write config
4. Write federation.yaml (with hub remote)
5. IF hub exists AND has agency data:
     wg agency pull hub --no-evaluations    # pull roles/tradeoffs/agents
   ELSE:
     wg agency init                          # bootstrap from starters
6. Create root task
7. (D/E) Assign agent identity
```

### 5.3 Modified Teardown — Add Evaluation + Push

Current flow (end of `run()`):
```
1. Stop service
2. Collect metrics
3. Copy graph state to logs
4. Cleanup temp dir
```

New flow:
```
1. Stop service
2. Run evaluation on completed tasks:
     wg evaluate <root-task-id>             # generates evaluation JSON
3. Collect metrics
4. IF hub configured AND push_evaluations:
     wg agency push hub                      # push evaluations + performance to hub
5. IF evolve_after_n > 0 AND trial_count % evolve_after_n == 0:
     wg evolve run --dir <hub>               # trigger evolution on hub
6. Copy graph state to logs
7. Cleanup temp dir
```

### 5.4 `federation.yaml` Template for Trial Graphs

```python
async def _write_trial_federation_config(wg_dir: str, hub_path: str) -> None:
    """Write .workgraph/federation.yaml pointing to the hub."""
    config = {
        "remotes": {
            "hub": {
                "path": hub_path,
                "description": "TB evaluation hub for federation",
            }
        }
    }
    federation_path = os.path.join(wg_dir, "federation.yaml")
    with open(federation_path, "w") as f:
        yaml.dump(config, f, default_flow_style=False)
```

### 5.5 Hub Initialization (One-Time Setup)

```python
async def _ensure_hub_initialized(hub_path: str, wg_bin: str) -> None:
    """Initialize the federation hub if it doesn't exist."""
    wg_dir = os.path.join(hub_path, ".workgraph")
    if os.path.isdir(os.path.join(wg_dir, "agency")):
        return  # already initialized
    
    await _exec_wg_cmd_host(wg_dir, wg_bin, ["init"])
    await _exec_wg_cmd_host(wg_dir, wg_bin, ["agency", "init"])
    logger.info(f"Initialized federation hub at {hub_path}")
```

### 5.6 Condition-Specific Federation Behavior

| Condition | Pull from Hub? | Push to Hub? | Agency Bootstrap |
|-----------|---------------|-------------|------------------|
| A (control) | No | No | None — no agency |
| B (treatment) | No | No | None — no agency |
| C (treatment) | No | No | None — no agency |
| D (treatment) | Yes — pull roles/tradeoffs for assignment | Yes — push evaluation | Pull agent from hub, or create with hub's role/tradeoff |
| E (treatment) | Yes — pull full primitive pool | Yes — push evaluation | Pull agent from hub, or create with hub's role/tradeoff |
| F (treatment) | Yes — pull full primitive pool | Yes — push evaluation | Use hub's auto-assignment pool |

Conditions A–C don't use agency, so federation is irrelevant for them. Conditions D–F are the ones that benefit from an evolving primitive pool.

## 6. Evolution Integration Plan

### 6.1 When to Trigger Evolution

Three strategies, from simplest to most sophisticated:

**Strategy 1: Manual (recommended for initial deployment)**
```bash
# After running a batch of trials:
wg evolve run --dir terminal-bench/tb-evaluations/.workgraph
```

This keeps the operator in control. Run it after completing a full trial sweep (e.g., all 6 conditions × N replicas).

**Strategy 2: After N Trials (automatic)**
```python
# In adapter teardown:
trial_count = _count_hub_evaluations(hub_path)
if evolve_after_n > 0 and trial_count % evolve_after_n == 0:
    await _exec_wg_cmd_host(hub_wg_dir, wg_bin, ["evolve", "run"])
```

Risk: concurrent trials could trigger multiple evolve runs. Mitigate with file locking (the hub's `.workgraph/` already uses flock).

**Strategy 3: Hub-Resident Cycle (fully automated)**
Create a cycle task in the hub project:
```bash
wg add "Evolve agency pool" --dir tb-evaluations/.workgraph \
  --max-iterations 100 \
  --verify "wg evolve run --dry-run | grep -q 'operations'"
```

This runs as a wg task, managed by the hub's coordinator. Most sophisticated, but requires the hub to run its own service.

**Recommendation: Start with Strategy 1, implement Strategy 2 as a configuration option.**

### 6.2 Evolution Parameters for TB

```bash
wg evolve run \
  --dir terminal-bench/tb-evaluations/.workgraph \
  --strategy balanced \         # use balanced mutation/crossover
  --budget 5                    # max 5 new primitives per cycle
```

The `balanced` strategy is appropriate because:
- TB generates diverse evaluation signals (different task types, conditions, models)
- We want both mutation (refine existing) and crossover (combine strong primitives)
- Budget of 5 prevents explosive growth of the primitive pool

### 6.3 Evolution Feedback Loop

```
Trial batch N
    ↓
Evaluations accumulate in hub
    ↓
wg evolve run → new/mutated roles + tradeoffs
    ↓
Trial batch N+1 pulls evolved primitives
    ↓
Better agent assignment → (hopefully) better task outcomes
    ↓
Evaluations accumulate in hub
    ↓
(repeat)
```

This is the core value proposition: **benchmark trials drive agency evolution, and evolved agencies improve benchmark performance.**

## 7. Hub Project Structure

### 7.1 Files to Create

```
terminal-bench/tb-evaluations/
├── .workgraph/
│   ├── config.toml
│   ├── graph.jsonl              # empty initially (hub doesn't need tasks)
│   └── agency/
│       └── (initialized by wg agency init)
├── .gitignore
└── README.md
```

### 7.2 `config.toml`

```toml
[coordinator]
max_agents = 0                   # hub doesn't run agents
model = "sonnet"                 # for evolve LLM calls

[agent]
context_scope = "clean"
```

### 7.3 `.gitignore`

```gitignore
# Large generated data — don't check in
.workgraph/agency/evaluations/
.workgraph/service/

# DO check in evolved primitives (they're the value)
!.workgraph/agency/cache/
!.workgraph/agency/primitives/
```

### 7.4 Optional: Upstream Federation to Main Project

```yaml
# terminal-bench/tb-evaluations/.workgraph/federation.yaml
remotes:
  upstream:
    path: "../../.workgraph/agency"
    description: "Main workgraph project agency pool"
```

This allows:
- `wg agency pull upstream` — seed the hub from the main project's primitives
- `wg agency push upstream --entity-type role --entity-ids <proven-role-hash>` — promote a benchmark-validated role to production

## 8. Migration Plan for Existing Trial Data

### 8.1 Current State

The adapter currently calls `shutil.copytree(wg_dir, logs_dir / "workgraph_state")` before cleanup. If any previous trial runs exist with this archived state, we can recover evaluation data.

### 8.2 Recovery Script

```python
#!/usr/bin/env python3
"""Recover evaluations from archived trial workgraph states."""

import os
import subprocess
import sys

def recover_evaluations(logs_root: str, hub_path: str, wg_bin: str = "wg"):
    """Scan archived trial states and push evaluations to hub."""
    hub_wg = os.path.join(hub_path, ".workgraph")
    
    for trial_dir in sorted(os.listdir(logs_root)):
        state_dir = os.path.join(logs_root, trial_dir, "workgraph_state")
        agency_dir = os.path.join(state_dir, "agency")
        
        if not os.path.isdir(agency_dir):
            continue
        
        evals_dir = os.path.join(agency_dir, "evaluations")
        if not os.path.isdir(evals_dir) or not os.listdir(evals_dir):
            continue
        
        # Push evaluations from archived state to hub
        result = subprocess.run(
            [wg_bin, "--dir", hub_wg, "agency", "pull", agency_dir],
            capture_output=True, text=True,
        )
        print(f"  {trial_dir}: {result.stdout.strip()}")
```

### 8.3 Realistic Assessment

Most existing trial data was generated with the old litellm-based adapter, which did NOT run `wg evaluate`. Those archived `.workgraph/` directories contain task graphs but no evaluation JSONs. Recovery is only possible for trials run after the native wg adapter was deployed AND evaluation was wired in.

**Bottom line: treat migration as best-effort. The main value is forward-looking.**

## 9. Concurrency Considerations

### 9.1 Parallel Trials Pushing to Same Hub

TB trials can run in parallel (different conditions, different tasks). Multiple trials pushing evaluations to the same hub concurrently is safe because:

1. **File-level isolation**: Each evaluation is a separate JSON file in `evaluations/`. Separate trials create separate files — no conflict.
2. **Performance record merging**: `transfer()` in `federation.rs` reads-modifies-writes role/tradeoff YAML files. This IS a race condition if two trials finish simultaneously.

**Mitigation**: The existing flock-based locking in `federation.rs`'s `transfer()` function handles this — it takes a write lock on the target store. Verify this is actually the case; if not, add flock around the push operation.

### 9.2 Evolution During Active Trials

If `wg evolve` runs while trials are pulling primitives, a trial might get a partially-evolved pool. This is acceptable:

- Evolution adds new primitives and retires old ones
- A trial that pulls mid-evolution gets a valid (if slightly inconsistent) snapshot
- The next trial batch will get the fully evolved pool

For strict consistency, evolve only between trial batches (Strategy 1).

## 10. Implementation Tasks

### Phase 1: Hub Setup (no code changes)
1. Create `terminal-bench/tb-evaluations/` directory structure
2. Initialize with `wg init` + `wg agency init`
3. Seed from main project: `wg agency pull ../../.workgraph/agency`

### Phase 2: Adapter Federation Wiring
1. Add `federation_hub` parameter to `WorkgraphAgent.__init__()`
2. Add `_write_trial_federation_config()` helper
3. Add `_ensure_hub_initialized()` helper
4. Modify `setup()`: federation pull before trial
5. Modify teardown: `wg evaluate` + federation push after trial
6. Add `evolve_after_n` parameter and post-trial evolve trigger

### Phase 3: Evaluation Integration
1. Wire `wg evaluate <root-task-id>` into the adapter teardown
2. Ensure evaluation runs even for failed tasks (valuable signal)
3. Verify evaluation data includes condition, model, and timing metadata

### Phase 4: Evolution Loop
1. Test `wg evolve run` on the hub after manual trial batch
2. Add `evolve_after_n` automatic trigger
3. Validate that evolved primitives improve trial outcomes

### Phase 5: Analysis Tooling
1. Extend `tb_collect_results.py` to read hub evaluation data
2. Add cross-trial performance tracking (role fitness over time)
3. Add evolution lineage visualization (which primitives produced which)

## 11. Validation Checklist

- [x] Federation topology documented (hub project, spoke trials)
- [x] Data flow specified (roles/tradeoffs pull to trials, evaluations push to hub)
- [x] Trial adapter modifications identified (setup: pull, teardown: evaluate + push)
- [x] Evolution trigger strategy defined (manual first, automatic after N as option)
- [x] Content-hash ID consistency across trial boundaries addressed (natural dedup via transfer())

## 12. Open Questions

1. **Should Condition A–C trials also push metadata?** Currently no agency = no evaluations to push. But we could create synthetic evaluations for non-agency conditions to track baseline performance over time.

2. **Hub persistence across CI/machines**: If TB runs in CI, the hub would need to be on a shared filesystem or committed to the repo. For local development, a path relative to the repo root works fine.

3. **Evaluation model**: `wg evaluate` uses an LLM to score tasks. Should we use the same `BENCHMARK_MODEL` for evaluation, or a stronger model? Using a stronger model (e.g., sonnet) for evaluation while using the benchmark model for execution is standard practice in LLM-as-judge benchmarks.

4. **Evolve model**: Similarly, `wg evolve` uses an LLM to propose mutations. This should use a reasoning-capable model (opus or sonnet), not the benchmark model.

---

*This design document is an artifact of task `design-tb-federation`. Implementation tasks should be created as subtasks.*
