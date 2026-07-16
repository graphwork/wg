//! Bounded, versioned asynchronous bootstrap for the TUI.
//!
//! The terminal thread owns only this broker.  All project storage access is
//! behind [`StorageBackend`] and runs on a detached, fixed worker.  Requests
//! are coalesced to the newest generation, results are bounded, stale
//! generations are rejected, and dropping the broker never joins a worker
//! stuck in an uninterruptible network-filesystem call.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::commands::viz::VizOptions;

use super::state::{BootstrapApply, VizApp};

const REQUEST_CAPACITY: usize = 1;
const RESULT_CAPACITY: usize = 4;
const FEEDBACK_THRESHOLD: Duration = Duration::from_millis(150);

#[derive(Clone)]
pub struct BootstrapArgs {
    pub workgraph_dir: PathBuf,
    pub viz_options: VizOptions,
    pub mouse_override: Option<bool>,
    pub history_depth_override: Option<usize>,
    pub no_history: bool,
    pub trace_path: Option<PathBuf>,
    pub force_show_keys: bool,
}

struct Request {
    generation: u64,
    args: BootstrapArgs,
}

struct Response {
    generation: u64,
    value: Result<BootstrapApply, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum Phase {
    Idle = 0,
    Discover = 1,
    Complete = 2,
    Error = 3,
}

impl Phase {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Discover,
            2 => Self::Complete,
            3 => Self::Error,
            _ => Self::Idle,
        }
    }
}

struct Shared {
    phase: AtomicU8,
    started_at: Mutex<Option<Instant>>,
    cancelled: AtomicBool,
}

/// The only interface through which bootstrap code may access project
/// storage.  Tests substitute delayed/reordering backends without slowing the
/// terminal thread.
pub trait StorageBackend: Send + Sync + 'static {
    fn load(&self, args: BootstrapArgs) -> Result<BootstrapApply, String>;
}

struct FilesystemStorage;

impl StorageBackend for FilesystemStorage {
    fn load(&self, args: BootstrapArgs) -> Result<BootstrapApply, String> {
        VizApp::load_bootstrap(
            args.workgraph_dir,
            args.viz_options,
            args.mouse_override,
            args.history_depth_override,
            args.no_history,
            args.trace_path,
            args.force_show_keys,
        )
        .map_err(|error| error.to_string())
    }
}

pub struct BootstrapEngine {
    request_tx: SyncSender<Request>,
    result_rx: Receiver<Response>,
    pending: Option<Request>,
    next_generation: u64,
    desired_generation: u64,
    shared: Arc<Shared>,
    last_error: Option<String>,
}

impl BootstrapEngine {
    pub fn new() -> Self {
        Self::with_backend(Arc::new(FilesystemStorage))
    }

    fn with_backend(backend: Arc<dyn StorageBackend>) -> Self {
        let (request_tx, request_rx) = mpsc::sync_channel(REQUEST_CAPACITY);
        let (result_tx, result_rx) = mpsc::sync_channel(RESULT_CAPACITY);
        let shared = Arc::new(Shared {
            phase: AtomicU8::new(Phase::Idle as u8),
            started_at: Mutex::new(None),
            cancelled: AtomicBool::new(false),
        });
        let worker_shared = shared.clone();
        let _ = std::thread::Builder::new()
            .name("wg-tui-bootstrap".into())
            .spawn(move || worker_loop(request_rx, result_tx, backend, worker_shared));
        Self {
            request_tx,
            result_rx,
            pending: None,
            next_generation: 0,
            desired_generation: 0,
            shared,
            last_error: None,
        }
    }

    /// Request a coherent bootstrap generation.  This never blocks: if the
    /// bounded worker queue is occupied, only the newest pending generation is
    /// retained locally.
    pub fn request(&mut self, args: BootstrapArgs) -> u64 {
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.desired_generation = self.next_generation;
        self.pending = Some(Request {
            generation: self.desired_generation,
            args,
        });
        self.last_error = None;
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
            Err(TrySendError::Disconnected(_)) => {
                self.last_error = Some("bootstrap worker stopped".to_string());
                self.shared
                    .phase
                    .store(Phase::Error as u8, Ordering::Release);
            }
        }
    }

    /// Return only the currently desired generation.  Older completions are
    /// discarded, then the latest coalesced request is submitted.
    pub fn try_result(&mut self) -> Option<Result<BootstrapApply, String>> {
        self.pump();
        loop {
            match self.result_rx.try_recv() {
                Ok(response) if response.generation == self.desired_generation => {
                    self.shared
                        .phase
                        .store(Phase::Complete as u8, Ordering::Release);
                    return Some(response.value);
                }
                Ok(_) => {
                    // Stale result.  Never publish it, even when it completed
                    // after a newer request was submitted.
                    self.pump();
                }
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    return Some(Err("bootstrap worker stopped".to_string()));
                }
            }
        }
    }

    pub fn feedback(&self) -> Option<String> {
        let phase = Phase::from_u8(self.shared.phase.load(Ordering::Acquire));
        match phase {
            Phase::Idle | Phase::Complete => None,
            Phase::Error => Some(format!(
                "Load failed · {}",
                self.last_error.as_deref().unwrap_or("discover")
            )),
            Phase::Discover => {
                let elapsed = self
                    .shared
                    .started_at
                    .lock()
                    .ok()
                    .and_then(|start| *start)
                    .map(|start| start.elapsed())?;
                if elapsed < FEEDBACK_THRESHOLD {
                    None
                } else {
                    Some(format!(
                        "Storage slow · discover ({:.1}s)",
                        elapsed.as_secs_f64()
                    ))
                }
            }
        }
    }

    pub fn record_error(&mut self, message: String) {
        self.last_error = Some(message);
        self.shared
            .phase
            .store(Phase::Error as u8, Ordering::Release);
    }

    #[cfg(test)]
    fn desired_generation(&self) -> u64 {
        self.desired_generation
    }
}

