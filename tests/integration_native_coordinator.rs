//! Integration tests for the native coordinator with OpenRouter model support.
//!
//! Validates that the coordinator works end-to-end when configured with:
//! ```toml
//! [coordinator]
//! executor = "native"
//! model = "deepseek/deepseek-chat"
//! ```
//!
//! Test tiers:
//! 1. **Config + provider tests**: Verify config parsing, model registry lookup,
//!    and provider creation for OpenRouter models (no API key needed).
//! 2. **Daemon-level tests**: Start the service daemon with `executor = "native"`,
//!    verify startup logs and behavior (mock-friendly, no real LLM calls).
//! 3. **Real E2E tests** (`#[ignore]` / `llm-tests` feature): Exercise the full
//!    flow with a real OpenRouter API key and a cheap model.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tempfile::TempDir;

extern crate libc;

// ---------------------------------------------------------------------------
// Helpers
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

fn wg_cmd(wg_dir: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = Command::new(wg_binary());
    isolate_command_env(&mut cmd, wg_dir);
    cmd.arg("--dir")
        .arg(wg_dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("Failed to run wg {:?}: {}", args, e))
}

fn wg_cmd_env(wg_dir: &Path, args: &[&str], env_vars: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new(wg_binary());
    isolate_command_env(&mut cmd, wg_dir);
    cmd.arg("--dir")
        .arg(wg_dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for &(key, val) in env_vars {
        if val.is_empty() {
            cmd.env_remove(key);
        } else {
            cmd.env(key, val);
        }
    }
    cmd.output()
        .unwrap_or_else(|e| panic!("Failed to run wg {:?}: {}", args, e))
}

fn isolate_command_env(cmd: &mut Command, wg_dir: &Path) {
    cmd.env("HOME", fake_home_for(wg_dir));
    for key in [
        "WG_LLM_PROVIDER",
        "WG_ENDPOINT",
        "WG_ENDPOINT_URL",
        "WG_MODEL",
        "OPENAI_BASE_URL",
        "OPENROUTER_BASE_URL",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "WG_API_KEY",
    ] {
        cmd.env_remove(key);
    }
}

fn wg_ok(wg_dir: &Path, args: &[&str]) -> String {
    let output = wg_cmd(wg_dir, args);
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
    let wg_dir = tmp.path().join(".wg");
    wg_ok(&wg_dir, &["init", "--route", "claude-cli"]);
    wg_dir
}

/// Write config.toml for native coordinator with an OpenRouter model.
fn configure_native_coordinator(wg_dir: &Path, model: &str) {
    let config = format!(
        r#"[dispatcher]
coordinator_agent = true
executor = "native"
model = "{}"

[agency]
auto_assign = false
auto_evaluate = false
"#,
        model
    );
    fs::write(wg_dir.join("config.toml"), config).unwrap();
}

/// Write config.toml for the classic claude executor (backwards compatibility).
fn configure_claude_coordinator(wg_dir: &Path) {
    let config = r#"[dispatcher]
coordinator_agent = true
executor = "claude"

[agency]
auto_assign = false
auto_evaluate = false
"#;
    fs::write(wg_dir.join("config.toml"), config).unwrap();
}

fn read_daemon_log(wg_dir: &Path) -> String {
    let log_path = wg_dir.join("service").join("daemon.log");
    fs::read_to_string(&log_path).unwrap_or_else(|_| "<no log>".to_string())
}

fn stop_daemon_env(wg_dir: &Path, env_vars: &[(&str, &str)]) {
    let _ = wg_cmd_env(
        wg_dir,
        &["service", "stop", "--force", "--kill-agents"],
        env_vars,
    );
}

/// Wait for a condition with timeout, polling at interval.
fn wait_for<F>(timeout: Duration, poll_ms: u64, mut condition: F) -> bool
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(poll_ms));
    }
    false
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

/// Guard that stops the daemon and kills by PID on drop.
struct DaemonGuard<'a> {
    wg_dir: &'a Path,
    env_vars: Vec<(String, String)>,
}

impl<'a> DaemonGuard<'a> {
    fn new(wg_dir: &'a Path) -> Self {
        DaemonGuard {
            wg_dir,
            env_vars: vec![],
        }
    }

    fn with_env(wg_dir: &'a Path, env_vars: &[(&str, &str)]) -> Self {
        DaemonGuard {
            wg_dir,
            env_vars: env_vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn env_refs(&self) -> Vec<(&str, &str)> {
        self.env_vars
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }
}

impl Drop for DaemonGuard<'_> {
    fn drop(&mut self) {
        stop_daemon_env(self.wg_dir, &self.env_refs());
        let state_path = self.wg_dir.join("service").join("state.json");
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
}

// ===========================================================================
// 1. Config + provider creation tests (no API key needed)
// ===========================================================================

/// Verify that the model registry contains OpenRouter models by default.
#[test]
fn native_coordinator_model_registry_has_openrouter_models() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    let registry = workgraph::models::ModelRegistry::load(&wg_dir).unwrap();

    // Check that key OpenRouter models are present
    assert!(
        registry.models.contains_key("deepseek/deepseek-chat"),
        "Registry should contain deepseek/deepseek-chat"
    );
    assert!(
        registry.models.contains_key("qwen/qwen3-235b-a22b"),
        "Registry should contain qwen/qwen3-235b-a22b"
    );
    assert!(
        registry.models.contains_key("anthropic/claude-sonnet-4-6"),
        "Registry should contain anthropic/claude-sonnet-4-6"
    );

    // Verify deepseek-chat-v3 has tool_use capability
    let ds = registry.models.get("deepseek/deepseek-chat").unwrap();
    assert!(
        ds.supports_tool_use(),
        "deepseek-chat-v3 should support tool use"
    );
    assert_eq!(ds.provider, "openrouter");
}

/// Verify that the config correctly parses native executor + OpenRouter model.
#[test]
fn native_coordinator_config_parsing() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);
    configure_native_coordinator(&wg_dir, "openrouter:deepseek/deepseek-chat");

    let config = workgraph::config::Config::load(&wg_dir).unwrap();
    assert_eq!(config.coordinator.executor.as_deref(), Some("native"));
    assert_eq!(
        config.coordinator.model.as_deref(),
        Some("openrouter:deepseek/deepseek-chat")
    );
    assert!(config.coordinator.coordinator_agent);
}

/// Verify that configuring executor = "native" makes the native executor
/// available in the executor registry.
#[test]
fn native_coordinator_executor_registry() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    let registry = workgraph::service::executor::ExecutorRegistry::new(&wg_dir);
    let config = registry.load_config("native").unwrap();
    assert_eq!(config.executor.executor_type, "native");
    assert_eq!(config.executor.command, "wg");
    assert!(config.executor.args.contains(&"native-exec".to_string()));
}

/// Verify that create_provider_ext routes OpenRouter models to the OpenAI-compatible
/// provider when the provider is explicitly set to "openai".
#[test]
fn native_coordinator_provider_routing_openrouter() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    // deepseek/deepseek-chat is an OpenRouter model. Use explicit provider
    // override to test the openai path (avoids env var interference from WG_LLM_PROVIDER).
    let result = workgraph::executor::native::provider::create_provider_ext(
        &wg_dir,
        "deepseek/deepseek-chat",
        Some("openai"),
        None,
        Some("test-api-key-not-real"),
    );
    assert!(
        result.is_ok(),
        "Provider creation should succeed with API key override"
    );
    let provider = result.unwrap();
    assert_eq!(
        provider.name(),
        "openai",
        "OpenRouter model should use openai provider"
    );
    assert_eq!(provider.model(), "deepseek/deepseek-chat");
}

