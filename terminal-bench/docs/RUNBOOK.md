# Terminal-Bench Reproduction Runbook

Self-contained guide for reproducing the Terminal-Bench experiment (Conditions A
and F) on a remote Linux host. Uses the Harbor framework with the wg-native
executor inside Docker containers, with Minimax M2.7 via OpenRouter.

**Target audience:** Automated agents or humans on a clean machine with no prior
context. Every command is copy-pasteable.

---

## 1. Prerequisites

| Requirement | Details |
|-------------|---------|
| **OS** | Linux x86_64 (tested on Debian 12 / Ubuntu 22.04+) |
| **Docker** | Installed and running (`docker info` succeeds) |
| **Python** | 3.10+ with pip |
| **Rust toolchain** | For building the `wg` binary |
| **OPENROUTER_API_KEY** | Set in environment (Minimax M2.7 is free-tier) |
| **Disk space** | ~80 GB for Docker images + results |
| **RAM** | 8 GB minimum, 16 GB recommended |
| **Network** | Stable outbound HTTPS to OpenRouter and Docker registries |

---

## 2. One-Time Setup

### 2.1 System Packages

```bash
sudo apt update && sudo apt install -y \
  build-essential gcc g++ git curl jq \
  python3 python3-dev python3-pip python3-venv \
  tmux
```

### 2.2 Docker

If Docker is not already installed:

```bash
sudo apt-get install -y docker.io docker-compose-v2
sudo usermod -aG docker "$USER"
sudo systemctl enable docker
sudo systemctl start docker
# Log out and back in (or: newgrp docker) for group membership
```

Verify: `docker info` should succeed without `sudo`.

Alternatively, use the provided setup script:

```bash
bash terminal-bench/setup-docker.sh
```

### 2.3 Rust Toolchain

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustc --version
```

### 2.4 Clone the Repo

```bash
git clone <your-repo-url> ~/workgraph
cd ~/workgraph
```

Or if already cloned:

```bash
cd ~/workgraph
git pull --ff-only
```

### 2.5 Build and Install `wg`

The host-native `wg` binary (used for `cargo test`, local operations):

```bash
cd ~/workgraph
cargo install --path .
wg --version
```

### 2.6 Build the Bookworm-Compatible Binary

Terminal-Bench Docker containers run Debian bookworm (glibc 2.36). The
host-native binary may require a newer glibc (e.g., 2.39 on Ubuntu 25.04).
You must build a bookworm-compatible binary that can run inside the containers.

**Build inside a bookworm Docker container:**

```bash
cd ~/workgraph

# Run cargo build inside a Debian bookworm container with Rust pre-installed
docker run --rm \
  -v "$(pwd)":/workspace \
  -w /workspace \
  rust:bookworm \
  cargo build --release --target-dir /workspace/target/bookworm-build

# Copy the binary to the expected location
mkdir -p target/bookworm-out
cp target/bookworm-build/release/wg target/bookworm-out/wg
chmod +x target/bookworm-out/wg
```

**Verify glibc compatibility:**

```bash
# Should show max GLIBC_2.34 (bookworm has 2.36)
readelf -V target/bookworm-out/wg | grep -oP 'GLIBC_\d+\.\d+' | sort -t. -k2 -n | tail -1
```

The adapter automatically prefers `target/bookworm-out/wg` when it exists (see
`_find_wg_binary()` in `wg/adapter.py`). If this path doesn't exist, it falls
back to `~/.cargo/bin/wg`, which may fail with a GLIBC mismatch inside
containers.

> **Note:** If your host already runs Debian bookworm (glibc 2.36) or older,
> the host binary may work directly. Test with:
> `docker run --rm -v $(which wg):/usr/local/bin/wg debian:bookworm wg --version`

### 2.7 Install Python Dependencies

```bash
cd ~/workgraph
pip install -e terminal-bench/
```

This installs the `wg-terminal-bench` package (Harbor adapter + dependencies:
`harbor>=0.3.0`, `litellm`, `ddgs`, `httpx`, `trafilatura`).

### 2.8 Download TB Task Data and Pre-Pull Docker Images

```bash
# Download Terminal-Bench 2.0 task definitions
harbor download terminal-bench@2.0

