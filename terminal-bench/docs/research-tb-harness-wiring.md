# Research: TB Harness Wiring for `wg nex --eval-mode`

**Task:** research-tb-harness-wiring
**Date:** 2026-04-20

---

## 1. Eval-Mode JSON Contract

When `wg nex --eval-mode` is invoked, it emits exactly **one line** of JSON on stdout after the agent loop completes. All other output (banners, progress, errors) goes to stderr.

### JSON Schema

```json
{
  "status": "ok" | "abnormal",
  "turns": <integer>,
  "input_tokens": <integer>,
  "output_tokens": <integer>,
  "exit_reason": <string>
}
```

### Field Details

| Field | Type | Description |
|-------|------|-------------|
| `status` | `"ok"` or `"abnormal"` | `"ok"` when `AgentResult::terminated_cleanly()` returns true; `"abnormal"` otherwise |
| `turns` | integer | Number of agent turns executed |
| `input_tokens` | integer (u32) | Total input tokens consumed across all turns |
| `output_tokens` | integer (u32) | Total output tokens consumed across all turns |
| `exit_reason` | string (JSON-encoded) | The reason the loop exited. Clean values: `"end_turn"`, `"user_quit"`, `"eof"`. Abnormal values: `"context_limit"`, `"max_turns"`, `"release_requested"`, `"empty_first_input"` |

### Exit Code Semantics

- **Exit 0**: Loop terminated cleanly (`end_turn`, `user_quit`, `eof`). Caller should treat the task as successful.
- **Non-zero exit**: Loop terminated abnormally. The JSON summary is **still emitted** (before the bail). Harness should parse stdout regardless of exit code.

### Example Outputs

Successful run:
```json
{"status":"ok","turns":4,"input_tokens":12340,"output_tokens":2100,"exit_reason":"end_turn"}
```

Context limit hit:
```json
{"status":"abnormal","turns":34,"input_tokens":198000,"output_tokens":45000,"exit_reason":"context_limit"}
```

Source: `src/commands/nex.rs:409-422`

---

## 2. Eval-Mode Behavioral Contract

Eval-mode is a preset that implies:

| Behavior | Normal nex | Eval-mode |
|----------|-----------|-----------|
| Autonomous | configurable | **always on** (one-shot, EndTurn exits) |
| MCP servers | configurable | **always off** (deterministic tool surface) |
| Chat surface (inbox/outbox) | on for autonomous | **off** (no `.streaming`/`outbox.jsonl` pollution) |
| Session lock | acquired | **skipped** (short-lived, no lock files) |
| stderr banner | shown | **suppressed** (clean stderr for harness logs) |
| tool_progress! stderr | shown | **suppressed** via `stderr_scope(true, ...)` |
| stdout | normal output | **reserved for JSON summary line** |

Source: `src/commands/nex.rs:36-47, 249-262, 341, 361, 389-422`

### Invocation Signature

```bash
WG_DIR="$dir" wg nex --eval-mode --max-turns 10 \
  -m "provider:model" \
  'Task instruction text here'
```

Key flags:
- `--eval-mode`: activates the preset
- `--max-turns N`: caps agent iterations
- `-m MODEL`: model identifier (provider:model format)
- Positional arg: the task instruction (single string)
- `--system-prompt TEXT`: optional system prompt override

---

## 3. Existing Smoke Test Contract (`eval-harness-smoke.sh`)

The smoke test at `scripts/eval-harness-smoke.sh` implements a minimal SWE-bench-shaped harness that validates the eval-mode design end-to-end:

### Protocol

1. **Setup**: Create temp dir, `git init`, write `greet.sh` (prints "hello world") and `run_test.sh` (checks for "hello moon")
2. **Pre-check**: Confirm `run_test.sh` fails before agent runs
3. **Invoke agent**: `WG_DIR="$tmp/.wg_state" timeout 180 wg nex --eval-mode --max-turns 10 'instruction'`
   - stdout → captured to file
   - stderr → captured to file (should be empty under eval-mode)
4. **Post-check**: Run `run_test.sh` again — must pass
5. **Parse JSON**: Extract `status`, `turns`, `exit_reason` with `jq`; verify `status == "ok"`

### Contract Summary

- Harness creates a git repo with a known-broken test
- Agent runs in the repo's working directory
- Agent's only job: modify files to make the test pass
- Harness checks the filesystem state after agent exits
- JSON summary parsed for telemetry/scoring
- Process exit code + JSON status both contribute to pass/fail

