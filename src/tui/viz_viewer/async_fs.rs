//! Background filesystem service for the TUI.
//!
//! All disk I/O that the TUI needs (graph reads, stats, streaming-text reads,
//! chat-interaction touches) is performed on a worker thread. The main thread
//! reads cached values from a `Mutex` and dispatches refreshes via a channel —
//! it never blocks on disk, even on a high-latency filesystem (NFS, sshfs).
//!
//! Pattern: optimistic concurrency. The render path always uses the most
//! recent cached value (possibly stale). When a request completes, the cache
//! is updated; the next render frame picks up the fresh value.
//!
//! Slow-disk detection: every operation records its wall-clock duration. If a
//! single operation exceeds [`SLOW_OP_THRESHOLD`], the most recent slow op is
//! exposed via [`AsyncFs::slow_disk_indicator`] for display in the status bar.
//!
//! See task `fix-tui-must` for the architectural rationale.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use workgraph::graph::WorkGraph;

/// Operations slower than this surface a "disk slow" indicator in the status bar.
pub const SLOW_OP_THRESHOLD: Duration = Duration::from_millis(500);
/// How long the slow-disk indicator stays visible after the last slow op.
pub const SLOW_OP_VISIBLE: Duration = Duration::from_secs(5);

/// One snapshot of a slow disk operation, surfaced for diagnostic display.
#[derive(Clone, Debug)]
pub struct SlowOp {
    pub label: String,
    pub duration: Duration,
    pub at: Instant,
}

/// Identifies an in-flight request for de-duplication. We never queue two
/// concurrent requests for the same target — that just floods the worker
/// when it's already slow.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
enum RequestKey {
    LoadGraph,
    Stat(PathBuf),
    Streaming(u32),
}

/// Request sent from main → worker.
enum FsRequest {
    LoadGraph(PathBuf),
    Stat(PathBuf),
    ReadStreaming {
        path: PathBuf,
        coord_id: u32,
    },
    BumpChatInteraction {
        workgraph_dir: PathBuf,
        session_ref: String,
    },
    Shutdown,
}

/// Completion notification sent from worker → main. The actual results are
/// written into the shared cache; this message just tells the main thread
/// "you can dispatch a fresh request for this key now."
enum FsResponse {
    LoadGraphDone { duration: Duration, success: bool },
    StatDone { path: PathBuf, duration: Duration },
    StreamingDone { coord_id: u32, duration: Duration },
    BumpDone { duration: Duration },
}

/// Shared state between worker and main thread.
struct AsyncFsInner {
    /// Most recent successful graph load, with the mtime read at the time
    /// of the load.
    graph_cache: Mutex<Option<(Arc<WorkGraph>, Option<SystemTime>)>>,
    /// Bumped each time `graph_cache` is replaced with a fresh load.
    graph_version: AtomicU64,

    /// Per-path mtime cache (None = stat'd but file absent / unreadable).
    stat_cache: Mutex<HashMap<PathBuf, Option<SystemTime>>>,

    /// Per-coordinator streaming-text cache.
    streaming_cache: Mutex<HashMap<u32, String>>,
    /// Bumped each time any streaming entry changes.
    streaming_version: AtomicU64,

    /// Most recent slow operation, if any (cleared by visibility timeout
    /// in `slow_disk_indicator`).
    last_slow_op: Mutex<Option<SlowOp>>,
}

pub struct AsyncFs {
    request_tx: Sender<FsRequest>,
    response_rx: Receiver<FsResponse>,
    inner: Arc<AsyncFsInner>,
    /// Requests we've sent but haven't seen a response for yet.
    in_flight: HashSet<RequestKey>,
    /// Last graph_version we observed; used to detect "fresh graph available."
    last_seen_graph_version: u64,
    /// Last streaming_version we observed.
    last_seen_streaming_version: u64,
}

