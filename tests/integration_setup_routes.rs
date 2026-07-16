//! Integration tests for the 5-route `wg setup` / `wg init` flow and the
//! `wg config reset` command. Validation criteria from
//! `wg-setup-5-smooth-2`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use worksgood::config::{Config, DispatchRole};
use worksgood::config_defaults::{RouteParams, SetupRoute, config_for_route};

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

fn run_wg_in_isolation(fake_home: &Path, args: &[&str]) -> std::process::Output {
    run_wg_in_isolation_with_env(fake_home, args, &[])
}

fn run_wg_in_isolation_with_env(
    fake_home: &Path,
    args: &[&str],
    extra_env: &[(&str, &str)],
) -> std::process::Output {
    let mut cmd = Command::new(wg_binary());
    cmd.args(args);
    cmd.env("HOME", fake_home);
    cmd.env_remove("ANTHROPIC_API_KEY");
    cmd.env_remove("OPENROUTER_API_KEY");
    cmd.env_remove("OPENAI_API_KEY");
    cmd.env_remove("WG_DIR");
    cmd.env_remove("WG_TASK_ID");
    cmd.env_remove("WG_AGENT_ID");
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.output()
        .unwrap_or_else(|e| panic!("Failed to run wg: {}", e))
}

fn run_wg_in_isolation_with_stdin(
    fake_home: &Path,
    args: &[&str],
    stdin_body: &str,
) -> std::process::Output {
    let mut cmd = Command::new(wg_binary());
    cmd.args(args);
    cmd.env("HOME", fake_home);
    cmd.env_remove("ANTHROPIC_API_KEY");
    cmd.env_remove("OPENROUTER_API_KEY");
    cmd.env_remove("OPENAI_API_KEY");
    cmd.env_remove("WG_DIR");
    cmd.env_remove("WG_TASK_ID");
    cmd.env_remove("WG_AGENT_ID");
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .unwrap_or_else(|e| panic!("Failed to spawn wg with stdin: {}", e));
    child
        .stdin
        .take()
        .expect("missing stdin")
        .write_all(stdin_body.as_bytes())
        .expect("write stdin");
    child
        .wait_with_output()
        .unwrap_or_else(|e| panic!("Failed waiting for wg stdin run: {}", e))
}

#[derive(Clone)]
struct Route {
    method: &'static str,
    path: &'static str,
    status: u16,
    body: &'static str,
}

fn request_complete(buf: &[u8]) -> bool {
    let Some(header_end) = buf.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&buf[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.strip_prefix("content-length:")
                .or_else(|| line.strip_prefix("Content-Length:"))
        })
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    buf.len() >= header_end + 4 + content_length
}

fn start_mock_server(
    routes: Vec<Route>,
) -> (
    String,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    std::thread::JoinHandle<()>,
) {
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let request_log = std::sync::Arc::clone(&requests);
    let expected = routes.len();
    let addr = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());

    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut served = 0usize;
        while served < expected && Instant::now() < deadline {
            let (mut stream, _) = match listener.accept() {
                Ok(stream) => stream,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(_) => break,
            };
            served += 1;
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();

            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if request_complete(&buf) {
                            break;
                        }
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        break;
                    }
                    Err(_) => break,
                }
            }

            let request = String::from_utf8_lossy(&buf).to_string();
            request_log.lock().unwrap().push(request.clone());
            let request_line = request.lines().next().unwrap_or_default();
            let mut parts = request_line.split_whitespace();
            let method = parts.next().unwrap_or_default();
            let path = parts.next().unwrap_or_default();
            let (status, body) = routes
                .iter()
                .find(|route| route.method == method && route.path == path)
                .map(|route| (route.status, route.body))
                .unwrap_or((404, r#"{"error":"not found"}"#));
            let reason = if status >= 400 { "ERR" } else { "OK" };
            let response = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status,
                reason,
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });

    (addr, requests, handle)
}

fn load_global_config(fake_home: &Path) -> Config {
    // Mirrors Config::global_dir resolution: prefer modern `~/.wg`, fall
    // back to legacy `~/.wg` if only that exists.
    let modern = fake_home.join(".wg/config.toml");
    let legacy = fake_home.join(".wg/config.toml");
    let path = if modern.exists() {
        modern
    } else if legacy.exists() {
        legacy
    } else {
        modern
    };
    let content = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read config at {:?}: {}", path, e));
    toml::from_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse config.toml:\n{}\nError: {}", content, e))
}

