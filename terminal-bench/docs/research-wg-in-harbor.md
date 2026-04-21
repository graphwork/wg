# Research: wg in Harbor Service Definition

**Task:** `research-wg-in-harbor`
**Date:** 2026-04-20

---

## 1. Harbor's Agent/Service Definition Format

Harbor uses a Python class protocol via `BaseAgent`. The adapter is referenced by its Python import path in config JSON.

### BaseAgent Protocol (setup/run/teardown contract)

```python
class BaseAgent:
    @staticmethod
    def name() -> str: ...           # Agent identifier string

    def version(self) -> str | None: ...  # Optional version

    def __init__(
        self,
        logs_dir: Path | None = None,
        model_name: str | None = None,
        *args, **kwargs,              # Arbitrary extra config via kwargs
    ): ...

    async def setup(self, environment: BaseEnvironment) -> None:
        """Called once before run(). Prepare the container environment."""
        ...

    async def run(
        self,
        instruction: str,             # The task's instruction text
        environment: BaseEnvironment, # Docker exec/upload/download handle
        context: AgentContext,         # Mutable metrics bag (tokens, cost, metadata)
    ) -> None:
        """Execute the agent logic. Populate context with results."""
        ...

    # teardown() is implicit — Harbor stops the container after run() returns
```

### BaseEnvironment API (Docker abstraction)

```python
class BaseEnvironment:
    async def exec(
        self,
        command: str,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
        timeout_sec: int | None = None,
        user: str | int | None = None,
    ) -> ExecResult:  # .stdout, .stderr, .return_code

    async def upload_file(self, source_path: Path | str, target_path: str)
    async def upload_dir(self, source_dir: Path | str, target_dir: str)
    async def download_file(self, source_path: str, target_path: Path | str)
    async def download_dir(self, source_dir: str, target_dir: Path | str)
    async def start(self, force_build: bool) -> None
    async def stop(self, delete: bool)
    async def is_dir(self, path: str, user=None) -> bool
    async def is_file(self, path: str, user=None) -> bool
```

The Docker implementation of `exec()` runs `docker compose exec -T main bash` with the command piped as stdin. Per-call `env` vars are passed as `-e K=V` flags and only persist for that single call.

### Agent Registration

Agents are referenced in config JSON by `import_path`:
```json
{
    "agents": [{
        "import_path": "wg.adapter:ConditionAAgent",
        "model_name": "openrouter:minimax/minimax-m2.7",
        "kwargs": {"max_turns": 50, "temperature": 0.0}
    }]
}
```

Harbor discovers the class via Python's importlib. The `kwargs` dict is forwarded to `__init__()`.

---

## 2. Harbor Run Config JSON Format

A Harbor run config specifies the complete experiment. Example from `nemotron-3-super-condition-g-config.json`:

```json
{
    "job_name": "nemotron-3-super-condition-G",
    "jobs_dir": "results/nemotron-3-super-condition-G",
    "n_attempts": 1,
    "timeout_multiplier": 1.0,
    "n_concurrent_trials": 5,
    "retry": {
        "max_retries": 2,
        "exclude_exceptions": [
            "VerifierTimeoutError", "AgentTimeoutError",
            "VerifierOutputParseError", "RewardFileNotFoundError",
            "RewardFileEmptyError"
        ]
    },
    "environment": {
        "type": "docker",
        "force_build": false,
        "delete": false,
        "mounts_json": null,
        "env": {},
        "kwargs": {}
    },
    "agents": [{
        "import_path": "wg.adapter:ConditionGAgent",
        "model_name": "openrouter:nvidia/nemotron-3-super-120b-a12b:free",
        "kwargs": {"max_turns": 50, "temperature": 0.0}
    }],
    "datasets": [{
        "name": "terminal-bench",
        "version": "2.0"
    }]
}
```

### Config JSON for `wg nex` Harbor run

A `wg nex`-based Harbor config would look like:

```json
{
    "job_name": "wg-nex-eval-condition-A",
    "jobs_dir": "results/wg-nex-eval-condition-A",
    "n_attempts": 1,
    "timeout_multiplier": 3.0,
    "n_concurrent_trials": 1,
    "retry": {
        "max_retries": 2,
        "exclude_exceptions": [
            "VerifierTimeoutError", "AgentTimeoutError",
            "VerifierOutputParseError", "RewardFileNotFoundError",
            "RewardFileEmptyError"
        ]
    },
    "environment": {
        "type": "docker",
        "delete": false,
        "mounts_json": null,
        "env": {}
    },
    "agents": [{
        "import_path": "wg.adapter:ConditionAAgent",
        "model_name": "ollama:qwen3-coder:30b-a3b-q8_0",
        "kwargs": {"max_turns": 50, "temperature": 0.0}
    }],
    "datasets": [{
        "name": "terminal-bench",
        "version": "2.0"
    }]
}
```

The format is identical — the agent class implementation decides whether to use `wg service start` (multi-agent coordinator loop) or `wg nex --eval-mode` (single-shot).

---

## 3. Current Binary Upload Approach vs Pre-baked Image

### Current: Binary Upload in `setup()`

The adapter's `setup()` method (adapter.py lines 1445-1568) does:

1. **Find binary on host** — `_find_wg_binary()` searches for `target/bookworm-out/wg` first (glibc 2.36-compatible cross-build), then `~/.cargo/bin/wg`, `target/release/wg`, `target/debug/wg`, finally `which wg`
2. **Upload into container** — `await environment.upload_file(wg_bin, "/usr/local/bin/wg")` (uses `docker cp` under the hood)
3. **Chmod** — `await environment.exec(command="chmod +x /usr/local/bin/wg")`
4. **Verify** — `await environment.exec(command="wg --version")`
5. **Initialize graph** — Creates isolated trial dir at `/var/tmp/tb-trial-<uuid>`, runs `wg init`
6. **Write config** — Writes `config.toml`, bundles, agency setup via base64-encoded `exec()` calls

### What a Pre-baked Docker Image Would Save

| Step | Current (binary upload) | Pre-baked image | Time saved |
|------|------------------------|-----------------|------------|
| Binary delivery | `upload_file` (~2-5s) | Already in image | ~2-5s |
| `chmod +x` | exec call (~1s) | Baked in | ~1s |
| `wg --version` check | exec call (~1s) | Could skip | ~1s |
| `git` availability | exec call (~1s) | Guaranteed | ~1s |
| Total setup overhead | ~5-8s per trial | ~0s | ~5-8s/trial |

For 89 tasks × 3 trials = 267 containers, that's **22-36 minutes** saved across a full benchmark run. Not transformative but meaningful.

More importantly, a pre-baked image eliminates **failure modes**:
- Binary/glibc version mismatch (host glibc > container glibc)
- `upload_file` failures on network issues or Docker daemon hiccups
- Stale binary from host (seen in smoke test iteration 2: container had Apr 7 binary missing `unblock_stuck_tasks`)

### Sketch: Minimal wg-in-Docker Image

```dockerfile
# Dockerfile.wg-harbor
FROM rust:bookworm AS builder
WORKDIR /build
COPY . .
RUN cargo build --release
RUN strip target/release/wg

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    git ca-certificates curl python3 python3-pip \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/wg /usr/local/bin/wg
RUN chmod +x /usr/local/bin/wg && wg --version
```

This is a **build image for wg itself**, not a TB task image. TB tasks have their own images (`alexgshaw/*:20251031` or `ghcr.io/laude-institute/terminal-bench/*:2.0`). The wg binary must be injected into those per-task images.

#### Two paths to pre-baked:

**Path A: wg sidecar layer** — Build a small image containing just `/usr/local/bin/wg`, then use Docker multi-stage copy in Harbor's environment setup:
```
FROM alexgshaw/<task>:20251031 AS task
COPY --from=wg-image:latest /usr/local/bin/wg /usr/local/bin/wg
```
Requires Harbor to support custom Dockerfiles or build hooks. Not currently available.

**Path B: Custom Harbor environment class** — Subclass `DockerEnvironment` to inject wg into any container at startup. This is essentially what `setup()` already does, but could be moved to the environment layer.