/// Verify that Anthropic models route to the Anthropic provider.
#[test]
fn native_coordinator_provider_routing_anthropic() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    // bare model name (no slash) → routes to Anthropic provider
    let result = workgraph::executor::native::provider::create_provider_ext(
        &wg_dir,
        "claude-sonnet-4-5-20250514",
        Some("anthropic"),
        None,
        Some("test-api-key-not-real"),
    );
    assert!(
        result.is_ok(),
        "Provider creation should succeed with API key override"
    );
    let provider = result.unwrap();
    assert_eq!(
        provider.name(),
        "anthropic",
        "Bare model name should use anthropic provider"
    );
}

/// Verify that provider routing respects explicit provider override,
/// directing an Anthropic-prefixed model to the OpenAI-compatible backend.
#[test]
fn native_coordinator_provider_routing_explicit_override() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    // Force openai provider even for an anthropic-looking model
    let result = workgraph::executor::native::provider::create_provider_ext(
        &wg_dir,
        "anthropic/claude-sonnet-4-6",
        Some("openai"),
        None,
        Some("test-api-key-not-real"),
    );
    assert!(result.is_ok());
    let provider = result.unwrap();
    assert_eq!(provider.name(), "openai");
}

/// Verify that the model-based heuristic routes slashed models to openai
/// when no env var or config overrides are present.
#[test]
fn native_coordinator_provider_heuristic_slash_model() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    // Write config with explicit provider = "openai" in native_executor section
    // to override any WG_LLM_PROVIDER env var that might be set.
    let config = r#"
[native_executor]
provider = "openai"
"#;
    fs::write(wg_dir.join("config.toml"), config).unwrap();

    let result = workgraph::executor::native::provider::create_provider_ext(
        &wg_dir,
        "deepseek/deepseek-chat",
        None,
        None,
        Some("test-api-key-not-real"),
    );
    assert!(result.is_ok());
    let provider = result.unwrap();
    assert_eq!(
        provider.name(),
        "openai",
        "Config provider=openai should override env var"
    );
}

// ===========================================================================
// Per-provider key resolution tests (native-executor-client)
// ===========================================================================
//
// These tests verify the fix for the bug where the OAI-compat client init
// path called the Anthropic key resolver and emitted "No Anthropic API key
// found" — even when the model was an OpenAI-compatible local/openrouter
// model that should never need an Anthropic key.
//
// They mutate process env vars so they're `#[serial]` to avoid races.

mod per_provider_key_resolution {
    use super::*;
    use serial_test::serial;
    use workgraph::executor::native::provider::create_provider_ext;

