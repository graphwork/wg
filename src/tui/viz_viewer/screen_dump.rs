//! TUI screen dump server: provides an IPC mechanism for external agents to
//! read the current TUI screen contents as structured plain text.
//!
//! The server listens on a Unix domain socket at `.wg/service/tui.sock`.
//! After each frame render, the event loop updates a shared buffer.  Clients
//! connect, send a JSON request, and receive a JSON response containing the
//! current screen text plus metadata (dimensions, active tab, selected task,
//! input mode).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use ratatui::buffer::Buffer;
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::os::unix::net::UnixListener;

// ── Shared screen state ─────────────────────────────────────────────────────

/// Snapshot of the TUI screen, updated after each render frame.
#[derive(Clone, Default)]
pub struct ScreenSnapshot {
    /// Plain-text representation of the screen (no ANSI codes).
    pub text: String,
    /// Terminal width in columns.
    pub width: u16,
    /// Terminal height in rows.
    pub height: u16,
    /// Currently active right-panel tab name.
    pub active_tab: String,
    /// Currently focused panel ("graph" or "panel").
    pub focused_panel: String,
    /// Currently selected task ID (if any).
    pub selected_task: Option<String>,
    /// Current input mode.
    pub input_mode: String,
    /// Active coordinator ID.
    pub coordinator_id: u32,
}

/// Thread-safe handle to the latest screen snapshot.
pub type SharedScreen = Arc<Mutex<ScreenSnapshot>>;

/// Create a new shared screen handle.
pub fn new_shared_screen() -> SharedScreen {
    Arc::new(Mutex::new(ScreenSnapshot::default()))
}

/// Update the shared screen snapshot from a ratatui buffer and app state.
pub fn update_snapshot(
    shared: &SharedScreen,
    buf: &Buffer,
    active_tab: &str,
    focused_panel: &str,
    selected_task: Option<&str>,
    input_mode: &str,
    coordinator_id: u32,
) {
    let text = buffer_to_text(buf);
    let area = buf.area;
    let snapshot = ScreenSnapshot {
        text,
        width: area.width,
        height: area.height,
        active_tab: active_tab.to_string(),
        focused_panel: focused_panel.to_string(),
        selected_task: selected_task.map(|s| s.to_string()),
        input_mode: input_mode.to_string(),
        coordinator_id,
    };
    if let Ok(mut guard) = shared.lock() {
        *guard = snapshot;
    }
}

// ── Buffer to plain text ────────────────────────────────────────────────────

/// Convert a ratatui buffer to plain text (no ANSI escape codes).
///
/// Each row becomes one line. Trailing spaces on each line are trimmed.
pub fn buffer_to_text(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::with_capacity((area.width as usize + 1) * area.height as usize);

    for y in area.top()..area.bottom() {
        if y > area.top() {
            out.push('\n');
        }

        let mut line = String::with_capacity(area.width as usize);
        for x in area.left()..area.right() {
            let cell = &buf[(x, y)];
            let symbol = cell.symbol();
            if symbol.is_empty() {
                // Continuation cell of a wide character — skip
                continue;
            }
            line.push_str(symbol);
        }

        // Trim trailing spaces to reduce output size for LLM consumption.
        out.push_str(line.trim_end());
    }

    out
}

// ── IPC protocol ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum DumpRequest {
    /// Get the current screen contents as text.
    Dump,
}

#[derive(Debug, Serialize, Deserialize)]
struct DumpResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_tab: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    focused_panel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    coordinator_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

impl DumpResponse {
    fn success(snap: &ScreenSnapshot) -> Self {
        Self {
            ok: true,
            error: None,
            width: Some(snap.width),
            height: Some(snap.height),
            active_tab: Some(snap.active_tab.clone()),
            focused_panel: Some(snap.focused_panel.clone()),
            selected_task: snap.selected_task.clone(),
            input_mode: Some(snap.input_mode.clone()),
            coordinator_id: Some(snap.coordinator_id),
            text: Some(snap.text.clone()),
        }
    }

    fn error(msg: &str) -> Self {
        Self {
            ok: false,
            error: Some(msg.to_string()),
            width: None,
            height: None,
            active_tab: None,
            focused_panel: None,
            selected_task: None,
            input_mode: None,
            coordinator_id: None,
            text: None,
        }
    }
}

// ── Socket server ───────────────────────────────────────────────────────────

/// Path to the TUI dump socket within a workgraph directory.
pub fn tui_socket_path(workgraph_dir: &Path) -> PathBuf {
    workgraph_dir.join("service").join("tui.sock")
}

/// Start the screen dump server in a background thread.
///
/// Returns `Ok(())` after spawning the listener.  The listener runs until
/// `shutdown` is set to `true` (typically when the TUI exits).
#[cfg(unix)]
pub fn start_server(
    workgraph_dir: &Path,
    shared: SharedScreen,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let socket_path = tui_socket_path(workgraph_dir);

    // Ensure the service directory exists.
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Remove stale socket file if present.
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    let listener = UnixListener::bind(&socket_path)?;

    // Non-blocking so we can check the shutdown flag periodically.
    listener.set_nonblocking(true)?;

    let socket_path_clone = socket_path.clone();
    std::thread::Builder::new()
        .name("tui-dump-server".into())
        .spawn(move || {
            while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // Handle connection inline (fast — just reading/writing a snapshot).
                        let _ = handle_dump_connection(stream, &shared);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // No pending connection — sleep briefly and retry.
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(_) => {
                        // Accept error — sleep and retry.
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                }
            }
            // Clean up socket file.
            let _ = std::fs::remove_file(&socket_path_clone);
        })?;

    Ok(())
}

