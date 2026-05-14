# Audit: adapter.py + wg native-exec for TB Harbor Integration

Task: `tb-adapter-audit`

## 1. adapter.py Current State

### Class hierarchy
- `WorkgraphAgent(BaseAgent)` — the main Harbor adapter (line 761)
- `ConditionAAgent` through `ConditionFAgent` — thin subclasses that set `condition=X` and `model_name` defaults (lines 975–1058)

### `setup()` (line 834)
1. Creates a host-side temp dir (`tempfile.mkdtemp(prefix="tb-wg-")`)
2. Calls `wg init` on the **host** via `_exec_wg_cmd_host()` (line 843)
3. Normalizes the model string from Harbor's `/` format to WG's `:` format (line 849)
4. Writes `.wg/config.toml` via `_write_trial_wg_config()` (line 853) — sets coordinator.executor=native, coordinator.model, agent.model, context_scope, exec_mode, max_agents=1
5. Writes custom bundle TOML for Condition A via `_write_trial_bundle()` (line 859) — excludes wg tools
6. For federation conditions (D, E, F): initializes hub, writes federation.yaml, pulls agency primitives (lines 862–886)
7. For agency conditions (D, E): creates a named agent identity (solver/orchestrator) via `wg agent create` (lines 876–892)

### `run()` (line 894)
1. Generates a UUID-based root task ID (`tb-<8hex>`)
2. Initializes a `TrialLogger`
3. **Calls `_run_docker_agent_loop()`** (line 918) — this is the Python LiteLLM loop, NOT wg native-exec
4. Populates `AgentContext` with metrics (tokens, cost, metadata)
5. Writes trial summary log
6. Cleans up temp dir

### `_run_docker_agent_loop()` (line 543)
- **This is the code being replaced.**
- Runs a Python LLM loop using `litellm.acompletion()` (line 621)
- Converts wg model format back to LiteLLM format (`:` → `/`) (line 584)
- Defines 3 tools: `bash`, `write_file`, `read_file` (lines 484–540)
- Routes all tool executions through `environment.exec()` into Docker (lines 689–737)
- Tracks turns, tokens, cost, termination type
- Returns a metrics dict

### `_collect_agent_metrics()` (line 361)
- Reads `<wg_dir>/agents/*/stream.jsonl` on the **host filesystem**
- Parses `turn` events for token usage and `result` events for cost
- Returns aggregated metrics dict

### `_poll_task_completion()` (line 322)
- Polls `wg show <task-id>` on the host until status is `done`, `failed`, or `abandoned`
- Returns `(status, elapsed_seconds)`
- Currently used only by the host-native path (standalone runners)

---

## 2. wg native-exec

### How `wg service start` works
- **Forks a daemon process** (`src/commands/service/mod.rs:890`) and returns immediately
- The daemon runs a coordinator loop that:
  1. Reads `.wg/config.toml` for executor type, model, context scope
  2. Polls for ready tasks
  3. Spawns `wg native-exec` for each ready task
  4. After agent finishes, runs `--verify` command
  5. Transitions task to done/failed

### `wg native-exec` (src/commands/native_exec.rs)
- Reads prompt from file
- Resolves bundle for exec_mode (tool filtering)
- Creates LLM provider via `create_provider_ext()` (supports OpenRouter, Anthropic, OpenAI-compat)
- Runs `AgentLoop` to completion — blocks until the agent is done
- Writes to `<agent_dir>/stream.jsonl` (Turn, ToolStart, ToolEnd, Result events)
- Writes to `<agent_dir>/agent.ndjson` (raw conversation log)

### Required env vars
- **`OPENROUTER_API_KEY`** — required for OpenRouter models (the Rust HTTP client reads this directly)
- **`WG_MODEL`** — optional fallback for model if not in config.toml or CLI
- **`WG_AGENT_ID`** — set by the spawn wrapper, used for agent isolation and stream.jsonl paths
- **`WG_LLM_PROVIDER`** — optional, overrides provider auto-detection
- **`WG_ENDPOINT`** / **`WG_ENDPOINT_URL`** — optional, for custom endpoints