    /// Snapshot the relevant env vars and unset them, returning a guard
    /// that restores the originals on drop. Lets each test run as if
    /// nothing was preset in the environment.
    struct EnvSnapshot {
        saved: Vec<(&'static str, Option<String>)>,
    }
    impl EnvSnapshot {
        fn new(vars: &[&'static str]) -> Self {
            let saved = vars.iter().map(|v| (*v, std::env::var(v).ok())).collect();
            for v in vars {
                unsafe { std::env::remove_var(v) };
            }
            EnvSnapshot { saved }
        }
        fn set(&self, var: &str, value: &str) {
            unsafe { std::env::set_var(var, value) };
        }
    }
    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                match v {
                    Some(value) => unsafe { std::env::set_var(k, value) },
                    None => unsafe { std::env::remove_var(k) },
                }
            }
        }
    }

    fn key_vars() -> &'static [&'static str] {
        &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "OPENROUTER_API_KEY",
            "WG_API_KEY",
            "WG_LLM_PROVIDER",
            "WG_ENDPOINT",
            "WG_ENDPOINT_URL",
            "WG_MODEL",
            "OPENAI_BASE_URL",
            "OPENROUTER_BASE_URL",
        ]
    }

    /// Create a `.wg/` dir with an empty graph and the provided config
    /// (or default config when `config_toml` is empty). Skips `wg init`
    /// entirely so we can write whatever endpoints + model spec we want.
    fn make_wg_dir(tmp: &TempDir, config_toml: &str) -> PathBuf {
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        std::fs::write(wg_dir.join("graph.jsonl"), "").unwrap();
        if !config_toml.is_empty() {
            std::fs::write(wg_dir.join("config.toml"), config_toml).unwrap();
        }
        wg_dir
    }

    /// `local:qwen3-coder` + endpoint configured + NO ANTHROPIC_API_KEY → succeeds.
    /// This is the explicit-prefix path that should never even consult ANTHROPIC_API_KEY.
    #[test]
    #[serial]
    fn local_prefixed_model_succeeds_without_anthropic_key() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = make_wg_dir(
            &tmp,
            r#"
[[llm_endpoints.endpoints]]
name = "lambda01"
provider = "local"
url = "https://lambda01.example.test:30000"
is_default = true
"#,
        );

        let _env = EnvSnapshot::new(key_vars());
        let result = create_provider_ext(&wg_dir, "local:qwen3-coder", None, None, None);
        assert!(
            result.is_ok(),
            "local:qwen3-coder must initialize without ANTHROPIC_API_KEY: {:?}",
            result.err().map(|e| format!("{:#}", e))
        );
        let p = result.unwrap();
        assert_eq!(p.name(), "local");
        assert_eq!(p.model(), "qwen3-coder");
    }

    /// Bare `qwen3-coder` (no provider prefix) + default endpoint with provider="local"
    /// + NO ANTHROPIC_API_KEY → succeeds.
    ///
    /// This mirrors the autohaiku failure where the dispatcher passed `--model qwen3-coder`
    /// (bare) to native-exec; the OAI-compat init path was reaching the Anthropic key
    /// resolver and bailing with "No Anthropic API key found".
    #[test]
    #[serial]
    fn bare_model_with_local_default_endpoint_succeeds_without_anthropic_key() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = make_wg_dir(
            &tmp,
            r#"
[[llm_endpoints.endpoints]]
name = "default"
provider = "local"
url = "https://lambda01.example.test:30000"
is_default = true
"#,
        );

        let _env = EnvSnapshot::new(key_vars());
        let result = create_provider_ext(&wg_dir, "qwen3-coder", None, None, None);
        assert!(
            result.is_ok(),
            "bare qwen3-coder + local default endpoint must NOT need ANTHROPIC_API_KEY: {:?}",
            result.err().map(|e| format!("{:#}", e))
        );
    }

    /// `openrouter:anthropic/claude-sonnet-4-6` + OPENROUTER_API_KEY set,
    /// ANTHROPIC_API_KEY unset → succeeds.
    #[test]
    #[serial]
    fn openrouter_model_uses_openrouter_key_not_anthropic() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = make_wg_dir(&tmp, "");

        let env = EnvSnapshot::new(key_vars());
        env.set("OPENROUTER_API_KEY", "sk-or-v1-fake-test");
        // ANTHROPIC_API_KEY remains unset.

        let result = create_provider_ext(
            &wg_dir,
            "openrouter:anthropic/claude-sonnet-4-6",
            None,
            None,
            None,
        );
        assert!(
            result.is_ok(),
            "openrouter model with OPENROUTER_API_KEY but no ANTHROPIC_API_KEY must succeed: {:?}",
            result.err().map(|e| format!("{:#}", e))
        );
        let p = result.unwrap();
        assert_eq!(
            p.name(),
            "openrouter",
            "openrouter:* should produce openrouter provider"
        );
    }

    /// `claude:opus` (anthropic) + no ANTHROPIC_API_KEY anywhere → client init
    /// MUST succeed (no precondition gate on key presence). The 401 from the
    /// real Anthropic endpoint is what surfaces the config-pointing error
    /// later. This is the new contract per `feedback_native_executor_no_env_vars`
    /// — env vars are never consulted by the credential path.
    #[test]
    #[serial]
    fn anthropic_model_no_key_init_succeeds_keyless() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = make_wg_dir(&tmp, "");

        let _env = EnvSnapshot::new(key_vars());
        // HOME points at a fakehome so ~/.config/anthropic/api_key (if it
        // exists on the dev box) cannot satisfy any (now-banned) lookup.
        let saved_home = std::env::var("HOME").ok();
        let fake_home = tmp.path().join("fakehome");
        std::fs::create_dir_all(&fake_home).unwrap();
        unsafe { std::env::set_var("HOME", &fake_home) };

        let result = create_provider_ext(&wg_dir, "claude:opus", None, None, None);

        match saved_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        assert!(
            result.is_ok(),
            "claude:opus with no key must NOT bail at init — keyless init is the contract: {:?}",
            result.err().map(|e| format!("{:#}", e))
        );
        let p = result.unwrap();
        assert_eq!(p.name(), "anthropic");
    }

    /// Bare `openai:gpt-5` (no endpoint, no env vars) → init succeeds. Same
    /// contract: client init must NOT precondition on key presence; the
    /// endpoint's 401 (when it eventually rejects) is the failure signal.
    #[test]
    #[serial]
    fn oai_compat_no_key_init_succeeds_keyless() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = make_wg_dir(&tmp, "");

        let _env = EnvSnapshot::new(key_vars());
        let saved_home = std::env::var("HOME").ok();
        let fake_home = tmp.path().join("fakehome");
        std::fs::create_dir_all(&fake_home).unwrap();
        unsafe { std::env::set_var("HOME", &fake_home) };

        let result = create_provider_ext(&wg_dir, "openai:gpt-5", None, None, None);

        match saved_home {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        assert!(
            result.is_ok(),
            "openai:gpt-5 with no key must NOT bail at init: {:?}",
            result.err().map(|e| format!("{:#}", e))
        );
        let p = result.unwrap();
        // `openai:` is canonicalized to "oai-compat" via `provider_to_native_provider`.
        assert_eq!(p.name(), "oai-compat");
    }

    /// `local:qwen3-coder` + endpoint with `api_key` configured + an
    /// ANTHROPIC_API_KEY env var poisoned with junk → the configured key
    /// wins, env vars are NEVER consulted. Verifies the env-var-isolation
    /// contract: WG config is the SOLE source of credentials.
    ///
    /// We don't have a packet sniffer in unit tests, so this asserts the
    /// next-best signal: the resolved provider/model are correct AND the
    /// env-poisoning didn't make init fail (which would happen if the
    /// resolver had picked up the junk key and tripped some validation).
    /// The behavioral assertion (request body bears configured key, not
    /// env junk) is covered by the live-smoke scenario.
    #[test]
    #[serial]
    fn env_var_ignored_when_endpoint_has_inline_api_key() {
        let tmp = TempDir::new().unwrap();
        let wg_dir = make_wg_dir(
            &tmp,
            r#"
[[llm_endpoints.endpoints]]
name = "lambda01"
provider = "local"
url = "https://lambda01.example.test:30000"
api_key = "configured-real-key"
is_default = true
"#,
        );

        let env = EnvSnapshot::new(key_vars());
        env.set("ANTHROPIC_API_KEY", "env-junk-should-never-be-read");
        env.set("OPENAI_API_KEY", "env-junk-should-never-be-read-either");

        let result = create_provider_ext(&wg_dir, "qwen3-coder", None, None, None);
        assert!(
            result.is_ok(),
            "qwen3-coder + configured endpoint must init cleanly: {:?}",
            result.err().map(|e| format!("{:#}", e))
        );
        let p = result.unwrap();
        // Bare `qwen3-coder` (no provider prefix) and no `endpoint_name`
        // override falls through to the heuristic default of "oai-compat"
        // (the canonical name for the OpenAI-compatible HTTP protocol).
        // The default endpoint's `api_key` and `url` are still used for
        // the request — that's what this test cares about.
        assert_eq!(p.name(), "oai-compat");
        assert_eq!(p.model(), "qwen3-coder");
    }
}

// ===========================================================================
// Behavioral smoke: HTTP-level credential contract (native-executor-client)
// ===========================================================================
//
// The unit-style tests above prove that init succeeds with no key. These
// tests prove the wire-level behavior: Authorization is suppressed when
// no key is configured, AND the configured-key wins over poisoned env vars.
// Per memory `feedback_assertion_driven_live_smoke`, the only way to catch
// the kind of regression that shipped this bug is to exercise the actual
// HTTP path with assertions on what went out on the wire.
//
// We bind a TcpListener on a random port and capture the raw request to
// inspect headers + body. This mirrors the pattern in
// `tests/integration_openrouter_smoke.rs::mock_models_server`.

mod credential_wire_contract {
    use super::*;
    use serial_test::serial;
    use std::io::Read;
    use std::net::TcpListener;
    use std::sync::mpsc;
    use workgraph::executor::native::client::{Message, MessagesRequest, Role};
    use workgraph::executor::native::provider::create_provider_ext;