fn load_local_config(project_root: &Path) -> Config {
    let path = project_root.join(".wg/config.toml");
    let content = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read local config at {:?}: {}", path, e));
    toml::from_str(&content).unwrap_or_else(|e| {
        panic!(
            "Failed to parse local config.toml:\n{}\nError: {}",
            content, e
        )
    })
}

// ---------------------------------------------------------------------------
// Per-route config completeness — pure-Rust tests of config_for_route.
// (Same names as the validation checklist — also covered in lib unit tests.)
// ---------------------------------------------------------------------------

#[test]
fn test_route_openrouter_complete_config() {
    let cfg = config_for_route(SetupRoute::Openrouter, RouteParams::default());
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("native"));
    assert_eq!(cfg.agent.executor, "native");
    assert!(cfg.tiers.fast.is_some());
    assert!(cfg.tiers.standard.is_some());
    assert!(cfg.tiers.premium.is_some());
    assert_eq!(
        cfg.tiers.standard.as_deref(),
        Some("openrouter:anthropic/claude-sonnet-4-6")
    );
    assert_eq!(
        cfg.resolve_model_for_role(DispatchRole::TaskAgent).model,
        "anthropic/claude-opus-4-7"
    );
    assert_eq!(cfg.llm_endpoints.endpoints.len(), 1);
    assert_eq!(cfg.llm_endpoints.endpoints[0].provider, "openrouter");
    // Round-trip
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    let _: Config = toml::from_str(&toml_str).unwrap();
}

#[test]
fn test_route_claude_cli_complete_config() {
    let cfg = config_for_route(SetupRoute::ClaudeCli, RouteParams::default());
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("claude"));
    assert_eq!(cfg.agent.executor, "claude");
    assert!(cfg.tiers.fast.is_some());
    assert!(cfg.tiers.standard.is_some());
    assert!(cfg.tiers.premium.is_some());
    assert_eq!(cfg.tiers.standard.as_deref(), Some("claude:opus"));
    assert_eq!(
        cfg.resolve_model_for_role(DispatchRole::TaskAgent).model,
        "opus"
    );
    // Claude CLI doesn't need an endpoint.
    assert!(cfg.llm_endpoints.endpoints.is_empty());
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    let _: Config = toml::from_str(&toml_str).unwrap();
}

#[test]
fn test_route_codex_cli_complete_config() {
    let cfg = config_for_route(SetupRoute::CodexCli, RouteParams::default());
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("codex"));
    assert_eq!(cfg.agent.executor, "codex");
    assert!(cfg.tiers.fast.is_some());
    assert!(cfg.tiers.standard.is_some());
    assert!(cfg.tiers.premium.is_some());
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    let _: Config = toml::from_str(&toml_str).unwrap();
}

#[test]
fn test_route_local_complete_config() {
    let cfg = config_for_route(
        SetupRoute::Local,
        RouteParams {
            url: Some("http://localhost:11434/v1".to_string()),
            model: Some("qwen3:4b".to_string()),
            ..Default::default()
        },
    );
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("native"));
    assert!(cfg.tiers.fast.is_some());
    assert!(cfg.tiers.standard.is_some());
    assert!(cfg.tiers.premium.is_some());
    assert_eq!(cfg.llm_endpoints.endpoints.len(), 1);
    assert_eq!(cfg.llm_endpoints.endpoints[0].provider, "local");
    assert!(cfg.llm_endpoints.endpoints[0].api_key_env.is_none());
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    let _: Config = toml::from_str(&toml_str).unwrap();
}

#[test]
fn test_route_nex_custom_complete_config() {
    let cfg = config_for_route(
        SetupRoute::NexCustom,
        RouteParams {
            url: Some("https://example.com/v1".to_string()),
            api_key_env: Some("MY_KEY".to_string()),
            model: Some("foo".to_string()),
            ..Default::default()
        },
    );
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("native"));
    assert!(cfg.tiers.fast.is_some());
    assert!(cfg.tiers.standard.is_some());
    assert!(cfg.tiers.premium.is_some());
    assert_eq!(cfg.llm_endpoints.endpoints.len(), 1);
    assert_eq!(cfg.llm_endpoints.endpoints[0].provider, "oai-compat");
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    let _: Config = toml::from_str(&toml_str).unwrap();
}