impl AsyncFs {
    pub fn new() -> Self {
        let (request_tx, request_rx) = mpsc::channel::<FsRequest>();
        let (response_tx, response_rx) = mpsc::channel::<FsResponse>();
        let inner = Arc::new(AsyncFsInner {
            graph_cache: Mutex::new(None),
            graph_version: AtomicU64::new(0),
            stat_cache: Mutex::new(HashMap::new()),
            streaming_cache: Mutex::new(HashMap::new()),
            streaming_version: AtomicU64::new(0),
            last_slow_op: Mutex::new(None),
        });
        let inner_for_worker = inner.clone();
        let _ = thread::Builder::new()
            .name("wg-tui-fs".into())
            .spawn(move || worker_loop(request_rx, response_tx, inner_for_worker));
        Self {
            request_tx,
            response_rx,
            inner,
            in_flight: HashSet::new(),
            last_seen_graph_version: 0,
            last_seen_streaming_version: 0,
        }
    }

    /// Drain completed-request notifications. Call once per main-loop tick
    /// before dispatching new requests. Returns true if any cache was
    /// updated since the last call (graph or streaming).
    pub fn drain_responses(&mut self) -> CacheChanges {
        while let Ok(resp) = self.response_rx.try_recv() {
            match resp {
                FsResponse::LoadGraphDone { duration, .. } => {
                    self.in_flight.remove(&RequestKey::LoadGraph);
                    self.note_op("graph.jsonl", duration);
                }
                FsResponse::StatDone { path, duration } => {
                    self.in_flight.remove(&RequestKey::Stat(path.clone()));
                    if duration >= SLOW_OP_THRESHOLD {
                        let label = format!("stat {}", path.display());
                        self.note_op(&label, duration);
                    }
                }
                FsResponse::StreamingDone { coord_id, duration } => {
                    self.in_flight.remove(&RequestKey::Streaming(coord_id));
                    if duration >= SLOW_OP_THRESHOLD {
                        self.note_op("streaming read", duration);
                    }
                }
                FsResponse::BumpDone { duration } => {
                    if duration >= SLOW_OP_THRESHOLD {
                        self.note_op("chat-interaction bump", duration);
                    }
                }
            }
        }
        let graph_v = self.inner.graph_version.load(Ordering::Relaxed);
        let stream_v = self.inner.streaming_version.load(Ordering::Relaxed);
        let graph_changed = graph_v != self.last_seen_graph_version;
        let streaming_changed = stream_v != self.last_seen_streaming_version;
        self.last_seen_graph_version = graph_v;
        self.last_seen_streaming_version = stream_v;
        CacheChanges {
            graph: graph_changed,
            streaming: streaming_changed,
        }
    }

    fn note_op(&self, label: &str, duration: Duration) {
        if duration >= SLOW_OP_THRESHOLD {
            *self.inner.last_slow_op.lock().unwrap() = Some(SlowOp {
                label: label.to_string(),
                duration,
                at: Instant::now(),
            });
        }
    }

    /// Dispatch a graph reload (no-op if one is already in flight).
    pub fn request_graph_load(&mut self, path: PathBuf) {
        if !self.in_flight.insert(RequestKey::LoadGraph) {
            return;
        }
        let _ = self.request_tx.send(FsRequest::LoadGraph(path));
    }

    /// Read the cached graph (clones `Arc`, never blocks on disk).
    pub fn cached_graph(&self) -> Option<(Arc<WorkGraph>, Option<SystemTime>)> {
        self.inner.graph_cache.lock().unwrap().clone()
    }

