use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::Value;
use tempfile::TempDir;
use workgraph::config::{Config, DispatchRole};
use workgraph::nex_runtime::{
    NexRuntimeResolveInput, load_config, resolve_standalone, resolve_wg_autonomous,
};

fn wg_binary() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_wg") {
        return p.into();
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    Path::new(&manifest).join("target/debug/wg")
}

fn nex_binary() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_nex") {
        return p.into();
    }
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    Path::new(&manifest).join("target/debug/nex")
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_eval_success_without_key(label: &str, output: &Output, expected_key: &str) -> String {
    let text = output_text(output);
    assert!(
        !text.contains(expected_key),
        "{label} output leaked configured API key"
    );
    let redacted = text.replace(expected_key, "<redacted>");
    assert!(output.status.success(), "{label} failed:\n{redacted}");
    assert!(
        text.contains("\"status\":\"ok\""),
        "{label} should emit ok JSON:\n{redacted}"
    );
    text
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().expect("path has parent")).expect("create parent dir");
    fs::write(path, body).expect("write fixture");
}

fn write_model_config(path: &Path, model_id: &str) {
    write(
        path,
        &format!(
            r#"
[models.task_agent]
model = "nex:{model_id}"
"#
        ),
    );
}

fn write_fake_llm(path: &Path, marker: &str) {
    write(path, &format!("{marker}\n"));
}

#[derive(Debug, Clone)]
struct CapturedHttpRequest {
    request_line: String,
    authorization: Option<String>,
    body: String,
}

fn start_auth_required_oai_stub(
    expected_requests: usize,
    expected_key: &str,
) -> (String, Arc<Mutex<Vec<CapturedHttpRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind oai stub");
    let addr = listener.local_addr().expect("stub local addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_thread = Arc::clone(&captured);
    let expected_auth = format!("Bearer {expected_key}");

    thread::spawn(move || {
        for _ in 0..expected_requests {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let Some((request_line, headers, body)) = read_http_request(&mut stream) else {
                return;
            };
            let authorization = headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.clone());
            captured_thread.lock().unwrap().push(CapturedHttpRequest {
                request_line,
                authorization: authorization.clone(),
                body,
            });

            if authorization.as_deref() != Some(expected_auth.as_str()) {
                let payload = br#"{"error":{"message":"No cookie auth credentials found","type":"invalid_request_error"}}"#;
                write_http_response(&mut stream, "401 Unauthorized", "application/json", payload);
                continue;
            }

            let sse = concat!(
                "data: {\"id\":\"fake-stream\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"auth ok\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"fake-stream\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1}}\n\n",
                "data: [DONE]\n\n",
            );
            write_http_response(&mut stream, "200 OK", "text/event-stream", sse.as_bytes());
        }
    });

    (base_url, captured)
}

fn read_http_request(
    stream: &mut std::net::TcpStream,
) -> Option<(String, Vec<(String, String)>, String)> {
    let mut buf = Vec::with_capacity(16 * 1024);
    let mut tmp = [0u8; 4096];
    let mut content_length: Option<usize> = None;
    let mut header_end: Option<usize> = None;
    loop {
        match stream.read(&mut tmp) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if header_end.is_none()
                    && let Some(idx) = buf.windows(4).position(|w| w == b"\r\n\r\n")
                {
                    header_end = Some(idx + 4);
                    let header_str = String::from_utf8_lossy(&buf[..idx]);
                    for line in header_str.lines() {
                        if let Some((name, value)) = line.split_once(':')
                            && name.eq_ignore_ascii_case("content-length")
                        {
                            content_length = value.trim().parse().ok();
                        }
                    }
                }
                if let (Some(he), Some(cl)) = (header_end, content_length)
                    && buf.len() >= he + cl
                {
                    break;
                }
            }
            Err(_) => return None,
        }
    }

    let he = header_end?;
    let header_str = String::from_utf8_lossy(&buf[..he.saturating_sub(4)]);
    let request_line = header_str.lines().next().unwrap_or("").to_string();
    let headers = header_str
        .lines()
        .skip(1)
        .filter_map(|line| {
            line.split_once(':')
                .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        })
        .collect();
    let body = String::from_utf8_lossy(&buf[he..]).to_string();
    Some((request_line, headers, body))
}

fn write_http_response(
    stream: &mut std::net::TcpStream,
    status: &str,
    content_type: &str,
    payload: &[u8],
) {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        payload.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(payload);
    let _ = stream.flush();
}

