//! Bounded storage lane for TUI data that is not part of the graph snapshot.
//!
//! The terminal thread may submit work and drain completed snapshots, but it
//! never executes a job or waits for either channel.  Requests are coalesced
//! per panel kind and the queue holds at most one job behind the running job.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};

use super::state::VizApp;

pub(crate) type Apply = Box<dyn FnOnce(&mut VizApp) + Send + 'static>;
type Work = Box<dyn FnOnce() -> Apply + Send + 'static>;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum Kind {
    Config,
    Settings,
    Detail,
    Log,
    Messages,
    Agency,
    CoordinatorLog,
    Firehose,
    Output,
    AgentMonitor,
    Chat,
    Service,
    ChatHistory,
    HistoryBrowser,
    ChatManager,
    FileBrowser,
}

struct Job {
    kind: Kind,
    work: Work,
}

struct Completion {
    kind: Kind,
    apply: Apply,
}

/// A single persistent worker and a latest-request-sized queue.
///
/// `request` and `drain` use only `try_*` channel operations.  A slow or
/// unavailable filesystem can hold the worker indefinitely without holding
/// the input/render thread or growing an unbounded backlog.
pub(crate) struct Lane {
    job_tx: SyncSender<Job>,
    completion_rx: Receiver<Completion>,
    pending: HashSet<Kind>,
}

impl Lane {
    pub(crate) fn new() -> Self {
        let (job_tx, job_rx) = mpsc::sync_channel::<Job>(1);
        let (completion_tx, completion_rx) = mpsc::sync_channel::<Completion>(1);
        std::thread::Builder::new()
            .name("wg-tui-aux-storage".into())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    let completion = Completion {
                        kind: job.kind,
                        apply: (job.work)(),
                    };
                    if completion_tx.send(completion).is_err() {
                        break;
                    }
                }
            })
            .expect("failed to spawn TUI auxiliary storage worker");
        Self {
            job_tx,
            completion_rx,
            pending: HashSet::new(),
        }
    }

    /// Submit without waiting. Returns false when this kind is already
    /// pending or the one-slot queue is occupied; callers may retry next tick.
    pub(crate) fn request<F>(&mut self, kind: Kind, work: F) -> bool
    where
        F: FnOnce() -> Apply + Send + 'static,
    {
        if self.pending.contains(&kind) {
            return false;
        }
        let job = Job {
            kind,
            work: Box::new(work),
        };
        match self.job_tx.try_send(job) {
            Ok(()) => {
                self.pending.insert(kind);
                true
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Drain every currently available completion without waiting.
    pub(crate) fn drain(&mut self) -> Vec<Apply> {
        let mut completed = Vec::new();
        loop {
            match self.completion_rx.try_recv() {
                Ok(result) => {
                    self.pending.remove(&result.kind);
                    completed.push(result.apply);
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        completed
    }

    #[cfg(test)]
    pub(crate) fn pending_len(&self) -> usize {
        self.pending.len()
    }

    #[cfg(test)]
    pub(crate) fn is_pending(&self, kind: Kind) -> bool {
        self.pending.contains(&kind)
    }
}

impl Default for Lane {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    use super::{Kind, Lane};

    #[test]
    fn request_is_coalesced_and_never_waits_for_slow_work() {
        let mut lane = Lane::new();
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = finished.clone();
        let started = Instant::now();
        assert!(lane.request(Kind::Config, move || {
            std::thread::sleep(Duration::from_millis(500));
            worker_finished.store(true, Ordering::Release);
            Box::new(|_| {})
        }));
        assert!(!lane.request(Kind::Config, || Box::new(|_| {})));
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "submission waited for auxiliary storage"
        );
        assert_eq!(lane.pending_len(), 1);

        let deadline = Instant::now() + Duration::from_secs(2);
        while !finished.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(finished.load(Ordering::Acquire));
    }
}
