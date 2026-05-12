# Research: TB Harbor Harness Setup for A/G Conditions

**Task:** research-tb-harbor-ag
**Date:** 2026-04-12

---

## 1. How are 'a' (agent-only) and 'g' (graph/wg) conditions invoked for a TB task?

There are **two execution paths**: Harbor (Docker-based, for official TB benchmarks) and host-native (for quick local pilots).

### Harbor Path (official TB evaluation)

Both conditions use the same adapter (`wg/adapter.py`) with condition-specific agent classes:

```bash
# Condition A: bare agent, no wg tools
harbor run \
  --agent-import-path "wg.adapter:ConditionAAgent" \
  -m "openrouter:openai/gpt-oss-120b" \
  -d terminal-bench@2.0 \
  -k 1 \
  --n-concurrent 4 \
  --no-delete \
  --task-ids build-cython-ext \
  --jobs-dir terminal-bench/results/gpt-oss-120b-condition-A \
  -y

# Condition G: autopoietic graph-building agent (8 parallel agents)
harbor run \
  --agent-import-path "wg.adapter:ConditionGAgent" \
  -m "openrouter:openai/gpt-oss-120b" \
  -d terminal-bench@2.0 \
  -k 1 \
  --n-concurrent 4 \
  --no-delete \
  --task-ids build-cython-ext \
  --jobs-dir terminal-bench/results/gpt-oss-120b-condition-G \
  -y
```

**What happens under the hood:**
1. Harbor spins up a Docker container per trial (from TB task definitions)
2. `setup()` uploads the `wg` binary into the container, runs `wg init`, writes `config.toml`
3. `run()` calls `_run_native_executor()` which:
   - Creates a root task via `wg add`
   - Starts `wg service start` (daemon that spawns native-exec agents)
   - Polls `wg show <task-id>` until terminal status
4. Harbor's verifier checks the task's test script independently

### Host-Native Path (local pilots, no Docker)

Used by scripts like `run_pilot_a_vs_g_haiku.py` and `run_pilot_free_smoke.py`:

```bash
# Condition A (wg-coordinated, single agent)
python terminal-bench/run_pilot_free_smoke.py \
  --model "openrouter:openai/gpt-oss-120b" \
  --run-id pilot-gpt-oss-120b-a

# Condition G (raw Claude Code, no wg) — uses `claude` CLI
# NOTE: This path is Claude-specific and doesn't work with Harbor models.
# For non-Claude models, use the Harbor path.
```

The host-native pilots create isolated temp directories with `wg init`, write config, create root tasks, start `wg service`, and poll.

---

## 2. What does the 'g' harness do exactly — how does it decompose a TB task into a graph of subtasks?

Condition G is the **autopoietic** condition. Here's the exact mechanism:

### Config differences from Condition A

```python
CONDITION_CONFIG = {
    "A": {
        "exec_mode": "full",
        "context_scope": "clean",       # No graph context
        "agency": None,
        "exclude_wg_tools": True,       # Custom bundle: bash + file tools ONLY
        "max_agents": 1,                # Single agent
    },
    "G": {
        "exec_mode": "full",
        "context_scope": "graph",       # Full dependency graph visible
        "agency": None,
        "exclude_wg_tools": False,      # All tools including wg_*
        "max_agents": 8,                # Up to 8 parallel agents
        "autopoietic": False,           # Phase 3: coordinator orchestrates
        "coordinator_agent": True,      # Persistent coordinator agent
        "heartbeat_interval": 30,       # 30s autonomous heartbeat
        "coordinator_model": "sonnet",  # ← THIS IS A MODEL PURITY CONCERN
    },
}
```

### Meta-prompt injection

When `cfg.get("autopoietic")` is True (older path), a meta-prompt is prepended to the task instruction telling the agent to:
1. Read the task and understand it
2. Explore the working directory
3. Build a wg with `wg add` (parallel tasks where possible)
4. Create a verify task that closes a self-correcting cycle
5. Mark its own seed task done, letting the coordinator dispatch workers

In the current Phase 3 config (`autopoietic=False, coordinator_agent=True`), the coordinator agent orchestrates instead.

### Architect bundle