fn isolated_command(binary: PathBuf, home: &Path, cwd: &Path) -> Command {
    let mut cmd = Command::new(binary);
    cmd.env_clear();
    cmd.env("HOME", home);
    cmd.env("USER", "wg-test");
    cmd.env("TERM", "xterm-256color");
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.current_dir(cwd);
    cmd
}

fn list_journals(root: &Path, child: &str) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    let base = root.join(child);
    let Ok(entries) = fs::read_dir(&base) else {
        return out;
    };
    for entry in entries.flatten() {
        let journal = entry.path().join("conversation.jsonl");
        if journal.is_file() {
            out.insert(journal);
        }
    }
    out
}

fn standalone_journals(state_root: &Path) -> BTreeSet<PathBuf> {
    list_journals(state_root, "sessions")
}

fn wg_chat_journals(wg_dir: &Path) -> BTreeSet<PathBuf> {
    list_journals(wg_dir, "chat")
}

fn journal_init_model(journal: &Path) -> String {
    let text = fs::read_to_string(journal).expect("read journal");
    for line in text.lines() {
        let value: Value = serde_json::from_str(line).expect("journal line is json");
        if value.get("entry_type").and_then(Value::as_str) == Some("init") {
            return value
                .get("model")
                .and_then(Value::as_str)
                .expect("init has model")
                .to_string();
        }
    }
    panic!("journal has no init entry: {}", journal.display());
}

fn journal_contains(journal: &Path, needle: &str) -> bool {
    fs::read_to_string(journal)
        .expect("read journal")
        .contains(needle)
}

fn base_nex_args(prompt: &str) -> Vec<OsString> {
    vec![
        "--autonomous".into(),
        "--no-mcp".into(),
        "--minimal-tools".into(),
        "--max-turns".into(),
        "4".into(),
        prompt.into(),
    ]
}

fn run_standalone_nex(
    home: &Path,
    cwd: &Path,
    state_root: &Path,
    fake_llm: &Path,
    args: Vec<OsString>,
    envs: &[(&str, OsString)],
) -> (PathBuf, String) {
    let before = standalone_journals(state_root);
    let mut cmd = isolated_command(nex_binary(), home, cwd);
    cmd.env("WG_FAKE_LLM", fake_llm);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.args(args);

    let output = cmd.output().expect("spawn standalone nex");
    let text = output_text(&output);
    assert!(output.status.success(), "nex failed:\n{text}");

    let after = standalone_journals(state_root);
    let new: Vec<_> = after.difference(&before).cloned().collect();
    assert_eq!(
        new.len(),
        1,
        "expected one new standalone journal under {}, before={before:?}, after={after:?}, output:\n{text}",
        state_root.display()
    );
    (new[0].clone(), text)
}

fn run_wg_nex_autonomous(
    home: &Path,
    cwd: &Path,
    wg_dir: &Path,
    fake_llm: &Path,
    prompt: &str,
) -> (PathBuf, String) {
    let before = wg_chat_journals(wg_dir);
    let mut cmd = isolated_command(wg_binary(), home, cwd);
    cmd.env("WG_FAKE_LLM", fake_llm);
    cmd.args([
        "nex",
        "--autonomous",
        "--no-mcp",
        "--minimal-tools",
        "--max-turns",
        "4",
        prompt,
    ]);

    let output = cmd.output().expect("spawn wg nex");
    let text = output_text(&output);
    assert!(output.status.success(), "wg nex failed:\n{text}");

    let after = wg_chat_journals(wg_dir);
    let new: Vec<_> = after.difference(&before).cloned().collect();
    assert_eq!(
        new.len(),
        1,
        "expected one new wg chat journal under {}, before={before:?}, after={after:?}, output:\n{text}",
        wg_dir.display()
    );
    (new[0].clone(), text)
}

#[test]
fn standalone_nex_help_exposes_shared_options() {
    let output = Command::new(nex_binary())
        .arg("--help")
        .output()
        .expect("spawn nex --help");
    let text = output_text(&output);

    assert!(output.status.success(), "nex --help failed:\n{text}");
    assert!(
        text.contains("Usage: nex"),
        "standalone help should render as nex, got:\n{text}"
    );
    for flag in [
        "--model",
        "--endpoint",
        "--resume",
        "--chat",
        "--read-only",
        "--minimal-tools",
        "--eval-mode",
        "--api-key",
    ] {
        assert!(
            text.contains(flag),
            "standalone nex help missing {flag}:\n{text}"
        );
    }
}