---

## 4. Eval-Mode vs. Adapter Path: Architectural Comparison

### Eval-Mode Path (nex drives one task directly)

```
Harness (bash/python)
    │
    └─ wg nex --eval-mode -m MODEL 'instruction'
           │
           └─ AgentLoop::run_interactive()
                  │
                  ├─ ToolRegistry (bash, read_file, write_file, edit_file, glob, grep)
                  │  (no wg mutation tools unless --role coordinator)
                  └─ Single agent, single task, single process
                       │
                       └─ JSON summary on stdout
```

**Characteristics:**
- Single process, no daemon, no polling
- Agent runs in CWD (the repo being evaluated)
- No graph.jsonl, no coordinator, no worktree isolation
- Tool surface: bash + file tools (no wg_done, wg_add, etc.)
- Model configured via `-m` flag or `WG_MODEL` env var
- JSON summary on stdout, clean stderr

### Adapter Path (wg service start + native-exec)

```
Harbor Runner
    │
    └─ WorkgraphAgent.setup()
           │ upload wg binary, wg init, write config.toml, write bundle
           │
    └─ WorkgraphAgent.run()
           │
           ├─ wg add "TB task" --id <id> --verify "test cmd"
           ├─ wg service start --model MODEL
           │      │
           │      └─ Coordinator
           │             │
           │             └─ native-exec agent(s)
           │                    │
           │                    └─ tool calls in Docker container
           │
           └─ Poll wg show <id> until terminal status
           └─ Collect metrics from stream.jsonl
           └─ Download .workgraph/ artifacts
```

**Characteristics:**
- Multi-process: coordinator daemon + spawned agent(s)
- Agent runs inside Docker container (via Harbor `environment.exec()`)
- Full graph.jsonl with task lifecycle (open → in-progress → done/failed)
- Tool surface controlled by bundle TOML + config.toml
- `--verify` gates auto-check test commands before marking done
- Metrics collected from `stream.jsonl` files
- Worktree isolation available for multi-agent conditions

---

## 5. Condition Mapping: Eval-Mode vs. Coordinator Path

| Condition | Description | Can Use Eval-Mode? | Notes |
|-----------|-------------|--------------------|----|
| **A** | Bare agent, no wg tools | **YES — ideal fit** | Eval-mode's default tool surface (bash + file tools, no wg tools) matches exactly. Just strip wg tools or don't register them. |
| **B** | Agent + wg tools + journal | **Partial** | B needs wg tools (wg_log, wg_done, etc.) and journal persistence. Eval-mode strips wg mutation tools. Would need `--role` to restore them, plus a running graph. |
| **C** | B + skill injection + planning | **Partial** | Same as B, plus needs skill prompt injection. Could use `--system-prompt` + `--role` but adds complexity. |
| **D** | B + verification + agency identity | **NO** | Requires agency bootstrap (role/tradeoff assignment), autopoietic verification loop, federation. These need the full coordinator. |
| **E** | B + organization + independent verification | **NO** | Requires organization generation, independent verifier agent, federation. Multi-agent. |
| **F** | B + distilled context injection | **Partial** | Like B but with memory injection. Could use `--system-prompt` with CONDITION_F_MEMORY. Same caveats as B. |
| **G** | Autopoietic multi-agent | **NO** | Requires coordinator, multi-agent dispatch, worktree isolation, heartbeat. Fundamentally multi-process. |
| **G-smart** | Smart fanout | **NO** | Same as G. |

### Summary

- **Eval-mode is the right fit for Condition A** (and A-prime variants). It's the simplest, most direct path.
- **Conditions B, C, F could partially use eval-mode** if wg tools are restored and a graph is pre-initialized, but the value of eval-mode (simplicity, no daemon) diminishes when you need the graph.
- **Conditions D, E, G require the full coordinator path**. They are fundamentally multi-agent or require agency pipelines.

---

## 6. Minimal NexEvalAgent Sketch for Harbor

A `NexEvalAgent` would be a Harbor `BaseAgent` subclass that shells out to `wg nex --eval-mode` inside the Docker container instead of running `_run_native_executor()` (which uses wg service start + coordinator + polling).

### Key Differences from WorkgraphAgent

