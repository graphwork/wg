//! Background job management for the native executor.
//!
//! Provides `Job`, `JobStatus`, and `JobStore` for running and managing
//! detached background tasks that persist across agent restarts.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(unix)]
use tokio::process::Command as TokioCommand;

/// Maximum concurrent background jobs.
const DEFAULT_MAX_CONCURRENT: usize = 10;

/// Grace period (seconds) before SIGKILL after SIGTERM.
const KILL_GRACE_PERIOD_SECS: u64 = 5;

/// Job file name pattern.
const JOB_FILE_PREFIX: &str = "job-";

/// Lock file extension.
const LOCK_EXT: &str = ".lock";

/// PID file extension.
const PID_EXT: &str = ".pid";

/// Log file extension.
const LOG_EXT: &str = ".log";

/// Represents a background task that persists across agent restarts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    /// Unique identifier (generated UUID).
    pub id: String,
    /// Human-readable name for display.
    pub name: String,
    /// The shell command that was executed.
    pub command: String,
    /// Current status of the job.
    pub status: JobStatus,
    /// Process ID of the session/process-group leader (if running).
    pub pid: Option<u32>,
    /// Unix process group that owns this job and all of its shell descendants.
    ///
    /// This is additive for compatibility with old job rows. A running legacy
    /// row without a group and start identity is treated as orphaned rather
    /// than risking a signal to a recycled PID.
    #[serde(default)]
    pub process_group: Option<u32>,
    /// Platform process-start identity used to reject stale/recycled PIDs.
    #[serde(default)]
    pub process_start_identity: Option<String>,
    /// Exit code (if completed or failed).
    pub exit_code: Option<i32>,
    /// Timestamp when the job was created.
    pub created_at: DateTime<Utc>,
    /// Timestamp when the job last updated (status change).
    pub updated_at: DateTime<Utc>,
    /// When the job finished (if terminal state).
    pub finished_at: Option<DateTime<Utc>>,
    /// Path to the log file containing stdout/stderr.
    pub log_path: PathBuf,
    /// Working directory for the command.
    pub working_dir: PathBuf,
}

/// Possible states for a background job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    /// Job is running in the background.
    Running,
    /// Job completed successfully (exit code 0).
    Completed,
    /// Job failed (non-zero exit code).
    Failed,
    /// Job was cancelled by user request.
    Cancelled,
    /// Job is in an unknown state (orphan detection).
    Orphaned,
}

impl JobStatus {
    /// Returns true if this is a terminal state.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobStatus::Completed | JobStatus::Failed | JobStatus::Cancelled | JobStatus::Orphaned
        )
    }
}

/// Manages persistence and discovery of background jobs.
#[derive(Debug, Clone)]
pub struct JobStore {
    /// Cache of loaded jobs (refreshed on demand).
    jobs: HashMap<String, Job>,
    /// Maximum concurrent jobs allowed.
    max_concurrent: usize,
    /// Jobs directory path.
    jobs_dir: PathBuf,
}

impl JobStore {
    /// Create a new JobStore at the given path.
    pub fn new(base_dir: PathBuf) -> Result<Self> {
        let jobs_dir = base_dir.join("jobs");
        fs::create_dir_all(&jobs_dir).context("Failed to create jobs directory")?;

        let mut store = Self {
            jobs: HashMap::new(),
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            jobs_dir,
        };

        // Load existing jobs from disk
        store.load_all()?;

        Ok(store)
    }

    /// Set the maximum concurrent jobs allowed.
    pub fn set_max_concurrent(&mut self, max: usize) {
        self.max_concurrent = max;
    }

    /// Get the jobs directory path.
    pub fn jobs_dir(&self) -> &Path {
        &self.jobs_dir
    }

    /// Get a job by ID or name.
    pub fn get(&self, id_or_name: &str) -> Option<&Job> {
        // First try exact ID match
        if let Some(job) = self.jobs.get(id_or_name) {
            return Some(job);
        }
        // Then try name match
        self.jobs.values().find(|j| j.name == id_or_name)
    }

    /// Get all jobs (sorted by created_at).
    pub fn list(&self) -> Vec<&Job> {
        let mut jobs: Vec<&Job> = self.jobs.values().collect();
        jobs.sort_by_key(|a| a.created_at);
        jobs
    }