# Pre-pull all Docker images (avoids Docker Hub rate limits during runs)
bash terminal-bench/pre-pull-images.sh
```

The pre-pull script tries GHCR mirrors first (no rate limit) and falls back to
Docker Hub with retry logic. There are ~89 unique Docker images; total pull
size is ~75 GB.

Options:
- `--check` : only report which images are missing (don't pull)
- `--login` : authenticate with Docker Hub first (doubles rate limit to 200/6h)
- `--no-ghcr-fallback` : skip GHCR mirror attempts

### 2.9 Set Environment Variables

```bash
export OPENROUTER_API_KEY="sk-or-v1-YOUR-KEY-HERE"
export PATH="$HOME/.cargo/bin:$PATH"
```

Add to `~/.bashrc` for persistence:

```bash
echo 'export OPENROUTER_API_KEY="sk-or-v1-YOUR-KEY-HERE"' >> ~/.bashrc
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

### 2.10 Verify Setup

```bash
# Quick verification
python -c 'from wg.adapter import ConditionAAgent, ConditionFAgent; print("ok")'

# Full preflight
echo "=== Pre-flight ==="
echo -n "OPENROUTER_API_KEY: "; [ -n "$OPENROUTER_API_KEY" ] && echo "OK" || echo "MISSING"
echo -n "wg binary: "; wg --version 2>/dev/null || echo "MISSING"
echo -n "bookworm binary: "; [ -f target/bookworm-out/wg ] && echo "OK" || echo "MISSING"
echo -n "Docker: "; docker info >/dev/null 2>&1 && echo "OK" || echo "NOT RUNNING"
echo -n "Python: "; python3 --version
echo -n "Harbor: "; harbor --version 2>/dev/null || echo "MISSING (pip install harbor-framework)"
echo -n "Adapter: "; python3 -c "from wg.adapter import ConditionAAgent; print('OK')" 2>/dev/null || echo "IMPORT FAILED"
echo -n "API reachable: "; curl -s -o /dev/null -w "%{http_code}" https://openrouter.ai/api/v1/models; echo
```

---

## 3. Running the Experiment

### 3.1 Quick Smoke Test (1 Task, 1 Trial)

Run a single task with each condition to verify the full pipeline works:

```bash
cd ~/workgraph

# Condition A (control: bash + file tools only, no graph context)
harbor run \
  --agent-import-path "wg.adapter:ConditionAAgent" \
  -m "openrouter:minimax/minimax-m2.7" \
  -d terminal-bench@2.0 \
  -k 1 \
  --n-concurrent 1 \
  --no-delete \
  --max-turns 50 \
  --timeout 600 \
  --task-ids build-cython-ext \
  --trials-dir /tmp/tb-smoke-a

# Condition F (treatment: full wg tools + graph context)
harbor run \
  --agent-import-path "wg.adapter:ConditionFAgent" \
  -m "openrouter:minimax/minimax-m2.7" \
  -d terminal-bench@2.0 \
  -k 1 \
  --n-concurrent 1 \
  --no-delete \
  --max-turns 50 \
  --timeout 600 \
  --task-ids build-cython-ext \
  --trials-dir /tmp/tb-smoke-f
```

Expected: both complete within ~5 minutes. Check `reward.txt` in the output:

```bash
cat /tmp/tb-smoke-a/*/build-cython-ext__*/verifier/reward.txt
cat /tmp/tb-smoke-f/*/build-cython-ext__*/verifier/reward.txt
```

### 3.2 Condition A Full Run (89 Tasks, 3-5 Trials)

```bash
cd ~/workgraph
tmux new -s condition-a

harbor run \
  --agent-import-path "wg.adapter:ConditionAAgent" \
  -m "openrouter:minimax/minimax-m2.7" \
  -d terminal-bench@2.0 \
  -k 5 \
  --n-concurrent 4 \
  --no-delete \
  --max-turns 50 \
  --timeout 1800 \
  --trials-dir terminal-bench/results/reproduction/condition-A
```

