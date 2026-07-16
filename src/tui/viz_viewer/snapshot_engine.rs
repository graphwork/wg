//! Bounded CPU broker for graph-derived TUI view snapshots.
//!
//! The filesystem lanes deliberately stop at parsed graph bytes.  This broker
//! owns the expensive second half: accounting, filtering, sorting, trace
//! traversal, layout, and text projection.  The terminal thread only submits
//! immutable build inputs and installs one completed product at a time.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::time::Duration;

use super::state::GraphViewSnapshot;

const REQUEST_CAPACITY: usize = 1;
const RESULT_CAPACITY: usize = 1;
const RETIRE_CAPACITY: usize = 1;

/// Cooperative generation token checked between the derivation phases that
/// WG controls.  A monolithic third-party layout call may still finish late;
/// its result is rejected both in the worker and at publication.
#[derive(Clone)]
pub(super) struct GenerationToken {
    desired: Arc<AtomicU64>,
    generation: u64,
}

impl GenerationToken {
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.desired.load(Ordering::Acquire) != self.generation
    }
}

type Build = Box<dyn FnOnce(GenerationToken) -> Result<GraphViewSnapshot, String> + Send + 'static>;

struct Request {
    generation: u64,
    build: Build,
}

struct Response {
    generation: u64,
    snapshot: Result<GraphViewSnapshot, String>,
}

/// A fixed-size, latest-wins snapshot pipeline.
///
/// There is one running build, one channel slot, and one newest pending build.
/// Replaced pending closures are dropped immediately, so a watcher storm can
/// never create an unbounded graph-work backlog.  Retired generations are
/// dropped on the worker before the next build, keeping destruction of large
/// maps and strings off the terminal thread as well.
pub(super) struct SnapshotEngine {
    request_tx: SyncSender<Request>,
    result_rx: Receiver<Response>,
    retire_tx: SyncSender<GraphViewSnapshot>,
    pending: Option<Request>,
    pending_retire: Option<GraphViewSnapshot>,
    desired: Arc<AtomicU64>,
    next_generation: u64,
    cancelled: Arc<AtomicBool>,
}

impl SnapshotEngine {
    pub(super) fn new() -> Self {
        let (request_tx, request_rx) = mpsc::sync_channel(REQUEST_CAPACITY);
        let (result_tx, result_rx) = mpsc::sync_channel(RESULT_CAPACITY);
        let (retire_tx, retire_rx) = mpsc::sync_channel(RETIRE_CAPACITY);
        let desired = Arc::new(AtomicU64::new(0));
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_desired = desired.clone();
        let worker_cancelled = cancelled.clone();
        let _ = std::thread::Builder::new()
            .name("wg-tui-snapshot".into())
            .spawn(move || {
                worker_loop(
                    request_rx,
                    result_tx,
                    retire_rx,
                    worker_desired,
                    worker_cancelled,
                )
            });
        Self {
            request_tx,
            result_rx,
            retire_tx,
            pending: None,
            pending_retire: None,
            desired,
            next_generation: 0,
            cancelled,
        }
    }

    /// Queue a new desired view without waiting.  A pending older view is
    /// replaced in-place and the running generation observes cancellation at
    /// its next phase boundary.
    pub(super) fn request(&mut self, build: Build) -> u64 {
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        let generation = self.next_generation;
        self.desired.store(generation, Ordering::Release);
        self.pending = Some(Request { generation, build });
        self.pump();
        generation
    }