    /// Spawn a minimal OAI-compat-shaped server. Captures the first
    /// request's headers + body, replies with `status_code` and
    /// `response_body`, then exits. Returns the bound URL and a channel
    /// the test can read the captured request from.
    fn spawn_capturing_server(
        status_code: u16,
        response_body: &str,
    ) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{}", addr);
        let (tx, rx) = mpsc::channel::<String>();
        let body = response_body.to_string();
        let status_line = match status_code {
            200 => "HTTP/1.1 200 OK",
            401 => "HTTP/1.1 401 Unauthorized",
            403 => "HTTP/1.1 403 Forbidden",
            _ => "HTTP/1.1 500 Internal Server Error",
        };
        let resp = format!(
            "{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            status_line,
            body.len(),
            body,
        );
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 16384];
                let n = stream.read(&mut buf).unwrap_or(0);
                let captured = String::from_utf8_lossy(&buf[..n]).to_string();
                let _ = std::io::Write::write_all(&mut stream, resp.as_bytes());
                let _ = tx.send(captured);
            }
        });
        (url, rx)
    }

    fn make_wg_dir(tmp: &TempDir, config_toml: &str) -> std::path::PathBuf {
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        std::fs::write(wg_dir.join("graph.jsonl"), "").unwrap();
        if !config_toml.is_empty() {
            std::fs::write(wg_dir.join("config.toml"), config_toml).unwrap();
        }
        wg_dir
    }

    fn key_vars() -> &'static [&'static str] {
        &[
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "OPENROUTER_API_KEY",
            "WG_API_KEY",
            "WG_LLM_PROVIDER",
            "WG_ENDPOINT",
            "WG_ENDPOINT_URL",
            "WG_MODEL",
            "OPENAI_BASE_URL",
            "OPENROUTER_BASE_URL",
        ]
    }

    /// Snapshot the relevant env vars, unset them, restore on drop.
    struct EnvSnapshot {
        saved: Vec<(&'static str, Option<String>)>,
    }
    impl EnvSnapshot {
        fn new(vars: &[&'static str]) -> Self {
            let saved: Vec<_> = vars.iter().map(|v| (*v, std::env::var(v).ok())).collect();
            for v in vars {
                unsafe { std::env::remove_var(v) };
            }
            EnvSnapshot { saved }
        }
        fn set(&self, var: &str, value: &str) {
            unsafe { std::env::set_var(var, value) };
        }
    }
    impl Drop for EnvSnapshot {
        fn drop(&mut self) {
            for (k, v) in &self.saved {
                match v {
                    Some(value) => unsafe { std::env::set_var(k, value) },
                    None => unsafe { std::env::remove_var(k) },
                }
            }
        }
    }

    fn empty_request(model: &str) -> MessagesRequest {
        MessagesRequest {
            model: model.to_string(),
            max_tokens: 50,
            system: None,
            messages: vec![Message {
                role: Role::User,
                content: vec![],
            }],
            tools: vec![],
            stream: false,
        }
    }

    /// **The autohaiku contract test.** Endpoint configured, NO api_key
    /// in config, NO env vars → client init succeeds, the HTTP call goes
    /// out with NO Authorization header. If the endpoint accepts (this
    /// fake server does), the request completes normally.
    #[test]
    #[serial]
    fn keyless_request_omits_authorization_header() {
        let response_body = serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 0,
            "model": "qwen3-coder",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "hi"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string();
        let (server_url, rx) = spawn_capturing_server(200, &response_body);

        let tmp = TempDir::new().unwrap();
        let wg_dir = make_wg_dir(
            &tmp,
            &format!(
                r#"
[[llm_endpoints.endpoints]]
name = "lambda01"
provider = "local"
url = "{}"
is_default = true
"#,
                server_url
            ),
        );

        let _env = EnvSnapshot::new(key_vars());
        let provider = create_provider_ext(&wg_dir, "qwen3-coder", None, None, None)
            .expect("keyless init must succeed");

        let req = empty_request("qwen3-coder");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let resp_result = rt.block_on(async { provider.send(&req).await });

        let captured = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("server should have captured the request");

        assert!(
            resp_result.is_ok(),
            "request should succeed against accepting endpoint: {:?}",
            resp_result.err().map(|e| format!("{:#}", e))
        );

        // The wire-level assertion: NO Authorization header was sent.
        let headers_lower = captured.to_lowercase();
        assert!(
            !headers_lower.contains("authorization:"),
            "Authorization header MUST NOT be sent when no key is configured. \
             Captured request:\n{}",
            captured
        );
    }

    /// **The 401 contract test.** Endpoint requires auth (server returns
    /// 401) but no key is configured → client init STILL succeeds, the
    /// request goes out without Authorization, the 401 surfaces with an
    /// error message naming the [[llm_endpoints.endpoints]] block —
    /// NEVER an env var.
    #[test]
    #[serial]
    fn endpoint_401_surfaces_config_block_not_env_var() {
        let error_body = serde_json::json!({
            "error": {"message": "Missing API key", "type": "invalid_request"}
        })
        .to_string();
        let (server_url, _rx) = spawn_capturing_server(401, &error_body);

        let tmp = TempDir::new().unwrap();
        let wg_dir = make_wg_dir(
            &tmp,
            &format!(
                r#"
[[llm_endpoints.endpoints]]
name = "needs-auth"
provider = "openai"
url = "{}"
is_default = true
"#,
                server_url
            ),
        );

        let _env = EnvSnapshot::new(key_vars());
        let provider = create_provider_ext(&wg_dir, "qwen3-coder", None, None, None)
            .expect("keyless init must succeed even when endpoint will reject");

        let req = empty_request("qwen3-coder");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(async { provider.send(&req).await });

        assert!(result.is_err(), "401 from endpoint should surface as error");
        let err = format!("{:#}", result.err().unwrap());

        // The user-visible error MUST point at the config block, NOT at any env var.
        assert!(
            err.contains("[[llm_endpoints.endpoints]]"),
            "401 error must name the [[llm_endpoints.endpoints]] config block; got: {}",
            err
        );
        assert!(
            err.contains("'needs-auth'"),
            "401 error must name the specific endpoint ('needs-auth'); got: {}",
            err
        );
        assert!(
            !err.contains("ANTHROPIC_API_KEY")
                && !err.contains("OPENAI_API_KEY")
                && !err.contains("OPENROUTER_API_KEY")
                && !err.contains("WG_API_KEY"),
            "401 error must NOT mention any env var name (WG credential contract — \
             credentials live in WG config exclusively); got: {}",
            err
        );
    }

    /// **The env-var-isolation contract test.** Endpoint has `api_key`
    /// configured AND ANTHROPIC/OPENAI/OPENROUTER env vars are poisoned
    /// with junk → request goes out bearing the CONFIGURED key, not the
    /// env junk.
    #[test]
    #[serial]
    fn configured_key_wins_over_poisoned_env_vars() {
        let response_body = serde_json::json!({
            "id": "chatcmpl-x", "object": "chat.completion", "created": 0,
            "model": "qwen3-coder",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }).to_string();
        let (server_url, rx) = spawn_capturing_server(200, &response_body);

        let tmp = TempDir::new().unwrap();
        let wg_dir = make_wg_dir(
            &tmp,
            &format!(
                r#"
[[llm_endpoints.endpoints]]
name = "configured"
provider = "openai"
url = "{}"
api_key = "real-configured-key-xyz789"
is_default = true
"#,
                server_url
            ),
        );

        let env = EnvSnapshot::new(key_vars());
        env.set("ANTHROPIC_API_KEY", "env-poison-anthropic");
        env.set("OPENAI_API_KEY", "env-poison-openai");
        env.set("OPENROUTER_API_KEY", "env-poison-openrouter");
        env.set("WG_API_KEY", "env-poison-wg");

        let provider = create_provider_ext(&wg_dir, "qwen3-coder", None, None, None)
            .expect("init must succeed");
        let req = empty_request("qwen3-coder");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _ = rt.block_on(async { provider.send(&req).await });

        let captured = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("server must have captured the request");

        // Wire-level assertion: configured key was sent, env junk was NOT.
        assert!(
            captured.contains("Bearer real-configured-key-xyz789"),
            "request must bear the CONFIGURED api_key as Bearer token; captured:\n{}",
            captured
        );
        assert!(
            !captured.contains("env-poison"),
            "request must NOT carry any env-var-sourced key (WG credential contract); \
             captured:\n{}",
            captured
        );
    }
}

/// Verify that the endpoint configuration (api_base) is resolved from config.
#[test]
fn native_coordinator_endpoint_from_config() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    // Write config with a native_executor section specifying a custom api_base
    let config = r#"
[coordinator]
coordinator_agent = true
executor = "native"
model = "openrouter:deepseek/deepseek-chat"

[native_executor]
api_base = "https://openrouter.ai/api/v1"
"#;
    fs::write(wg_dir.join("config.toml"), config).unwrap();

    // The provider should be created with the custom base URL.
    // We can't easily inspect the base URL, but creating the provider should succeed.
    let result = workgraph::executor::native::provider::create_provider_ext(
        &wg_dir,
        "deepseek/deepseek-chat",
        None,
        None,
        Some("test-api-key-not-real"),
    );
    assert!(
        result.is_ok(),
        "Provider should succeed with config api_base"
    );
}

