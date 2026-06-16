//! Integration tests for `wg endpoints` CLI commands.
//!
//! Exercises the full add/list/remove/set-default lifecycle through the CLI
//! binary, verifying output format, error messages, and config persistence.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;
use worksgood::config::CLAUDE_SONNET_MODEL_ID;
use worksgood::graph::WorkGraph;
use worksgood::parser::save_graph;

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

fn wg_cmd(wg_dir: &Path, args: &[&str]) -> std::process::Output {
    // Use a fake HOME derived from the wg_dir path so that the user's real
    // ~/.wg/config.toml does not bleed into the test (the fake home
    // has no .wg/ subdir, so global config is empty).
    let fake_home = wg_dir.parent().and_then(|p| p.parent()).unwrap_or(wg_dir);
    Command::new(wg_binary())
        .arg("--dir")
        .arg(wg_dir)
        .args(args)
        .env("HOME", fake_home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap_or_else(|e| panic!("Failed to run wg {:?}: {}", args, e))
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

fn wg_fail(wg_dir: &Path, args: &[&str]) -> (String, String) {
    let output = wg_cmd(wg_dir, args);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        !output.status.success(),
        "wg {:?} should have failed but succeeded.\nstdout: {}\nstderr: {}",
        args,
        stdout,
        stderr
    );
    (stdout, stderr)
}

fn setup_workgraph(tmp: &TempDir) -> PathBuf {
    let wg_dir = tmp.path().join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    let graph_path = wg_dir.join("graph.jsonl");
    let graph = WorkGraph::new();
    save_graph(&graph, &graph_path).unwrap();
    wg_dir
}

#[derive(Clone)]
struct EndpointMockRoute {
    method: &'static str,
    path: &'static str,
    status: u16,
    body: &'static str,
}

fn request_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn endpoint_request_complete(buf: &[u8]) -> bool {
    let Some(header_end) = request_header_end(buf) else {
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

fn endpoint_mock_server(
    routes: Vec<EndpointMockRoute>,
) -> (
    String,
    std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    std::thread::JoinHandle<()>,
) {
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{}", port);
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let request_log = std::sync::Arc::clone(&requests);
    let expected_requests = routes.len();

    let handle = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut served = 0usize;
        while served < expected_requests && Instant::now() < deadline {
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
                        if endpoint_request_complete(&buf) {
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
            let mut request_parts = request_line.split_whitespace();
            let method = request_parts.next().unwrap_or_default();
            let path = request_parts.next().unwrap_or_default();
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

    (base_url, requests, handle)
}

// ===========================================================================
// 1. wg endpoints add — creates valid config entry
// ===========================================================================

#[test]
fn cli_endpoints_add_creates_config_entry() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);
    let sonnet_model = format!("anthropic/{CLAUDE_SONNET_MODEL_ID}");

    let output = wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "test-ep",
            "--provider",
            "openrouter",
            "--api-key",
            "sk-or-test-123",
            "--model",
            &sonnet_model,
        ],
    );
    assert!(output.contains("Added endpoint 'test-ep'"));
    assert!(output.contains("openrouter"));

    // Verify the config file was written
    let config_path = wg_dir.join("config.toml");
    let config_text = fs::read_to_string(&config_path).unwrap();
    assert!(
        config_text.contains("test-ep"),
        "Config should contain endpoint name, got: {}",
        config_text
    );
    assert!(config_text.contains("openrouter"));
}

#[test]
fn cli_endpoints_add_first_becomes_default() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "first-ep",
            "--provider",
            "openai",
            "--api-key",
            "sk-test",
        ],
    );

    // Verify via JSON output
    let json_list = wg_ok(&wg_dir, &["--json", "endpoints", "list"]);
    let parsed: serde_json::Value = serde_json::from_str(&json_list).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["is_default"], true);
}

#[test]
fn cli_endpoints_add_with_key_file() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let key_file = tmp.path().join("api.key");
    {
        let mut f = fs::File::create(&key_file).unwrap();
        writeln!(f, "sk-or-from-file-test").unwrap();
    }

    let output = wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "file-ep",
            "--provider",
            "openrouter",
            "--api-key-file",
            &key_file.to_string_lossy(),
        ],
    );
    assert!(output.contains("Added endpoint 'file-ep'"));

    // List should show "(from file)" for the key
    let list = wg_ok(&wg_dir, &["endpoints", "list"]);
    assert!(
        list.contains("(from file)"),
        "Expected '(from file)' in list output, got: {}",
        list
    );
}