// ---------------------------------------------------------------------------
// CLI flow: wg setup --route <name> --yes writes complete configs.
// ---------------------------------------------------------------------------

#[test]
fn test_setup_non_interactive_route_writes_config() {
    // wg setup --route claude-cli --yes produces a config with populated tiers.
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = run_wg_in_isolation(&fake_home, &["setup", "--route", "claude-cli", "--yes"]);
    assert!(
        output.status.success(),
        "wg setup --route claude-cli --yes failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let cfg = load_global_config(&fake_home);
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("claude"));
    assert_eq!(cfg.agent.executor, "claude");
    assert!(cfg.tiers.fast.is_some(), "tiers.fast must be populated");
    assert!(
        cfg.tiers.standard.is_some(),
        "tiers.standard must be populated"
    );
    assert!(
        cfg.tiers.premium.is_some(),
        "tiers.premium must be populated"
    );
    assert_eq!(cfg.agent.model, "claude:opus");
    assert_eq!(cfg.coordinator.model.as_deref(), Some("claude:opus"));
    assert_eq!(cfg.tiers.standard.as_deref(), Some("claude:opus"));
    assert_eq!(
        cfg.resolve_model_for_role(DispatchRole::TaskAgent).model,
        "opus"
    );
}

#[test]
fn test_setup_route_codex_writes_top_standard_and_task_agent() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = run_wg_in_isolation(&fake_home, &["setup", "--route", "codex-cli", "--yes"]);
    assert!(
        output.status.success(),
        "wg setup --route codex-cli --yes failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let cfg = load_global_config(&fake_home);
    assert_eq!(cfg.agent.model, "codex:gpt-5.5");
    assert_eq!(cfg.coordinator.model.as_deref(), Some("codex:gpt-5.5"));
    assert_eq!(cfg.tiers.standard.as_deref(), Some("codex:gpt-5.5"));
    assert_eq!(
        cfg.models
            .task_agent
            .as_ref()
            .and_then(|m| m.model.as_deref()),
        Some("codex:gpt-5.5")
    );
    assert_eq!(
        cfg.resolve_model_for_role(DispatchRole::TaskAgent).model,
        "gpt-5.5"
    );
}

#[test]
fn test_setup_route_openrouter_writes_endpoint_and_tiers() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let model_body = r#"{
        "data": [
            {
                "id": "anthropic/claude-sonnet-4-6",
                "name": "Claude Sonnet 4.6",
                "description": "test",
                "context_length": 200000,
                "pricing": {"prompt":"0.000003","completion":"0.000015"},
                "supported_parameters": ["tools"]
            }
        ]
    }"#;
    let (base_url, requests, handle) = start_mock_server(vec![Route {
        method: "GET",
        path: "/api/v1/models",
        status: 200,
        body: model_body,
    }]);

    let output = run_wg_in_isolation_with_env(
        &fake_home,
        &[
            "setup",
            "--route",
            "openrouter",
            "--url",
            &format!("{base_url}/api/v1"),
            "--api-key-env",
            "OPENROUTER_API_KEY",
            "--yes",
        ],
        &[("OPENROUTER_API_KEY", "sk-or-setup-login-test")],
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cfg = load_global_config(&fake_home);
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("native"));
    assert_eq!(cfg.llm_endpoints.endpoints.len(), 1);
    let ep = &cfg.llm_endpoints.endpoints[0];
    assert_eq!(ep.provider, "openrouter");
    assert_eq!(ep.api_key_ref.as_deref(), Some("env:OPENROUTER_API_KEY"));
    assert!(ep.api_key_env.is_none());
    assert!(cfg.tiers.fast.is_some());
    assert!(cfg.tiers.standard.is_some());
    assert!(cfg.tiers.premium.is_some());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("OpenRouter (WG)"));
    assert!(stdout.contains("scope: WG-managed auth"));
    assert!(stdout.contains("auth: ok"));
    assert!(stdout.contains("OpenRouter (Pi)"));
    assert!(stdout.contains("scope: Pi-managed auth for `pi:` routes only"));
    assert!(stdout.contains("wg profile pi"));

    let requests = requests.lock().unwrap();
    assert!(
        requests.iter().any(|request| {
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer sk-or-setup-login-test")
        }),
        "mock server never observed the configured Authorization header: {:?}",
        *requests
    );
    handle.join().unwrap();
}