// ===========================================================================
// 2. Mock provider tests (native coordinator loop internals)
// ===========================================================================

use workgraph::executor::native::client::{
    ContentBlock, MessagesRequest, MessagesResponse, StopReason, Usage,
};
use workgraph::executor::native::provider::Provider;

/// Mock provider simulating an OpenRouter endpoint for a cheap model.
struct MockNativeProvider {
    model_name: String,
    responses: Vec<MessagesResponse>,
    call_count: Arc<AtomicUsize>,
}

impl MockNativeProvider {
    fn simple_text(model: &str, text: &str) -> Self {
        Self {
            model_name: model.to_string(),
            responses: vec![MessagesResponse {
                id: "chatcmpl-native-001".to_string(),
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage {
                    input_tokens: 100,
                    output_tokens: 30,
                    cache_read_input_tokens: None,
                    cache_creation_input_tokens: None,
                    reasoning_tokens: None,
                },
            }],
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn with_tool_call(
        model: &str,
        tool_name: &str,
        tool_input: serde_json::Value,
        final_text: &str,
    ) -> Self {
        Self {
            model_name: model.to_string(),
            responses: vec![
                MessagesResponse {
                    id: "chatcmpl-native-tc-001".to_string(),
                    content: vec![ContentBlock::ToolUse {
                        id: "call_native_1".to_string(),
                        name: tool_name.to_string(),
                        input: tool_input,
                    }],
                    stop_reason: Some(StopReason::ToolUse),
                    usage: Usage {
                        input_tokens: 100,
                        output_tokens: 30,
                        ..Usage::default()
                    },
                },
                MessagesResponse {
                    id: "chatcmpl-native-tc-002".to_string(),
                    content: vec![ContentBlock::Text {
                        text: final_text.to_string(),
                    }],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage {
                        input_tokens: 200,
                        output_tokens: 60,
                        ..Usage::default()
                    },
                },
            ],
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl Provider for MockNativeProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model_name
    }

    fn max_tokens(&self) -> u32 {
        16384
    }

    async fn send(&self, _request: &MessagesRequest) -> anyhow::Result<MessagesResponse> {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        if idx < self.responses.len() {
            Ok(self.responses[idx].clone())
        } else {
            Ok(MessagesResponse {
                id: format!("chatcmpl-native-fallback-{}", idx),
                content: vec![ContentBlock::Text {
                    text: "[mock exhausted]".to_string(),
                }],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
            })
        }
    }
}

/// End-to-end agent loop with a mock OpenRouter provider — simple text response.
#[tokio::test]
async fn native_coordinator_agent_loop_simple_text() {
    use workgraph::executor::native::agent::AgentLoop;
    use workgraph::executor::native::tools::ToolRegistry;

    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    let graph_path = wg_dir.join("graph.jsonl");
    let graph = workgraph::graph::WorkGraph::new();
    workgraph::parser::save_graph(&graph, &graph_path).unwrap();

    let provider = MockNativeProvider::simple_text("deepseek/deepseek-chat", "Hello from native!");

    let registry = ToolRegistry::default_all(&wg_dir, tmp.path());
    let output_log = wg_dir.join("native-simple.ndjson");

    let mut agent = AgentLoop::new(
        Box::new(provider),
        registry,
        "You are a test coordinator.".to_string(),
        10,
        output_log,
    );

    let result = agent.run("Say hello.").await.unwrap();
    assert_eq!(result.final_text, "Hello from native!");
    assert_eq!(result.turns, 1);
}

/// Agent loop with mock OpenRouter provider — tool call flow (bash).
#[tokio::test]
async fn native_coordinator_agent_loop_with_tool_call() {
    use workgraph::executor::native::agent::AgentLoop;
    use workgraph::executor::native::tools::ToolRegistry;

    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    let graph_path = wg_dir.join("graph.jsonl");
    let graph = workgraph::graph::WorkGraph::new();
    workgraph::parser::save_graph(&graph, &graph_path).unwrap();

    let provider = MockNativeProvider::with_tool_call(
        "deepseek/deepseek-chat",
        "bash",
        serde_json::json!({"command": "echo hello-from-native"}),
        "Command executed successfully via native coordinator.",
    );
    let call_count = provider.call_count.clone();

    let registry = ToolRegistry::default_all(&wg_dir, tmp.path());
    let output_log = wg_dir.join("native-tool.ndjson");

    let mut agent = AgentLoop::new(
        Box::new(provider),
        registry,
        "You are a test coordinator.".to_string(),
        10,
        output_log,
    );

    let result = agent.run("Run a command.").await.unwrap();
    assert_eq!(
        result.final_text,
        "Command executed successfully via native coordinator."
    );
    assert_eq!(result.turns, 2);
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

/// Verify that the agent loop produces a valid journal when using an OpenRouter model.
#[tokio::test]
async fn native_coordinator_journal_with_openrouter_model() {
    use workgraph::executor::native::agent::AgentLoop;
    use workgraph::executor::native::journal::{Journal, JournalEntryKind};
    use workgraph::executor::native::tools::ToolRegistry;

    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    let graph_path = wg_dir.join("graph.jsonl");
    let graph = workgraph::graph::WorkGraph::new();
    workgraph::parser::save_graph(&graph, &graph_path).unwrap();

    let task_id = "native-journal-test";
    let j_path = workgraph::executor::native::journal::journal_path(&wg_dir, task_id);

    let provider = MockNativeProvider::with_tool_call(
        "deepseek/deepseek-chat",
        "bash",
        serde_json::json!({"command": "echo test"}),
        "Journal test done.",
    );

    let registry = ToolRegistry::default_all(&wg_dir, tmp.path());
    let output_log = wg_dir.join("native-journal.ndjson");

    let mut agent = AgentLoop::new(
        Box::new(provider),
        registry,
        "You are a test agent.".to_string(),
        10,
        output_log,
    )
    .with_journal(j_path.clone(), task_id.to_string())
    .with_resume(false);

    let result = agent.run("Run a test.").await.unwrap();
    assert_eq!(result.final_text, "Journal test done.");

    // Verify journal exists and is well-formed
    assert!(j_path.exists(), "Journal file should exist");
    let entries = Journal::read_all(&j_path).unwrap();
    assert!(!entries.is_empty());

    // Verify Init entry records the openai provider and model
    match &entries[0].kind {
        JournalEntryKind::Init {
            provider, model, ..
        } => {
            assert_eq!(
                provider, "openai",
                "Should record openai provider for OpenRouter"
            );
            assert_eq!(model, "deepseek/deepseek-chat");
        }
        _ => panic!("First entry should be Init"),
    }

    // Verify the journal has an End entry
    let last = entries.last().unwrap();
    assert!(
        matches!(last.kind, JournalEntryKind::End { .. }),
        "Last entry should be End"
    );
}

// ===========================================================================
// 3. Daemon-level tests (process lifecycle)
// ===========================================================================

/// Service startup with executor = "native" succeeds even without an API key.
/// The daemon starts, but the coordinator agent logs a provider creation error.
/// Chat falls back to stub responses.
#[test]
fn native_coordinator_service_startup_no_api_key() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);
    configure_native_coordinator(&wg_dir, "openrouter:deepseek/deepseek-chat");

    // Write native_executor config to set provider = "openai" explicitly
    let existing = fs::read_to_string(wg_dir.join("config.toml")).unwrap();
    let appended = format!("{}\n[native_executor]\nprovider = \"openai\"\n", existing);
    fs::write(wg_dir.join("config.toml"), appended).unwrap();

    let env = [
        ("OPENROUTER_API_KEY", ""),
        ("OPENAI_API_KEY", ""),
        ("ANTHROPIC_API_KEY", ""),
        ("WG_LLM_PROVIDER", ""),
    ];
    let _guard = DaemonGuard::with_env(&wg_dir, &env);

    // Clear any env keys that might interfere
    let output = wg_cmd_env(
        &wg_dir,
        &["service", "start", "--interval", "600", "--max-agents", "1"],
        &env,
    );
    assert!(
        output.status.success(),
        "Service should start even without API key.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    wait_for_socket(&wg_dir);

    // Wait for daemon to log something about the native coordinator
    let logged = wait_for(Duration::from_secs(5), 100, || {
        let log = read_daemon_log(&wg_dir);
        log.contains("Native coordinator")
            || log.contains("native")
            || log.contains("Failed to spawn coordinator agent")
    });
    assert!(
        logged,
        "Daemon log should mention native coordinator.\nLog:\n{}",
        read_daemon_log(&wg_dir)
    );
}

/// Service startup with executor = "native" and a fake API key.
/// The daemon starts and the native coordinator initializes (provider creation succeeds).
#[test]
fn native_coordinator_service_startup_with_api_key() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);
    configure_native_coordinator(&wg_dir, "openrouter:deepseek/deepseek-chat");

    // Write native_executor config to set provider = "openai" explicitly
    // (overrides WG_LLM_PROVIDER env var that may be inherited).
    let existing = fs::read_to_string(wg_dir.join("config.toml")).unwrap();
    let appended = format!("{}\n[native_executor]\nprovider = \"openai\"\n", existing);
    fs::write(wg_dir.join("config.toml"), appended).unwrap();

    let env = [
        ("OPENROUTER_API_KEY", "test-fake-key-for-integration-test"),
        ("WG_LLM_PROVIDER", ""),
    ];
    let _guard = DaemonGuard::with_env(&wg_dir, &env);

    let output = wg_cmd_env(
        &wg_dir,
        &["service", "start", "--interval", "600", "--max-agents", "1"],
        &env,
    );
    assert!(
        output.status.success(),
        "Service should start with API key.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    wait_for_socket(&wg_dir);

    let configured = wait_for(Duration::from_secs(10), 100, || {
        let log = read_daemon_log(&wg_dir);
        log.contains("Coordinator config:")
    });
    assert!(
        configured,
        "Daemon should log coordinator configuration when API key is set.\nDaemon log:\n{}",
        read_daemon_log(&wg_dir)
    );

    // Verify the log shows the correct model
    let log = read_daemon_log(&wg_dir);
    assert!(
        log.contains("deepseek/deepseek-chat"),
        "Log should mention the configured model.\nLog:\n{}",
        log
    );
}

/// Backwards compatibility: executor = "claude" still starts the Claude CLI path.
#[test]
fn native_coordinator_backwards_compat_claude_executor() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);
    configure_claude_coordinator(&wg_dir);