#[test]
fn cli_endpoints_add_defaults_provider_to_anthropic() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(
        &wg_dir,
        &["endpoints", "add", "bare-ep", "--api-key", "sk-test"],
    );
    assert!(output.contains("anthropic"));

    let json_list = wg_ok(&wg_dir, &["--json", "endpoints", "list"]);
    let parsed: serde_json::Value = serde_json::from_str(&json_list).unwrap();
    assert_eq!(parsed[0]["provider"], "anthropic");
}

// ===========================================================================
// 2. wg endpoints list — output format
// ===========================================================================

#[test]
fn cli_endpoints_list_empty() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(&wg_dir, &["endpoints", "list"]);
    assert!(
        output.contains("No endpoints configured"),
        "Expected empty message, got: {}",
        output
    );
}

#[test]
fn cli_endpoints_list_json_empty() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let output = wg_ok(&wg_dir, &["--json", "endpoints", "list"]);
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed, serde_json::json!([]));
}

#[test]
fn cli_endpoints_list_shows_all_fields() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);
    let sonnet_model = format!("anthropic/{CLAUDE_SONNET_MODEL_ID}");

    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "full-ep",
            "--provider",
            "openrouter",
            "--api-key",
            "sk-or-test-key-abcdef",
            "--model",
            &sonnet_model,
            "--url",
            "https://openrouter.ai/api/v1",
        ],
    );

    let list = wg_ok(&wg_dir, &["endpoints", "list"]);
    assert!(list.contains("full-ep"));
    assert!(list.contains("openrouter"));
    assert!(list.contains("(default)"));
    assert!(list.contains(&sonnet_model));

    // JSON format includes structured fields
    let json_list = wg_ok(&wg_dir, &["--json", "endpoints", "list"]);
    let parsed: serde_json::Value = serde_json::from_str(&json_list).unwrap();
    let ep = &parsed[0];
    assert_eq!(ep["name"], "full-ep");
    assert_eq!(ep["provider"], "openrouter");
    assert_eq!(ep["model"], sonnet_model.as_str());
    assert_eq!(ep["is_default"], true);
    // API key should be masked in output
    let key_str = ep["api_key"].as_str().unwrap();
    assert!(
        key_str.contains("...") || key_str.contains("***") || key_str.len() < 20,
        "API key should be masked in list output, got: {}",
        key_str
    );
}

#[test]
fn cli_endpoints_list_multiple() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "ep-a",
            "--provider",
            "openrouter",
            "--api-key",
            "sk-a",
        ],
    );
    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "ep-b",
            "--provider",
            "openai",
            "--api-key",
            "sk-b",
        ],
    );

    let json_list = wg_ok(&wg_dir, &["--json", "endpoints", "list"]);
    let parsed: serde_json::Value = serde_json::from_str(&json_list).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    let names: Vec<&str> = arr.iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"ep-a"));
    assert!(names.contains(&"ep-b"));
}

// ===========================================================================
// 3. wg endpoints remove — cleans up, warns on default
// ===========================================================================

#[test]
fn cli_endpoints_remove_cleans_up() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "rm-ep",
            "--provider",
            "openai",
            "--api-key",
            "sk-rm",
        ],
    );

    let output = wg_ok(&wg_dir, &["endpoints", "remove", "rm-ep"]);
    assert!(output.contains("Removed endpoint 'rm-ep'"));

    let list = wg_ok(&wg_dir, &["endpoints", "list"]);
    assert!(list.contains("No endpoints configured"));
}

#[test]
fn cli_endpoints_remove_nonexistent_errors() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let (stdout, stderr) = wg_fail(&wg_dir, &["endpoints", "remove", "ghost-ep"]);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("not found"),
        "Expected 'not found' error, got: {}",
        combined
    );
}

#[test]
fn cli_endpoints_remove_default_promotes_next() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    // Add two endpoints; first becomes default
    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "primary",
            "--provider",
            "openrouter",
            "--api-key",
            "sk-p",
        ],
    );
    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "secondary",
            "--provider",
            "openai",
            "--api-key",
            "sk-s",
        ],
    );

    // Remove the default
    wg_ok(&wg_dir, &["endpoints", "remove", "primary"]);

    // Secondary should now be default
    let json_list = wg_ok(&wg_dir, &["--json", "endpoints", "list"]);
    let parsed: serde_json::Value = serde_json::from_str(&json_list).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "secondary");
    assert_eq!(arr[0]["is_default"], true);
}