### Single-task mode
When there's exactly one task in the graph:
1. `wg service start` forks daemon → returns immediately
2. Daemon coordinator finds the ready task → spawns `wg native-exec`
3. Agent completes → coordinator runs `--verify` → task transitions to done/failed
4. **Daemon keeps running** — need to either poll for completion or `wg service stop` after

### Does it block?
**No.** `wg service start` returns immediately after forking the daemon. The caller must poll for completion using `_poll_task_completion()` or equivalent.

---

## 3. Harbor Environment API

### `BaseEnvironment` (harbor.environments.base)

#### `exec()` — the critical method
```python
async def exec(
    self,
    command: str,
    cwd: str | None = None,
    env: dict[str, str] | None = None,
    timeout_sec: int | None = None,
    user: str | int | None = None,
) -> ExecResult:
```
- Returns `ExecResult(stdout: str|None, stderr: str|None, return_code: int)`
- `env` dict is passed as `-e KEY=VALUE` flags to `docker compose exec`
- `command` is passed as stdin to `bash` (docker compose exec -T main bash < command)
- Persistent env vars (from constructor) are merged with per-exec env vars

#### File transfer methods
```python
async def upload_file(self, source_path: Path | str, target_path: str)
async def upload_dir(self, source_dir: Path | str, target_dir: str)
async def download_file(self, source_path: str, target_path: Path | str)
async def download_dir(self, source_dir: str, target_dir: Path | str)
```
- `upload_file()` is available and **is the right tool for binary delivery**
- Docker implementation uses `docker cp` under the hood

#### Lifecycle
```python
async def start(self, force_build: bool) -> None
async def stop(self, delete: bool)
```

#### Utility
```python
async def is_dir(self, path: str, user=None) -> bool
async def is_file(self, path: str, user=None) -> bool
```

### How Docker `exec()` works specifically
The DockerEnvironment's exec():
1. Builds `docker compose exec -T [-w cwd] [-e K=V ...] [-u user] main bash`
2. Passes the `command` string as stdin to bash
3. Returns stdout/stderr/return_code

**Key insight**: env vars set via `exec(env={"K":"V"})` only persist for that one call. For `wg service start` (which forks a daemon that then spawns child processes), the env vars must be set inside the container's shell environment, not just passed to a single exec call.

---

## 4. Binary Delivery

### Option 1: `upload_file()` (recommended)
```python
await environment.upload_file(self._wg_binary_host_path, "/usr/local/bin/wg")
await environment.exec(command="chmod +x /usr/local/bin/wg")
```
- Works across all environment types (Docker, Modal, E2B, etc.)
- No special flags needed in `harbor run`
- Self-contained in `setup()`

### Option 2: `--mounts-json` volume mount
```bash
harbor run ... --mounts-json '["/home/bot/.cargo/bin/wg:/usr/local/bin/wg:ro"]'
```
- Requires user to pass the flag — less self-contained
- Read-only mount, which is fine for a binary
- Only works with Docker (not Modal, E2B, etc.)

### Option 3: Build inside container
- `cargo install` is too slow (minutes of compilation)
- **Not viable**

### Recommendation: Use `upload_file()` in `setup()`
It's portable across all Harbor environment types. No external coordination needed.

### Architecture concern
The host wg binary is compiled for `x86_64-unknown-linux-gnu`. Docker containers for TB are also Linux x86_64, so the binary should be compatible. If the container uses musl or a different arch, we'll need a static build. This is a low-risk concern — TB containers use standard Ubuntu images.

---

## 5. Metric Collection

### Where stream.jsonl lives
- Inside the container: `.wg/agents/<agent-id>/stream.jsonl`
- The native executor writes this file directly (`src/executor/native/agent.rs:154-158`)
- The path is derived from the output_log path (same directory)