### 3.3 Condition F Full Run (89 Tasks, 3-5 Trials)

```bash
cd ~/workgraph
tmux new -s condition-f

harbor run \
  --agent-import-path "wg.adapter:ConditionFAgent" \
  -m "openrouter:minimax/minimax-m2.7" \
  -d terminal-bench@2.0 \
  -k 5 \
  --n-concurrent 4 \
  --no-delete \
  --max-turns 50 \
  --timeout 1800 \
  --trials-dir terminal-bench/results/reproduction/condition-F
```

### 3.4 Both Conditions via reproduce.sh

The `reproduce.sh` script automates running conditions sequentially:

```bash
cd ~/workgraph

# Both A and F, 5 trials each
bash terminal-bench/reproduce.sh --trials 5 --condition A
bash terminal-bench/reproduce.sh --trials 5 --condition F

# Or all conditions at once (A through F)
bash terminal-bench/reproduce.sh --trials 5
```

`reproduce.sh` options:

| Flag | Default | Description |
|------|---------|-------------|
| `--trials N` | 3 | Number of trials per task (leaderboard requires 5) |
| `--condition X` | `all` | `A`, `B`, `C`, `D`, `E`, `F`, or `all` |
| `--output-dir DIR` | `results/reproduction` | Where results go |
| `--model MODEL` | `openrouter:minimax/minimax-m2.7` | Model identifier |
| `--concurrent N` | 4 | Concurrent tasks |

---

## 4. Configuration

### Changing the Model

Pass a different model to Harbor or reproduce.sh:

```bash
# Harbor
harbor run --agent-import-path "wg.adapter:ConditionAAgent" \
  -m "openrouter:other-provider/other-model" ...

# reproduce.sh
bash terminal-bench/reproduce.sh --model "openrouter:other/model"
```

The model string uses workgraph format (`provider:model`). Harbor format
(`provider/model`) is also accepted and auto-normalized by the adapter.

The benchmark model is hardcoded in `wg/adapter.py` as:
`BENCHMARK_MODEL = "openrouter:minimax/minimax-m2.7"`

### Changing Concurrency

```bash
# Harbor: --n-concurrent flag
harbor run --n-concurrent 8 ...

# reproduce.sh: --concurrent flag
bash terminal-bench/reproduce.sh --concurrent 8
```

Each concurrent trial is one Docker container + one API call stream. At 4
concurrent, expect ~8 GB RAM usage and moderate CPU.

### Timeout

The default timeout is 1800 seconds (30 minutes) per trial.

```bash
# Harbor: --timeout flag (seconds)
harbor run --timeout 3600 ...
```

In `reproduce.sh`, edit the `TIMEOUT=1800` variable or pass a modified version.

The adapter's `DEFAULT_TRIAL_TIMEOUT` in `wg/adapter.py` controls the internal
polling timeout for the wg native executor.

### Max Turns

```bash
# Harbor: --max-turns flag
harbor run --max-turns 100 ...
```

Default is 50 turns per trial. The native executor inside Docker runs until the
task reaches a terminal status (`done`, `failed`, `abandoned`) or the timeout
fires.

---

## 5. Artifacts and Results

### Output Directory Structure

Harbor writes results per-trial:

```
<trials-dir>/<job-timestamp>/
  config.json                          # Job-level configuration
  result.json                          # Job-level aggregate results
  job.log                              # Harbor job log
  <task-name>__<hash>/                 # One directory per trial
    config.json                        # Trial configuration
    result.json                        # Trial result (pass/fail, timing)
    trial.log                          # Trial execution log
    agent/
      trial_summary.json               # Agent summary (tokens, turns, etc.)
      agent_loop.ndjson                # Full agent interaction trace
      wg-artifacts/                    # Extracted .workgraph/ from container
        .workgraph/
          graph.jsonl                  # Task graph state
          config.toml                  # Condition-specific configuration
          service/
            daemon.log                 # Coordinator/service daemon log
          agents/
            agent-1/
              stream.jsonl             # Native executor event stream
          log/
            operations.jsonl           # Graph operations log
          output/
            <task-id>/
              conversation.jsonl       # Agent conversation trace
    artifacts/                         # (empty unless task produces files)
    verifier/
      reward.txt                       # 1.0 (pass) or 0.0 (fail)
      ctrf.json                        # Common Test Report Format
      test-stdout.txt                  # Verification command output
```