// ===========================================================================
// 4. wg endpoints set-default — updates config
// ===========================================================================

#[test]
fn cli_endpoints_set_default_switches() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "alpha",
            "--provider",
            "openrouter",
            "--api-key",
            "sk-a",
        ],
    );
    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "beta",
            "--provider",
            "openai",
            "--api-key",
            "sk-b",
        ],
    );

    let output = wg_ok(&wg_dir, &["endpoints", "set-default", "beta"]);
    assert!(output.contains("Set 'beta' as default"));

    let json_list = wg_ok(&wg_dir, &["--json", "endpoints", "list"]);
    let parsed: serde_json::Value = serde_json::from_str(&json_list).unwrap();
    let arr = parsed.as_array().unwrap();
    let alpha = arr.iter().find(|v| v["name"] == "alpha").unwrap();
    let beta = arr.iter().find(|v| v["name"] == "beta").unwrap();
    assert_eq!(alpha["is_default"], false);
    assert_eq!(beta["is_default"], true);
}

#[test]
fn cli_endpoints_set_default_nonexistent_errors() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let (stdout, stderr) = wg_fail(&wg_dir, &["endpoints", "set-default", "nope"]);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("not found"),
        "Expected 'not found' error, got: {}",
        combined
    );
}

// ===========================================================================
// 5. wg endpoints test — models plus generation probe
// ===========================================================================

#[test]
fn cli_endpoints_test_generation_failure_after_models_success() {
    let (base_url, _requests, handle) = endpoint_mock_server(vec![
        EndpointMockRoute {
            method: "GET",
            path: "/models",
            status: 200,
            body: r#"{"data":[{"id":"test-model"}]}"#,
        },
        EndpointMockRoute {
            method: "POST",
            path: "/chat/completions",
            status: 500,
            body: r#"{"error":"upstream failed"}"#,
        },
    ]);

    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);
    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "mock-or",
            "--provider",
            "openrouter",
            "--url",
            &base_url,
            "--api-key",
            "sk-or-test",
            "--model",
            "test-model",
        ],
    );

    let (stdout, stderr) = wg_fail(&wg_dir, &["endpoints", "test", "mock-or"]);
    handle.join().unwrap();
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        stdout.contains("Models: OK"),
        "models success should be visible before generation failure: {}",
        stdout
    );
    assert!(
        stdout.contains("Generation: FAILED (HTTP 500)"),
        "expected generation failure in stdout, got: {}",
        stdout
    );
    assert!(
        combined.contains("Generation failed"),
        "expected generation failure error, got: {}",
        combined
    );
}

#[test]
fn cli_endpoints_test_generation_success_sends_bearer_auth() {
    let (base_url, requests, handle) = endpoint_mock_server(vec![
        EndpointMockRoute {
            method: "GET",
            path: "/models",
            status: 200,
            body: r#"{"data":[{"id":"test-model"}]}"#,
        },
        EndpointMockRoute {
            method: "POST",
            path: "/chat/completions",
            status: 200,
            body: r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#,
        },
    ]);

    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);
    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "mock-or-ok",
            "--provider",
            "openrouter",
            "--url",
            &base_url,
            "--api-key",
            "sk-or-generation",
            "--model",
            "test-model",
        ],
    );

    let output = wg_ok(&wg_dir, &["endpoints", "test", "mock-or-ok"]);
    handle.join().unwrap();
    assert!(
        output.contains("Generation: OK"),
        "expected concise generation OK output, got: {}",
        output
    );

    let captured = requests.lock().unwrap().clone();
    assert_eq!(
        captured.len(),
        2,
        "expected /models and /chat/completions requests, got: {:?}",
        captured
    );
    let chat_request = captured
        .iter()
        .find(|request| request.starts_with("POST /chat/completions "))
        .expect("chat completion request should be captured");
    assert!(
        chat_request
            .to_ascii_lowercase()
            .contains("authorization: bearer sk-or-generation"),
        "chat request must include bearer auth when configured:\n{}",
        chat_request
    );
    assert!(
        chat_request.contains(r#""model":"test-model""#),
        "chat request must route the configured model:\n{}",
        chat_request
    );
}