**Path C: `mounts_json` volume mount** — Mount the host binary read-only:
```json
{"environment": {"mounts_json": "[\"target/bookworm-out/wg:/usr/local/bin/wg:ro\"]"}}
```
Already supported by Harbor config. Faster than upload_file (no copy, just bind mount). But requires the host binary to be glibc-compatible with the container.

**Recommendation:** Path C (mounts_json) is the quickest win. Path A (wg Docker image) is the cleanest long-term solution but requires either Harbor changes or a custom environment class.

---

## 4. Dockerfile Base Image Requirement

### TB Container Base

Terminal Bench tasks use **Debian bookworm**-based images:
- Pre-built: `alexgshaw/<task>:20251031` (Docker Hub) or `ghcr.io/laude-institute/terminal-bench/<task>:2.0` (GHCR)
- Custom-built: FROM `python:3.11-bookworm`, `node:20-bookworm`, `ubuntu:22.04`, etc.

**glibc version: 2.36** (Debian bookworm ships glibc 2.36)

### wg Binary Compatibility

The wg binary must be compiled against glibc ≤ 2.36 to run in TB containers. The project already handles this with a cross-build target:

```bash
docker run --rm -v "$(pwd)":/workspace -w /workspace rust:bookworm \
    cargo build --release --target-dir /workspace/target/bookworm-build
mkdir -p target/bookworm-out
cp target/bookworm-build/release/wg target/bookworm-out/wg
```

The bookworm-compatible binary already exists at `target/bookworm-out/wg` in the main repo. The adapter's `_find_wg_binary()` checks this path first.

### Minimal wg Docker Image

If we want a standalone `wg` Docker image that Harbor could reference:

```dockerfile
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY wg /usr/local/bin/wg
RUN chmod +x /usr/local/bin/wg
ENTRYPOINT ["wg"]
```

Build with:
```bash
docker build -t wg:latest -f Dockerfile.wg --build-context wg=target/bookworm-out/ .
```

This image would be ~80-100MB (bookworm-slim base + git + wg binary).

---

## 5. Volumes/Mounts wg Needs

### Required State Directories

| Path (in container) | Purpose | Persistence |
|---------------------|---------|-------------|
| `/var/tmp/tb-trial-<uuid>/.workgraph/` | Graph state (graph.jsonl, config.toml) | Per-trial, ephemeral |
| `.workgraph/agents/<id>/` | Agent logs (stream.jsonl, agent.ndjson) | Per-trial, downloaded post-run |
| `.workgraph/service/` | Daemon state (state.json, daemon.log) | Per-trial, ephemeral |
| `.workgraph/bundles/` | Tool bundles (condition-specific TOML) | Written in setup() |
| `.workgraph/agency/` | Agency primitives (for conditions D/E) | Written in setup() |

### Current Approach: No Host Mounts

The adapter creates everything inside the container during `setup()`. The `.workgraph/` directory is fully ephemeral — created in an isolated `/var/tmp/tb-trial-<uuid>` directory, downloaded post-run via `_download_wg_artifacts()`, then the container is destroyed.

This isolation is critical: some containers share the host `/home/erik` filesystem, so defaulting to CWD would corrupt the host's `.workgraph/`.

### If We Wanted Persistent Mounts

For development/debugging, you could mount host directories:
```json
{
    "environment": {
        "mounts_json": "[
            \"target/bookworm-out/wg:/usr/local/bin/wg:ro\",
            \"/tmp/tb-debug/.workgraph:/workspace/.workgraph\"
        ]"
    }
}
```

But this defeats trial isolation. Not recommended for benchmark runs.

---

## 6. Network Access

### Container → External API (OpenRouter, SGLang, etc.)

Docker containers in Harbor have **outbound network access by default**. The adapter exports `OPENROUTER_API_KEY` into the container environment before starting `wg service`:

```python
api_key = os.environ.get("OPENROUTER_API_KEY", "")
env_exports = f'export OPENROUTER_API_KEY="{api_key}"'
start_cmd = f'{env_exports} && cd {trial_workdir} && wg service start --model "{model}"'
await environment.exec(command=start_cmd, timeout_sec=30)
```