#[test]
fn wg_nex_help_keeps_compatibility_options() {
    let output = Command::new(wg_binary())
        .args(["nex", "--help"])
        .output()
        .expect("spawn wg nex --help");
    let text = output_text(&output);

    assert!(output.status.success(), "wg nex --help failed:\n{text}");
    assert!(
        text.contains("Usage: wg nex") || text.contains("Usage: wg [OPTIONS] nex"),
        "wg nex help should render as a wg subcommand, got:\n{text}"
    );
    for flag in [
        "--model",
        "--endpoint",
        "--resume",
        "--chat",
        "--read-only",
        "--minimal-tools",
        "--eval-mode",
        "--api-key",
    ] {
        assert!(text.contains(flag), "wg nex help missing {flag}:\n{text}");
    }
}

#[test]
fn wg_scoped_eval_mode_uses_configured_openrouter_endpoint_credentials() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let wg_dir = project.join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    fs::create_dir_all(&home).unwrap();

    let expected_key = "wg-configured-openrouter-key";
    let key_file = tmp.path().join("openrouter.key");
    fs::write(&key_file, format!("{expected_key}\n")).unwrap();
    let (base_url, captured) = start_auth_required_oai_stub(2, expected_key);
    let endpoint = format!("{base_url}/v1");
    let model = "openrouter:minimax/minimax-m2.7";

    write(
        &wg_dir.join("config.toml"),
        &format!(
            r#"
[agent]
model = "{model}"

[dispatcher]
model = "{model}"

[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "{endpoint}"
api_key_file = "{}"
is_default = true
"#,
            key_file.display()
        ),
    );

    let mut wg_cmd = isolated_command(wg_binary(), &home, &project);
    wg_cmd.args([
        "nex",
        "--eval-mode",
        "--minimal-tools",
        "--model",
        model,
        "--endpoint",
        "openrouter",
        "--max-turns",
        "1",
        "Reply with exactly: WG_AUTH_OK",
    ]);
    let wg_output = wg_cmd.output().expect("spawn wg nex eval smoke");
    let wg_text = output_text(&wg_output);
    assert!(
        wg_output.status.success(),
        "wg nex eval-mode should use WG endpoint credentials:\n{wg_text}"
    );
    assert!(
        wg_text.contains("\"status\":\"ok\""),
        "wg nex eval-mode should emit ok JSON:\n{wg_text}"
    );

    let mut nex_cmd = isolated_command(nex_binary(), &home, &project);
    nex_cmd.args([
        "--wg",
        "--eval-mode",
        "--minimal-tools",
        "--model",
        model,
        "--endpoint",
        "openrouter",
        "--max-turns",
        "1",
        "Reply with exactly: NEX_AUTH_OK",
    ]);
    let nex_output = nex_cmd.output().expect("spawn nex --wg eval smoke");
    let nex_text = output_text(&nex_output);
    assert!(
        nex_output.status.success(),
        "nex --wg eval-mode should use WG endpoint credentials:\n{nex_text}"
    );
    assert!(
        nex_text.contains("\"status\":\"ok\""),
        "nex --wg eval-mode should emit ok JSON:\n{nex_text}"
    );

    let requests = captured.lock().unwrap().clone();
    assert_eq!(
        requests.len(),
        2,
        "both WG-scoped eval invocations should reach the configured endpoint; got {requests:?}"
    );
    for request in requests {
        assert!(
            request
                .request_line
                .starts_with("POST /v1/chat/completions"),
            "request should target the configured OAI endpoint, got {:?}",
            request.request_line
        );
        assert_eq!(
            request.authorization.as_deref(),
            Some("Bearer wg-configured-openrouter-key"),
            "configured endpoint key must be attached as Bearer auth"
        );
        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(
            body.get("model").and_then(Value::as_str),
            Some("minimax/minimax-m2.7"),
            "OpenRouter provider prefix should be stripped before the wire request"
        );
    }

    assert!(
        !project.join(".nex-eval").exists(),
        "WG-scoped eval mode must not divert state/config resolution into .nex-eval"
    );
}