impl Drop for BootstrapEngine {
    fn drop(&mut self) {
        // Do not join: a worker may be blocked in an NFS syscall.  Closing the
        // result receiver and setting cancellation makes eventual completion
        // harmless while terminal restoration proceeds immediately.
        self.shared.cancelled.store(true, Ordering::Release);
    }
}

fn worker_loop(
    request_rx: Receiver<Request>,
    result_tx: SyncSender<Response>,
    backend: Arc<dyn StorageBackend>,
    shared: Arc<Shared>,
) {
    while let Ok(request) = request_rx.recv() {
        if shared.cancelled.load(Ordering::Acquire) {
            return;
        }
        shared.phase.store(Phase::Discover as u8, Ordering::Release);
        if let Ok(mut started_at) = shared.started_at.lock() {
            *started_at = Some(Instant::now());
        }
        let value = backend.load(request.args);
        if shared.cancelled.load(Ordering::Acquire) {
            return;
        }
        // A saturated result queue is bounded backpressure.  Dropping a
        // completion is safe: stale generations are disposable and the UI's
        // latest request remains coalesced.
        let _ = result_tx.try_send(Response {
            generation: request.generation,
            value,
        });
    }
}

/// Test-only latency injection for the real PTY path.  It runs exclusively on
/// the bootstrap storage worker and therefore exercises the first-frame/input
/// guarantee without sleeping the terminal thread.
pub fn inject_test_storage_latency() {
    let delay = std::env::var("WG_TUI_TEST_STORAGE_LATENCY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    if delay > 0 {
        std::thread::sleep(Duration::from_millis(delay));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DelayedBackend {
        delay: Duration,
        calls: AtomicUsize,
    }

    impl StorageBackend for DelayedBackend {
        fn load(&self, args: BootstrapArgs) -> Result<BootstrapApply, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            std::thread::sleep(self.delay);
            let label = args.workgraph_dir;
            Ok(Box::new(move |app: &mut VizApp| {
                app.workgraph_dir = label;
                app.bootstrap_complete = true;
            }))
        }
    }

    fn args(label: &str) -> BootstrapArgs {
        BootstrapArgs {
            workgraph_dir: PathBuf::from(label),
            viz_options: VizOptions::default(),
            mouse_override: Some(false),
            history_depth_override: None,
            no_history: true,
            trace_path: None,
            force_show_keys: false,
        }
    }

    #[test]
    fn request_queue_is_bounded_and_latest_generation_wins() {
        let backend = Arc::new(DelayedBackend {
            delay: Duration::from_millis(20),
            calls: AtomicUsize::new(0),
        });
        let mut engine = BootstrapEngine::with_backend(backend.clone());
        for generation in 0..10_000 {
            engine.request(args(&generation.to_string()));
        }
        assert_eq!(engine.desired_generation(), 10_000);
        assert!(
            engine.pending.is_some(),
            "only one latest request is retained"
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let result = loop {
            if let Some(result) = engine.try_result() {
                break result;
            }
            assert!(
                Instant::now() < deadline,
                "latest generation did not finish"
            );
            std::thread::sleep(Duration::from_millis(2));
        };
        let apply = result.unwrap();
        let mut app = VizApp::new(
            PathBuf::new(),
            VizOptions::default(),
            Some(false),
            None,
            true,
        );
        apply(&mut app);
        assert_eq!(app.workgraph_dir, PathBuf::from("9999"));
        assert!(backend.calls.load(Ordering::Relaxed) < 10_000);
    }

    #[test]
    fn mutation_during_load_cannot_apply_stale_snapshot() {
        let backend = Arc::new(DelayedBackend {
            delay: Duration::from_millis(40),
            calls: AtomicUsize::new(0),
        });
        let mut engine = BootstrapEngine::with_backend(backend);
        engine.request(args("before-mutation"));
        std::thread::sleep(Duration::from_millis(5));
        engine.request(args("after-mutation"));

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = engine.try_result() {
                let mut app = VizApp::new(
                    PathBuf::new(),
                    VizOptions::default(),
                    Some(false),
                    None,
                    true,
                );
                result.unwrap()(&mut app);
                assert_eq!(app.workgraph_dir, PathBuf::from("after-mutation"));
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn loading_feedback_is_thresholded_phase_aware_and_clears() {
        let backend = Arc::new(DelayedBackend {
            delay: Duration::from_millis(220),
            calls: AtomicUsize::new(0),
        });
        let mut engine = BootstrapEngine::with_backend(backend);
        engine.request(args("slow"));
        std::thread::sleep(Duration::from_millis(30));
        assert!(engine.feedback().is_none());
        std::thread::sleep(Duration::from_millis(140));
        assert!(
            engine
                .feedback()
                .unwrap()
                .contains("Storage slow · discover")
        );
        while engine.try_result().is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(engine.feedback().is_none());
    }

    #[test]
    fn shutdown_does_not_join_stuck_storage_lane() {
        let backend = Arc::new(DelayedBackend {
            delay: Duration::from_secs(5),
            calls: AtomicUsize::new(0),
        });
        let started = Instant::now();
        let mut engine = BootstrapEngine::with_backend(backend);
        engine.request(args("stuck"));
        std::thread::sleep(Duration::from_millis(5));
        drop(engine);
        assert!(started.elapsed() < Duration::from_millis(100));
    }
}