### stream.jsonl event format (src/stream_event.rs)
```json
{"type": "init", "executor_type": "native", "model": "openrouter:minimax/minimax-m2.7", "timestamp_ms": ...}
{"type": "turn", "turn_number": 1, "tools_used": ["bash"], "usage": {"input_tokens": 100, "output_tokens": 50}, "timestamp_ms": ...}
{"type": "tool_start", "name": "bash", "timestamp_ms": ...}
{"type": "tool_end", "name": "bash", "is_error": false, "duration_ms": 500, "timestamp_ms": ...}
{"type": "result", "success": true, "usage": {"input_tokens": 500, "output_tokens": 200, "cost_usd": 0.05}, "timestamp_ms": ...}
```

### Current `_collect_agent_metrics()` (line 361)
- Reads from host filesystem: `os.path.join(wg_dir, "agents")` → loops over agent dirs → reads `stream.jsonl`
- Parses `turn` events for `total_turns`, `input_tokens`, `output_tokens`, `tools_used`
- Parses `result` events for `cost_usd`

### What needs to change for in-container execution
Since wg runs inside Docker, stream.jsonl is inside the container. Two options:

**Option A: Read via `exec()`**
```python
result = await environment.exec(command="cat .wg/agents/*/stream.jsonl")
# Parse result.stdout line by line
```
- Simple but has limits: glob expansion, large output truncation

**Option B: `download_dir()` after completion**
```python
await environment.download_dir(".wg/agents/", local_agents_dir)
# Then use existing _collect_agent_metrics(local_agents_dir_parent)
```
- More robust: exact same parsing code, handles multiple agent dirs
- Slightly more I/O

**Recommendation: Option B** — download `.wg/` (or just `.wg/agents/`) to a host temp dir after task completion, then reuse `_collect_agent_metrics()` with minimal changes.

---

## Implementation Plan

### Phase 1: Modify `setup()` to install wg inside the container

```python
async def setup(self, environment: BaseEnvironment) -> None:
    # 1. Upload wg binary into the container
    await environment.upload_file(self._wg_binary_host_path, "/usr/local/bin/wg")
    await environment.exec(command="chmod +x /usr/local/bin/wg")
    
    # 2. Initialize wg INSIDE the container
    await environment.exec(command="wg init")
    
    # 3. Write config.toml inside the container
    config_content = self._build_config_toml(condition, model)
    await environment.exec(command=f"cat > .wg/config.toml << 'WGEOF'\n{config_content}\nWGEOF")
    
    # 4. Write custom bundle if needed (Condition A)
    if CONDITION_CONFIG[self.condition].get("exclude_wg_tools"):
        bundle_content = self._build_bundle_toml()
        await environment.exec(command="mkdir -p .wg/bundles")
        await environment.exec(command=f"cat > .wg/bundles/implementer.toml << 'WGEOF'\n{bundle_content}\nWGEOF")
    
    # 5. Store environment reference for run() and teardown
    self._environment = environment
```

**Reuse**: `_write_trial_wg_config()` and `_write_trial_bundle()` logic stays the same, just write via `exec()` instead of host filesystem.

**Federation**: For conditions D/E/F, federation pull must happen on the host (hub is host-side), then we upload the pulled agency data into the container. Or skip federation for the Docker path and use `wg agency init` + `wg agent create` inside the container directly (simpler, sufficient for benchmarks).

### Phase 2: Add `_run_native_executor()` replacing `_run_docker_agent_loop()`