For the autopoietic path, an architect bundle limits tools to:
```
bash, read_file, glob, grep, wg_show, wg_list, wg_add, wg_done, wg_fail, wg_log, wg_artifact
```
No `write_file` or `edit_file` — the architect only designs the graph.

### Completion detection

For multi-agent (G), polling checks `wg list --status open,in-progress` instead of a single task's status. When no tasks remain open/in-progress, the trial completes.

---

## 3. How to ensure model purity — ZERO Claude/Opus/Sonnet mixing

This is the **critical concern** for Harbor models. Multiple layers must be configured consistently:

### Where model leakage can occur

| Layer | Config point | Risk |
|-------|-------------|------|
| **config.toml `[coordinator]`** | `model = "..."` | Coordinator scheduling decisions |
| **config.toml `[agent]`** | `model = "..."` | Spawned agent LLM calls |
| **`wg service start --model`** | CLI override | Service-level default |
| **`wg add --model`** | Per-task override | Individual task model |
| **`coordinator_model`** in CONDITION_CONFIG | Condition G Phase 3 | **HARDCODED to "sonnet"** |
| **Env vars `WG_MODEL`** | Parent process leak | Inherited model override |
| **BENCHMARK_MODEL constant** | `wg/adapter.py` line 67 | Fallback default |

### Ensuring purity

**Step 1: Adapter config.toml** — `_build_config_toml_content()` writes both `[coordinator].model` and `[agent].model` from the `model` parameter passed to `_run_native_executor()`:

```toml
[coordinator]
model = "openrouter:openai/gpt-oss-120b"
executor = "native"

[agent]
model = "openrouter:openai/gpt-oss-120b"
```

**Step 2: Task creation** — The adapter passes `--model` to `wg add` (not done in the Docker path, but done in host-native pilots).

**Step 3: Service start** — The adapter passes `--model` to `wg service start`.

**Step 4: Environment isolation** — `_exec_wg_cmd_host()` and all pilot scripts strip `WG_*` env vars and `CLAUDECODE` from subprocesses.

### KNOWN PURITY ISSUE: Condition G's `coordinator_model`

In `CONDITION_CONFIG["G"]`, there's `"coordinator_model": "sonnet"`. This is used for the persistent coordinator agent in Phase 3. **This MUST be overridden** for non-Claude models. Options:

1. **Override in CONDITION_CONFIG** before running — set `coordinator_model` to the target model
2. **Disable coordinator agent** — use `--no-coordinator-agent` flag, falling back to the simpler autopoietic path
3. **Modify `_build_config_toml_content()`** to inject the trial model for coordinator_model

**Recommendation**: For Harbor runs with non-Claude models, explicitly disable the coordinator agent or patch `CONDITION_CONFIG["G"]["coordinator_model"]` to match the trial model. The simplest fix:

```python
# In the runner script, before creating the agent:
from wg.adapter import CONDITION_CONFIG
CONDITION_CONFIG["G"]["coordinator_model"] = "openrouter:openai/gpt-oss-120b"
```

### Model verification after runs

```bash
# Check stream.jsonl for actual model used
find <results-dir> -name "stream.jsonl" -exec grep -o '"model":"[^"]*"' {} \; | sort -u

# Verify no Claude/Anthropic model leakage
find <results-dir> -name "stream.jsonl" -exec grep -il "claude\|anthropic\|sonnet\|opus\|haiku" {} \;
# Should return empty
```

---

## 4. Harbor endpoint/config for GPT-OSS-120B and Nemotron 3 Super

### OpenRouter Model IDs

| Model | OpenRouter ID | Free variant | Pricing (per M tokens) |
|-------|--------------|-------------|----------------------|
| GPT-OSS-120B | `openai/gpt-oss-120b` | `openai/gpt-oss-120b:free` | $0.039 in / $0.19 out |
| Nemotron 3 Super | `nvidia/nemotron-3-super-120b-a12b` | `nvidia/nemotron-3-super-120b-a12b:free` | $0.10 in / $0.50 out |

### wg model format

Harbor and wg use different formats:
- **Harbor `-m` flag**: `openrouter:openai/gpt-oss-120b` (normalized by `_normalize_model()`)
- **config.toml**: `model = "openrouter:openai/gpt-oss-120b"`
- **`wg service start --model`**: `"openrouter:openai/gpt-oss-120b"`