    /// Read cached graph mtime only (without cloning the graph).
    pub fn cached_graph_mtime(&self) -> Option<SystemTime> {
        self.inner
            .graph_cache
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|(_g, m)| *m)
    }

    /// Has a graph ever been loaded?
    pub fn has_graph(&self) -> bool {
        self.inner.graph_cache.lock().unwrap().is_some()
    }

    /// Dispatch a stat() call (no-op if already in flight for this path).
    pub fn request_stat(&mut self, path: PathBuf) {
        let key = RequestKey::Stat(path.clone());
        if !self.in_flight.insert(key) {
            return;
        }
        let _ = self.request_tx.send(FsRequest::Stat(path));
    }

    /// Read cached mtime for a path. None if never stat'd or if stat'd
    /// and file was absent/unreadable. Use [`Self::has_stat`] to
    /// distinguish "never stat'd" from "stat'd, file absent."
    pub fn cached_stat(&self, path: &Path) -> Option<SystemTime> {
        self.inner
            .stat_cache
            .lock()
            .unwrap()
            .get(path)
            .copied()
            .flatten()
    }

    /// Has this path ever been stat'd successfully or unsuccessfully?
    pub fn has_stat(&self, path: &Path) -> bool {
        self.inner.stat_cache.lock().unwrap().contains_key(path)
    }

    /// Synchronously seed the stat cache for a path. Used at startup so the
    /// first frames don't see "never stat'd" sentinels for files that
    /// definitely exist. Off the hot path, only called during `VizApp::new`.
    pub fn seed_stat(&self, path: PathBuf, mtime: Option<SystemTime>) {
        self.inner.stat_cache.lock().unwrap().insert(path, mtime);
    }

    /// Synchronously seed the graph cache. Used at startup for the same reason.
    pub fn seed_graph(&self, graph: WorkGraph, mtime: Option<SystemTime>) {
        *self.inner.graph_cache.lock().unwrap() = Some((Arc::new(graph), mtime));
        self.inner.graph_version.fetch_add(1, Ordering::Relaxed);
    }

    /// Dispatch a streaming-text read.
    pub fn request_streaming(&mut self, path: PathBuf, coord_id: u32) {
        let key = RequestKey::Streaming(coord_id);
        if !self.in_flight.insert(key) {
            return;
        }
        let _ = self
            .request_tx
            .send(FsRequest::ReadStreaming { path, coord_id });
    }

    /// Read cached streaming text for a coordinator.
    pub fn cached_streaming(&self, coord_id: u32) -> String {
        self.inner
            .streaming_cache
            .lock()
            .unwrap()
            .get(&coord_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Fire-and-forget chat-interaction bump. Performed off the main thread
    /// so a slow graph.jsonl write can't block keystroke echo.
    pub fn bump_chat_interaction(&self, workgraph_dir: PathBuf, session_ref: String) {
        let _ = self.request_tx.send(FsRequest::BumpChatInteraction {
            workgraph_dir,
            session_ref,
        });
    }

    /// The most recent slow operation, if it happened within the last
    /// [`SLOW_OP_VISIBLE`] window. Used by the status-bar renderer.
    pub fn slow_disk_indicator(&self) -> Option<SlowOp> {
        let guard = self.inner.last_slow_op.lock().unwrap();
        guard
            .as_ref()
            .filter(|s| s.at.elapsed() < SLOW_OP_VISIBLE)
            .cloned()
    }
}

impl Default for AsyncFs {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AsyncFs {
    fn drop(&mut self) {
        let _ = self.request_tx.send(FsRequest::Shutdown);
    }
}

/// Returned by [`AsyncFs::drain_responses`] to tell the main thread which
/// caches saw fresh values since the previous call.
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheChanges {
    pub graph: bool,
    pub streaming: bool,
}

impl CacheChanges {
    pub fn any(&self) -> bool {
        self.graph || self.streaming
    }
}

/// Test-only injected latency. The worker sleeps this long before each disk
/// op so smoke benchmarks can simulate a high-latency filesystem without
/// requiring an actual slow mount (LD_PRELOAD shim, FUSE, NFS, etc.). Read
/// from `WG_ASYNC_FS_TEST_LATENCY_MS` once on first call and cached. Zero
/// (the unset default) means no injection.
fn injected_latency() -> Duration {
    use std::sync::OnceLock;
    static CACHED: OnceLock<Duration> = OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var("WG_ASYNC_FS_TEST_LATENCY_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or_default()
    })
}

fn worker_loop(rx: Receiver<FsRequest>, tx: Sender<FsResponse>, inner: Arc<AsyncFsInner>) {
    while let Ok(req) = rx.recv() {
        let inject = injected_latency();
        if !inject.is_zero() {
            thread::sleep(inject);
        }
        match req {
            FsRequest::Shutdown => break,
            FsRequest::LoadGraph(path) => {
                let start = Instant::now();
                let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                let result = workgraph::parser::load_graph(&path);
                let success = result.is_ok();
                if let Ok(graph) = result {
                    *inner.graph_cache.lock().unwrap() = Some((Arc::new(graph), mtime));
                    inner.graph_version.fetch_add(1, Ordering::Relaxed);
                }
                let _ = tx.send(FsResponse::LoadGraphDone {
                    duration: start.elapsed(),
                    success,
                });
            }
            FsRequest::Stat(path) => {
                let start = Instant::now();
                let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                inner.stat_cache.lock().unwrap().insert(path.clone(), mtime);
                let _ = tx.send(FsResponse::StatDone {
                    path,
                    duration: start.elapsed(),
                });
            }
            FsRequest::ReadStreaming { path, coord_id } => {
                let start = Instant::now();
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                {
                    let mut cache = inner.streaming_cache.lock().unwrap();
                    let prev = cache.get(&coord_id);
                    let changed = prev.map(|s| s != &text).unwrap_or(true);
                    if changed {
                        cache.insert(coord_id, text);
                        inner.streaming_version.fetch_add(1, Ordering::Relaxed);
                    }
                }
                let _ = tx.send(FsResponse::StreamingDone {
                    coord_id,
                    duration: start.elapsed(),
                });
            }
            FsRequest::BumpChatInteraction {
                workgraph_dir,
                session_ref,
            } => {
                let start = Instant::now();
                workgraph::chat::bump_chat_interaction(&workgraph_dir, &session_ref);
                let _ = tx.send(FsResponse::BumpDone {
                    duration: start.elapsed(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_responses_no_panic_when_empty() {
        let mut afs = AsyncFs::new();
        let changes = afs.drain_responses();
        assert!(!changes.any());
    }

    #[test]
    fn stat_request_populates_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("hello.txt");
        std::fs::write(&path, "x").unwrap();

        let mut afs = AsyncFs::new();
        afs.request_stat(path.clone());

        // Wait up to 2 seconds for the worker to populate the cache.
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            afs.drain_responses();
            if afs.has_stat(&path) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(afs.has_stat(&path), "stat cache should be populated");
        assert!(afs.cached_stat(&path).is_some());
    }

    #[test]
    fn graph_load_populates_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("graph.jsonl");
        std::fs::write(&path, "").unwrap();

        let mut afs = AsyncFs::new();
        afs.request_graph_load(path.clone());

        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            afs.drain_responses();
            if afs.has_graph() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(afs.has_graph(), "graph cache should be populated");
    }

    #[test]
    fn duplicate_stat_dispatches_are_deduped() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("missing");

        let mut afs = AsyncFs::new();
        // Fire many duplicate requests.
        for _ in 0..100 {
            afs.request_stat(path.clone());
        }
        // Only one is in-flight (de-duped).
        assert_eq!(afs.in_flight.len(), 1);
    }

    #[test]
    fn slow_op_indicator_set_when_threshold_exceeded() {
        let afs = AsyncFs::new();
        // Synthetic slow op via the internal hook.
        afs.note_op("synthetic", Duration::from_millis(800));
        let ind = afs.slow_disk_indicator();
        assert!(ind.is_some());
        let s = ind.unwrap();
        assert_eq!(s.label, "synthetic");
    }

    #[test]
    fn slow_op_indicator_filtered_below_threshold() {
        let afs = AsyncFs::new();
        afs.note_op("fast", Duration::from_millis(50));
        assert!(afs.slow_disk_indicator().is_none());
    }

    /// The architectural property tested by `fix-tui-must`: every main-thread
    /// API on `AsyncFs` must return in < 10ms even when the worker is busy
    /// servicing a slow request. We simulate "the worker is stuck on a slow
    /// disk read" by issuing many requests and asserting that each
    /// `request_*` / `cached_*` / `drain_responses` call on the main thread
    /// remains microsecond-fast.
    ///
    /// This is the regression gate for `tui_responsive_under_500ms_latency`
    /// in the smoke manifest.
    #[test]
    fn main_thread_api_never_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let graph_path = tmp.path().join("graph.jsonl");
        // Use a small graph so the load itself is fast; the test is about
        // dispatch overhead, not the worker's read time.
        std::fs::write(&graph_path, "").unwrap();

        let mut afs = AsyncFs::new();

        // Fire 500 mixed requests in a tight loop. None of these calls are
        // permitted to block the main thread.
        let start = Instant::now();
        let mut max_call_duration = Duration::ZERO;
        for i in 0..500u32 {
            let t0 = Instant::now();
            afs.request_stat(graph_path.clone());
            max_call_duration = max_call_duration.max(t0.elapsed());

            let t0 = Instant::now();
            afs.request_graph_load(graph_path.clone());
            max_call_duration = max_call_duration.max(t0.elapsed());

            let t0 = Instant::now();
            let _ = afs.cached_stat(&graph_path);
            max_call_duration = max_call_duration.max(t0.elapsed());

            let t0 = Instant::now();
            let _ = afs.cached_graph();
            max_call_duration = max_call_duration.max(t0.elapsed());

            let t0 = Instant::now();
            let _ = afs.drain_responses();
            max_call_duration = max_call_duration.max(t0.elapsed());

            let t0 = Instant::now();
            let _ = afs.slow_disk_indicator();
            max_call_duration = max_call_duration.max(t0.elapsed());

            // Fire-and-forget chat bump (the keystroke path).
            let t0 = Instant::now();
            afs.bump_chat_interaction(tmp.path().to_path_buf(), format!(".coordinator-{}", i));
            max_call_duration = max_call_duration.max(t0.elapsed());
        }
        let total = start.elapsed();
        // 500 iterations × 7 calls = 3500 calls. Even on a slow CI box,
        // 3500 channel-sends + cache reads should complete well under 1s.
        assert!(
            total < Duration::from_secs(1),
            "main-thread API loop took {:?} (>1s); something is blocking",
            total
        );
        // No single call may exceed 50ms — that's the chat-input p99 budget
        // from the task spec ("keystrokes echo within 50ms p99 even under load").
        assert!(
            max_call_duration < Duration::from_millis(50),
            "max single-call duration {:?} > 50ms — main thread is blocking",
            max_call_duration
        );
    }

    /// With injected latency simulating a 500ms-latency filesystem, the
    /// main thread's API calls must STILL be non-blocking. The worker's
    /// reads are slow (each takes ~500ms), but `request_*` and `cached_*`
    /// on the main thread continue to return immediately.
    ///
    /// This test corresponds directly to the task's primary acceptance
    /// criterion: "TUI startup completes; chat input is responsive
    /// (keystrokes echo within 50ms p99 even under load)" with simulated
    /// 500ms FS latency.
    ///
    /// NOTE: this test sets a process-wide env var via `WG_ASYNC_FS_TEST_LATENCY_MS`,
    /// but the latency is read once per process via `OnceLock`, so we
    /// gate it behind a fresh-process invocation. The smoke scenario
    /// runs this in its own `cargo test` invocation.
    #[test]
    #[ignore] // runs only via the smoke scenario which sets the env var
    fn main_thread_api_unblocked_under_simulated_500ms_latency() {
        // Sanity: env var must be set for this to be meaningful.
        let injected = injected_latency();
        assert!(
            injected >= Duration::from_millis(100),
            "WG_ASYNC_FS_TEST_LATENCY_MS must be >= 100 for this test to be meaningful (got {:?})",
            injected
        );

        let tmp = tempfile::tempdir().unwrap();
        let graph_path = tmp.path().join("graph.jsonl");
        std::fs::write(&graph_path, "").unwrap();

        let mut afs = AsyncFs::new();
        // Fire one slow request to occupy the worker.
        afs.request_graph_load(graph_path.clone());

        // Now hammer the main-thread API while the worker is stuck. Every
        // call should remain microsecond-fast — the spec requires < 50ms p99.
        let mut max_call_duration = Duration::ZERO;
        for _ in 0..200u32 {
            let t0 = Instant::now();
            afs.request_stat(graph_path.clone());
            max_call_duration = max_call_duration.max(t0.elapsed());

            let t0 = Instant::now();
            let _ = afs.cached_graph();
            max_call_duration = max_call_duration.max(t0.elapsed());

            let t0 = Instant::now();
            let _ = afs.drain_responses();
            max_call_duration = max_call_duration.max(t0.elapsed());

            // Tight loop — give other threads a tick.
            std::thread::yield_now();
        }
        assert!(
            max_call_duration < Duration::from_millis(50),
            "max main-thread call duration {:?} exceeded 50ms p99 budget under \
             simulated {:?} FS latency",
            max_call_duration,
            injected
        );
    }
}