#[test]
fn test_setup_route_openrouter_without_key_prints_exact_login_step() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = run_wg_in_isolation(&fake_home, &["setup", "--route", "openrouter", "--yes"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Next independent login step: wg login openrouter"));
    assert!(stdout.contains("WG-managed auth"));
    assert!(stdout.contains("Pi keeps its own provider login separately"));
    assert!(stdout.contains("wg login openrouter --check"));
    assert!(stdout.contains("wg model-scout --no-cache"));
}

#[test]
fn test_setup_route_local_uses_supplied_model() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = run_wg_in_isolation(
        &fake_home,
        &[
            "setup",
            "--route",
            "local",
            "--url",
            "http://localhost:11434/v1",
            "--model",
            "qwen3:4b",
            "--yes",
        ],
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cfg = load_global_config(&fake_home);
    assert_eq!(cfg.coordinator.executor.as_deref(), Some("native"));
    assert_eq!(cfg.tiers.fast.as_deref(), Some("nex:qwen3:4b"));
    assert_eq!(cfg.tiers.standard.as_deref(), Some("nex:qwen3:4b"));
    assert_eq!(cfg.tiers.premium.as_deref(), Some("nex:qwen3:4b"));
}

#[test]
fn test_setup_route_nex_custom_requires_url_and_model() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    // Missing --url
    let output = run_wg_in_isolation(&fake_home, &["setup", "--route", "nex-custom", "--yes"]);
    assert!(
        !output.status.success(),
        "should fail without --url: stdout {} stderr {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("nex-custom") && stderr.contains("--url"),
        "error should mention nex-custom and --url, got: {}",
        stderr,
    );
}

#[test]
fn test_setup_dry_run_does_not_write() {
    // --dry-run prints the would-be config but doesn't touch the filesystem.
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = run_wg_in_isolation(
        &fake_home,
        &["setup", "--route", "claude-cli", "--dry-run", "--yes"],
    );
    assert!(output.status.success());

    // No global config should have been created — under either the modern
    // `.wg` or legacy `.wg` global dir.
    let modern = fake_home.join(".wg/config.toml");
    let legacy = fake_home.join(".wg/config.toml");
    assert!(
        !modern.exists() && !legacy.exists(),
        "dry-run must not create global config (neither {} nor {} should exist)",
        modern.display(),
        legacy.display(),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("dry-run") || stdout.contains("dispatcher") || stdout.contains("agent"),
        "dry-run output should include the would-be config, got: {}",
        stdout,
    );
}

// ---------------------------------------------------------------------------
// wg init --dry-run: no write
// ---------------------------------------------------------------------------

#[test]
fn test_init_dry_run_no_write() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let wg_dir = project.join(".wg");
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = Command::new(wg_binary())
        .arg("--dir")
        .arg(&wg_dir)
        .args(["init", "--route", "claude-cli", "--dry-run"])
        .env("HOME", &fake_home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "init --dry-run failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    // The .wg directory should NOT have been created.
    assert!(
        !wg_dir.exists(),
        ".wg directory should not exist after --dry-run"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("dry-run") || stdout.contains("[dispatcher]") || stdout.contains("[agent]"),
        "stdout should show the would-be config, got: {}",
        stdout,
    );
}

// ---------------------------------------------------------------------------
// wg init -x claude → populated [tiers] (the bug)
// ---------------------------------------------------------------------------

