//! Integration coverage for `wg login openrouter`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use tempfile::TempDir;

fn wg_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("could not get current exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("wg");
    assert!(path.exists(), "wg binary not found at {:?}", path);
    path
}

fn init_wg_dir(root: &Path) -> PathBuf {
    let wg_dir = root.join(".wg");
    fs::create_dir_all(&wg_dir).unwrap();
    fs::write(wg_dir.join("graph.jsonl"), "").unwrap();
    wg_dir
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

#[test]
fn login_openrouter_from_stdin_writes_secret_ref_and_unblocks_models_fetch() {
    let temp = TempDir::new().unwrap();
    let fake_home = temp.path().join("home");
    let project_root = temp.path().join("project");
    fs::create_dir_all(&fake_home).unwrap();
    fs::create_dir_all(&project_root).unwrap();
    let wg_dir = init_wg_dir(&project_root);

    let model_body = r#"{
        "data": [
            {
                "id": "openai/gpt-4o-mini",
                "name": "GPT-4o mini",
                "description": "test",
                "context_length": 128000,
                "pricing": {"prompt":"0.00000015","completion":"0.0000006"},
                "supported_parameters": ["tools"]
            }
        ]
    }"#;
    let (base_url, requests, handle) = start_mock_server(vec![
        Route {
            method: "GET",
            path: "/api/v1/models",
            status: 200,
            body: model_body,
        },
        Route {
            method: "GET",
            path: "/api/v1/models",
            status: 200,
            body: model_body,
        },
        Route {
            method: "GET",
            path: "/api/v1/models",
            status: 200,
            body: model_body,
        },
    ]);

    fs::create_dir_all(fake_home.join(".wg")).unwrap();
    fs::write(
        wg_dir.join("config.toml"),
        format!(
            r#"[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "{base_url}/api/v1"
is_default = true
"#
        ),
    )
    .unwrap();

    let mut login_cmd = Command::new(wg_binary());
    login_cmd
        .arg("--dir")
        .arg(&wg_dir)
        .args([
            "login",
            "openrouter",
            "--from-stdin",
            "--backend",
            "keystore",
            "--local",
        ])
        .env("HOME", &fake_home)
        .env("OPENROUTER_BASE_URL", format!("{base_url}/api/v1"))
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("WG_GLOBAL_DIR")
        .env_remove("WG_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = login_cmd.spawn().expect("spawn wg login");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"sk-or-fake-login-test\n")
        .unwrap();
    let login_out = child.wait_with_output().unwrap();
    assert!(
        login_out.status.success(),
        "wg login openrouter failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&login_out.stdout),
        String::from_utf8_lossy(&login_out.stderr),
    );
    let login_stdout = String::from_utf8_lossy(&login_out.stdout);
    assert!(login_stdout.contains("secret: present (keystore:openrouter)"));
    assert!(login_stdout.contains("auth: ok"));

    let saved_config = fs::read_to_string(wg_dir.join("config.toml")).unwrap();
    assert!(saved_config.contains(r#"api_key_ref = "keystore:openrouter""#));
    assert!(!saved_config.contains("sk-or-fake-login-test"));
    assert!(!saved_config.contains("api_key ="));

    let fetch = Command::new(wg_binary())
        .arg("--dir")
        .arg(&wg_dir)
        .args(["models", "fetch", "--no-cache"])
        .env("HOME", &fake_home)
        .env("OPENROUTER_BASE_URL", format!("{base_url}/api/v1"))
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("WG_GLOBAL_DIR")
        .env_remove("WG_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        fetch.status.success(),
        "wg models fetch --no-cache failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&fetch.stdout),
        String::from_utf8_lossy(&fetch.stderr),
    );

    let check = Command::new(wg_binary())
        .arg("--dir")
        .arg(&wg_dir)
        .args(["login", "openrouter", "--check"])
        .env("HOME", &fake_home)
        .env("OPENROUTER_BASE_URL", format!("{base_url}/api/v1"))
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("WG_GLOBAL_DIR")
        .env_remove("WG_DIR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "wg login openrouter --check failed.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr),
    );
    let check_stdout = String::from_utf8_lossy(&check.stdout);
    assert!(check_stdout.contains("OpenRouter (WG)"));
    assert!(check_stdout.contains("secret: present (keystore:openrouter)"));
    assert!(check_stdout.contains("OpenRouter (Pi)"));

    let requests = requests.lock().unwrap();
    assert!(
        requests.iter().any(|request| {
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer sk-or-fake-login-test")
        }),
        "mock server never observed the configured secret-backed Authorization header: {:?}",
        *requests
    );
    handle.join().unwrap();
}