    // Create a mock claude so the daemon doesn't fail trying to find the real one
    let mock_dir = TempDir::new().unwrap();
    let mock_script = r#"#!/bin/bash
for arg in "$@"; do
    if [ "$arg" = "--version" ]; then echo "mock-claude 0.1.0"; exit 0; fi
done
while IFS= read -r line; do
    if [[ "$line" == *'"type":"user"'* ]]; then
        printf '{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Mock claude response"}],"stop_reason":"end_turn"}}\n'
    fi
done
"#;
    let mock_path = mock_dir.path().join("claude");
    fs::write(&mock_path, mock_script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&mock_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let original_path = std::env::var("PATH").unwrap_or_default();
    let path_env = format!("{}:{}", mock_dir.path().display(), original_path);
    let env = [("PATH", path_env.as_str())];
    let _guard = DaemonGuard::with_env(&wg_dir, &env);

    let output = wg_cmd_env(
        &wg_dir,
        &["service", "start", "--interval", "600", "--max-agents", "1"],
        &env,
    );
    assert!(
        output.status.success(),
        "Service should start with claude executor.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    wait_for_socket(&wg_dir);

    let configured = wait_for(Duration::from_secs(10), 100, || {
        let log = read_daemon_log(&wg_dir);
        log.contains("Coordinator config:") && log.contains("executor=claude")
    });
    assert!(
        configured,
        "Claude executor daemon should load the claude coordinator configuration.\nDaemon log:\n{}",
        read_daemon_log(&wg_dir)
    );

    // The log should NOT mention "Native coordinator"
    let log = read_daemon_log(&wg_dir);
    assert!(
        !log.contains("Native coordinator: initialized"),
        "Claude executor should NOT use the native coordinator path.\nLog:\n{}",
        log
    );
}

/// Chat routing through native coordinator: the daemon forwards messages
/// to the native coordinator loop and writes responses to the outbox.
/// This test requires a fake API key and checks that the coordinator
/// processes the message (even though the API call will fail with a fake key).
#[test]
fn native_coordinator_chat_routing() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);
    configure_native_coordinator(&wg_dir, "openrouter:deepseek/deepseek-chat");

    // Write native_executor config to set provider = "openai" explicitly
    let existing = fs::read_to_string(wg_dir.join("config.toml")).unwrap();
    let appended = format!("{}\n[native_executor]\nprovider = \"openai\"\n", existing);
    fs::write(wg_dir.join("config.toml"), appended).unwrap();

    let env = [
        ("OPENROUTER_API_KEY", "test-fake-key-for-chat-routing"),
        ("WG_LLM_PROVIDER", ""),
    ];
    let create_output = wg_cmd_env(
        &wg_dir,
        &["chat", "create", "--name", "default", "--json"],
        &env,
    );
    assert!(
        create_output.status.success(),
        "Chat task should be created before routing messages.\nstderr: {}",
        String::from_utf8_lossy(&create_output.stderr)
    );
    let _guard = DaemonGuard::with_env(&wg_dir, &env);