### Key Files for Analysis

| File | What it tells you |
|------|-------------------|
| `verifier/reward.txt` | Pass (1.0) or fail (0.0) |
| `agent/wg-artifacts/.workgraph/agents/*/stream.jsonl` | Token counts, model used, tool calls |
| `agent/wg-artifacts/.workgraph/config.toml` | Confirms condition config was applied |
| `agent/wg-artifacts/.workgraph/graph.jsonl` | Task state and dependencies |
| `agent/wg-artifacts/.workgraph/service/daemon.log` | Service lifecycle and errors |
| `result.json` (trial-level) | Harbor's result including timing and metadata |

### Verifying Model Purity

After a run, verify that only the intended model was used:

```bash
# Check all stream.jsonl files for model references
find <trials-dir> -name "stream.jsonl" -exec grep -l "model" {} \; | \
  while read f; do
    echo "=== $f ==="
    grep -o '"model":"[^"]*"' "$f" | sort -u
  done

# Verify no litellm, anthropic, or claude references
find <trials-dir> -name "stream.jsonl" -exec grep -il "litellm\|anthropic\|claude\|gpt-4" {} \;
# Should return nothing
```

### Condition Differences (A vs F)

| Aspect | Condition A | Condition F |
|--------|------------|------------|
| `context_scope` | `clean` | `graph` |
| WG tools | Excluded (custom bundle) | Full access |
| System prompt | Basic engineer prompt | Engineer + WG Quick Guide + distilled memory |
| Graph awareness | None | Full dependency graph visible |
| Bundle | `implementer.toml` (bash, file tools only) | Default (all tools) |

Both use the same model (`minimax/minimax-m2.7`), same executor (`native`),
same `max_agents = 1`, and same verification commands.

---

## 6. Preparing a Leaderboard Submission

After collecting results:

```bash
cd ~/workgraph

# Organize trial data into submission format
bash terminal-bench/prepare-leaderboard.sh [--dry-run]
```

The leaderboard requires **minimum 5 trials per task**. Submit to:
https://huggingface.co/datasets/harborframework/terminal-bench-2-leaderboard

Submission structure per condition:

```
submissions/terminal-bench/2.0/workgraph-condition-a__minimax-m2.7/
  metadata.yaml
  <job-name>/
    result.json
    config.json
    <task>__<hash>/
      result.json
      config.json
```

See `terminal-bench/docs/HOWTO-submit-to-leaderboard.md` for `metadata.yaml`
template and full submission instructions.

---

## 7. Troubleshooting

### GLIBC Mismatch

**Symptom:** `wg --version` fails inside Docker with:
`/usr/local/bin/wg: /lib/x86_64-linux-gnu/libc.so.6: version 'GLIBC_2.39' not found`

**Cause:** Host binary requires a newer glibc than the container (bookworm) has.

**Fix:** Build the bookworm-compatible binary (Section 2.6). The adapter
automatically uses `target/bookworm-out/wg` when it exists.

### Heredoc Failures

**Status:** Already fixed in the adapter.

Harbor's `environment.exec()` pipes commands to bash via stdin. Heredocs also
read from stdin, causing silent failures. The adapter uses base64 encoding for
all config writes instead. No action needed.

### Timeout Issues

**Symptom:** Trials show status "timeout" after 1800s.

**Fix:** Increase the timeout:

```bash
harbor run --timeout 3600 ...
```

Some complex tasks (e.g., `iterative-test-fix`) may need more than 30 minutes.

### Docker Not Running