#[test]
fn eval_mode_uses_configured_openrouter_endpoint_env_credentials() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let wg_dir = project.join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    fs::create_dir_all(&home).unwrap();

    let expected_key = "wg-configured-openrouter-env-key";
    let (base_url, captured) = start_auth_required_oai_stub(3, expected_key);
    let endpoint = format!("{base_url}/v1");
    let model = "openrouter:minimax/minimax-m2.7";

    write(
        &wg_dir.join("config.toml"),
        &format!(
            r#"
[agent]
model = "{model}"

[dispatcher]
model = "{model}"

[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "{endpoint}"
api_key_env = "OPENROUTER_API_KEY"
is_default = true
"#
        ),
    );

    let mut wg_cmd = isolated_command(wg_binary(), &home, &project);
    wg_cmd.env("OPENROUTER_API_KEY", expected_key);
    wg_cmd.args([
        "nex",
        "--eval-mode",
        "--minimal-tools",
        "--model",
        model,
        "--endpoint",
        "openrouter",
        "--max-turns",
        "1",
        "Reply with exactly: WG_ENV_AUTH_OK",
    ]);
    let wg_output = wg_cmd.output().expect("spawn wg nex env eval smoke");
    assert_eval_success_without_key("wg nex env eval-mode", &wg_output, expected_key);

    let mut nex_wg_cmd = isolated_command(nex_binary(), &home, &project);
    nex_wg_cmd.env("OPENROUTER_API_KEY", expected_key);
    nex_wg_cmd.args([
        "--wg",
        "--eval-mode",
        "--minimal-tools",
        "--model",
        model,
        "--endpoint",
        "openrouter",
        "--max-turns",
        "1",
        "Reply with exactly: NEX_WG_ENV_AUTH_OK",
    ]);
    let nex_wg_output = nex_wg_cmd.output().expect("spawn nex --wg env eval smoke");
    assert_eval_success_without_key("nex --wg env eval-mode", &nex_wg_output, expected_key);

    assert!(
        !project.join(".nex-eval").exists(),
        "WG-scoped eval mode must not divert state/config resolution into .nex-eval"
    );

    let mut standalone_cmd = isolated_command(nex_binary(), &home, &project);
    standalone_cmd.env("OPENROUTER_API_KEY", expected_key);
    standalone_cmd.args([
        "--eval-mode",
        "--minimal-tools",
        "--model",
        model,
        "--endpoint",
        "openrouter",
        "--max-turns",
        "1",
        "Reply with exactly: STANDALONE_ENV_AUTH_OK",
    ]);
    let standalone_output = standalone_cmd
        .output()
        .expect("spawn standalone nex env eval smoke");
    assert_eval_success_without_key(
        "standalone nex env eval-mode",
        &standalone_output,
        expected_key,
    );

    let requests = captured.lock().unwrap().clone();
    assert_eq!(
        requests.len(),
        3,
        "wg nex, nex --wg, and standalone nex should reach the configured endpoint; got {} requests",
        requests.len()
    );
    let expected_auth = format!("Bearer {expected_key}");
    for request in requests {
        assert!(
            request
                .request_line
                .starts_with("POST /v1/chat/completions"),
            "request should target the configured OAI endpoint, got {:?}",
            request.request_line
        );
        assert!(
            request.authorization.as_deref() == Some(expected_auth.as_str()),
            "configured api_key_env value must be attached as Bearer auth"
        );
        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(
            body.get("model").and_then(Value::as_str),
            Some("minimax/minimax-m2.7"),
            "OpenRouter provider prefix should be stripped before the wire request"
        );
    }
}

#[test]
fn standalone_nex_binary_starts_fresh_project_sessions_with_isolated_home_and_cwd() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let nested = project.join("src").join("deep");
    let project_nex = project.join(".nex");
    fs::create_dir_all(&project_nex).unwrap();
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(home.join(".nex")).unwrap();
    let fake = tmp.path().join("fake-llm.txt");
    write_fake_llm(&fake, "FRESH_SESSION_MARKER");

    let (first, first_output) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        {
            let mut args = base_nex_args("first fresh session");
            args.splice(0..0, ["--model".into(), "fresh-model".into()]);
            args
        },
        &[],
    );
    assert!(
        journal_contains(&first, "FRESH_SESSION_MARKER"),
        "fake LLM response should be recorded in the journal; output:\n{first_output}"
    );

    let (second, second_output) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        {
            let mut args = base_nex_args("second fresh session");
            args.splice(0..0, ["--model".into(), "fresh-model".into()]);
            args
        },
        &[],
    );
    assert!(
        journal_contains(&second, "FRESH_SESSION_MARKER"),
        "fake LLM response should be recorded in the journal; output:\n{second_output}"
    );

    assert_ne!(first, second, "bare standalone nex must not auto-resume");
    assert_eq!(standalone_journals(&project_nex).len(), 2);
    assert!(
        standalone_journals(&home.join(".nex")).is_empty(),
        "project .nex should own state when discovered from cwd"
    );
    assert_eq!(journal_init_model(&first), "fresh-model");
    assert_eq!(journal_init_model(&second), "fresh-model");
    assert!(journal_contains(&first, "first fresh session"));
    assert!(journal_contains(&second, "second fresh session"));
}