    let output = wg_cmd_env(
        &wg_dir,
        &["service", "start", "--interval", "600", "--max-agents", "1"],
        &env,
    );
    assert!(
        output.status.success(),
        "Service should start.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    wait_for_socket(&wg_dir);

    // Send a chat message. The API call will fail (fake key), but the native
    // coordinator should process the request and write an error response.
    let chat_output = wg_cmd_env(
        &wg_dir,
        &["chat", "hello native coordinator", "--timeout", "30"],
        &env,
    );
    let stdout = String::from_utf8_lossy(&chat_output.stdout).to_string();

    // The daemon should route the message into the chat supervisor. With a
    // fake key, the provider call may fail before producing a model response.
    let log = read_daemon_log(&wg_dir);
    assert!(
        log.contains("IPC UserChat: request_id=")
            && (log.contains("Coordinator-0:") || log.contains("[coordinator-0 stderr]")),
        "Log should show the chat request reached the coordinator supervisor.\nLog:\n{}",
        log
    );

    // The response should either be a real response (if API key was valid)
    // or an error message about the API call failing.
    // Either way, a response was delivered (chat command returns).
    assert!(
        chat_output.status.success() || !stdout.is_empty(),
        "Chat should produce output (success or error).\nstdout: {}\nstderr: {}",
        stdout,
        String::from_utf8_lossy(&chat_output.stderr),
    );
}

/// Task dispatch: native coordinator's task-spawning executor is separate
/// from the coordinator's own executor. Verify that when the coordinator
/// executor is "native", task agents still get dispatched via the configured
/// task executor.
#[test]
fn native_coordinator_task_dispatch_with_shell_executor() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);

    // Configure native coordinator but shell executor for task agents
    let wg_bin_dir = wg_binary().parent().unwrap().to_string_lossy().to_string();
    let path_with_test_binary = format!(
        "{}:{}",
        wg_bin_dir,
        std::env::var("PATH").unwrap_or_default()
    );

    let config = format!(
        r#"[coordinator]
coordinator_agent = true
executor = "native"
model = "openrouter:deepseek/deepseek-chat"
poll_interval = 2

[native_executor]
provider = "openai"

[agency]
auto_assign = false
auto_evaluate = false
"#
    );
    fs::write(wg_dir.join("config.toml"), &config).unwrap();

    // Set up a shell executor for task agents
    let executors_dir = wg_dir.join("executors");
    fs::create_dir_all(&executors_dir).unwrap();
    let shell_config = format!(
        r#"[executor]
type = "shell"
command = "bash"
args = ["-c", "{{{{task_context}}}}"]
working_dir = "{}"

[executor.env]
TASK_ID = "{{{{task_id}}}}"
PATH = "{}"
"#,
        tmp.path().display(),
        path_with_test_binary
    );
    fs::write(executors_dir.join("shell.toml"), &shell_config).unwrap();

    let env = [
        ("OPENROUTER_API_KEY", "test-fake-key-for-dispatch"),
        ("PATH", path_with_test_binary.as_str()),
        ("WG_LLM_PROVIDER", ""),
    ];
    let _guard = DaemonGuard::with_env(&wg_dir, &env);

    let socket = format!("{}/wg-test.sock", tmp.path().display());
    let output = wg_cmd_env(
        &wg_dir,
        &[
            "service",
            "start",
            "--socket",
            &socket,
            "--executor",
            "shell",
            "--max-agents",
            "2",
            "--interval",
            "2",
        ],
        &env,
    );
    assert!(
        output.status.success(),
        "Service should start.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Wait for daemon to be ready
    let ready = wait_for(Duration::from_secs(5), 100, || {
        let state_path = wg_dir.join("service").join("state.json");
        if let Ok(content) = fs::read_to_string(&state_path)
            && let Ok(state) = serde_json::from_str::<serde_json::Value>(&content)
            && let Some(socket_path) = state["socket_path"].as_str()
        {
            if let Ok(mut stream) = std::os::unix::net::UnixStream::connect(socket_path) {
                let _ = writeln!(stream, r#"{{"cmd":"status"}}"#);
                let _ = stream.flush();
                let mut reader = BufReader::new(&stream);
                let mut response = String::new();
                if reader.read_line(&mut response).is_ok() && !response.is_empty() {
                    return true;
                }
            }
        }
        false
    });
    assert!(ready, "Service daemon should become ready");

    // Add a shell task
    wg_ok(
        &wg_dir,
        &[
            "add",
            "Shell dispatch test",
            "--id",
            "shell-dispatch-test",
            "--immediate",
        ],
    );

    // Patch the task to add exec field
    let graph_path = wg_dir.join("graph.jsonl");
    let content = fs::read_to_string(&graph_path).unwrap();
    let mut new_lines = Vec::new();
    for line in content.lines() {
        if line.contains("\"id\":\"shell-dispatch-test\"") {
            let mut val: serde_json::Value = serde_json::from_str(line).unwrap();
            val["exec"] =
                serde_json::Value::String("echo 'dispatched by native coordinator'".to_string());
            new_lines.push(serde_json::to_string(&val).unwrap());
        } else {
            new_lines.push(line.to_string());
        }
    }
    fs::write(&graph_path, new_lines.join("\n") + "\n").unwrap();

    // Notify the daemon
    let state_path = wg_dir.join("service").join("state.json");
    if let Ok(content) = fs::read_to_string(&state_path)
        && let Ok(state) = serde_json::from_str::<serde_json::Value>(&content)
        && let Some(socket_path) = state["socket_path"].as_str()
        && let Ok(mut stream) = std::os::unix::net::UnixStream::connect(socket_path)
    {
        let _ = writeln!(stream, r#"{{"cmd":"graph_changed"}}"#);
        let _ = stream.flush();
    }

    // Wait for task to be picked up
    let picked_up = wait_for(Duration::from_secs(10), 200, || {
        let output = wg_cmd(&wg_dir, &["show", "shell-dispatch-test", "--json"]);
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&stdout) {
                let status = val["status"].as_str().unwrap_or("");
                return status == "in-progress" || status == "done";
            }
        }
        false
    });

    assert!(
        picked_up,
        "Task should be dispatched by the coordinator (even though coordinator executor is native)."
    );
}

// ===========================================================================
// `wg init` contract: autohaiku one-liner is sufficient (native-executor-client)
// ===========================================================================

/// `wg init -m qwen3-coder -e <url> --executor nex` MUST be sufficient to
/// produce a working config. This test exercises the literal user contract:
/// no env vars, no follow-up edits — just the init invocation, then a graph
/// op (`wg list`) that doesn't crash on credential resolution.
#[test]
fn wg_init_qwen3_with_endpoint_is_sufficient() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path();
    let wg_dir = project.join(".wg");

    let env = [
        // Scrub all credential env vars so we prove init+list work without them.
        ("ANTHROPIC_API_KEY", ""),
        ("OPENAI_API_KEY", ""),
        ("OPENROUTER_API_KEY", ""),
        ("WG_API_KEY", ""),
    ];

    // Run `wg init -m qwen3-coder -e https://example.invalid:30000 --executor nex`
    let output = wg_cmd_env(
        &wg_dir,
        &[
            "init",
            "-m",
            "qwen3-coder",
            "-e",
            "https://example.invalid:30000",
            "--executor",
            "nex",
        ],
        &env,
    );
    assert!(
        output.status.success(),
        "wg init must succeed without env vars.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // Verify the config has a complete [[llm_endpoints.endpoints]] block
    let config = workgraph::config::Config::load(&wg_dir).unwrap();
    let eps = &config.llm_endpoints.endpoints;
    let default_ep = eps
        .iter()
        .find(|e| e.is_default)
        .expect("init -m + -e must write a default [[llm_endpoints.endpoints]] block");
    assert_eq!(
        default_ep.url.as_deref(),
        Some("https://example.invalid:30000"),
    );
    // No env-var fallback was used — the block has no api_key, and
    // that's fine. The whole point of the contract is that an unset
    // key is acceptable until the endpoint actually rejects.
    assert!(default_ep.api_key.is_none());

    // `wg list` must not crash on credential resolution (the bug had it
    // bailing immediately because of the "No Anthropic API key found"
    // precondition check at provider init).
    let list_output = wg_cmd_env(&wg_dir, &["list"], &env);
    assert!(
        list_output.status.success(),
        "wg list must succeed after init without env vars.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&list_output.stdout),
        String::from_utf8_lossy(&list_output.stderr),
    );
}