    /// Check if a named job exists (prevents duplicate launches).
    pub fn exists(&self, name: &str) -> bool {
        self.jobs.values().any(|j| j.name == name)
    }

    /// Get the current count of running jobs.
    pub fn running_count(&self) -> usize {
        self.jobs
            .values()
            .filter(|j| j.status == JobStatus::Running)
            .count()
    }

    /// Refresh job states from disk (check PIDs, exit codes).
    pub fn refresh(&mut self) -> Result<()> {
        let running: Vec<String> = self
            .jobs
            .values()
            .filter(|job| job.status == JobStatus::Running)
            .map(|job| job.id.clone())
            .collect();
        for job_id in running {
            self.check_and_update_status(&job_id)?;
        }
        Ok(())
    }

    /// Load all jobs from disk.
    fn load_all(&mut self) -> Result<()> {
        if !self.jobs_dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(&self.jobs_dir)? {
            let entry = entry?;
            let path = entry.path();

            // Only process .json files
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }

            let content = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read job file: {:?}", path))?;

            let job: Job = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse job file: {:?}", path))?;

            self.jobs.insert(job.id.clone(), job);
        }

        // Refresh to detect orphaned jobs
        self.refresh()?;

        Ok(())
    }

    /// Save a job to disk.
    fn save_job(&self, job: &Job) -> Result<()> {
        let path = self
            .jobs_dir
            .join(format!("{}{}.json", JOB_FILE_PREFIX, job.id));
        let content = serde_json::to_string_pretty(job).context("Failed to serialize job")?;
        crate::atomic_file::write_atomic(&path, content.as_bytes())
            .context("Failed to write job file")?;
        Ok(())
    }

    /// Create a new lock file for a job.
    fn create_lock(&self, job_id: &str, pid: u32) -> Result<PathBuf> {
        let _lock_path =
            self.jobs_dir
                .join(format!("{}{}{}", job_id, ".lock", PID_EXT.replace('.', "")));
        // Actually the lock file should be like: job-{id}.lock
        let lock_path = self.jobs_dir.join(format!("{}{}", job_id, LOCK_EXT));
        let content = format!("{}\n", pid);
        fs::write(&lock_path, content).context("Failed to write lock file")?;
        Ok(lock_path)
    }

    /// Create a PID file for a job.
    fn create_pid_file(&self, job_id: &str, pid: u32) -> Result<PathBuf> {
        let pid_path = self.jobs_dir.join(format!("{}{}", job_id, PID_EXT));
        fs::write(&pid_path, format!("{}\n", pid)).context("Failed to write PID file")?;
        Ok(pid_path)
    }

    /// Get the log file path for a job.
    fn log_path(&self, job_id: &str) -> PathBuf {
        self.jobs_dir.join(format!("{}{}", job_id, LOG_EXT))
    }

    /// Run a new background job.
    pub async fn run(&mut self, name: &str, command: &str, working_dir: &Path) -> Result<Job> {
        // Check max concurrent
        if self.running_count() >= self.max_concurrent {
            return Err(anyhow!(
                "Maximum concurrent jobs ({}) reached. Wait for a job to complete.",
                self.max_concurrent
            ));
        }

        // Check for duplicate name
        if self.exists(name) {
            return Err(anyhow!("Job with name '{}' already exists", name));
        }

        // Generate job ID
        let id = format!("{}{}", JOB_FILE_PREFIX, uuid_simple());
        let log_path = self.log_path(&id);

        let now = Utc::now();
        let job = Job {
            id: id.clone(),
            name: name.to_string(),
            command: command.to_string(),
            status: JobStatus::Running,
            pid: None,
            process_group: None,
            process_start_identity: None,
            exit_code: None,
            created_at: now,
            updated_at: now,
            finished_at: None,
            log_path: log_path.clone(),
            working_dir: working_dir.to_path_buf(),
        };

        // Spawn the process
        let process = spawn_detached(command, working_dir, &log_path)?;

        // Persist the complete containment identity before exposing the job.
        let mut job = job;
        job.pid = Some(process.leader_pid);
        job.process_group = Some(process.process_group);
        job.process_start_identity = Some(process.start_identity);
        let pid = process.leader_pid;

        // Persist all metadata before returning. If any write fails, terminate
        // the newly-created group so a storage error cannot leak an
        // unregistered background process.
        if let Err(error) = self
            .save_job(&job)
            .and_then(|_| self.create_pid_file(&job.id, pid).map(|_| ()))
            .and_then(|_| self.create_lock(&job.id, pid).map(|_| ()))
        {
            let _ = signal_process_group(process.process_group, true);
            return Err(error);
        }

        // Store in memory
        self.jobs.insert(job.id.clone(), job.clone());

        Ok(job)
    }

    /// Kill a job's entire recorded process group with TERM, then KILL.
    pub async fn kill(&mut self, id_or_name: &str) -> Result<()> {
        let job_id = {
            let job = self.get(id_or_name).ok_or_else(|| {
                let known: Vec<String> = self.jobs.keys().take(10).cloned().collect();
                let hint = if known.is_empty() {
                    " (no jobs registered; call bg(action:'list') or start one with bg(action:'run'))".to_string()
                } else {
                    format!(" (known job ids: {})", known.join(", "))
                };
                anyhow!("Job not found: '{}'{}", id_or_name, hint)
            })?;
            job.id.clone()
        };

        let state = inspect_job_process(self.jobs.get(&job_id).expect("job exists"));
        let (pid, process_group) = match state {
            JobProcessState::Alive {
                leader_pid,
                process_group,
            } => (leader_pid, process_group),
            JobProcessState::Gone => {
                // A natural exit won the race. Preserve that distinction:
                // without a retained wait handle its exit code is unknown, so
                // it is Orphaned rather than falsely reported Cancelled/Failed.
                self.mark_orphaned(&job_id)?;
                return Ok(());
            }
            JobProcessState::Unsafe(reason) => {
                self.mark_orphaned(&job_id)?;
                return Err(anyhow!(
                    "Refusing to signal background job '{}': {}",
                    job_id,
                    reason
                ));
            }
        };

        // Linux defense in depth for a descendant that deliberately leaves the
        // session. Each captured PID carries its own start identity so a PID
        // recycled during the grace period is never signalled.
        let descendants = capture_descendant_identities(pid);

        signal_process_group(process_group, false)
            .with_context(|| format!("Failed to send SIGTERM to process group {process_group}"))?;
        signal_captured_descendants(&descendants, false);

        let grace_period = Duration::from_secs(KILL_GRACE_PERIOD_SECS);
        let check_interval = Duration::from_millis(100);
        let mut elapsed = Duration::ZERO;
        while elapsed < grace_period && process_group_exists(process_group) {
            tokio::time::sleep(check_interval).await;
            elapsed += check_interval;
        }

        if process_group_exists(process_group) {
            signal_process_group(process_group, true)
                .with_context(|| format!("Failed to SIGKILL process group {process_group}"))?;
        }
        signal_captured_descendants(&descendants, true);

        // Give the runtime's orphan reaper and the kernel time to remove the
        // direct child/group. Never re-target a PID here: all signals above are
        // bounded by the identity validated before TERM.
        for _ in 0..20 {
            if !process_group_exists(process_group) && !captured_descendants_exist(&descendants) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        if process_group_exists(process_group) || captured_descendants_exist(&descendants) {
            self.mark_orphaned(&job_id)?;
            return Err(anyhow!(
                "Background job '{}' still has live processes after SIGKILL",
                job_id
            ));
        }

        let now = Utc::now();
        if let Some(job) = self.jobs.get_mut(&job_id) {
            job.status = JobStatus::Cancelled;
            job.updated_at = now;
            job.finished_at = Some(now);
        }
        if let Some(job) = self.jobs.get(&job_id) {
            self.save_job(job)?;
        }

        Ok(())
    }

    fn mark_orphaned(&mut self, job_id: &str) -> Result<()> {
        let now = Utc::now();
        if let Some(job) = self.jobs.get_mut(job_id)
            && job.status == JobStatus::Running
        {
            job.status = JobStatus::Orphaned;
            job.updated_at = now;
            job.finished_at = Some(now);
        }
        if let Some(job) = self.jobs.get(job_id) {
            self.save_job(job)?;
        }
        Ok(())
    }

    /// Delete a job and its associated files.
    pub async fn delete(&mut self, id_or_name: &str) -> Result<()> {
        // Get job info first
        let (job_id, is_running) = {
            let job = self
                .get(id_or_name)
                .ok_or_else(|| anyhow!("Job not found: {}", id_or_name))?;
            (job.id.clone(), job.status == JobStatus::Running)
        };

        // Kill if running (we can use job_id directly now since we don't need the borrow)
        if is_running {
            // Re-acquire mutable access for kill
            self.kill(&job_id).await.ok();
        }

        // Remove files
        let json_path = self
            .jobs_dir
            .join(format!("{}{}.json", JOB_FILE_PREFIX, job_id));
        let lock_path = self.jobs_dir.join(format!("{}{}", job_id, LOCK_EXT));
        let pid_path = self.jobs_dir.join(format!("{}{}", job_id, PID_EXT));
        let log_path = self.log_path(&job_id);

        for path in &[json_path, lock_path, pid_path, log_path] {
            if path.exists() {
                fs::remove_file(path).ok();
            }
        }

        // Remove from memory
        self.jobs.remove(&job_id);

        Ok(())
    }

    /// Get output from a job's log file.
    pub fn output(&self, id_or_name: &str, lines: Option<usize>) -> Result<String> {
        let job = self.get(id_or_name).ok_or_else(|| {
            let known: Vec<String> = self.jobs.keys().take(10).cloned().collect();
            let hint = if known.is_empty() {
                " (no jobs registered)".to_string()
            } else {
                format!(" (known job ids: {})", known.join(", "))
            };
            anyhow!("Job not found: '{}'{}", id_or_name, hint)
        })?;

        if !job.log_path.exists() {
            // Log file not created yet. Empty string alone misleads the
            // agent into thinking the job actually produced nothing;
            // be explicit about the "not yet" case.
            return Ok(format!(
                "(no output yet — job '{}' is {:?}; log file not yet created at {})",
                job.id,
                job.status,
                job.log_path.display()
            ));
        }

        let file = File::open(&job.log_path)?;
        let reader = BufReader::new(file);

        let all_lines: Vec<String> = reader.lines().map_while(Result::ok).collect();
        if all_lines.is_empty() {
            return Ok(format!(
                "(log file exists but is empty — job '{}' is {:?}; check back in a moment if it's still Running)",
                job.id, job.status
            ));
        }
        let count = lines.unwrap_or(all_lines.len());

        if count >= all_lines.len() {
            Ok(all_lines.join("\n"))
        } else {
            Ok(all_lines[all_lines.len() - count..].join("\n"))
        }
    }

    /// Update job status from the same identity-aware containment check used by kill.
    pub fn check_and_update_status(&mut self, job_id: &str) -> Result<()> {
        let job = self
            .get(job_id)
            .ok_or_else(|| anyhow!("Job not found: {}", job_id))?;
        if job.status != JobStatus::Running {
            return Ok(());
        }

        match inspect_job_process(job) {
            JobProcessState::Alive { .. } => Ok(()),
            JobProcessState::Gone | JobProcessState::Unsafe(_) => self.mark_orphaned(job_id),
        }
    }
}