#[test]
fn test_init_with_executor_only_populates_tiers() {
    // The validation criteria say `wg init -x claude` should produce
    // populated [tiers] — this is the bug the spec calls out.
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let wg_dir = project.join(".wg");
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = Command::new(wg_binary())
        .arg("--dir")
        .arg(&wg_dir)
        .args(["init", "-x", "claude", "--no-agency"])
        .env("HOME", &fake_home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wg init -x claude failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let cfg_path = wg_dir.join("config.toml");
    let cfg_str = fs::read_to_string(&cfg_path).expect("config.toml must be created");
    let cfg: Config = toml::from_str(&cfg_str).expect("config must parse");

    assert!(
        cfg.tiers.fast.is_some() && cfg.tiers.standard.is_some() && cfg.tiers.premium.is_some(),
        "all three tiers must be populated after `wg init -x claude`. Got: fast={:?}, standard={:?}, premium={:?}",
        cfg.tiers.fast,
        cfg.tiers.standard,
        cfg.tiers.premium,
    );
}

#[test]
fn test_init_route_openrouter_prints_login_handoff() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let wg_dir = project.join(".wg");
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = Command::new(wg_binary())
        .arg("--dir")
        .arg(&wg_dir)
        .args(["init", "--route", "openrouter", "--no-agency"])
        .env("HOME", &fake_home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "wg init --route openrouter failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Next independent login step: wg login openrouter"));
    assert!(stdout.contains("Pi keeps its own provider login separately"));
}

// ---------------------------------------------------------------------------
// wg config reset: backup + --keep-keys
// ---------------------------------------------------------------------------

#[test]
fn test_config_reset_keep_keys_preserves_endpoints() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let global_dir = fake_home.join(".wg");
    fs::create_dir_all(&global_dir).unwrap();

    // Pre-populate a global config with an openrouter endpoint
    let pre = r#"
[dispatcher]
executor = "native"

[agent]
executor = "native"
model = "openrouter:anthropic/claude-sonnet-4"

[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"
is_default = true
"#;
    fs::write(global_dir.join("config.toml"), pre).unwrap();

    // Reset to claude-cli with --keep-keys --yes
    let output = run_wg_in_isolation(
        &fake_home,
        &[
            "config",
            "reset",
            "--route",
            "claude-cli",
            "--keep-keys",
            "--yes",
        ],
    );
    assert!(
        output.status.success(),
        "config reset failed.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cfg = load_global_config(&fake_home);
    assert_eq!(
        cfg.coordinator.executor.as_deref(),
        Some("claude"),
        "executor must change to claude per route"
    );
    // Tiers must be populated by the new route
    assert!(cfg.tiers.fast.is_some());
    assert!(cfg.tiers.standard.is_some());
    assert!(cfg.tiers.premium.is_some());
    // Endpoints preserved by --keep-keys
    assert_eq!(
        cfg.llm_endpoints.endpoints.len(),
        1,
        "openrouter endpoint must be preserved"
    );
    let ep = &cfg.llm_endpoints.endpoints[0];
    assert_eq!(ep.name, "openrouter");
    assert_eq!(ep.api_key_env.as_deref(), Some("OPENROUTER_API_KEY"));
}

#[test]
fn test_config_reset_creates_backup() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let global_dir = fake_home.join(".wg");
    fs::create_dir_all(&global_dir).unwrap();

    let pre = r#"
[dispatcher]
executor = "claude"

[agent]
executor = "claude"
model = "claude:opus"
"#;
    fs::write(global_dir.join("config.toml"), pre).unwrap();

    let output = run_wg_in_isolation(
        &fake_home,
        &["config", "reset", "--route", "openrouter", "--yes"],
    );
    assert!(
        output.status.success(),
        "config reset failed.\nstderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );

    // A backup file should exist.
    let backups: Vec<_> = fs::read_dir(&global_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("config.toml.bak-")
        })
        .collect();
    assert_eq!(
        backups.len(),
        1,
        "exactly one backup should be created. Found: {:?}",
        fs::read_dir(&global_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>()
    );

    // Backup content matches the pre-reset config
    let backup_content = fs::read_to_string(backups[0].path()).unwrap();
    assert!(backup_content.contains("claude"));
    assert!(backup_content.contains("opus"));
}

#[test]
fn test_config_reset_dry_run_does_not_write() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let global_dir = fake_home.join(".wg");
    fs::create_dir_all(&global_dir).unwrap();

    let pre = r#"
[dispatcher]
executor = "claude"

[agent]
executor = "claude"
model = "claude:sonnet"
"#;
    fs::write(global_dir.join("config.toml"), pre).unwrap();
    let original = fs::read_to_string(global_dir.join("config.toml")).unwrap();

    let output = run_wg_in_isolation(
        &fake_home,
        &["config", "reset", "--route", "openrouter", "--dry-run"],
    );
    assert!(output.status.success());

    // Config unchanged
    let after = fs::read_to_string(global_dir.join("config.toml")).unwrap();
    assert_eq!(after, original, "dry-run must not modify the config");

    // No backup file
    let backups: Vec<_> = fs::read_dir(&global_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("config.toml.bak-")
        })
        .collect();
    assert!(backups.is_empty(), "dry-run must not create a backup");
}

