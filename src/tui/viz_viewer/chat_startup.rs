//! Prioritized asynchronous chat startup and startup milestone reporting.
//!
//! This lane is deliberately independent from the full graph view bootstrap.
//! It loads only the persisted active-tab pointer, the authoritative graph task
//! metadata needed to prove that chat still exists, and the atomic route needed
//! for a new process. Existing tmux sessions can therefore reattach before graph
//! layout, log enrichment, or history projection finish.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::time::Instant;

use super::state::{BootstrapApply, PendingChatPtySpawn, VizApp};
use crate::tui::pty_pane::PtyPane;

const REQUEST_CAPACITY: usize = 1;
const RESULT_CAPACITY: usize = 2;

struct Request {
    generation: u64,
    workgraph_dir: PathBuf,
}

struct Response {
    generation: u64,
    value: Result<BootstrapApply, String>,
}

/// Bounded, generation-checked broker for the active-chat metadata lane.
pub struct Engine {
    request_tx: SyncSender<Request>,
    result_rx: Receiver<Response>,
    pending: Option<Request>,
    next_generation: u64,
    desired_generation: u64,
}

impl Engine {
    pub fn new() -> Self {
        let (request_tx, request_rx) = mpsc::sync_channel::<Request>(REQUEST_CAPACITY);
        let (result_tx, result_rx) = mpsc::sync_channel::<Response>(RESULT_CAPACITY);
        let _ = std::thread::Builder::new()
            .name("wg-tui-chat-startup".into())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    let value = VizApp::load_chat_startup(request.workgraph_dir)
                        .map_err(|error| error.to_string());
                    let _ = result_tx.try_send(Response {
                        generation: request.generation,
                        value,
                    });
                }
            });
        Self {
            request_tx,
            result_rx,
            pending: None,
            next_generation: 0,
            desired_generation: 0,
        }
    }

    /// Submit without blocking. At most the latest unsent request is retained.
    pub fn request(&mut self, workgraph_dir: PathBuf) -> u64 {
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.desired_generation = self.next_generation;
        self.pending = Some(Request {
            generation: self.desired_generation,
            workgraph_dir,
        });
        self.pump();
        self.desired_generation
    }

    fn pump(&mut self) {
        let Some(request) = self.pending.take() else {
            return;
        };
        match self.request_tx.try_send(request) {
            Ok(()) => {}
            Err(TrySendError::Full(request)) => self.pending = Some(request),
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    pub fn try_result(&mut self) -> Option<Result<BootstrapApply, String>> {
        self.pump();
        loop {
            match self.result_rx.try_recv() {
                Ok(response) if response.generation == self.desired_generation => {
                    return Some(response.value);
                }
                Ok(_) => self.pump(),
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    return Some(Err("chat startup worker stopped".to_string()));
                }
            }
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

struct PtyRequest {
    generation: u64,
    pending: PendingChatPtySpawn,
    rows: u16,
    cols: u16,
}

pub struct PtyResult {
    pub generation: u64,
    pub pending: PendingChatPtySpawn,
    pub value: Result<PtyPane, String>,
}

/// Fixed, bounded process lane. tmux discovery/new-session/attach and PTY
/// creation happen here, never in `draw_chat_tab`.
pub struct PtyEngine {
    request_tx: SyncSender<PtyRequest>,
    result_rx: Receiver<PtyResult>,
    next_generation: u64,
    desired_generation: u64,
    in_flight: bool,
}

impl PtyEngine {
    pub fn new() -> Self {
        let (request_tx, request_rx) = mpsc::sync_channel::<PtyRequest>(1);
        let (result_tx, result_rx) = mpsc::sync_channel::<PtyResult>(2);
        let _ = std::thread::Builder::new()
            .name("wg-tui-chat-pty".into())
            .spawn(move || {
                while let Ok(request) = request_rx.recv() {
                    let refs: Vec<&str> = request.pending.args.iter().map(String::as_str).collect();
                    let value = if let Some(session) = request.pending.tmux_session.as_deref() {
                        PtyPane::spawn_via_tmux(
                            session,
                            &request.pending.bin,
                            &refs,
                            &request.pending.env,
                            request.pending.cwd.as_deref(),
                            request.rows,
                            request.cols,
                        )
                        .or_else(|tmux_error| {
                            if request.pending.reattach {
                                Err(tmux_error)
                            } else {
                                PtyPane::spawn_in(
                                    &request.pending.bin,
                                    &refs,
                                    &request.pending.env,
                                    request.pending.cwd.as_deref(),
                                    request.rows,
                                    request.cols,
                                )
                            }
                        })
                        .map_err(|error| error.to_string())
                    } else {
                        PtyPane::spawn_in(
                            &request.pending.bin,
                            &refs,
                            &request.pending.env,
                            request.pending.cwd.as_deref(),
                            request.rows,
                            request.cols,
                        )
                        .map_err(|error| error.to_string())
                    };
                    // The tmux client process can emit its first repaint before
                    // it has finished installing input forwarding. Give that
                    // handshake a small bounded grace period on this worker so
                    // startup-buffered keys cannot be acknowledged then lost.
                    if request.pending.reattach && value.is_ok() {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    let _ = result_tx.try_send(PtyResult {
                        generation: request.generation,
                        pending: request.pending,
                        value,
                    });
                }
            });
        Self {
            request_tx,
            result_rx,
            next_generation: 0,
            desired_generation: 0,
            in_flight: false,
        }
    }

    pub fn request(&mut self, pending: PendingChatPtySpawn, rows: u16, cols: u16) -> bool {
        if self.in_flight || rows == 0 || cols == 0 {
            return false;
        }
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.desired_generation = self.next_generation;
        match self.request_tx.try_send(PtyRequest {
            generation: self.desired_generation,
            pending,
            rows,
            cols,
        }) {
            Ok(()) => {
                self.in_flight = true;
                true
            }
            Err(_) => false,
        }
    }

    pub fn try_result(&mut self) -> Option<PtyResult> {
        loop {
            match self.result_rx.try_recv() {
                Ok(result) if result.generation == self.desired_generation => {
                    self.in_flight = false;
                    return Some(result);
                }
                Ok(_) => {}
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return None,
            }
        }
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight
    }
}

/// Nonblocking milestone sink used by smoke tests and field diagnostics.
///
/// Set `WG_TUI_STARTUP_TRACE=/path/to/startup.jsonl`. The UI thread only
/// `try_send`s bounded messages; a dedicated writer owns all filesystem I/O.
pub struct Reporter {
    started: Instant,
    tx: Option<SyncSender<String>>,
}

impl Reporter {
    pub fn new() -> Self {
        let started = Instant::now();
        let tx = std::env::var_os("WG_TUI_STARTUP_TRACE").and_then(|raw| {
            let path = PathBuf::from(raw);
            let (tx, rx) = mpsc::sync_channel::<String>(32);
            std::thread::Builder::new()
                .name("wg-tui-startup-trace".into())
                .spawn(move || {
                    let mut file = OpenOptions::new().create(true).append(true).open(path).ok();
                    while let Ok(line) = rx.recv() {
                        if let Some(file) = file.as_mut() {
                            let _ = file.write_all(line.as_bytes());
                            let _ = file.write_all(b"\n");
                            let _ = file.flush();
                        }
                    }
                })
                .ok()
                .map(|_| tx)
        });
        Self { started, tx }
    }

    pub fn record(&self, milestone: &str, detail: Option<&str>) {
        let Some(tx) = self.tx.as_ref() else {
            return;
        };
        let line = serde_json::json!({
            "milestone": milestone,
            "elapsed_ms": self.started.elapsed().as_millis(),
            "detail": detail,
        })
        .to_string();
        let _ = tx.try_send(line);
    }
}

impl Default for Reporter {
    fn default() -> Self {
        Self::new()
    }
}