// ── Helper functions ────────────────────────────────────────────────────────

/// Generate a simple UUID-like string.
fn uuid_simple() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let random: u32 = rand_simple();
    format!("{:x}-{:x}", now, random)
}

/// Simple deterministic-ish random for unique IDs.
fn rand_simple() -> u32 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut hasher = DefaultHasher::new();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    (hasher.finish() as u32)
        .wrapping_mul(1103515245)
        .wrapping_add(12345)
}

#[derive(Debug)]
struct DetachedProcess {
    leader_pid: u32,
    process_group: u32,
    start_identity: String,
}

#[derive(Debug, PartialEq, Eq)]
enum JobProcessState {
    Alive { leader_pid: u32, process_group: u32 },
    Gone,
    Unsafe(String),
}

/// `kill(pid, 0)` liveness, including EPERM (alive but not signalable).
fn process_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
            return true;
        }
        io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn process_group_exists(process_group: u32) -> bool {
    #[cfg(unix)]
    {
        if process_group <= 1 {
            return false;
        }
        if unsafe { libc::kill(-(process_group as libc::pid_t), 0) } == 0 {
            return true;
        }
        io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = process_group;
        false
    }
}

#[cfg(unix)]
fn kill_process(pid: u32, force: bool) -> io::Result<()> {
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    if unsafe { libc::kill(pid as libc::pid_t, signal) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

fn signal_process_group(process_group: u32, force: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        if process_group <= 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "refusing to signal process group 0 or 1",
            ));
        }
        let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
        if unsafe { libc::kill(-(process_group as libc::pid_t), signal) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(error)
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (process_group, force);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "background process groups are unsupported on this platform",
        ))
    }
}