The env var propagates through the daemon fork because `wg service start` uses `std::process::Command::spawn()` which inherits the parent process environment.

### Container → Local Model Endpoint

For local models (e.g., `ollama:qwen3-coder:30b-a3b-q8_0` on `lambda01:30000/v1`), the container must be able to reach the host or another machine on the network:

- **Host network**: `docker run --network host` — container shares host's network. The adapter could request this via `environment.kwargs`.
- **Docker bridge**: Default. Container can reach external IPs but not `localhost` (host). Use the host's LAN IP or `host.docker.internal` (Docker Desktop only, not available on Linux by default).
- **SGLang on remote host**: Container routes to `lambda01:30000/v1` via the Docker bridge's default gateway. Works if `lambda01` resolves from inside the container.

The `WG_ENDPOINT` / `WG_ENDPOINT_URL` env vars or `config.toml` `[provider]` section point the native executor to custom endpoints:

```toml
[provider]
endpoint = "http://lambda01:30000/v1"
```

For local Ollama: the endpoint must be reachable from inside the container. If Ollama runs on the Docker host, the container needs `--network host` or the host's actual IP.

---

## 7. Path to a `wg` Docker Image

### Option 1: Multi-arch `wg` image on GHCR

```bash
# Build (CI step)
docker build -t ghcr.io/ekg/wg:latest -f Dockerfile.wg .
docker push ghcr.io/ekg/wg:latest

# Use in adapter setup()
# Instead of upload_file(), copy from the wg image
# Requires Harbor to support init containers or multi-stage environment setup
```

### Option 2: `wg` as a tool in a custom environment class

```python
class WgDockerEnvironment(DockerEnvironment):
    """Docker environment that pre-installs wg from a known image."""

    async def start(self, force_build: bool) -> None:
        await super().start(force_build)
        # Copy wg from a known location (mounted volume, pre-built image, etc.)
        await self.exec(command="curl -sL https://github.com/ekg/workgraph/releases/latest/wg -o /usr/local/bin/wg && chmod +x /usr/local/bin/wg")
```

### Option 3: Cargo binstall (no custom image)

If `wg` were published as a cargo binary (`cargo binstall wg`), the adapter could install it inside the container without uploading:
```python
await environment.exec(command="cargo binstall -y wg", timeout_sec=120)
```
This requires publishing to crates.io with binary releases. Slower than upload but zero host dependency.

### Recommendation

The current `upload_file()` approach is pragmatic and works. The lowest-friction improvement is:

1. **Short term**: Use `mounts_json` in the Harbor config to bind-mount `target/bookworm-out/wg` read-only
2. **Medium term**: Publish a `ghcr.io/ekg/wg:<version>` image as a CI artifact. The adapter can check if `wg` already exists in the container before uploading.
3. **Long term**: If Harbor supports init containers or custom build steps, use the wg image as a build stage.

---

## 8. `setup-docker.sh` and `pre-pull-images.sh`

### `setup-docker.sh`

A one-time setup script that:
1. Installs Docker (`apt-get install docker.io docker-compose-v2`)
2. Adds user to `docker` group
3. Verifies Docker with `docker run --rm hello-world`
4. Checks Harbor Python package is installed (`pip install harbor-bench`)
5. Points user to `pre-pull-images.sh`

### `pre-pull-images.sh`

Pre-caches all TB Docker images to avoid Docker Hub rate limiting:
1. Scans `~/.cache/harbor/tasks/*/task.toml` for `docker_image` entries
2. Also scans `Dockerfile` base images (`FROM` lines)
3. Tries GHCR mirror first (`ghcr.io/laude-institute/terminal-bench/<task>:2.0`) — no rate limit
4. Falls back to Docker Hub with retry logic (`--max-retries 3`, `--retry-wait 60s`)
5. Supports `--check` (dry run), `--login` (authenticate for 2x rate limit)

Neither script needs modification for wg-in-Docker. They handle TB task images, not the wg binary/image.

