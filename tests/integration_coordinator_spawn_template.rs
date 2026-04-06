//! Integration tests verifying that the coordinator agent correctly resolves
//! `command_template` (executor config) and `provider` when spawning agents.
//!
//! These tests use a mock CLI that captures its own invocation arguments to a
//! file, allowing us to verify the exact command, model, and provider flags
//! passed by the coordinator's spawn logic.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use workgraph::config::CLAUDE_SONNET_MODEL_ID;

extern crate libc;

// ---------------------------------------------------------------------------
// Helpers (shared with other integration tests)
// ---------------------------------------------------------------------------

fn wg_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("could not get current exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("wg");
    assert!(
        path.exists(),
        "wg binary not found at {:?}. Run `cargo build` first.",
        path
    );
    path
}

/// Derive a fake HOME from the wg_dir path so global config doesn't leak in.
fn fake_home_for(wg_dir: &Path) -> PathBuf {
    wg_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| wg_dir.to_path_buf())
}

fn wg_cmd_env(wg_dir: &Path, args: &[&str], env_vars: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new(wg_binary());
    cmd.arg("--dir")
        .arg(wg_dir)
        .args(args)
        .env("HOME", fake_home_for(wg_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for &(key, val) in env_vars {
        cmd.env(key, val);
    }
    cmd.output()
        .unwrap_or_else(|e| panic!("Failed to run wg {:?}: {}", args, e))
}

fn wg_ok(wg_dir: &Path, args: &[&str]) -> String {
    let output = wg_cmd_env(wg_dir, args, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "wg {:?} failed.\nstdout: {}\nstderr: {}",
        args,
        stdout,
        stderr
    );
    stdout
}

fn init_workgraph(tmp: &TempDir) -> PathBuf {
    let wg_dir = tmp.path().join(".workgraph");
    wg_ok(&wg_dir, &["init"]);
    wg_dir
}

fn write_config(wg_dir: &Path, content: &str) {
    let config_path = wg_dir.join("config.toml");
    fs::write(&config_path, content).unwrap();
}

fn stop_daemon(wg_dir: &Path, env_vars: &[(&str, &str)]) {
    let _ = wg_cmd_env(
        wg_dir,
        &["service", "stop", "--force", "--kill-agents"],
        env_vars,
    );
    // Kill by PID as belt-and-suspenders
    let state_path = wg_dir.join("service").join("state.json");
    if let Ok(content) = fs::read_to_string(&state_path) {
        if let Ok(state) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(pid) = state["pid"].as_u64() {
                unsafe {
                    libc::kill(pid as i32, libc::SIGKILL);
                }
            }
        }
    }
}

fn wait_for_socket(wg_dir: &Path) {
    let socket = wg_dir.join("service").join("daemon.sock");
    let start = Instant::now();
    while !socket.exists() {
        if start.elapsed() > Duration::from_secs(10) {
            panic!("Daemon socket did not appear within 10s at {:?}", socket);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_coordinator_agent(wg_dir: &Path) {
    let log_path = wg_dir.join("service").join("daemon.log");
    let start = Instant::now();
    loop {
        if start.elapsed() > Duration::from_secs(15) {
            let log = fs::read_to_string(&log_path).unwrap_or_default();
            panic!(
                "Coordinator agent did not start within 15s.\nDaemon log:\n{}",
                log
            );
        }
        if let Ok(content) = fs::read_to_string(&log_path)
            && (content.contains("Claude CLI started")
                || content.contains("Coordinator agent spawned successfully")
                || content.contains("Native coordinator: initialized"))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn read_daemon_log(wg_dir: &Path) -> String {
    let log_path = wg_dir.join("service").join("daemon.log");
    fs::read_to_string(&log_path).unwrap_or_else(|_| "<no log>".to_string())
}

// ---------------------------------------------------------------------------
// Arg-capturing mock CLI
// ---------------------------------------------------------------------------

/// Creates a mock CLI script at the given path that:
/// 1. Writes all its args to `$MOCK_ARGS_FILE` (one per line)
/// 2. Behaves as a minimal stream-json Claude mock
fn create_arg_capturing_mock(dir: &Path, name: &str) -> PathBuf {
    let mock_path = dir.join(name);
    let script = r#"#!/bin/bash
# Arg-capturing mock: writes invocation args to MOCK_ARGS_FILE then acts as mock Claude

# Write all args to the capture file
if [ -n "$MOCK_ARGS_FILE" ]; then
    printf '%s\n' "$0" "$@" > "$MOCK_ARGS_FILE"
fi

# Handle --version check
for arg in "$@"; do
    if [ "$arg" = "--version" ]; then
        echo "mock-cli 0.1.0"
        exit 0
    fi
done

# Stream-JSON mode: read stdin line-by-line, respond to user messages
msg_count=0
while IFS= read -r line; do
    if [[ "$line" == *'"type":"user"'* ]]; then
        msg_count=$((msg_count + 1))
        printf '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Mock response #%d"}],"stop_reason":"end_turn"}}\n' "$msg_count"
    fi
done
"#;
    fs::write(&mock_path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&mock_path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    mock_path
}

/// Guard that starts the daemon with custom env and stops on drop.
struct DaemonGuard<'a> {
    wg_dir: &'a Path,
    env_vars: Vec<(String, String)>,
}

impl<'a> DaemonGuard<'a> {
    fn start(wg_dir: &'a Path, env_vars: &[(&str, &str)], extra_args: &[&str]) -> Self {
        let env_vec: Vec<(String, String)> = env_vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let mut args = vec!["service", "start", "--interval", "600", "--max-agents", "0"];
        args.extend_from_slice(extra_args);

        let output = wg_cmd_env(wg_dir, &args, env_vars);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert!(
            output.status.success(),
            "service start failed.\nstdout: {}\nstderr: {}",
            stdout,
            stderr
        );

        wait_for_socket(wg_dir);
        wait_for_coordinator_agent(wg_dir);
        std::thread::sleep(Duration::from_millis(200));

        DaemonGuard {
            wg_dir,
            env_vars: env_vec,
        }
    }

    fn env_refs(&self) -> Vec<(&str, &str)> {
        self.env_vars
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    fn chat_ok(&self, message: &str, timeout_secs: u32) -> String {
        let timeout = timeout_secs.to_string();
        let output = wg_cmd_env(
            self.wg_dir,
            &["chat", message, "--timeout", &timeout],
            &self.env_refs(),
        );
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert!(
            output.status.success(),
            "wg chat {:?} failed.\nstdout: {}\nstderr: {}\nDaemon log:\n{}",
            message,
            stdout,
            stderr,
            read_daemon_log(self.wg_dir),
        );
        stdout
    }
}

impl Drop for DaemonGuard<'_> {
    fn drop(&mut self) {
        stop_daemon(self.wg_dir, &self.env_refs());
    }
}

/// Read the captured args file and return its lines.
fn read_captured_args(args_file: &Path) -> Vec<String> {
    let start = Instant::now();
    // The args file may take a moment to appear (process spawning is async)
    loop {
        if let Ok(content) = fs::read_to_string(args_file) {
            if !content.is_empty() {
                return content.lines().map(String::from).collect();
            }
        }
        if start.elapsed() > Duration::from_secs(10) {
            panic!(
                "Args capture file did not appear within 10s at {:?}",
                args_file
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ===========================================================================
// Test 1: Default behavior — no custom config → spawns "claude" command
// ===========================================================================

#[test]
fn coordinator_spawn_default_command() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    // Create a mock "claude" binary on PATH
    let mock_dir = TempDir::new().unwrap();
    let args_file = tmp.path().join("captured_args.txt");
    create_arg_capturing_mock(mock_dir.path(), "claude");

    let path_env = format!(
        "{}:{}",
        mock_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    write_config(&wg_dir, "[coordinator]\ncoordinator_agent = true\n");

    let env = [
        ("PATH", path_env.as_str()),
        ("MOCK_ARGS_FILE", args_file.to_str().unwrap()),
    ];
    let guard = DaemonGuard::start(&wg_dir, &env, &[]);

    // Send a chat message to trigger the coordinator agent
    let stdout = guard.chat_ok("Hello", 15);
    assert!(
        stdout.contains("Mock response"),
        "Expected mock response, got:\n{}\nDaemon log:\n{}",
        stdout,
        read_daemon_log(&wg_dir),
    );

    // Verify the mock "claude" binary was invoked (arg[0] contains "claude")
    let args = read_captured_args(&args_file);
    assert!(!args.is_empty(), "No args captured — mock not invoked?");
    assert!(
        args[0].ends_with("/claude"),
        "Expected default command 'claude', but arg[0] = {:?}",
        args[0]
    );

    // Verify no --provider flag in default config
    assert!(
        !args.contains(&"--provider".to_string()),
        "Default config should not pass --provider flag. Args: {:?}",
        args
    );
}

// ===========================================================================
// Test 2: Custom executor config → coordinator uses custom command
// ===========================================================================

#[test]
fn coordinator_spawn_custom_executor_command() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    // Create a mock CLI with a custom name
    let mock_dir = TempDir::new().unwrap();
    let args_file = tmp.path().join("captured_args.txt");
    create_arg_capturing_mock(mock_dir.path(), "my-custom-cli");

    let path_env = format!(
        "{}:{}",
        mock_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // Write a custom executor config for "claude" executor that overrides the command
    let executors_dir = wg_dir.join("executors");
    fs::create_dir_all(&executors_dir).unwrap();
    fs::write(
        executors_dir.join("claude.toml"),
        r#"[executor]
type = "claude"
command = "my-custom-cli"
args = ["--print", "--verbose", "--permission-mode", "bypassPermissions", "--output-format", "stream-json"]
"#,
    )
    .unwrap();

    write_config(&wg_dir, "[coordinator]\ncoordinator_agent = true\n");

    let env = [
        ("PATH", path_env.as_str()),
        ("MOCK_ARGS_FILE", args_file.to_str().unwrap()),
    ];
    let guard = DaemonGuard::start(&wg_dir, &env, &[]);

    let stdout = guard.chat_ok("Hello", 15);
    assert!(
        stdout.contains("Mock response"),
        "Expected mock response, got:\n{}\nDaemon log:\n{}",
        stdout,
        read_daemon_log(&wg_dir),
    );

    // Verify the custom command was used
    let args = read_captured_args(&args_file);
    assert!(!args.is_empty(), "No args captured — mock not invoked?");
    assert!(
        args[0].ends_with("/my-custom-cli"),
        "Expected custom command 'my-custom-cli', but arg[0] = {:?}",
        args[0]
    );
}

// ===========================================================================
// Test 3: Non-Anthropic provider → coordinator uses native path (not Claude CLI)
// ===========================================================================

#[test]
fn coordinator_spawn_with_provider() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    let mock_dir = TempDir::new().unwrap();
    // Still create a claude mock so is_claude_available() passes
    create_arg_capturing_mock(mock_dir.path(), "claude");

    let path_env = format!(
        "{}:{}",
        mock_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // Configure with provider:model format (provider embedded in model spec)
    write_config(
        &wg_dir,
        "[coordinator]\ncoordinator_agent = true\nmodel = \"openrouter:minimax-m2.5\"\n",
    );

    // Set a dummy API key so the native coordinator can initialize its client.
    // Override HOME so the global ~/.workgraph/config.toml (which may set executor
    // explicitly) doesn't interfere with auto-detection.
    let fake_home = TempDir::new().unwrap();
    let env = [
        ("PATH", path_env.as_str()),
        (
            "OPENROUTER_API_KEY",
            "sk-test-dummy-key-for-provider-routing",
        ),
        ("HOME", fake_home.path().to_str().unwrap()),
    ];
    let guard = DaemonGuard::start(&wg_dir, &env, &[]);

    // When provider is openrouter, the coordinator should use the native path
    // (direct API calls) instead of spawning the Claude CLI. Verify via daemon log.
    let log = read_daemon_log(&wg_dir);
    assert!(
        log.contains("Native coordinator: initialized"),
        "Expected native coordinator to be used for openrouter provider.\nDaemon log:\n{}",
        log
    );
    assert!(
        log.contains("provider=openrouter"),
        "Expected provider=openrouter in native coordinator log.\nDaemon log:\n{}",
        log
    );

    // The Claude CLI mock should NOT have been invoked (no args file written)
    let args_file = tmp.path().join("captured_args.txt");
    assert!(
        !args_file.exists(),
        "Claude CLI mock should not be invoked when provider=openrouter"
    );

    drop(guard);
}

// ===========================================================================
// Test 4: Custom executor overrides global default command
// ===========================================================================

#[test]
fn coordinator_spawn_executor_config_overrides_default() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    // Create TWO mock CLIs: "claude" (default) and "custom-executor"
    let mock_dir = TempDir::new().unwrap();
    let args_file = tmp.path().join("captured_args.txt");
    // Only create the custom one — if the default "claude" were used, the args
    // file would show "claude" instead.
    create_arg_capturing_mock(mock_dir.path(), "custom-executor");
    // Also create a "claude" mock so the daemon can at least find it on PATH
    // (for the is_claude_available check), but we want the executor config to
    // redirect to "custom-executor".
    create_arg_capturing_mock(mock_dir.path(), "claude");

    let path_env = format!(
        "{}:{}",
        mock_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // Write executor config that overrides the claude command
    let executors_dir = wg_dir.join("executors");
    fs::create_dir_all(&executors_dir).unwrap();
    fs::write(
        executors_dir.join("claude.toml"),
        r#"[executor]
type = "claude"
command = "custom-executor"
args = ["--print", "--verbose", "--permission-mode", "bypassPermissions", "--output-format", "stream-json"]
"#,
    )
    .unwrap();

    // Set coordinator config — use an unknown provider name so it stays on the
    // CLI path (only openrouter/openai/local route to native).
    write_config(&wg_dir, "[coordinator]\ncoordinator_agent = true\n");

    let env = [
        ("PATH", path_env.as_str()),
        ("MOCK_ARGS_FILE", args_file.to_str().unwrap()),
    ];
    let guard = DaemonGuard::start(&wg_dir, &env, &[]);

    let stdout = guard.chat_ok("Hello", 15);
    assert!(
        stdout.contains("Mock response"),
        "Expected mock response, got:\n{}\nDaemon log:\n{}",
        stdout,
        read_daemon_log(&wg_dir),
    );

    let args = read_captured_args(&args_file);
    assert!(!args.is_empty(), "No args captured");

    // Verify executor config overrode the default "claude" command
    assert!(
        args[0].ends_with("/custom-executor"),
        "Expected executor config to override default command to 'custom-executor', but got {:?}",
        args[0]
    );
}

// ===========================================================================
// Test 5: Model passthrough — {model} from config is passed as --model flag
// ===========================================================================

#[test]
fn coordinator_spawn_model_passthrough() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    let mock_dir = TempDir::new().unwrap();
    let args_file = tmp.path().join("captured_args.txt");
    create_arg_capturing_mock(mock_dir.path(), "claude");

    let path_env = format!(
        "{}:{}",
        mock_dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // Configure a specific model using provider:model format
    let config_str = format!(
        "[coordinator]\ncoordinator_agent = true\nmodel = \"claude:{CLAUDE_SONNET_MODEL_ID}\"\n"
    );
    write_config(&wg_dir, &config_str);

    let env = [
        ("PATH", path_env.as_str()),
        ("MOCK_ARGS_FILE", args_file.to_str().unwrap()),
    ];
    let guard = DaemonGuard::start(&wg_dir, &env, &[]);

    let stdout = guard.chat_ok("Hello", 15);
    assert!(
        stdout.contains("Mock response"),
        "Expected mock response, got:\n{}\nDaemon log:\n{}",
        stdout,
        read_daemon_log(&wg_dir),
    );

    // Verify --model flag with the configured model
    let args = read_captured_args(&args_file);
    let model_idx = args.iter().position(|a| a == "--model");
    assert!(
        model_idx.is_some(),
        "Expected --model flag in args. Args: {:?}",
        args
    );
    let model_value = &args[model_idx.unwrap() + 1];
    assert_eq!(
        model_value, CLAUDE_SONNET_MODEL_ID,
        "Expected model '{}', got '{}'",
        CLAUDE_SONNET_MODEL_ID, model_value
    );
}

// ===========================================================================
// Unit test: ExecutorRegistry resolves default vs custom configs
// ===========================================================================

#[test]
fn executor_registry_default_command_is_claude() {
    // Test that the executor registry returns "claude" as the default command
    // when no custom config file exists
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");
    fs::create_dir_all(&wg_dir).unwrap();

    let registry = workgraph::service::executor::ExecutorRegistry::new(&wg_dir);
    let config = registry.load_config("claude").unwrap();

    assert_eq!(
        config.executor.command, "claude",
        "Default executor command should be 'claude'"
    );
    assert_eq!(
        config.executor.executor_type, "claude",
        "Default executor type should be 'claude'"
    );
}

#[test]
fn executor_registry_custom_toml_overrides_command() {
    // Test that a custom .workgraph/executors/claude.toml overrides the default
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".workgraph");
    let executors_dir = wg_dir.join("executors");
    fs::create_dir_all(&executors_dir).unwrap();

    fs::write(
        executors_dir.join("claude.toml"),
        r#"[executor]
type = "claude"
command = "my-custom-binary"
args = ["--print"]
"#,
    )
    .unwrap();

    let registry = workgraph::service::executor::ExecutorRegistry::new(&wg_dir);
    let config = registry.load_config("claude").unwrap();

    assert_eq!(
        config.executor.command, "my-custom-binary",
        "Custom TOML should override default command"
    );
}