The adapter's `_normalize_model()` handles conversion: `openrouter/openai/gpt-oss-120b` → `openrouter:openai/gpt-oss-120b`.

### API key requirement

Both models use OpenRouter, so `OPENROUTER_API_KEY` must be set:

```bash
export OPENROUTER_API_KEY="sk-or-v1-YOUR-KEY"
```

The adapter exports this into the Docker container environment for the wg daemon to inherit.

### Provider routing in native-exec

`create_provider_ext()` in `src/executor/native/provider.rs` parses the model spec:
- `openrouter:openai/gpt-oss-120b` → provider=`openrouter`, model=`openai/gpt-oss-120b`
- This maps to an `OpenAiClient` configured with `https://openrouter.ai/api/v1` as base URL
- API key resolved from `OPENROUTER_API_KEY` env var or endpoint config

---

## 5. Exact commands to run a single TB task in 'a' mode and 'g' mode

### Condition A: Single task via Harbor

```bash
cd ~/workgraph

# GPT-OSS-120B
harbor run \
  --agent-import-path "wg.adapter:ConditionAAgent" \
  -m "openrouter:openai/gpt-oss-120b" \
  -d terminal-bench@2.0 \
  -k 1 \
  --n-concurrent 1 \
  --no-delete \
  --task-ids text-processing \
  --jobs-dir terminal-bench/results/gpt-oss-120b-A-smoke \
  -y

# Nemotron 3 Super
harbor run \
  --agent-import-path "wg.adapter:ConditionAAgent" \
  -m "openrouter:nvidia/nemotron-3-super-120b-a12b" \
  -d terminal-bench@2.0 \
  -k 1 \
  --n-concurrent 1 \
  --no-delete \
  --task-ids text-processing \
  --jobs-dir terminal-bench/results/nemotron3-super-A-smoke \
  -y
```

### Condition G: Single task via Harbor

```bash
cd ~/workgraph

# GPT-OSS-120B — NOTE: patch coordinator_model first (see Section 3)
harbor run \
  --agent-import-path "wg.adapter:ConditionGAgent" \
  -m "openrouter:openai/gpt-oss-120b" \
  -d terminal-bench@2.0 \
  -k 1 \
  --n-concurrent 1 \
  --no-delete \
  --task-ids text-processing \
  --jobs-dir terminal-bench/results/gpt-oss-120b-G-smoke \
  -y

# Nemotron 3 Super
harbor run \
  --agent-import-path "wg.adapter:ConditionGAgent" \
  -m "openrouter:nvidia/nemotron-3-super-120b-a12b" \
  -d terminal-bench@2.0 \
  -k 1 \
  --n-concurrent 1 \
  --no-delete \
  --task-ids text-processing \
  --jobs-dir terminal-bench/results/nemotron3-super-G-smoke \
  -y
```

### Host-native path (no Docker, no Harbor)

```bash
cd ~/workgraph

# Uses run_pilot_free_smoke.py which creates isolated wg instances per trial
python terminal-bench/run_pilot_free_smoke.py \
  --model "openrouter:openai/gpt-oss-120b" \
  --run-id pilot-gpt-oss-120b-smoke \
  --tasks text-processing

python terminal-bench/run_pilot_free_smoke.py \
  --model "openrouter:nvidia/nemotron-3-super-120b-a12b" \
  --run-id pilot-nemotron3-super-smoke \
  --tasks text-processing
```

**Note:** The host-native path uses `executor = "native"` (not `"claude"`), so it works with any OpenRouter model. It does NOT use Docker containers — tasks run on the host filesystem.

### Via tb-harness.sh (native executor, Condition A only)

```bash
cd ~/workgraph

./terminal-bench/tb-harness.sh \
  --condition A \
  --model "openai/gpt-oss-120b" \
  --task "Create a Python script at /tmp/wordfreq.py that reads stdin and prints word frequencies" \
  --max-turns 50 \
  --timeout 600
```

**Note:** `tb-harness.sh` uses `WG_LLM_PROVIDER="openai"` and the OpenRouter endpoint URL directly. It only supports Conditions A, B, and C (not G).