---

## 9. Blockers for Containerized `wg nex` Runs

### Resolved (already working)

- **Binary delivery**: `upload_file()` works, bookworm cross-build exists
- **Graph initialization**: `wg init` inside container works
- **Config injection**: Base64-encoded config.toml write works
- **Env var propagation**: `export KEY=... && wg service start` propagates through daemon fork
- **Metric collection**: `_download_wg_artifacts()` + `_collect_agent_metrics()` works
- **Trial isolation**: Unique `/var/tmp/tb-trial-<uuid>` prevents host corruption
- **All conditions A-G**: Already implemented and tested

### Active Blockers

1. **`wg nex --eval-mode` is not yet wired into the adapter**. The adapter uses `wg service start` (coordinator-based), not `wg nex --eval-mode` (single-shot). For single-agent conditions (A-F), `wg nex --eval-mode` would be simpler and faster — it skips the daemon fork, coordinator loop, and polling overhead. The adapter would need a new code path:
   ```python
   # Single-agent path (Conditions A-F, max_agents=1):
   await environment.exec(
       command=f'export OPENROUTER_API_KEY=... && cd {trial_workdir} && '
               f'wg nex --eval-mode --model "{model}" -m "$(cat /tmp/tb-instruction.txt)"',
       timeout_sec=self.timeout,
   )
   # Multi-agent path (Condition G, max_agents>1): keep current wg service start
   ```

2. **Local model endpoint reachability from Docker containers**. For models served on the host (Ollama, SGLang), the container must reach `host.docker.internal` or the host's LAN IP. Standard Docker bridge networking on Linux doesn't support `host.docker.internal` by default. Options:
   - `--network host` in Docker run config
   - `--add-host host.docker.internal:host-gateway` (Docker 20.10+)
   - Use the host's actual IP in endpoint config

3. **No `wg` Docker image exists yet**. Binary upload works but a published image would improve reproducibility and CI integration. This is a nice-to-have, not a blocker.

4. **Stale binary risk**. The adapter logs binary metadata (size, mtime) but doesn't version-check. If the host binary is outdated, the container runs old code. A `wg --version` check + minimum version assertion in `setup()` would catch this.

---

## 10. Summary: What a wg Harbor Service Definition Looks Like

The existing adapter (`wg/adapter.py`) IS the wg Harbor service definition. It implements the full `BaseAgent` protocol:

| Component | Implementation | Status |
|-----------|---------------|--------|
| `setup()` | Upload binary, `wg init`, write config/bundles/agency | Working |
| `run()` | `wg service start` → poll → collect metrics | Working |
| teardown | Implicit (container destroyed by Harbor) | Working |
| Config JSON | `import_path: "wg.adapter:ConditionXAgent"` | Working |
| Conditions A-G | Differentiated by `CONDITION_CONFIG` dict | Working |
| Model routing | `_normalize_model()` + config.toml + env var | Working |
| Metric collection | Download `.workgraph/agents/*/stream.jsonl` | Working |
| Trial isolation | `/var/tmp/tb-trial-<uuid>` per container | Working |

### What Would Change for `wg nex --eval-mode`

For single-agent conditions, the adapter could use `wg nex --eval-mode` instead of `wg service start`:

```python
# Simpler: one process, no daemon, no polling
nex_cmd = (
    f'export OPENROUTER_API_KEY="{api_key}" && '
    f'cd {trial_workdir} && '
    f'wg nex --eval-mode --model "{model}" '
    f'-m "$(cat /tmp/tb-instruction.txt)" '
    f'2>/dev/null'
)
result = await environment.exec(command=nex_cmd, timeout_sec=self.timeout)
# result.stdout contains JSON summary
```

Benefits:
- No daemon fork/poll overhead (~2-5s saved per trial)
- Simpler failure modes (one process, one exit code)
- JSON summary on stdout (machine-parseable)
- Still uses the same native executor tools, context assembly, and model routing

Drawbacks:
- Only works for single-agent runs (max_agents=1)
- No coordinator agent for Condition G
- New code path to maintain alongside the existing one