#[test]
fn cli_endpoints_test_generation_model_not_found_is_distinct() {
    let (base_url, _requests, handle) = endpoint_mock_server(vec![
        EndpointMockRoute {
            method: "GET",
            path: "/models",
            status: 200,
            body: r#"{"data":[{"id":"other-model"}]}"#,
        },
        EndpointMockRoute {
            method: "POST",
            path: "/chat/completions",
            status: 404,
            body: r#"{"error":{"message":"model not found"}}"#,
        },
    ]);

    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);
    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "missing-model",
            "--provider",
            "openai",
            "--url",
            &base_url,
            "--api-key",
            "sk-test",
            "--model",
            "missing-model",
        ],
    );

    let (stdout, stderr) = wg_fail(&wg_dir, &["endpoints", "test", "missing-model"]);
    handle.join().unwrap();
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        stdout.contains("Generation: FAILED (model not found)"),
        "expected distinct model-not-found output, got: {}",
        stdout
    );
    assert!(
        combined.contains("Model not found"),
        "expected model-not-found error, got: {}",
        combined
    );
}

#[test]
fn cli_endpoints_test_generation_body_shape_is_distinct() {
    let (base_url, _requests, handle) = endpoint_mock_server(vec![
        EndpointMockRoute {
            method: "GET",
            path: "/models",
            status: 200,
            body: r#"{"data":[{"id":"test-model"}]}"#,
        },
        EndpointMockRoute {
            method: "POST",
            path: "/chat/completions",
            status: 200,
            body: r#"{"choices":[{"message":{"role":"assistant"}}]}"#,
        },
    ]);

    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);
    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "bad-shape",
            "--provider",
            "openai",
            "--url",
            &base_url,
            "--api-key",
            "sk-test",
            "--model",
            "test-model",
        ],
    );

    let (stdout, stderr) = wg_fail(&wg_dir, &["endpoints", "test", "bad-shape"]);
    handle.join().unwrap();
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        stdout.contains("Generation: FAILED (unexpected response body)"),
        "expected body-shape output, got: {}",
        stdout
    );
    assert!(
        combined.contains("body-shape"),
        "expected body-shape error, got: {}",
        combined
    );
}

#[test]
fn cli_endpoints_test_authentication_failure_is_distinct() {
    let (base_url, _requests, handle) = endpoint_mock_server(vec![EndpointMockRoute {
        method: "GET",
        path: "/models",
        status: 401,
        body: r#"{"error":"unauthorized"}"#,
    }]);

    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);
    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "bad-auth",
            "--provider",
            "openai",
            "--url",
            &base_url,
            "--api-key",
            "sk-bad",
            "--model",
            "test-model",
        ],
    );

    let (stdout, stderr) = wg_fail(&wg_dir, &["endpoints", "test", "bad-auth"]);
    handle.join().unwrap();
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        stdout.contains("Authentication: FAILED (models request)"),
        "expected authentication failure output, got: {}",
        stdout
    );
    assert!(
        combined.contains("Authentication failed"),
        "expected authentication error, got: {}",
        combined
    );
}

#[test]
fn cli_endpoints_test_models_only_for_generation_unavailable_provider() {
    let (base_url, requests, handle) = endpoint_mock_server(vec![EndpointMockRoute {
        method: "GET",
        path: "/v1/models",
        status: 200,
        body: r#"{"data":[]}"#,
    }]);

    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);
    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "anthropic-mock",
            "--provider",
            "anthropic",
            "--url",
            &base_url,
            "--api-key",
            "sk-ant",
            "--model",
            "claude-test",
        ],
    );

    let output = wg_ok(&wg_dir, &["endpoints", "test", "anthropic-mock"]);
    handle.join().unwrap();
    assert!(
        output.contains("Models: OK"),
        "expected models connectivity to remain available, got: {}",
        output
    );
    assert!(
        output.contains("Generation: SKIPPED (not available for provider 'anthropic')"),
        "expected generation skip for non-OAI provider, got: {}",
        output
    );
    let captured = requests.lock().unwrap().clone();
    assert_eq!(captured.len(), 1, "only /models should be requested");
    assert!(
        captured[0].starts_with("GET /v1/models "),
        "anthropic endpoint should still use /v1/models: {}",
        captured[0]
    );
}

// ===========================================================================
// 6. Duplicate endpoint name -> error
// ===========================================================================

#[test]
fn cli_endpoints_add_duplicate_errors() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "dup-ep",
            "--provider",
            "openai",
            "--api-key",
            "sk-1",
        ],
    );

    let (stdout, stderr) = wg_fail(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "dup-ep",
            "--provider",
            "openai",
            "--api-key",
            "sk-2",
        ],
    );
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("already exists"),
        "Expected 'already exists' error, got: {}",
        combined
    );
}