| Aspect | WorkgraphAgent (current) | NexEvalAgent (proposed) |
|--------|-------------------------|------------------------|
| Execution | `wg service start` + poll | `wg nex --eval-mode` (single command) |
| Completion detection | Poll `wg show` until terminal | Wait for process exit |
| Metrics | Parse `stream.jsonl` from container | Parse JSON summary from stdout |
| Verify gate | `--verify` flag on `wg add` | Harness runs test after agent exits |
| Graph state | Full .workgraph/ with task lifecycle | Minimal (just for model/endpoint config) |
| Conditions | A–G | A only (expandable to B/C/F with work) |

### Concrete Sketch

```python
class NexEvalAgent(BaseAgent):
    """Harbor agent that shells out to `wg nex --eval-mode` inside Docker.
    
    Designed for Condition A: bare agent with bash + file tools.
    Simpler than WorkgraphAgent — no coordinator, no polling, no service.
    """

    @staticmethod
    def name() -> str:
        return "workgraph-nex-eval"

    def version(self) -> str | None:
        return "0.1.0"

    def __init__(
        self,
        logs_dir: Path | None = None,
        model_name: str | None = None,
        timeout: float = DEFAULT_TRIAL_TIMEOUT,
        max_turns: int = 50,
        *args, **kwargs,
    ):
        if logs_dir is None:
            logs_dir = Path("/tmp/wg-nex-eval-logs")
            logs_dir.mkdir(parents=True, exist_ok=True)
        super().__init__(logs_dir=logs_dir, model_name=model_name, *args, **kwargs)
        self._model = _normalize_model(model_name or BENCHMARK_MODEL)
        self._timeout = timeout
        self._max_turns = max_turns

    async def setup(self, environment: BaseEnvironment) -> None:
        """Upload wg binary and initialize minimal config."""
        wg_bin = self._find_wg_binary()
        await environment.upload_file(wg_bin, "/usr/local/bin/wg")
        await environment.exec(command="chmod +x /usr/local/bin/wg")

        # Create isolated trial directory
        self._trial_workdir = f"{_TRIAL_WORKDIR_PREFIX}{uuid.uuid4().hex[:12]}"
        await environment.exec(command=f"mkdir -p {self._trial_workdir}")

        # Initialize workgraph (needed for model/endpoint config)
        await environment.exec(
            command=f"cd {self._trial_workdir} && wg init --no-agency"
        )

        # Write minimal config.toml with model + endpoint
        config = (
            f'[agent]\n'
            f'model = "{self._model}"\n'
        )
        b64 = base64.b64encode(config.encode()).decode()
        await environment.exec(
            command=f"echo '{b64}' | base64 -d > "
                    f"{self._trial_workdir}/.workgraph/config.toml"
        )

        # Export API key for the agent process
        self._environment = environment

    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        """Run wg nex --eval-mode and parse the JSON summary."""
        # Write instruction to file (avoid shell quoting issues)
        b64_instr = base64.b64encode(instruction.encode()).decode()
        await environment.exec(
            command=f"echo '{b64_instr}' | base64 -d > /tmp/nex-instruction.txt"
        )

        api_key = os.environ.get("OPENROUTER_API_KEY", "")
        
        # Run wg nex --eval-mode
        # stdout = JSON summary, stderr = logs (suppressed by eval-mode)
        cmd = (
            f'export OPENROUTER_API_KEY="{api_key}" && '
            f'cd {self._trial_workdir} && '
            f'WG_DIR="{self._trial_workdir}/.workgraph" '
            f'wg nex --eval-mode --max-turns {self._max_turns} '
            f'-m "{self._model}" '
            f'"$(cat /tmp/nex-instruction.txt)"'
        )

        result = await asyncio.wait_for(
            environment.exec(command=cmd, timeout_sec=int(self._timeout)),
            timeout=self._timeout + 30,
        )

        # Parse JSON summary from stdout
        metrics = {
            "status": "unknown",
            "turns": 0,
            "input_tokens": 0,
            "output_tokens": 0,
            "exit_reason": "",
        }
        
        if result.stdout:
            for line in result.stdout.strip().splitlines():
                try:
                    parsed = json.loads(line)
                    metrics.update(parsed)
                    break
                except json.JSONDecodeError:
                    continue

        # Populate Harbor's AgentContext
        context.n_input_tokens = metrics.get("input_tokens", 0)
        context.n_output_tokens = metrics.get("output_tokens", 0)
        context.metadata = {
            "condition": "A",
            "model": self._model,
            "nex_eval_mode": True,
            "exit_code": result.return_code,
            "status": metrics["status"],
            "exit_reason": metrics["exit_reason"],
            "turns": metrics["turns"],
        }
```