```python
async def _run_native_executor(
    environment: BaseEnvironment,
    task_instruction: str,
    verify_command: str | None,
    model: str,
    condition: str,
    timeout_secs: float = DEFAULT_TRIAL_TIMEOUT,
    poll_interval: float = DEFAULT_POLL_INTERVAL,
) -> dict[str, Any]:
    """Run the wg native executor inside the Docker container."""
    
    # 1. Add the task to the graph
    task_id = f"tb-{uuid.uuid4().hex[:8]}"
    add_cmd = f'wg add "{task_instruction}"'
    if verify_command:
        add_cmd += f' --verify "{verify_command}"'
    add_cmd += f' --id {task_id}'
    await environment.exec(command=add_cmd, env={"OPENROUTER_API_KEY": os.environ["OPENROUTER_API_KEY"]})
    
    # 2. Start the service (forks daemon, returns immediately)
    # Write OPENROUTER_API_KEY to a file so the daemon's child processes can read it.
    # (env= on exec() only applies to the immediate process, not daemons it forks)
    await environment.exec(
        command=(
            f'export OPENROUTER_API_KEY="{os.environ["OPENROUTER_API_KEY"]}" && '
            f'wg service start --model "{model}" --no-coordinator-agent'
        )
    )
    
    # 3. Poll for task completion
    start = time.monotonic()
    while True:
        elapsed = time.monotonic() - start
        if elapsed > timeout_secs:
            # Kill the service
            await environment.exec(command="wg service stop")
            return {"status": "timeout", "elapsed_s": elapsed, ...}
        
        result = await environment.exec(command=f"wg show {task_id}")
        # Parse status from output
        status = _parse_status(result.stdout)
        if status in ("done", "failed", "abandoned"):
            break
        await asyncio.sleep(poll_interval)
    
    # 4. Stop the service
    await environment.exec(command="wg service stop")
    
    # 5. Return status + elapsed
    return {"status": status, "task_id": task_id, "elapsed_s": time.monotonic() - start}
```

### Key detail: env var propagation for the daemon
`wg service start` forks a daemon. The daemon inherits its environment from the forking process. Since we use `exec(command="export OPENROUTER_API_KEY=... && wg service start ...")`, the exported var is in the shell's environment, and the forked daemon inherits it. This should work because `docker compose exec` runs a bash shell, and the daemon is a child of that shell.

**However**, there's a subtlety: `wg service start` spawns the daemon via `std::process::Command::new(current_exe).spawn()`. The child process inherits the OS environment of its parent. As long as the `export` precedes the `wg service start` call in the same shell, the daemon and its children (including `wg native-exec`) will see `OPENROUTER_API_KEY`.

**Alternative (more robust)**: Write the API key to a file, then source it:
```python
await environment.exec(command=f'echo "export OPENROUTER_API_KEY={key}" > /tmp/.wg-env')
await environment.exec(command='source /tmp/.wg-env && wg service start ...')
```
But the inline export should work. Test this first.

### Phase 3: Modify `run()` to call `_run_native_executor()`

```python
async def run(self, instruction: str, environment: BaseEnvironment, context: AgentContext) -> None:
    root_task_id = f"tb-{uuid.uuid4().hex[:8]}"
    trial_log = TrialLogger(...)
    trial_log.begin_turn(0)
    
    # Run native executor inside Docker
    exec_result = await _run_native_executor(
        environment=environment,
        task_instruction=instruction,
        verify_command=None,  # or from context if available
        model=self._model,
        condition=self.condition,
        timeout_secs=self.timeout,
        poll_interval=self.poll_interval,
    )
    
    trial_log.end_turn(had_tool_calls=True)
    
    # Collect metrics from inside the container
    metrics = await _collect_agent_metrics_from_container(environment)
    
    # Populate AgentContext (same as before)
    context.n_input_tokens = metrics["total_input_tokens"]
    context.n_output_tokens = metrics["total_output_tokens"]
    context.cost_usd = metrics.get("total_cost_usd", 0.0)
    context.metadata = { ... }
    
    trial_log.write_summary(...)
```

### Phase 4: Adapt metric collection