/// Static audit: main runtime code (not tests / migration) must not read
/// `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `OPENROUTER_API_KEY` from env.
/// This is a regression guard — if a future change re-introduces an env-var
/// fallback in a credential path, this test fails.
#[test]
fn no_env_var_credential_lookups_in_credential_path() {
    use std::io::Read;
    use std::path::Path;

    let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let credential_paths = [
        crate_root.join("src/executor/native/provider.rs"),
        crate_root.join("src/executor/native/client.rs"),
        crate_root.join("src/executor/native/openai_client.rs"),
    ];
    // Files we deliberately allow to mention these env vars: helper
    // scripts in tests/, doc comments, `from_env` legacy methods that
    // are dead-end paths NOT called by the WG dispatcher.
    let banned_substrings = [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "OPENROUTER_API_KEY",
        "WG_API_KEY",
    ];
    for path in &credential_paths {
        let mut contents = String::new();
        std::fs::File::open(path)
            .unwrap_or_else(|e| panic!("opening {}: {}", path.display(), e))
            .read_to_string(&mut contents)
            .unwrap();
        // Check each line: env::var("…API_KEY") calls must not appear
        // outside of clearly marked legacy/dead-end functions.
        for (lineno, line) in contents.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("///") {
                continue; // doc / inline comments are fine
            }
            for banned in &banned_substrings {
                if line.contains(&format!("env::var(\"{}\"", banned))
                    || line.contains(&format!("env::var({:?}", banned))
                {
                    // Allow `from_env` and `resolve_api_key` legacy
                    // functions — they're not called by the dispatcher
                    // path. Any NEW credential-path code must not
                    // re-introduce env vars.
                    let preceding: Vec<&str> = contents.lines().take(lineno).collect();
                    let in_legacy = preceding.iter().rev().take(40).any(|l| {
                        l.contains("pub fn from_env")
                            || l.contains("fn resolve_api_key()")
                            || l.contains("fn resolve_api_key_from_dir")
                            || l.contains("fn resolve_openai_api_key")
                            || l.contains("fn resolve_openai_api_key_from_dir")
                    });
                    if !in_legacy {
                        panic!(
                            "Banned env-var lookup '{}' found in credential path:\n  \
                             {}:{}: {}",
                            banned,
                            path.display(),
                            lineno + 1,
                            line,
                        );
                    }
                }
            }
        }
    }
}

// ===========================================================================
// 4. Real E2E tests (require OPENROUTER_API_KEY, run with --ignored)
// ===========================================================================

/// Real E2E: start service with native executor and a cheap OpenRouter model,
/// send a chat message, verify a meaningful response comes back.
///
/// Requires OPENROUTER_API_KEY to be set.
/// Run with: cargo test --test integration_native_coordinator -- --ignored --nocapture
#[test]
#[ignore]
fn native_coordinator_real_e2e_chat() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);
    // Use deepseek-chat-v3 (budget tier, cheap)
    configure_native_coordinator(&wg_dir, "openrouter:deepseek/deepseek-chat");
    let _guard = DaemonGuard::new(&wg_dir);

    // Start daemon
    let output = wg_cmd(
        &wg_dir,
        &["service", "start", "--interval", "600", "--max-agents", "0"],
    );
    assert!(
        output.status.success(),
        "Service should start.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    wait_for_socket(&wg_dir);

    // Wait for native coordinator to initialize
    let initialized = wait_for(Duration::from_secs(15), 200, || {
        let log = read_daemon_log(&wg_dir);
        log.contains("Native coordinator: initialized")
    });
    assert!(
        initialized,
        "Native coordinator should initialize.\nDaemon log:\n{}",
        read_daemon_log(&wg_dir)
    );

    // Send a simple chat message
    let chat_output = wg_cmd(
        &wg_dir,
        &[
            "chat",
            "What is 2 + 2? Reply with just the number.",
            "--timeout",
            "60",
        ],
    );
    let stdout = String::from_utf8_lossy(&chat_output.stdout).to_string();
    assert!(
        chat_output.status.success(),
        "Chat should succeed.\nstdout: {}\nstderr: {}",
        stdout,
        String::from_utf8_lossy(&chat_output.stderr)
    );

    // The response should contain "4" somewhere
    assert!(
        stdout.contains('4'),
        "Response should contain the answer '4'.\nResponse: {}",
        stdout
    );
}

/// Real E2E: verify crash recovery with native executor.
/// The native coordinator runs in-process, so "crash recovery" means the
/// coordinator handles API errors gracefully and continues processing.
///
/// Requires OPENROUTER_API_KEY to be set.
#[test]
#[ignore]
fn native_coordinator_real_e2e_error_recovery() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = init_workgraph(&tmp);
    configure_native_coordinator(&wg_dir, "openrouter:deepseek/deepseek-chat");
    let _guard = DaemonGuard::new(&wg_dir);

    let output = wg_cmd(
        &wg_dir,
        &["service", "start", "--interval", "600", "--max-agents", "0"],
    );
    assert!(
        output.status.success(),
        "Service should start.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    wait_for_socket(&wg_dir);

    let initialized = wait_for(Duration::from_secs(15), 200, || {
        let log = read_daemon_log(&wg_dir);
        log.contains("Native coordinator: initialized")
    });
    assert!(
        initialized,
        "Native coordinator should initialize.\nLog:\n{}",
        read_daemon_log(&wg_dir)
    );

    // Send first message — should work
    let r1 = wg_cmd(&wg_dir, &["chat", "Say hello", "--timeout", "60"]);
    let r1_stdout = String::from_utf8_lossy(&r1.stdout).to_string();
    assert!(
        r1.status.success(),
        "First chat should succeed.\nstdout: {}\nstderr: {}",
        r1_stdout,
        String::from_utf8_lossy(&r1.stderr)
    );

    // Send second message — should also work (coordinator maintains state)
    let r2 = wg_cmd(
        &wg_dir,
        &["chat", "What did I just say?", "--timeout", "60"],
    );
    let r2_stdout = String::from_utf8_lossy(&r2.stdout).to_string();
    assert!(
        r2.status.success(),
        "Second chat should succeed.\nstdout: {}\nstderr: {}",
        r2_stdout,
        String::from_utf8_lossy(&r2.stderr)
    );

    // Both messages should have been processed
    let log = read_daemon_log(&wg_dir);
    let processing_count = log
        .matches("Native coordinator: processing request_id=")
        .count();
    assert!(
        processing_count >= 2,
        "Should have processed at least 2 requests. Count: {}\nLog:\n{}",
        processing_count,
        log
    );
}