    /// Drain at most one result.  Stale products are destroyed here as part
    /// of the bounded per-tick call; the worker normally rejects them before
    /// publication, so this path is only the final race fence.
    pub(super) fn try_result(&mut self) -> Option<Result<GraphViewSnapshot, String>> {
        self.pump();
        loop {
            match self.result_rx.try_recv() {
                Ok(response) if response.generation == self.desired.load(Ordering::Acquire) => {
                    return Some(response.snapshot);
                }
                Ok(response) => {
                    // The generation changed after the worker's final fence.
                    // Large successful products are retired on the worker;
                    // never run their destructors in the bounded UI drain.
                    if let Ok(snapshot) = response.snapshot {
                        self.retire(snapshot);
                    }
                    self.pump();
                }
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    return Some(Err("snapshot worker stopped".to_string()));
                }
            }
        }
    }

    /// Transfer ownership of a replaced generation to the CPU worker.  The
    /// queue has one slot; if it is momentarily occupied we retain exactly one
    /// local retirement and stop submitting builds until it moves.
    pub(super) fn retire(&mut self, snapshot: GraphViewSnapshot) {
        debug_assert!(self.pending_retire.is_none());
        match self.retire_tx.try_send(snapshot) {
            Ok(()) => {}
            Err(TrySendError::Full(snapshot)) => self.pending_retire = Some(snapshot),
            Err(TrySendError::Disconnected(snapshot)) => {
                // Worker teardown is only expected during app teardown.  Drop
                // here rather than leak the generation.
                drop(snapshot);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn desired_generation(&self) -> u64 {
        self.desired.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(super) fn pending_len(&self) -> usize {
        usize::from(self.pending.is_some())
    }

    fn pump(&mut self) {
        if let Some(retired) = self.pending_retire.take() {
            match self.retire_tx.try_send(retired) {
                Ok(()) => {}
                Err(TrySendError::Full(retired)) => {
                    self.pending_retire = Some(retired);
                    return;
                }
                Err(TrySendError::Disconnected(retired)) => drop(retired),
            }
        }
        let Some(request) = self.pending.take() else {
            return;
        };
        match self.request_tx.try_send(request) {
            Ok(()) => {}
            Err(TrySendError::Full(request)) => self.pending = Some(request),
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

impl Drop for SnapshotEngine {
    fn drop(&mut self) {
        // Never join: a derivation may itself be waiting on slow enrichment
        // storage.  Generation fencing makes its eventual completion harmless.
        self.cancelled.store(true, Ordering::Release);
        self.desired.fetch_add(1, Ordering::AcqRel);
    }
}

fn worker_loop(
    request_rx: Receiver<Request>,
    result_tx: SyncSender<Response>,
    retire_rx: Receiver<GraphViewSnapshot>,
    desired: Arc<AtomicU64>,
    cancelled: Arc<AtomicBool>,
) {
    loop {
        // Retire replaced generations even when the graph is otherwise idle.
        // A blocking `recv()` here used to retain one complete large view
        // until the next mutation, doubling steady-state memory after a swap.
        let mut retired_any = false;
        while let Ok(retired) = retire_rx.try_recv() {
            drop(retired);
            retired_any = true;
        }
        if retired_any {
            release_retired_pages();
        }
        if cancelled.load(Ordering::Acquire) {
            return;
        }
        let request = match request_rx.recv_timeout(Duration::from_millis(25)) {
            Ok(request) => request,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let token = GenerationToken {
            desired: desired.clone(),
            generation: request.generation,
        };
        if token.is_cancelled() {
            continue;
        }
        let snapshot = (request.build)(token.clone());
        if token.is_cancelled() || cancelled.load(Ordering::Acquire) {
            continue;
        }
        if result_tx
            .send(Response {
                generation: request.generation,
                snapshot,
            })
            .is_err()
        {
            return;
        }
    }
}

/// Ask glibc to return fully-free arenas after destroying a graph-sized
/// generation. Without this, the allocator keeps the 200-300 MiB transient
/// second snapshot mapped indefinitely even though the Rust values are gone.
/// This runs only on the CPU worker, never in the terminal event loop.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn release_retired_pages() {
    // SAFETY: `malloc_trim` takes no pointer and is process-safe in glibc. It
    // merely asks the allocator to release unused heap pages to the OS.
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn release_retired_pages() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    #[test]
    fn request_queue_is_bounded_and_latest_generation_wins() {
        let mut engine = SnapshotEngine::new();
        let builds = Arc::new(AtomicUsize::new(0));
        for _ in 0..100_000 {
            let builds = builds.clone();
            engine.request(Box::new(move |_| {
                builds.fetch_add(1, Ordering::Relaxed);
                Err("fixture".to_string())
            }));
        }
        assert!(engine.pending_len() <= 1);
        assert_eq!(engine.desired_generation(), 100_000);

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if engine.try_result().is_some() {
                break;
            }
            std::thread::yield_now();
        }
        assert!(builds.load(Ordering::Relaxed) < 100_000);
    }
}