### Key Implementation Notes

1. **API key propagation**: `OPENROUTER_API_KEY` must be exported in the shell before `wg nex` runs. The env export happens in the `exec()` command string, just like the existing adapter.

2. **Instruction passing**: Use base64-encode → file → `$(cat file)` pattern (same as existing adapter) to avoid shell quoting issues with Harbor's `exec()` stdin piping.

3. **WG_DIR**: Point to the trial's `.workgraph/` so wg finds its config. The `--dir` flag or `WG_DIR` env var controls this.

4. **No polling needed**: Unlike `_run_native_executor()`, eval-mode is synchronous — `environment.exec()` blocks until the agent finishes. Harbor's timeout handles the abort case.

5. **Verify gate**: Not built into the agent. The harness (Harbor's verifier) runs the test command after the agent exits, same as eval-harness-smoke.sh does.

---

## 7. Blockers and Gaps

### Gap 1: Tool surface mismatch for non-A conditions
Eval-mode strips wg mutation tools by default (nex.rs:77). For Condition B/C/F, you'd need a way to restore them. Options:
- Use `--role coordinator` (restores all wg tools, but also changes the system prompt)
- Add a new flag like `--eval-mode-with-wg-tools` or `--tools full`
- Not a blocker for Condition A.

### Gap 2: No `--verify` gate in eval-mode
The adapter path uses `wg add --verify "test cmd"` so the agent iterates until tests pass. Eval-mode has no equivalent — the agent just runs until EndTurn or max_turns. The test check is external (harness runs it after).
- For Condition A this is fine (harness checks the test).
- For conditions that need self-correction loops, this is a limitation.

### Gap 3: Model endpoint configuration
Eval-mode uses `create_provider_ext()` which reads the model registry and config. Inside a Docker container, the wg binary needs:
- A `config.toml` with the model configured
- OR a registered endpoint (via `wg endpoint add`)
- OR `-m "provider:model"` with the provider prefix triggering auto-endpoint resolution
The existing adapter handles this by writing `config.toml` — the NexEvalAgent would do the same.

### Gap 4: wg init requires git
`wg init` needs git available in the container. The existing adapter already checks for this and installs if needed. NexEvalAgent inherits this requirement.

### Gap 5: Container filesystem assumptions
Eval-mode runs in CWD. Harbor's container CWD may or may not be the task's working directory. The `cd $trial_workdir &&` prefix in the exec command handles this, but the agent's tool calls (bash, write_file) will operate relative to CWD. Need to ensure the container's CWD is set to the task's repo directory, not the trial workdir.

**Specific concern**: The current adapter creates a trial workdir at `/var/tmp/tb-trial-<uuid>/` and runs all wg commands there. But the *task* files (the repo being evaluated) live at the container's default CWD. The NexEvalAgent needs to run `wg nex` from the task repo directory, with `WG_DIR` pointing to the trial's `.workgraph/` separately.

### Gap 6: stderr capture for debugging
Eval-mode suppresses most stderr output. When debugging failures, this makes it hard to see what happened. The existing adapter captures stderr to a log file — NexEvalAgent should do the same.

### Not a blocker: binary compatibility
The existing adapter already handles uploading a bookworm-compatible binary into Docker containers (`target/bookworm-out/wg`). NexEvalAgent reuses the same `_find_wg_binary()` logic.

---

## 8. Recommendations

1. **Start with Condition A only.** Eval-mode is a near-perfect match for Condition A (bare agent, no graph). Ship this first.

2. **NexEvalAgent should be a thin subclass or standalone class**, not a modification of WorkgraphAgent. Keep the existing adapter for conditions B–G.

3. **Separate CWD from WG_DIR.** Run `wg nex --eval-mode` from the task repo directory (Harbor's container CWD) but set `WG_DIR` to a separate trial directory. This keeps the agent working in the right place while wg finds its config.

4. **Parse JSON summary for Harbor metrics.** Map `input_tokens`/`output_tokens` to `context.n_input_tokens`/`context.n_output_tokens`. Map `exit_reason` to termination type.

5. **Consider adding `--verify-cmd` flag to eval-mode** for conditions that need self-correction. This would let the agent retry until a test command passes, bridging the gap between eval-mode simplicity and the adapter's `--verify` behavior. Not needed for MVP.