```python
async def _collect_agent_metrics_from_container(
    environment: BaseEnvironment,
) -> dict[str, Any]:
    """Download stream.jsonl files from container and parse metrics."""
    import tempfile
    
    local_tmp = tempfile.mkdtemp(prefix="tb-metrics-")
    try:
        # Download the agents directory from the container
        await environment.download_dir(".wg/agents/", os.path.join(local_tmp, "agents"))
        
        # Reuse existing parsing logic
        return await _collect_agent_metrics(local_tmp + "/.wg")
    except Exception as e:
        logger.warning(f"Failed to collect metrics from container: {e}")
        # Fallback: try reading via exec
        result = await environment.exec(command="cat .wg/agents/*/stream.jsonl 2>/dev/null || echo '{}'")
        return _parse_stream_jsonl_text(result.stdout or "")
    finally:
        shutil.rmtree(local_tmp, ignore_errors=True)
```

Note: `_collect_agent_metrics()` expects the path to contain an `agents/` subdirectory. The download path needs to match this structure. Actually, looking at the code (line 367), `agents_dir = os.path.join(wg_dir, "agents")` — so we should `download_dir(".wg/agents/", os.path.join(local_tmp, "agents"))` and pass `local_tmp` as `wg_dir`.

### Phase 5: Clean up dead code

- Remove `AGENT_TOOLS` (lines 484–540)
- Remove `_run_docker_agent_loop()` (lines 543–754) — or keep as a deprecated fallback behind a flag
- Remove `litellm` import (or make it conditional)
- Remove `WG_QUICK_GUIDE` and `CONDITION_F_MEMORY` — these are now handled by WG's native context assembly

### Files to modify
1. **`terminal-bench/wg/adapter.py`** — all the changes above
2. **`terminal-bench/reproduce.sh`** — no changes needed (already correct)
3. **`terminal-bench/pyproject.toml`** — remove `litellm` from dependencies (optional, may break other things)

### What stays the same
- `CONDITION_CONFIG` — unchanged, drives config.toml generation
- `ConditionAAgent` through `ConditionFAgent` — unchanged, just thin subclasses
- `_normalize_model()` — unchanged
- `_find_wg_binary()` — unchanged (still needed to find the host binary for upload)
- `TrialLogger` — unchanged

---

## Blockers / Unknowns

1. **Env var propagation through daemon fork**: The `export KEY=value && wg service start` pattern should work because `std::process::Command::spawn()` inherits the parent's environment. But it needs testing — if the daemon doesn't see the env var, we'll need to write it to a file inside the container that wg reads (e.g., `.env` or a config.toml `[provider]` section with the API key).

2. **`--no-coordinator-agent` flag**: For benchmarks we want a single task with no coordinator loop. The spec mentions this flag but need to verify it exists and works correctly — the coordinator agent would add overhead and unnecessary tasks. Confirmed: the flag exists in the `service start` CLI (src/commands/service/mod.rs:873).

3. **Task instruction quoting**: The `instruction` string from Harbor can contain quotes, newlines, special characters. Using `wg add "..."` in a shell command requires careful escaping. Better approach: write the instruction to a file inside the container, then use `wg add --description-file` if available, or use heredoc: `wg add "task" -d "$(cat /tmp/instruction.txt)"`.

4. **Container working directory**: Need to verify that `wg init` creates `.wg/` in the expected directory inside the container (likely `/home/agent/` or `/workspace/`). The exec() calls should use `cwd=` parameter if needed.

5. **git requirement**: `wg init` may require git to be installed in the container. TB containers should have git, but need to verify. If not, `apt-get install -y git` in setup.

6. **Service cleanup on timeout**: If the trial times out, we need to `wg service stop` inside the container to kill the daemon. Otherwise, the daemon continues running, burning API credits.

7. **Federation in Docker**: For conditions D/E with agency, the federation hub is on the host. Options: (a) skip federation and use `wg agency init` + `wg agent create` inside the container, (b) upload the hub's agency directory into the container. Option (a) is simpler and sufficient for benchmarks.