**Symptom:** `harbor run` fails with Docker connection errors.

**Fix:**

```bash
sudo systemctl start docker
docker info  # verify
```

### Missing OPENROUTER_API_KEY

**Symptom:** Adapter raises `RuntimeError: OPENROUTER_API_KEY not set`

**Fix:**

```bash
export OPENROUTER_API_KEY="sk-or-v1-YOUR-KEY-HERE"

# Verify
curl -s -o /dev/null -w "%{http_code}" https://openrouter.ai/api/v1/models
# Should print 200
```

### Docker Hub Rate Limits

**Symptom:** Image pulls fail with 429 or "too many requests".

**Fix:** Pre-pull images before running experiments:

```bash
# Authenticate (doubles rate limit to 200/6h)
bash terminal-bench/pre-pull-images.sh --login

# Or just re-run without auth (GHCR mirrors avoid Docker Hub for most images)
bash terminal-bench/pre-pull-images.sh
```

### Task Paused / Not Starting

**Symptom:** wg service starts but no agent is spawned.

**Cause:** The task was created without `--no-place`, leaving it in draft/paused
state.

**Status:** Already fixed in the adapter. The `--no-place` flag is passed to
`wg add` to make tasks immediately dispatchable.

### Harbor Import Errors

**Symptom:** `ModuleNotFoundError: No module named 'wg'` or similar.

**Fix:**

```bash
cd ~/workgraph
pip install -e terminal-bench/
python -c 'from wg.adapter import ConditionAAgent; print("ok")'
```

### Bookworm Binary Not Found

**Symptom:** Adapter falls back to host binary, then GLIBC mismatch in container.

**Fix:** The adapter checks for the binary at a hardcoded path
(`/home/erik/workgraph/target/bookworm-out/wg`). On a different host, either:

1. Build to the same relative path:
   ```bash
   cd ~/workgraph
   mkdir -p target/bookworm-out
   # Build as shown in Section 2.6
   ```

2. Or pass the path explicitly:
   ```python
   agent = ConditionAAgent(wg_binary_host_path="/path/to/bookworm/wg")
   ```

3. Or update `_find_wg_binary()` in `wg/adapter.py` to include your path.

> **Important:** The hardcoded path `/home/erik/workgraph/target/bookworm-out/wg`
> in the adapter will need to be updated for production use on other machines.
> For a quick fix, place the binary at `~/.cargo/bin/wg-bookworm` and add that
> path to the candidates list.

---

## 8. Quick Reference

### Condition A (Control)

```bash
harbor run \
  --agent-import-path "wg.adapter:ConditionAAgent" \
  -m "openrouter:minimax/minimax-m2.7" \
  -d terminal-bench@2.0 \
  -k 5 \
  --n-concurrent 4 \
  --no-delete \
  --max-turns 50 \
  --timeout 1800 \
  --trials-dir terminal-bench/results/reproduction/condition-A
```

Config: `context_scope=clean`, custom bundle (bash + file tools only, no wg).

### Condition F (Treatment)

```bash
harbor run \
  --agent-import-path "wg.adapter:ConditionFAgent" \
  -m "openrouter:minimax/minimax-m2.7" \
  -d terminal-bench@2.0 \
  -k 5 \
  --n-concurrent 4 \
  --no-delete \
  --max-turns 50 \
  --timeout 1800 \
  --trials-dir terminal-bench/results/reproduction/condition-F
```

Config: `context_scope=graph`, full wg tools, distilled context + memory injected.

### All Conditions via Script

```bash
bash terminal-bench/reproduce.sh --trials 5
```

### Cost Estimate

Minimax M2.7 is free-tier on OpenRouter. The only costs are compute time and
Docker Hub bandwidth (mitigated by GHCR mirrors and pre-pulling).

| Scenario | Tasks | Trials | Est. Time (4 concurrent) |
|----------|-------|--------|--------------------------|
| Smoke test | 1 | 2 | ~5 min |
| Single condition | 89 | 445 | ~15 hours |
| Both A + F | 89 | 890 | ~30 hours |