#[test]
fn test_setup_route_openrouter_from_stdin_writes_secret_ref_not_embedded_key() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let project = tmp.path().join("project");
    fs::create_dir_all(&fake_home).unwrap();
    fs::create_dir_all(&project).unwrap();

    let output = Command::new(wg_binary())
        .current_dir(&project)
        .env("HOME", &fake_home)
        .args([
            "setup",
            "--route",
            "openrouter",
            "--scope",
            "local",
            "--from-stdin",
            "--backend",
            "keystore",
            "--yes",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .expect("stdin")
                .write_all(b"sk-or-setup-test\n")?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(
        output.status.success(),
        "wg setup openrouter from stdin failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let content = fs::read_to_string(project.join(".wg/config.toml")).unwrap();
    assert!(content.contains(r#"api_key_ref = "keystore:openrouter""#));
    assert!(!content.contains("sk-or-setup-test"));
    assert!(!content.contains("api_key ="));
}

#[test]
fn test_setup_route_openrouter_local_reuses_existing_global_login() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let project = tmp.path().join("project");
    fs::create_dir_all(fake_home.join(".wg")).unwrap();
    fs::create_dir_all(&project).unwrap();

    fs::write(
        fake_home.join(".wg/config.toml"),
        r#"
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_ref = "env:OPENROUTER_API_KEY"
is_default = true
"#,
    )
    .unwrap();

    let output = Command::new(wg_binary())
        .current_dir(&project)
        .env("HOME", &fake_home)
        .env("OPENROUTER_API_KEY", "sk-or-global-reuse")
        .args([
            "setup",
            "--route",
            "openrouter",
            "--scope",
            "local",
            "--yes",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wg setup local openrouter reuse failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let cfg = load_local_config(&project);
    assert!(cfg.llm_endpoints.inherit_global);
    assert!(cfg.llm_endpoints.endpoints.is_empty());
    assert_eq!(cfg.agent.model, "openrouter:anthropic/claude-opus-4-7");
}

#[test]
fn test_setup_route_claude_cli_needs_no_api_key_prompt() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = run_wg_in_isolation(&fake_home, &["setup", "--route", "claude-cli", "--yes"]);
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{}", combined);
    assert!(!combined.contains("OpenRouter API key"));
    assert!(!combined.contains("OPENROUTER_API_KEY"));
}

#[test]
fn test_setup_route_pi_local_reuses_global_openrouter_for_wg_managed_side() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    let project = tmp.path().join("project");
    fs::create_dir_all(fake_home.join(".wg")).unwrap();
    fs::create_dir_all(&project).unwrap();

    fs::write(
        fake_home.join(".wg/config.toml"),
        r#"
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_ref = "env:OPENROUTER_API_KEY"
is_default = true
"#,
    )
    .unwrap();

    let output = Command::new(wg_binary())
        .current_dir(&project)
        .env("HOME", &fake_home)
        .env("OPENROUTER_API_KEY", "sk-or-global-reuse")
        .args(["setup", "--route", "pi", "--scope", "local", "--yes"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "wg setup pi local reuse failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let cfg = load_local_config(&project);
    assert!(cfg.llm_endpoints.inherit_global);
    assert!(cfg.agent.model.starts_with("pi:"));
    assert_eq!(
        cfg.tiers.fast.as_deref(),
        Some("openrouter:deepseek/deepseek-chat")
    );
}

#[test]
fn test_setup_help_mentions_pi_and_provider_login_onboarding() {
    let tmp = TempDir::new().unwrap();
    let fake_home = tmp.path().join("home");
    fs::create_dir_all(&fake_home).unwrap();

    let output = run_wg_in_isolation(&fake_home, &["setup", "--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(stdout.contains("pi"));
    assert!(stdout.contains("--from-stdin"));
    assert!(stdout.contains("Secret backend"));
}