---

## 6. Where do results land (pass/fail, timing, logs)?

### Harbor path results

```
<jobs-dir>/<job-timestamp>/
  config.json                          # Job-level config
  result.json                          # Aggregate results (pass rate, timing)
  job.log                              # Harbor job log
  <task-name>__<hash>/                 # Per-trial directory
    result.json                        # Trial result (reward, timing)
    config.json                        # Trial config
    trial.log                          # Harbor trial execution log
    agent/
      trial_summary.json               # Agent metrics (tokens, turns, cost)
      agent_loop.ndjson                # Full interaction trace
      wg-artifacts/                    # Downloaded .wg/ from container
        .wg/
          graph.jsonl                  # Task graph state
          config.toml                  # Applied config
          service/daemon.log           # Coordinator/daemon log
          agents/agent-*/stream.jsonl  # Native executor event stream
    verifier/
      reward.txt                       # 1.0 (pass) or 0.0 (fail)
      ctrf.json                        # Common Test Report Format
      test-stdout.txt                  # Verification command output
```

**Quick pass/fail check:**
```bash
cat <jobs-dir>/*/text-processing__*/verifier/reward.txt
# "1.0" = pass, "0.0" = fail
```

### Host-native path results

```
terminal-bench/results/<run-id>/
  summary.json                         # Aggregate results
  <trial-id>/
    workgraph_state/                   # Copied .wg/ directory
      graph.jsonl
      config.toml
      agents/*/stream.jsonl
      service/daemon.log
```

**Quick pass/fail check:**
```bash
python3 -c "import json; d=json.load(open('terminal-bench/results/<run-id>/summary.json')); print(f'Pass rate: {d[\"passed\"]}/{d[\"total_trials\"]}')"
```

---

## Known Gotchas

### Rate limits
- Free-tier OpenRouter models have strict rate limits (especially `:free` variants)
- GPT-OSS-120B free: may hit 429s under concurrent load
- **Recommendation**: Use paid variants for multi-task runs; limit `--n-concurrent` to 1-2 for free tiers

### Timeout
- Default: 1800s (30min) per trial
- Complex tasks (iterative-test-fix, multi-module-type-migration) may need more
- Condition G is slower due to graph setup + multiple agent spawns

### Memory
- Condition G with `max_agents=8` runs 8 concurrent native-exec processes
- Each needs ~2-4GB RAM in Docker containers
- Total: ~16-32GB for Condition G at full concurrency

### Bookworm binary
- TB Docker containers run Debian bookworm (glibc 2.36)
- Host binary may require newer glibc → build bookworm-compatible:
  ```bash
  docker run --rm -v "$(pwd)":/workspace -w /workspace rust:bookworm \
    cargo build --release --target-dir /workspace/target/bookworm-build
  mkdir -p target/bookworm-out
  cp target/bookworm-build/release/wg target/bookworm-out/wg
  ```

### BENCHMARK_MODEL constant
- `wg/adapter.py` line 67: `BENCHMARK_MODEL = "openrouter:minimax/minimax-m2.7"`
- This is only a **default** — passing `-m` to Harbor overrides it
- The `kwargs.setdefault("model_name", BENCHMARK_MODEL)` in each ConditionXAgent only applies if no model is provided

### Condition G coordinator_model = "sonnet"
- **Critical**: CONDITION_CONFIG["G"]["coordinator_model"] is hardcoded to "sonnet"
- This WILL cause Claude API calls if not patched for non-Claude runs
- **Fix**: Either disable coordinator agent or patch this config value (see Section 3)

### Docker image pre-pull
- 89 tasks × N trials = many container starts
- Docker Hub rate limits: ~100 pulls/6h anonymous
- Pre-pull with `bash terminal-bench/pre-pull-images.sh` before full runs
- Use `--no-delete` with Harbor to keep cached images

### Environment variable isolation
- Parent agent WG_* vars MUST NOT leak into trial subprocesses
- The adapter strips them via `_exec_wg_cmd_host()` and `clean_env` filtering
- Verify: no `WG_MODEL`, `WG_EXECUTOR_TYPE`, or `CLAUDECODE` in trial environment