#[cfg(target_os = "linux")]
fn process_start_identity(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let comm_end = stat.rfind(')')?;
    // After pid + comm, index 19 is field 22 (`starttime`, clock ticks
    // since boot). Pair it with Linux's boot UUID so a reboot cannot make a
    // stale PID row accidentally valid again.
    let start_ticks = stat[comm_end + 2..]
        .split_whitespace()
        .nth(19)?
        .parse::<u64>()
        .ok()?;
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    Some(format!("linux:{}:{start_ticks}", boot_id.trim()))
}

#[cfg(target_os = "macos")]
fn process_start_identity(pid: u32) -> Option<String> {
    // `proc_pidinfo(PROC_PIDTBSDINFO)` is the native macOS equivalent of
    // Linux `/proc/<pid>/stat`: the start timeval survives parent exit and
    // distinguishes a recycled PID without invoking any external utility.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let read = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if read != size || info.pbi_pid != pid {
        return None;
    }
    Some(format!(
        "macos:{}:{}",
        info.pbi_start_tvsec, info.pbi_start_tvusec
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_start_identity(_pid: u32) -> Option<String> {
    None
}

fn inspect_job_process(job: &Job) -> JobProcessState {
    let Some(leader_pid) = job.pid else {
        return JobProcessState::Unsafe("missing leader PID".to_string());
    };
    let Some(process_group) = job.process_group else {
        return JobProcessState::Unsafe("legacy row has no process-group identity".to_string());
    };
    let Some(expected_start) = job.process_start_identity.as_deref() else {
        return JobProcessState::Unsafe("legacy row has no process-start identity".to_string());
    };
    if process_group <= 1 || leader_pid != process_group {
        return JobProcessState::Unsafe(format!(
            "invalid containment identity leader={leader_pid} group={process_group}"
        ));
    }

    if process_exists(leader_pid) {
        let Some(actual_start) = process_start_identity(leader_pid) else {
            return JobProcessState::Unsafe("cannot validate leader start identity".to_string());
        };
        if actual_start != expected_start {
            return JobProcessState::Unsafe("leader PID was recycled".to_string());
        }
        #[cfg(unix)]
        {
            let actual_group = unsafe { libc::getpgid(leader_pid as libc::pid_t) };
            if actual_group < 0 {
                return JobProcessState::Unsafe("cannot validate leader process group".to_string());
            }
            if actual_group as u32 != process_group {
                return JobProcessState::Unsafe(
                    "leader moved to a different process group".to_string(),
                );
            }
        }
        return JobProcessState::Alive {
            leader_pid,
            process_group,
        };
    }

    // A shell leader may exit while explicitly-backgrounded members remain.
    // A live group cannot reuse its PGID; conversely, if a new process has
    // reused the leader/PGID then `process_exists(leader_pid)` above validates
    // and rejects its different start identity.
    if process_group_exists(process_group) {
        JobProcessState::Alive {
            leader_pid,
            process_group,
        }
    } else {
        JobProcessState::Gone
    }
}

#[cfg(target_os = "linux")]
fn capture_descendant_identities(root_pid: u32) -> Vec<(u32, String)> {
    crate::service::collect_process_descendants(root_pid)
        .into_iter()
        .filter_map(|pid| process_start_identity(pid).map(|identity| (pid, identity)))
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn capture_descendant_identities(_root_pid: u32) -> Vec<(u32, String)> {
    Vec::new()
}

fn signal_captured_descendants(descendants: &[(u32, String)], force: bool) {
    #[cfg(unix)]
    for (pid, expected_start) in descendants {
        if process_start_identity(*pid).as_deref() == Some(expected_start.as_str()) {
            let _ = kill_process(*pid, force);
        }
    }
    #[cfg(not(unix))]
    let _ = (descendants, force);
}

fn captured_descendants_exist(descendants: &[(u32, String)]) -> bool {
    descendants.iter().any(|(pid, expected_start)| {
        process_start_identity(*pid).as_deref() == Some(expected_start.as_str())
    })
}

/// Spawn a detached session whose leader PID is also its process-group ID.
fn spawn_detached(command: &str, working_dir: &Path, log_path: &Path) -> Result<DetachedProcess> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let bash_path = crate::platform_bash::bash_exe_path(None)
            .map_err(|e| anyhow!("Failed to resolve bash: {e}"))?;
        spawn_detached_with_bash(command, working_dir, log_path, &bash_path)
    }
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
    {
        let _ = (command, working_dir, log_path);
        Err(anyhow!(
            "Background jobs are unsupported on this Unix platform: safe persisted process-start identity is implemented only for Linux and macOS"
        ))
    }
    #[cfg(not(unix))]
    {
        let _ = (command, working_dir, log_path);
        Err(anyhow!(
            "Background jobs are unsupported on this platform: Windows Job Object containment is not implemented (refusing unsafe PID 0 fallback)"
        ))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn spawn_detached_with_bash(
    command: &str,
    working_dir: &Path,
    log_path: &Path,
    bash_path: &Path,
) -> Result<DetachedProcess> {
    let log = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(log_path)
        .with_context(|| format!("Failed to open background log {}", log_path.display()))?;
    let stderr_log = log
        .try_clone()
        .context("Failed to clone background log fd")?;

    let mut cmd = TokioCommand::new(&bash_path);
    cmd.arg("-c")
        // `command` intentionally remains shell grammar. Internal paths
        // never enter that grammar: Rust-opened file descriptors carry
        // stdout/stderr, closing the old path-injection bug.
        .arg(command)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr_log));

    // SAFETY: setsid(2) is async-signal-safe. The closure performs only
    // that syscall and constructs an OS error on failure; it captures no
    // borrowed state and touches no application locks.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = cmd
        .spawn()
        .map_err(|e| anyhow!("Failed to spawn background session: {e}"))?;
    let leader_pid = child
        .id()
        .context("spawned background process has no PID")?;

    // `/proc` and proc_pidinfo normally expose the row immediately, but
    // tolerate a short scheduler race before failing closed.
    let mut start_identity = None;
    for _ in 0..50 {
        start_identity = process_start_identity(leader_pid);
        if start_identity.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let Some(start_identity) = start_identity else {
        let _ = signal_process_group(leader_pid, true);
        return Err(anyhow!(
            "Failed to capture a safe process-start identity for PID {leader_pid}"
        ));
    };

    Ok(DetachedProcess {
        leader_pid,
        process_group: leader_pid,
        start_identity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_job_store_creation() {
        let tmp = TempDir::new().unwrap();
        let store = JobStore::new(tmp.path().to_path_buf()).unwrap();
        assert_eq!(store.running_count(), 0);
    }

    #[tokio::test]
    async fn test_job_store_run_and_list() {
        let tmp = TempDir::new().unwrap();
        let mut store = JobStore::new(tmp.path().to_path_buf()).unwrap();

        // Run a simple background job
        let job = store
            .run("test-job", "sleep 0.1", tmp.path())
            .await
            .unwrap();

        assert_eq!(job.name, "test-job");
        assert_eq!(job.command, "sleep 0.1");
        assert_eq!(job.status, JobStatus::Running);
        assert!(job.pid.is_some());
        assert_eq!(job.process_group, job.pid);
        assert!(job.process_start_identity.is_some());

        // List jobs
        let jobs = store.list();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "test-job");

        // Cleanup
        store.delete("test-job").await.ok();
    }

    #[tokio::test]
    async fn test_job_store_exists() {
        let tmp = TempDir::new().unwrap();
        let mut store = JobStore::new(tmp.path().to_path_buf()).unwrap();

        assert!(!store.exists("my-job"));

        store.run("my-job", "sleep 1", tmp.path()).await.unwrap();

        assert!(store.exists("my-job"));
        assert!(!store.exists("other-job"));

        store.delete("my-job").await.ok();
    }

    #[tokio::test]
    async fn test_job_store_kill() {
        let tmp = TempDir::new().unwrap();
        let mut store = JobStore::new(tmp.path().to_path_buf()).unwrap();

        let _job = store
            .run("kill-test", "sleep 60", tmp.path())
            .await
            .unwrap();

        // Kill should succeed
        store.kill("kill-test").await.unwrap();

        let killed_job = store.get("kill-test").unwrap();
        assert_eq!(killed_job.status, JobStatus::Cancelled);

        store.delete("kill-test").await.ok();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn simple_command_pid_is_the_real_command() {
        let tmp = TempDir::new().unwrap();
        let mut store = JobStore::new(tmp.path().to_path_buf()).unwrap();
        let job = store
            .run("real-command", "/bin/sleep 60", tmp.path())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        let comm = fs::read_to_string(format!("/proc/{}/comm", job.pid.unwrap())).unwrap();
        assert_eq!(
            comm.trim(),
            "sleep",
            "stored PID must not be a transient shell"
        );
        store.kill("real-command").await.unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn background_launch_does_not_require_external_setsid() {
        let tmp = TempDir::new().unwrap();
        let bash_wrapper = tmp.path().join("bash-with-empty-path");
        fs::write(
            &bash_wrapper,
            "#!/bin/sh\nPATH=/definitely-empty exec /bin/bash \"$@\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bash_wrapper, fs::Permissions::from_mode(0o755)).unwrap();

        // Inject the shell path directly rather than mutating process-global
        // environment; this regression can safely run with the full suite.
        let process = spawn_detached_with_bash(
            "/bin/sleep 60",
            tmp.path(),
            &tmp.path().join("detached.log"),
            &bash_wrapper,
        )
        .unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        let alive = process_exists(process.leader_pid);

        assert!(
            alive,
            "background command died because launch depended on an external setsid utility"
        );
        signal_process_group(process.process_group, true).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn log_path_is_data_not_shell_syntax() {
        let tmp = TempDir::new().unwrap();
        let hostile_base = tmp
            .path()
            .join("jobs with spaces 'quote' $(touch PWNED); literal");
        fs::create_dir_all(&hostile_base).unwrap();
        let mut store = JobStore::new(hostile_base).unwrap();

        let job = store
            .run(
                "safe-log",
                "printf 'SAFE_OUTPUT\\n'; /bin/sleep 60",
                tmp.path(),
            )
            .await
            .unwrap();
        for _ in 0..50 {
            if fs::read_to_string(&job.log_path)
                .is_ok_and(|content| content.contains("SAFE_OUTPUT"))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        assert_eq!(fs::read_to_string(&job.log_path).unwrap(), "SAFE_OUTPUT\n");
        assert!(
            !tmp.path().join("PWNED").exists(),
            "internal log path was evaluated as shell syntax"
        );
        store.kill("safe-log").await.unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn reloaded_store_kills_entire_term_ignoring_process_group() {
        let tmp = TempDir::new().unwrap();
        let child_pid_file = tmp.path().join("child.pid");
        let command = format!(
            "trap '' TERM; /bin/sh -c 'trap \"\" TERM; echo $$ > {}; while :; do /bin/sleep 1; done' & wait",
            child_pid_file.display()
        );
        let mut original = JobStore::new(tmp.path().to_path_buf()).unwrap();
        let job = original.run("tree", &command, tmp.path()).await.unwrap();
        for _ in 0..50 {
            if child_pid_file.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let child_pid: u32 = fs::read_to_string(&child_pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(original);

        // A fresh store models an agent/WG restart: no Child handle survives.
        let mut reloaded = JobStore::new(tmp.path().to_path_buf()).unwrap();
        assert_eq!(reloaded.get("tree").unwrap().status, JobStatus::Running);
        reloaded.kill("tree").await.unwrap();
        assert_eq!(reloaded.get("tree").unwrap().status, JobStatus::Cancelled);

        for _ in 0..50 {
            if !process_exists(job.pid.unwrap()) && !process_exists(child_pid) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !process_exists(job.pid.unwrap()),
            "session leader survived kill"
        );
        assert!(
            !process_exists(child_pid),
            "TERM-ignoring child survived group kill"
        );
        assert!(!process_group_exists(job.process_group.unwrap()));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn compound_and_pipeline_groups_are_fully_cancelled() {
        let tmp = TempDir::new().unwrap();
        let mut store = JobStore::new(tmp.path().to_path_buf()).unwrap();
        for (name, command) in [
            ("compound", "/bin/sh -c '/bin/sleep 60; echo never'"),
            ("pipeline", "/bin/sleep 60 | /bin/cat"),
        ] {
            let job = store.run(name, command, tmp.path()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            store.kill(name).await.unwrap();
            assert_eq!(store.get(name).unwrap().status, JobStatus::Cancelled);
            assert!(
                !process_group_exists(job.process_group.unwrap()),
                "{name} left process group {} behind",
                job.process_group.unwrap()
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn stale_start_identity_refuses_to_kill_foreign_pid() {
        let tmp = TempDir::new().unwrap();
        let mut store = JobStore::new(tmp.path().to_path_buf()).unwrap();
        let job = store
            .run("identity", "/bin/sleep 60", tmp.path())
            .await
            .unwrap();
        let real_identity = job.process_start_identity.clone();
        store.jobs.get_mut(&job.id).unwrap().process_start_identity =
            Some("recycled-process".to_string());

        let error = store.kill("identity").await.unwrap_err();
        assert!(error.to_string().contains("recycled"));
        assert!(
            process_exists(job.pid.unwrap()),
            "wrong-PID guard killed live process"
        );
        assert_eq!(store.get("identity").unwrap().status, JobStatus::Orphaned);

        // Test cleanup uses the identity captured before deliberate corruption.
        let stored = store.jobs.get_mut(&job.id).unwrap();
        stored.status = JobStatus::Running;
        stored.process_start_identity = real_identity;
        store.kill("identity").await.unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn natural_exit_before_kill_is_not_misreported_cancelled() {
        let tmp = TempDir::new().unwrap();
        let mut store = JobStore::new(tmp.path().to_path_buf()).unwrap();
        store.run("natural", "exit 0", tmp.path()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        store.kill("natural").await.unwrap();
        assert_eq!(store.get("natural").unwrap().status, JobStatus::Orphaned);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn repeated_start_kill_leaves_no_process_groups() {
        let tmp = TempDir::new().unwrap();
        let mut store = JobStore::new(tmp.path().to_path_buf()).unwrap();
        for iteration in 0..50 {
            let name = format!("repeat-{iteration}");
            let job = store.run(&name, "/bin/sleep 60", tmp.path()).await.unwrap();
            store.kill(&name).await.unwrap();
            assert_eq!(store.get(&name).unwrap().status, JobStatus::Cancelled);
            assert!(
                !process_group_exists(job.process_group.unwrap()),
                "iteration {iteration} leaked process group {}",
                job.process_group.unwrap()
            );
        }
    }

    #[test]
    fn legacy_job_row_defaults_to_missing_safe_identity() {
        let value = serde_json::json!({
            "id": "job-old",
            "name": "old",
            "command": "sleep 1",
            "status": "running",
            "pid": 4242,
            "exit_code": null,
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "finished_at": null,
            "log_path": "/tmp/old.log",
            "working_dir": "/tmp"
        });
        let job: Job = serde_json::from_value(value).unwrap();
        assert_eq!(job.process_group, None);
        assert_eq!(job.process_start_identity, None);
        assert!(matches!(
            inspect_job_process(&job),
            JobProcessState::Unsafe(_)
        ));
    }

    #[tokio::test]
    async fn test_duplicate_name_rejected() {
        let tmp = TempDir::new().unwrap();
        let mut store = JobStore::new(tmp.path().to_path_buf()).unwrap();

        store.run("dup", "sleep 1", tmp.path()).await.unwrap();

        let result = store.run("dup", "sleep 2", tmp.path()).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("already exists"));

        store.delete("dup").await.ok();
    }
}