// ===========================================================================
// 7. Full CRUD lifecycle
// ===========================================================================

#[test]
fn cli_endpoints_full_lifecycle() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);
    let sonnet_model = format!("anthropic/{CLAUDE_SONNET_MODEL_ID}");

    // Add two endpoints
    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "ep-one",
            "--provider",
            "openrouter",
            "--api-key",
            "sk-or-1",
            "--model",
            &sonnet_model,
        ],
    );
    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "ep-two",
            "--provider",
            "openai",
            "--api-key",
            "sk-oai-2",
            "--model",
            "gpt-4o",
        ],
    );

    // List
    let json_list = wg_ok(&wg_dir, &["--json", "endpoints", "list"]);
    let parsed: serde_json::Value = serde_json::from_str(&json_list).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 2);

    // Switch default
    wg_ok(&wg_dir, &["endpoints", "set-default", "ep-two"]);

    // Remove first
    wg_ok(&wg_dir, &["endpoints", "remove", "ep-one"]);

    // Verify only ep-two remains and is default
    let json_list = wg_ok(&wg_dir, &["--json", "endpoints", "list"]);
    let parsed: serde_json::Value = serde_json::from_str(&json_list).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "ep-two");
    assert_eq!(arr[0]["is_default"], true);

    // Remove last
    wg_ok(&wg_dir, &["endpoints", "remove", "ep-two"]);
    let list = wg_ok(&wg_dir, &["endpoints", "list"]);
    assert!(list.contains("No endpoints configured"));
}

// ===========================================================================
// 8. wg endpoints update — patches existing endpoint in place
// ===========================================================================

#[test]
fn cli_endpoints_update_patches_api_key_file() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "upd-ep",
            "--provider",
            "openai",
            "--api-key",
            "sk-old",
            "--model",
            "gpt-4o",
        ],
    );

    let key_file = tmp.path().join("newkey.txt");
    fs::write(&key_file, "sk-new-from-file\n").unwrap();

    let output = wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "update",
            "upd-ep",
            "--api-key-file",
            &key_file.to_string_lossy(),
        ],
    );
    assert!(output.contains("Updated endpoint 'upd-ep'"));
    assert!(output.contains("api_key_file"));

    // Verify provider and model unchanged, key source changed
    let json_list = wg_ok(&wg_dir, &["--json", "endpoints", "list"]);
    let parsed: serde_json::Value = serde_json::from_str(&json_list).unwrap();
    let ep = &parsed[0];
    assert_eq!(ep["provider"], "openai");
    assert_eq!(ep["model"], "gpt-4o");
    let key_source = ep["key_source"].as_str().unwrap();
    assert!(
        key_source.starts_with("file"),
        "Expected key_source to start with 'file', got: {}",
        key_source
    );
}

#[test]
fn cli_endpoints_update_patches_provider() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "upd-ep2",
            "--provider",
            "openai",
            "--api-key",
            "sk-test",
        ],
    );

    let output = wg_ok(
        &wg_dir,
        &["endpoints", "update", "upd-ep2", "--provider", "anthropic"],
    );
    assert!(output.contains("Updated endpoint 'upd-ep2'"));
    assert!(output.contains("provider"));

    let json_list = wg_ok(&wg_dir, &["--json", "endpoints", "list"]);
    let parsed: serde_json::Value = serde_json::from_str(&json_list).unwrap();
    assert_eq!(parsed[0]["provider"], "anthropic");
}

#[test]
fn cli_endpoints_update_nonexistent_errors() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    let (stdout, stderr) = wg_fail(
        &wg_dir,
        &["endpoints", "update", "ghost-ep", "--provider", "openai"],
    );
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("not found"),
        "Expected 'not found' error, got: {}",
        combined
    );
}

#[test]
fn cli_endpoints_update_no_fields_errors() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = setup_workgraph(&tmp);

    wg_ok(
        &wg_dir,
        &[
            "endpoints",
            "add",
            "upd-ep3",
            "--provider",
            "openai",
            "--api-key",
            "sk-test",
        ],
    );

    let (stdout, stderr) = wg_fail(&wg_dir, &["endpoints", "update", "upd-ep3"]);
    let combined = format!("{}{}", stdout, stderr);
    assert!(
        combined.contains("No fields specified"),
        "Expected 'No fields specified' error, got: {}",
        combined
    );
}
