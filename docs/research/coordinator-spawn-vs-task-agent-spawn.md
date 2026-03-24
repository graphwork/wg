# Research: Coordinator Spawn Path vs Task Agent Spawn Path

## 1. Where does the coordinator spawn agents (the coordinator's own Claude CLI session)?

The coordinator agent (the LLM that makes dispatch decisions) is spawned in:

**`src/commands/service/coordinator_agent.rs:1847-1915`** — `fn spawn_claude_process()`

This function **hardcodes** `Command::new("claude")` (line 1860) and only accepts `model: Option<&str>`. It constructs the CLI invocation directly:

```rust
// coordinator_agent.rs:1860
let mut cmd = Command::new("claude");  // HARDCODED
cmd.args([
    "--print",
    "--input-format", "stream-json",
    "--output-format", "stream-json",
    "--verbose",
    "--dangerously-skip-permissions",
]);
cmd.args(["--system-prompt", &system_prompt]);
cmd.args(["--allowedTools", "Bash(wg:*)"]);

if let Some(m) = model {
    cmd.args(["--model", m]);  // Only config that's passed through
}
```

**Call chain:**
1. `src/commands/service/mod.rs:1906` — `CoordinatorAgent::spawn(&dir, 0, daemon_cfg.model.as_deref(), Some(&daemon_cfg.executor), ...)`
2. `src/commands/service/coordinator_agent.rs:305` — `pub fn spawn(dir, coordinator_id, model, executor, ...)`
3. `src/commands/service/coordinator_agent.rs:417` — `agent_thread_main(dir, coordinator_id, model, executor, ...)`
4. `src/commands/service/coordinator_agent.rs:489` — `spawn_claude_process(dir, model, logger)`

Note: `executor` is passed to `agent_thread_main` and used to branch between `"native"` (line 428) and the Claude CLI path (line 488), but **when executor is `"claude"`, the executor config files are never consulted** — it just calls `spawn_claude_process` which hardcodes `Command::new("claude")`.

## 2. Where do task agents get spawned (correctly resolving config)?

Task agents are spawned through:

**`src/commands/spawn/execution.rs:29-530`** — `fn spawn_agent_inner()`

This function:
1. Loads executor config from the registry: `ExecutorRegistry::new(dir).load_config(executor_name)` (line 142-143), which reads `.workgraph/executors/<name>.toml`
2. Applies template variables: `executor_config.apply_templates(&vars)` (line 210), producing `ExecutorSettings` with a resolved `command` field
3. Resolves model through a proper cascade (line 152-157):
   ```
   task.model > agent.preferred_model > executor.model > CLI/coordinator model
   ```
4. Resolves provider through a proper cascade (line 233-241):
   ```
   task.provider > registry_provider > agent.preferred_provider > role config provider > coordinator.provider
   ```
5. Resolves endpoint through a 6-level cascade (lines 250-270)
6. Builds the command string via `build_inner_command()` (line 305) using `settings.command` — **not hardcoded**

**`src/commands/spawn/execution.rs:588-827`** — `fn build_inner_command()`

For `"claude"` executor type (full mode, line 706):
```rust
let mut cmd_parts = vec![shell_escape(&settings.command)];  // FROM EXECUTOR CONFIG
for arg in &settings.args {
    cmd_parts.push(shell_escape(arg));
}
// ... adds --model, etc.
```

For `"native"` executor type (line 767), it passes `--model`, `--provider`, `--endpoint-name`, `--endpoint-url`, and `--api-key`.

## 3. What config fields exist for command_template and provider?

### `coordinator.provider` (`src/config.rs:1724-1727`)
```rust
pub struct CoordinatorConfig {
    /// Provider for the coordinator (e.g., "openrouter", "anthropic").
    #[serde(default)]
    pub provider: Option<String>,
}
```
- Loaded into `DaemonConfig.provider` at `src/commands/service/mod.rs:1809`
- **Never passed** to `CoordinatorAgent::spawn()` — the spawn function doesn't accept a provider parameter