#[test]
fn standalone_nex_binary_respects_cli_env_project_user_legacy_default_precedence() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let nested = project.join("nested");
    let project_nex = project.join(".nex");
    let env_nex = tmp.path().join("env-nex");
    let fake = tmp.path().join("fake-llm.txt");
    write_fake_llm(&fake, "PRECEDENCE_MARKER");

    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(&project_nex).unwrap();
    fs::create_dir_all(&env_nex).unwrap();
    write_model_config(&project_nex.join("config.toml"), "project-model");
    write_model_config(&env_nex.join("config.toml"), "env-dir-model");
    write_model_config(&home.join(".nex").join("config.toml"), "user-model");
    write_model_config(&home.join(".wg").join("config.toml"), "legacy-home-model");
    write_model_config(
        &project.join(".wg").join("config.toml"),
        "legacy-project-model",
    );

    let env_config = tmp.path().join("env-config.toml");
    let cli_config = tmp.path().join("cli-config.toml");
    write_model_config(&env_config, "env-config-model");
    write_model_config(&cli_config, "cli-config-model");

    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        {
            let mut args = base_nex_args("cli model wins");
            args.splice(0..0, ["--model".into(), "cli-model".into()]);
            args
        },
        &[
            ("NEX_MODEL", "env-model".into()),
            ("NEX_CONFIG", env_config.as_os_str().to_os_string()),
        ],
    );
    assert_eq!(journal_init_model(&journal), "cli-model");

    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        {
            let mut args = base_nex_args("cli config wins");
            args.splice(
                0..0,
                ["--config".into(), cli_config.as_os_str().to_os_string()],
            );
            args
        },
        &[("NEX_CONFIG", env_config.as_os_str().to_os_string())],
    );
    assert_eq!(journal_init_model(&journal), "cli-config-model");

    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("nex model wins"),
        &[
            ("NEX_MODEL", "env-model".into()),
            ("NEX_CONFIG", env_config.as_os_str().to_os_string()),
        ],
    );
    assert_eq!(journal_init_model(&journal), "env-model");

    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &env_nex,
        &fake,
        base_nex_args("nex dir wins"),
        &[("NEX_DIR", env_nex.as_os_str().to_os_string())],
    );
    assert_eq!(journal_init_model(&journal), "env-dir-model");

    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("nex config wins"),
        &[("NEX_CONFIG", env_config.as_os_str().to_os_string())],
    );
    assert_eq!(journal_init_model(&journal), "env-config-model");

    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("project config wins"),
        &[],
    );
    assert_eq!(journal_init_model(&journal), "project-model");

    fs::remove_file(project_nex.join("config.toml")).unwrap();
    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("user config wins"),
        &[],
    );
    assert_eq!(journal_init_model(&journal), "user-model");

    fs::remove_file(home.join(".nex").join("config.toml")).unwrap();
    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("legacy project wg fallback wins"),
        &[],
    );
    assert_eq!(journal_init_model(&journal), "legacy-project-model");

    fs::remove_file(project.join(".wg").join("config.toml")).unwrap();
    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("legacy home wg fallback wins"),
        &[],
    );
    assert_eq!(journal_init_model(&journal), "legacy-home-model");

    fs::remove_file(home.join(".wg").join("config.toml")).unwrap();
    let (journal, _) = run_standalone_nex(
        &home,
        &nested,
        &project_nex,
        &fake,
        base_nex_args("default model wins"),
        &[],
    );
    let default_model = Config::default()
        .resolve_model_for_role(DispatchRole::TaskAgent)
        .model;
    assert_eq!(journal_init_model(&journal), default_model);
}