#[cfg(unix)]
fn handle_dump_connection(
    stream: std::os::unix::net::UnixStream,
    shared: &SharedScreen,
) -> Result<()> {
    use std::time::Duration;

    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;

    let mut write_stream = stream.try_clone()?;
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        if line.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<DumpRequest>(&line) {
            Ok(DumpRequest::Dump) => {
                let snap = shared.lock().map_err(|e| anyhow::anyhow!("{}", e))?;
                DumpResponse::success(&snap)
            }
            Err(e) => DumpResponse::error(&format!("invalid request: {}", e)),
        };

        let mut json = serde_json::to_string(&response)?;
        json.push('\n');
        write_stream.write_all(json.as_bytes())?;
        write_stream.flush()?;
        break; // One request per connection.
    }

    Ok(())
}

// ── Client (for `wg tui dump`) ──────────────────────────────────────────────

/// Connect to a running TUI and retrieve the current screen dump.
#[cfg(unix)]
pub fn client_dump(workgraph_dir: &Path) -> Result<ScreenSnapshot> {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let socket_path = tui_socket_path(workgraph_dir);
    if !socket_path.exists() {
        anyhow::bail!(
            "TUI is not running (no socket at {}). Start it with `wg tui`.",
            socket_path.display()
        );
    }

    let mut stream = UnixStream::connect(&socket_path)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    // Send dump request.
    let request = r#"{"cmd":"dump"}"#;
    stream.write_all(request.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;

    // Read response.
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }

        let resp: DumpResponse = serde_json::from_str(&line)?;
        if !resp.ok {
            anyhow::bail!(
                "TUI dump failed: {}",
                resp.error.unwrap_or_else(|| "unknown error".into())
            );
        }

        return Ok(ScreenSnapshot {
            text: resp.text.unwrap_or_default(),
            width: resp.width.unwrap_or(0),
            height: resp.height.unwrap_or(0),
            active_tab: resp.active_tab.unwrap_or_default(),
            focused_panel: resp.focused_panel.unwrap_or_default(),
            selected_task: resp.selected_task,
            input_mode: resp.input_mode.unwrap_or_default(),
            coordinator_id: resp.coordinator_id.unwrap_or(0),
        });
    }

    anyhow::bail!("no response from TUI dump server")
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::Paragraph;

    #[test]
    fn buffer_to_text_basic() {
        let backend = TestBackend::new(10, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let para = Paragraph::new("Hello\nWorld\nTest");
                frame.render_widget(para, frame.area());
            })
            .unwrap();

        let text = buffer_to_text(terminal.backend().buffer());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "Hello");
        assert_eq!(lines[1], "World");
        assert_eq!(lines[2], "Test");
    }

    #[test]
    fn buffer_to_text_trims_trailing_spaces() {
        let backend = TestBackend::new(20, 2);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let para = Paragraph::new("Short");
                frame.render_widget(para, frame.area());
            })
            .unwrap();

        let text = buffer_to_text(terminal.backend().buffer());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Short");
        // Line should not have trailing spaces
        assert!(!lines[0].ends_with(' '));
    }

    #[test]
    fn snapshot_update_roundtrip() {
        let shared = new_shared_screen();

        let backend = TestBackend::new(10, 2);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let para = Paragraph::new("Test");
                frame.render_widget(para, frame.area());
            })
            .unwrap();

        update_snapshot(
            &shared,
            terminal.backend().buffer(),
            "Detail",
            "graph",
            Some("my-task"),
            "Normal",
            0,
        );

        let snap = shared.lock().unwrap();
        assert_eq!(snap.width, 10);
        assert_eq!(snap.height, 2);
        assert_eq!(snap.active_tab, "Detail");
        assert_eq!(snap.focused_panel, "graph");
        assert_eq!(snap.selected_task.as_deref(), Some("my-task"));
        assert_eq!(snap.input_mode, "Normal");
        assert!(snap.text.contains("Test"));
    }

    #[cfg(unix)]
    #[test]
    fn server_client_roundtrip() {
        use std::sync::atomic::AtomicBool;

        let tmp = tempfile::tempdir().unwrap();
        let wg_dir = tmp.path();
        std::fs::create_dir_all(wg_dir.join("service")).unwrap();

        let shared = new_shared_screen();
        let shutdown = Arc::new(AtomicBool::new(false));

        // Populate a snapshot.
        {
            let mut snap = shared.lock().unwrap();
            snap.text = "Hello from TUI".to_string();
            snap.width = 80;
            snap.height = 24;
            snap.active_tab = "Chat".to_string();
            snap.focused_panel = "graph".to_string();
            snap.input_mode = "Normal".to_string();
        }

        start_server(wg_dir, shared, shutdown.clone()).unwrap();

        // Give the server thread a moment to bind.
        std::thread::sleep(std::time::Duration::from_millis(100));

        // Client dump.
        let snap = client_dump(wg_dir).unwrap();
        assert_eq!(snap.text, "Hello from TUI");
        assert_eq!(snap.width, 80);
        assert_eq!(snap.height, 24);
        assert_eq!(snap.active_tab, "Chat");
        assert_eq!(snap.focused_panel, "graph");
        assert_eq!(snap.input_mode, "Normal");

        // Shut down.
        shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