### `agent.command_template` (`src/config.rs:1685-1686`)
```rust
pub struct AgentConfig {
    /// Command template for AI-based execution
    /// Placeholders: {model}, {prompt}, {task_id}, {workdir}
    #[serde(default = "default_command_template")]
    pub command_template: String,
}
```
- Default: `"claude --model {model} --print \"{prompt}\""` (line 1909-1910)
- **Only used in `config_cmd.rs:38`** for display — never used in any spawn path
- Appears to be vestigial/unused

### Executor config (`src/service/executor.rs:672-706`)
```rust
pub struct ExecutorSettings {
    pub executor_type: String,  // "claude", "shell", "native", etc.
    pub command: String,        // The actual command to run
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub prompt_template: Option<PromptTemplate>,
    pub model: Option<String>,
    // ...
}
```
- For built-in `claude` executor, defaults to `command: "claude"` (line 807)
- Task agents use this; coordinator agent does not

## 4. The Gap

| Aspect | Coordinator Agent | Task Agent |
|--------|------------------|------------|
| **Command** | Hardcoded `Command::new("claude")` (`coordinator_agent.rs:1860`) | `settings.command` from executor config (`execution.rs:709`) |
| **Model** | Passed through from `daemon_cfg.model` | 4-level cascade: task > agent > executor > coordinator |
| **Provider** | **Ignored** — `DaemonConfig.provider` exists but is never passed to spawn | 5-level cascade: task > registry > agent > role > coordinator |
| **Endpoint** | **Ignored** — no endpoint resolution at all | 6-level cascade with URL and API key resolution |
| **Executor config** | **Never loaded** — no `.workgraph/executors/` lookup | Loaded via `ExecutorRegistry` |
| **API key** | **None** — relies on Claude CLI's own auth | Resolved from endpoint config, passed as env var |
| **Environment vars** | Only removes `CLAUDECODE` and `CLAUDE_CODE_ENTRYPOINT` | Sets `WG_TASK_ID`, `WG_AGENT_ID`, `WG_EXECUTOR_TYPE`, `WG_MODEL`, `WG_ENDPOINT`, `WG_LLM_PROVIDER`, `WG_ENDPOINT_URL`, `WG_API_KEY` |

## 5. Proposed Fix Approach (Minimal Diff)

The coordinator agent has two execution modes: Claude CLI (`spawn_claude_process`) and native (`native_coordinator_loop`). The native path already uses `create_provider_ext` which may handle provider/endpoint correctly. The fix targets the Claude CLI path.

### Option A: Pass provider to `spawn_claude_process` (minimal)

1. **Add `provider` parameter** to `spawn_claude_process()` signature
2. **Add `--provider` flag** to the Claude CLI args when provider is set
3. **Thread `DaemonConfig.provider`** through the call chain:
   - `CoordinatorAgent::spawn()` already receives `executor` — add `provider: Option<&str>`
   - `agent_thread_main()` — add `provider` param
   - `spawn_claude_process()` — add `provider` param, use it:
     ```rust
     if let Some(p) = provider {
         cmd.args(["--provider", p]);
     }
     ```

4. **Wire up in `mod.rs`**: Change the two `CoordinatorAgent::spawn()` call sites (lines 1906 and 2091) to pass `daemon_cfg.provider.as_deref()`

This is ~15 lines changed across 2 files.

### Option B: Use executor config for command resolution (more complete)

Load the executor config in `spawn_claude_process` and use `settings.command` instead of hardcoding `"claude"`. This would also pick up custom args, env vars, etc. More invasive but aligns the two paths.

### Recommendation

**Option A** for an immediate fix — it solves the reported issue (provider is ignored) with minimal risk. Option B can follow as a separate task if full executor config parity is desired for the coordinator agent.

### Note on `agent.command_template`

The `agent.command_template` config field appears to be vestigial — it's defined in `AgentConfig`, has a default value, and is displayed in `wg config`, but is never read by any spawn path. It should either be wired in or removed in a separate cleanup task.