#[test]
fn standalone_runtime_merges_endpoints_and_model_registry_entries_by_identity() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let nested = project.join("nested");
    fs::create_dir_all(project.join(".nex")).unwrap();
    fs::create_dir_all(project.join(".wg")).unwrap();
    fs::create_dir_all(&nested).unwrap();

    write(
        &home.join(".wg").join("config.toml"),
        r#"
[[llm_endpoints.endpoints]]
name = "shared"
provider = "openai"
url = "https://legacy.invalid/v1"

[[llm_endpoints.endpoints]]
name = "legacy-only"
provider = "openai"
url = "https://legacy-only.invalid/v1"

[[model_registry]]
id = "shared-model"
provider = "openai"
model = "legacy-wire"
tier = "standard"

[[model_registry]]
id = "legacy-model"
provider = "openai"
model = "legacy-only-wire"
tier = "standard"
"#,
    );
    write(
        &home.join(".nex").join("config.toml"),
        r#"
[[llm_endpoints.endpoints]]
name = "shared"
provider = "openai"
url = "https://user.invalid/v1"

[[llm_endpoints.endpoints]]
name = "user-only"
provider = "openai"
url = "https://user-only.invalid/v1"

[[model_registry]]
id = "shared-model"
provider = "openai"
model = "user-wire"
tier = "standard"

[[model_registry]]
id = "user-model"
provider = "openai"
model = "user-only-wire"
tier = "standard"
"#,
    );
    write(
        &project.join(".nex").join("config.toml"),
        r#"
[[llm_endpoints.endpoints]]
name = "shared"
provider = "openai"
url = "https://project.invalid/v1"

[[llm_endpoints.endpoints]]
name = "project-only"
provider = "openai"
url = "https://project-only.invalid/v1"

[[model_registry]]
id = "shared-model"
provider = "openai"
model = "project-wire"
tier = "standard"

[[model_registry]]
id = "project-model"
provider = "openai"
model = "project-only-wire"
tier = "standard"
"#,
    );

    let runtime = resolve_standalone(&NexRuntimeResolveInput {
        cwd: Some(nested),
        home_dir: Some(home),
        ..Default::default()
    });
    let config = load_config(&runtime).unwrap();

    assert_eq!(
        config
            .llm_endpoints
            .find_by_name("shared")
            .and_then(|ep| ep.url.as_deref()),
        Some("https://project.invalid/v1")
    );
    for name in ["legacy-only", "user-only", "project-only"] {
        assert!(
            config.llm_endpoints.find_by_name(name).is_some(),
            "endpoint {name} should be preserved"
        );
    }
    assert_eq!(
        config
            .llm_endpoints
            .endpoints
            .iter()
            .filter(|ep| ep.name == "shared")
            .count(),
        1,
        "same endpoint name should merge instead of duplicating"
    );

    assert_eq!(
        config
            .registry_lookup("shared-model")
            .map(|entry| entry.model),
        Some("project-wire".to_string())
    );
    for id in ["legacy-model", "user-model", "project-model"] {
        assert!(
            config.registry_lookup(id).is_some(),
            "model registry id {id} should be preserved"
        );
    }
}

#[test]
fn wg_nex_autonomous_binary_ignores_human_standalone_nex_routing_state() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let project = tmp.path().join("project");
    let wg_dir = project.join(".wg");
    let fake = tmp.path().join("fake-llm.txt");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&wg_dir).unwrap();
    write_fake_llm(&fake, "WG_AUTONOMOUS_MARKER");
    write_model_config(
        &home.join(".nex").join("config.toml"),
        "human-standalone-model",
    );
    write(
        &home.join(".nex").join("config.toml"),
        r#"
[models.task_agent]
model = "nex:human-standalone-model"

[[llm_endpoints.endpoints]]
name = "human"
provider = "openai"
url = "https://human.invalid/v1"
is_default = true
"#,
    );
    write_model_config(&wg_dir.join("config.toml"), "wg-autonomous-model");

    let runtime = resolve_wg_autonomous(&wg_dir, Some(home.clone()));
    assert!(
        runtime
            .config_paths
            .iter()
            .all(|path| !path.starts_with(home.join(".nex"))),
        "autonomous wg runtime must not include human ~/.nex config paths: {:?}",
        runtime.config_paths
    );
    let config = load_config(&runtime).unwrap();
    assert_eq!(
        config.resolve_model_for_role(DispatchRole::TaskAgent).model,
        "wg-autonomous-model"
    );
    assert!(config.llm_endpoints.find_by_name("human").is_none());

    let (journal, output) =
        run_wg_nex_autonomous(&home, &project, &wg_dir, &fake, "wg autonomous isolation");
    assert!(
        journal_contains(&journal, "WG_AUTONOMOUS_MARKER"),
        "fake LLM response should be recorded in the journal; output:\n{output}"
    );
    assert_eq!(journal_init_model(&journal), "wg-autonomous-model");
    assert!(
        standalone_journals(&home.join(".nex")).is_empty(),
        "wg nex autonomous should not create or reuse standalone ~/.nex sessions"
    );
}
