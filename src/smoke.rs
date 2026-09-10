//! Smoke gate: structured manifest of regression-protecting scenarios that
//! `wg done` runs before letting a task be marked complete.
//!
//! The contract is intentionally minimal: each scenario is a script that
//! the manifest declares as a permanent regression check, owned by one or
//! more tasks. When `wg done <task>` runs, it executes every scenario whose
//! `owners` list contains the task id (or every scenario when `--full-smoke`
//! is passed). A scenario that exits non-zero blocks `wg done` and the
//! caller is told exactly which scenario broke.
//!
//! Scenarios use exit codes to communicate three states:
//!   * 0   → PASS
//!   * 77  → loud SKIP (endpoint unreachable, missing credential, etc.)
//!   * any other non-zero → FAIL
//!
//! 77 is the GNU autotools convention for "skipped"; we reuse it so scripts
//! can express "I cannot run here" without lying about a pass.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Default smoke fixture root. Bash-side `wg_smoke_root` in `_helpers.sh`
/// and the Rust-side sweeper agree on this path so a leak left by either
/// side is reachable from either side.
const DEFAULT_SMOKE_ROOT_NAME: &str = "wgsmoke";

/// Default location of the smoke manifest, relative to the repo root.
pub const DEFAULT_MANIFEST_PATH: &str = "tests/smoke/manifest.toml";

/// Exit code that scenario scripts must use to signal a loud SKIP.
pub const SKIP_EXIT_CODE: i32 = 77;

/// Default per-scenario timeout when not specified in the manifest.
const DEFAULT_TIMEOUT_SECS: u64 = 180;

/// Exact environment identity inherited by every process launched by one
/// scenario. Cleanup matches this random value, never an executable name.
pub const SMOKE_RUN_ID_ENV: &str = "WG_SMOKE_RUN_ID";
const SMOKE_OWNER_FILE_ENV: &str = "WG_SMOKE_OWNER_FILE";
const SMOKE_DIAGNOSTICS_ENV: &str = "WG_SMOKE_CLEANUP_DIAGNOSTICS";
const OWNERS_DIR_NAME: &str = ".owners";
const TERM_GRACE: Duration = Duration::from_secs(2);
const KILL_GRACE: Duration = Duration::from_secs(3);

/// `PR_SET_CHILD_SUBREAPER` and temporary signal/process ownership are
/// process-global. Smoke scenarios are deliberately sequential, and this lock
/// also keeps direct unit-test calls from racing under cargo's test threads.
fn scenario_process_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone)]
struct OwnedProcess {
    pid: u32,
    ppid: u32,
    process_group: u32,
    session: u32,
    start_ticks: u64,
    state: char,
    command: String,
}

#[derive(Debug)]
struct ScenarioOwnership {
    run_id: String,
    scenario: String,
    owner_dir: PathBuf,
    owner_file: PathBuf,
    diagnostics_file: PathBuf,
    _subreaper: Option<SubreaperGuard>,
    cleaned: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Manifest {
    #[serde(default, rename = "scenario")]
    pub scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Scenario {
    pub name: String,
    pub script: String,
    #[serde(default)]
    pub owners: Vec<String>,
    #[serde(default)]
    pub description: String,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenarioOutcome {
    Pass,
    Fail { exit_code: i32, stderr_tail: String },
    Skip { reason: String },
    Error { message: String },
}

#[derive(Debug, Clone)]
pub struct ScenarioResult {
    pub name: String,
    pub outcome: ScenarioOutcome,
}

impl Manifest {
    /// Load a manifest from a specific path. Returns an empty manifest if the
    /// file does not exist (smoke gate is a no-op when no manifest is defined).
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Manifest { scenarios: vec![] });
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read smoke manifest at {}", path.display()))?;
        let mut manifest: Manifest = toml::from_str(&text)
            .with_context(|| format!("failed to parse smoke manifest at {}", path.display()))?;
        // Detect duplicate scenario names eagerly — the manifest is grow-only
        // and a name collision masks a regression.
        let mut seen = std::collections::HashSet::new();
        for s in &manifest.scenarios {
            if !seen.insert(s.name.clone()) {
                anyhow::bail!(
                    "smoke manifest at {} contains duplicate scenario name '{}'",
                    path.display(),
                    s.name
                );
            }
        }
        // Normalise owner ids — trim whitespace, drop empties.
        for s in &mut manifest.scenarios {
            s.owners = std::mem::take(&mut s.owners)
                .into_iter()
                .map(|o| o.trim().to_string())
                .filter(|o| !o.is_empty())
                .collect();
        }
        Ok(manifest)
    }

    /// Resolve the manifest path used by `wg done`. Order:
    ///   1. `WG_SMOKE_MANIFEST` env var (absolute or relative to `dir`).
    ///   2. `<dir>/tests/smoke/manifest.toml`.
    ///   3. `<git toplevel>/tests/smoke/manifest.toml` (when `dir` is inside
    ///      a worktree).
    pub fn resolve_path(dir: &Path) -> PathBuf {
        if let Ok(env_path) = std::env::var("WG_SMOKE_MANIFEST") {
            let p = PathBuf::from(&env_path);
            return if p.is_absolute() { p } else { dir.join(p) };
        }
        let local = dir.join(DEFAULT_MANIFEST_PATH);
        if local.exists() {
            return local;
        }
        if let Some(parent) = dir.parent() {
            let candidate = parent.join(DEFAULT_MANIFEST_PATH);
            if candidate.exists() {
                return candidate;
            }
        }
        if let Some(top) = git_toplevel(dir) {
            let candidate = top.join(DEFAULT_MANIFEST_PATH);
            if candidate.exists() {
                return candidate;
            }
        }
        local
    }

    /// Load the manifest using the standard resolution order.
    pub fn load(dir: &Path) -> Result<Self> {
        let path = Self::resolve_path(dir);
        Self::load_from(&path)
    }

    /// Return scenarios owned by a specific task id.
    pub fn scenarios_for_task(&self, task_id: &str) -> Vec<&Scenario> {
        self.scenarios
            .iter()
            .filter(|s| s.owners.iter().any(|o| o == task_id))
            .collect()
    }
}

fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8(output.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

impl ScenarioOwnership {
    fn create(scenario: &str) -> Result<Self> {
        let run_id = format!("wg-smoke-v2:{}", uuid::Uuid::now_v7());
        let safe_name: String = scenario
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .take(80)
            .collect();
        let owner_dir = smoke_root().join(OWNERS_DIR_NAME).join(format!(
            "{}-{}",
            safe_name,
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&owner_dir).with_context(|| {
            format!(
                "failed to create smoke ownership directory {}",
                owner_dir.display()
            )
        })?;
        let owner_file = owner_dir.join("owner.env");
        let diagnostics_file = owner_dir.join("cleanup-diagnostics.log");
        let ownership = Self {
            run_id,
            scenario: scenario.to_string(),
            owner_dir,
            owner_file,
            diagnostics_file,
            _subreaper: None,
            cleaned: false,
        };
        // Publish the exact run identity before returning an ownership handle.
        // `run_scenario` cannot spawn anything until `create` succeeds, so even
        // a SIGKILL immediately after spawn leaves an authoritative marker for
        // the next global sweep.
        ownership.record_supervisor()?;
        Ok(ownership)
    }

    fn install_subreaper(&mut self) -> Result<()> {
        self._subreaper = Some(SubreaperGuard::install()?);
        Ok(())
    }

    fn record_supervisor(&self) -> Result<()> {
        let supervisor = process_info(std::process::id());
        let mut body = format!(
            "version=3\nrun_id={}\nscenario={}\nsupervisor_pid={}\n",
            self.run_id,
            self.scenario.replace('\n', " "),
            std::process::id()
        );
        if let Some(supervisor) = supervisor {
            body.push_str(&format!(
                "supervisor_start_ticks={}\nsupervisor_process_group={}\nsupervisor_session={}\n",
                supervisor.start_ticks, supervisor.process_group, supervisor.session
            ));
        }

        // A complete temporary record is renamed into place. There is no
        // launched child yet, so failure cannot strand a process; success
        // guarantees later root metadata can only append to valid authority.
        let pending = self.owner_dir.join("owner.env.pending");
        let mut file = File::create(&pending).with_context(|| {
            format!(
                "failed to create smoke ownership record {}",
                pending.display()
            )
        })?;
        file.write_all(body.as_bytes()).with_context(|| {
            format!(
                "failed to write smoke ownership record {}",
                pending.display()
            )
        })?;
        file.sync_all().with_context(|| {
            format!(
                "failed to sync smoke ownership record {}",
                pending.display()
            )
        })?;
        std::fs::rename(&pending, &self.owner_file).with_context(|| {
            format!(
                "failed to publish smoke ownership record {}",
                self.owner_file.display()
            )
        })
    }

    fn record_root(&self, pid: u32) -> Result<()> {
        let body = if let Some(info) = process_info(pid) {
            format!(
                "root_pid={}\nroot_start_ticks={}\nroot_process_group={}\nroot_session={}\n",
                info.pid, info.start_ticks, info.process_group, info.session
            )
        } else {
            format!("root_pid={}\n", pid)
        };
        // Append rather than rewrite: interruption can at worst leave partial
        // root diagnostics, never destroy the pre-spawn run-id authority used
        // by exact-marker cleanup.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.owner_file)
            .with_context(|| {
                format!(
                    "failed to open smoke ownership record {}",
                    self.owner_file.display()
                )
            })?;
        file.write_all(body.as_bytes()).with_context(|| {
            format!(
                "failed to append smoke root identity to {}",
                self.owner_file.display()
            )
        })?;
        file.sync_all().with_context(|| {
            format!(
                "failed to sync smoke root identity to {}",
                self.owner_file.display()
            )
        })
    }

    fn cleanup(&mut self) -> std::result::Result<(), String> {
        if self.cleaned {
            return Ok(());
        }
        terminate_owned_processes(&self.run_id, &self.scenario, &self.diagnostics_file, true)?;
        // The shell deliberately leaves fixture registries in owner_dir for
        // this subreaper. Adopted grandchildren must be waited out before a
        // cwd/log directory they used can be deleted.
        reap_adopted_children();
        if let Err(error) = remove_registered_scratch_dirs(&self.owner_dir) {
            let message = format!(
                "smoke cleanup could not remove registered fixtures for '{}': {error}; ownership retained at {}",
                self.scenario,
                self.owner_dir.display()
            );
            append_cleanup_diagnostic(&self.diagnostics_file, &message);
            return Err(message);
        }
        self.cleaned = true;
        std::fs::remove_dir_all(&self.owner_dir).map_err(|error| {
            format!(
                "smoke cleanup could not remove ownership directory {}: {error}",
                self.owner_dir.display()
            )
        })
    }
}

impl Drop for ScenarioOwnership {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Run a single scenario. Every invocation gets an unguessable ownership
/// marker plus a new Unix session. Descendants inherit the marker even when a
/// daemon double-forks or a fixture calls `setsid`; the post-run cleanup scans
/// that exact marker and therefore cannot select an unrelated user Pi session.
pub fn run_scenario(scenario: &Scenario, manifest_dir: &Path) -> ScenarioResult {
    let _process_guard = scenario_process_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let script_path = if Path::new(&scenario.script).is_absolute() {
        PathBuf::from(&scenario.script)
    } else {
        manifest_dir.join(&scenario.script)
    };

    if !script_path.exists() {
        return ScenarioResult {
            name: scenario.name.clone(),
            outcome: ScenarioOutcome::Error {
                message: format!("script not found: {}", script_path.display()),
            },
        };
    }

    let timeout = Duration::from_secs(scenario.timeout_seconds.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let mut ownership = match ScenarioOwnership::create(&scenario.name) {
        Ok(value) => value,
        Err(error) => {
            return ScenarioResult {
                name: scenario.name.clone(),
                outcome: ScenarioOutcome::Error {
                    message: error.to_string(),
                },
            };
        }
    };
    // Install the adoption boundary immediately after the durable pre-spawn
    // owner record and before constructing or launching the scenario command.
    // Fail closed on Linux: without subreaper ownership, an orphaned zombie
    // cannot be waited by this harness even though its live process is marked.
    if let Err(error) = ownership.install_subreaper() {
        return ScenarioResult {
            name: scenario.name.clone(),
            outcome: ScenarioOutcome::Error {
                message: format!("failed to install smoke child subreaper: {error}"),
            },
        };
    }

    let stdout_path = ownership.owner_dir.join("stdout.log");
    let stderr_path = ownership.owner_dir.join("stderr.log");
    let stdout = match File::create(&stdout_path) {
        Ok(file) => file,
        Err(error) => {
            return ScenarioResult {
                name: scenario.name.clone(),
                outcome: ScenarioOutcome::Error {
                    message: format!("failed to create smoke stdout log: {error}"),
                },
            };
        }
    };
    let stderr = match File::create(&stderr_path) {
        Ok(file) => file,
        Err(error) => {
            return ScenarioResult {
                name: scenario.name.clone(),
                outcome: ScenarioOutcome::Error {
                    message: format!("failed to create smoke stderr log: {error}"),
                },
            };
        }
    };

    // GNU timeout's --kill-after is essential: a TERM-ignoring scenario must
    // not pin `wg done` forever. The exact-marker cleanup below catches any
    // descendant that escaped timeout's process group with a double fork.
    let mut cmd = if which_timeout() {
        let mut command = Command::new("timeout");
        command
            .arg("--preserve-status")
            .arg("--kill-after=3s")
            .arg(format!("{}", timeout.as_secs()))
            .arg("bash")
            .arg(&script_path);
        command
    } else {
        let mut command = Command::new("bash");
        command.arg(&script_path);
        command
    };
    cmd.env("WG_SMOKE_SCENARIO", &scenario.name)
        .env("WG_SMOKE_TIMEOUT_SECS", timeout.as_secs().to_string())
        .env(SMOKE_RUN_ID_ENV, &ownership.run_id)
        .env(SMOKE_OWNER_FILE_ENV, &ownership.owner_file)
        .env(SMOKE_DIAGNOSTICS_ENV, &ownership.diagnostics_file)
        .env("WG_SMOKE_HARNESS_OWNED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: this runs after fork and before exec. `setsid` touches only
        // kernel process metadata and reports errors through errno.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    let _ = libc::setpgid(0, 0);
                }
                Ok(())
            });
        }
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ScenarioResult {
                name: scenario.name.clone(),
                outcome: ScenarioOutcome::Error {
                    message: format!("failed to spawn script: {error}"),
                },
            };
        }
    };
    if let Err(error) = ownership.record_root(child.id()) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
            libc::kill(child.id() as libc::pid_t, libc::SIGKILL);
        }
        #[cfg(not(unix))]
        let _ = child.kill();
        let _ = child.wait();
        return ScenarioResult {
            name: scenario.name.clone(),
            outcome: ScenarioOutcome::Error {
                message: error.to_string(),
            },
        };
    }

    // Independent Rust watchdog: GNU timeout is only the first line of
    // defence. This also bounds platforms without timeout(1), and its exact
    // marker scan catches a child that escaped the root process group.
    let watchdog_state = Arc::new((Mutex::new(false), Condvar::new()));
    let watchdog_thread = {
        let state = Arc::clone(&watchdog_state);
        let run_id = ownership.run_id.clone();
        let scenario_name = ownership.scenario.clone();
        let diagnostics = ownership.diagnostics_file.clone();
        let root_pid = child.id();
        std::thread::spawn(move || {
            let (done_lock, wake) = &*state;
            let done = done_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let wait = timeout.saturating_add(Duration::from_secs(4));
            let (done, timed_out) = wake
                .wait_timeout_while(done, wait, |done| !*done)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if timed_out.timed_out() && !*done {
                #[cfg(unix)]
                unsafe {
                    // The root was created as a session/group leader. This
                    // promptly breaks the wait even if its environment was
                    // corrupted; exact-marker cleanup handles escaped groups.
                    libc::kill(-(root_pid as libc::pid_t), libc::SIGKILL);
                    libc::kill(root_pid as libc::pid_t, libc::SIGKILL);
                }
                // Do not waitpid here: the main thread owns the root Child
                // handle and must collect its status. Final cleanup reaps the
                // now-adopted descendants after that wait completes.
                let _ = terminate_owned_processes(&run_id, &scenario_name, &diagnostics, false);
            }
        })
    };

    let status = child.wait();
    {
        let (done, wake) = &*watchdog_state;
        *done.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        wake.notify_all();
    }
    let _ = watchdog_thread.join();
    // Read the scenario's own diagnostics before successful cleanup removes
    // the private ownership directory. Descendants write to regular files,
    // not captured pipes, so an escaped process can never pin this read open.
    let stderr_bytes = read_bounded_file(&stderr_path, 256 * 1024);
    let cleanup_result = ownership.cleanup();
    reap_adopted_children();

    if let Err(cleanup_error) = cleanup_result {
        return ScenarioResult {
            name: scenario.name.clone(),
            outcome: ScenarioOutcome::Error {
                message: cleanup_error,
            },
        };
    }
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            return ScenarioResult {
                name: scenario.name.clone(),
                outcome: ScenarioOutcome::Error {
                    message: format!("failed waiting for scenario: {error}"),
                },
            };
        }
    };

    match status.code() {
        Some(0) => ScenarioResult {
            name: scenario.name.clone(),
            outcome: ScenarioOutcome::Pass,
        },
        Some(code) if code == SKIP_EXIT_CODE => {
            let reason = stderr_tail(&stderr_bytes, 4);
            ScenarioResult {
                name: scenario.name.clone(),
                outcome: ScenarioOutcome::Skip {
                    reason: if reason.is_empty() {
                        "scenario emitted SKIP (exit 77)".to_string()
                    } else {
                        reason
                    },
                },
            }
        }
        Some(code) => ScenarioResult {
            name: scenario.name.clone(),
            outcome: ScenarioOutcome::Fail {
                exit_code: code,
                stderr_tail: stderr_tail(&stderr_bytes, 12),
            },
        },
        None => ScenarioResult {
            name: scenario.name.clone(),
            outcome: ScenarioOutcome::Fail {
                exit_code: -1,
                stderr_tail: stderr_tail(&stderr_bytes, 12),
            },
        },
    }
}

fn which_timeout() -> bool {
    Command::new("timeout")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn append_cleanup_diagnostic(path: &Path, message: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{message}");
    }
}

fn remove_registered_scratch_dirs(owner_dir: &Path) -> std::result::Result<(), String> {
    let owners_root = owner_dir
        .parent()
        .ok_or_else(|| format!("invalid ownership path {}", owner_dir.display()))?;
    let root = owners_root
        .parent()
        .ok_or_else(|| format!("invalid ownership path {}", owner_dir.display()))?;
    let root_canonical = root
        .canonicalize()
        .map_err(|error| format!("{}: {error}", root.display()))?;
    let owners_canonical = owners_root
        .canonicalize()
        .map_err(|error| format!("{}: {error}", owners_root.display()))?;
    let Ok(entries) = std::fs::read_dir(owner_dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let scratches = entry.path().join("scratches");
        let Ok(contents) = std::fs::read_to_string(&scratches) else {
            continue;
        };
        for line in contents.lines() {
            let path = Path::new(line);
            let Ok(metadata) = std::fs::symlink_metadata(path) else {
                continue;
            };
            if line.is_empty() || metadata.file_type().is_symlink() || !metadata.is_dir() {
                continue;
            }
            let canonical = path
                .canonicalize()
                .map_err(|error| format!("{}: {error}", path.display()))?;
            if canonical == root_canonical
                || !canonical.starts_with(&root_canonical)
                || canonical.starts_with(&owners_canonical)
            {
                continue;
            }
            std::fs::remove_dir_all(&canonical)
                .map_err(|error| format!("{}: {error}", canonical.display()))?;
        }
    }
    Ok(())
}

fn read_bounded_file(path: &Path, max_bytes: usize) -> Vec<u8> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(metadata) = file.metadata() else {
        return Vec::new();
    };
    let len = metadata.len() as usize;
    if len > max_bytes {
        use std::io::Seek;
        let _ = file.seek(std::io::SeekFrom::Start((len - max_bytes) as u64));
    }
    let mut bytes = Vec::with_capacity(len.min(max_bytes));
    let _ = file.take(max_bytes as u64).read_to_end(&mut bytes);
    bytes
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct SubreaperGuard {
    previous: libc::c_int,
}

#[cfg(target_os = "linux")]
impl SubreaperGuard {
    fn install() -> Result<Self> {
        let mut previous = 0;
        // SAFETY: prctl writes one integer supplied by this process.
        unsafe {
            if libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut previous) == -1 {
                return Err(std::io::Error::last_os_error())
                    .context("PR_GET_CHILD_SUBREAPER failed");
            }
            if libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) == -1 {
                return Err(std::io::Error::last_os_error())
                    .context("PR_SET_CHILD_SUBREAPER failed");
            }
        }
        Ok(Self { previous })
    }
}

#[cfg(target_os = "linux")]
impl Drop for SubreaperGuard {
    fn drop(&mut self) {
        // SAFETY: restore the process-global flag captured during install.
        unsafe {
            libc::prctl(libc::PR_SET_CHILD_SUBREAPER, self.previous);
        }
    }
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug)]
struct SubreaperGuard;

#[cfg(not(target_os = "linux"))]
impl SubreaperGuard {
    fn install() -> Result<Self> {
        Ok(Self)
    }
}

#[cfg(target_os = "linux")]
fn process_info(pid: u32) -> Option<OwnedProcess> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = stat.rsplit_once(") ")?.1;
    let fields: Vec<&str> = after_name.split_whitespace().collect();
    if fields.len() <= 19 {
        return None;
    }
    let command = std::fs::read(format!("/proc/{pid}/cmdline"))
        .ok()
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .split('\0')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    Some(OwnedProcess {
        pid,
        state: fields[0].chars().next().unwrap_or('?'),
        ppid: fields[1].parse().ok()?,
        process_group: fields[2].parse().ok()?,
        session: fields[3].parse().ok()?,
        start_ticks: fields[19].parse().ok()?,
        command: command.chars().take(300).collect(),
    })
}

#[cfg(not(target_os = "linux"))]
fn process_info(_pid: u32) -> Option<OwnedProcess> {
    None
}

#[cfg(target_os = "linux")]
fn process_has_run_id(pid: u32, run_id: &str) -> bool {
    let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
        return false;
    };
    let expected = format!("{SMOKE_RUN_ID_ENV}={run_id}");
    environ
        .split(|byte| *byte == 0)
        .any(|entry| entry == expected.as_bytes())
}

#[cfg(target_os = "linux")]
fn scan_owned_processes(run_id: &str) -> Vec<OwnedProcess> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut processes = Vec::new();
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if process_has_run_id(pid, run_id)
            && let Some(info) = process_info(pid)
        {
            processes.push(info);
        }
    }
    processes.sort_by_key(|process| process.pid);
    processes
}

#[cfg(not(target_os = "linux"))]
fn scan_owned_processes(_run_id: &str) -> Vec<OwnedProcess> {
    Vec::new()
}

#[cfg(unix)]
fn signal_owned_process(process: &OwnedProcess, run_id: &str, signal: libc::c_int) {
    let Some(current) = process_info(process.pid) else {
        return;
    };
    // Re-check both the immutable kernel start identity and the unguessable
    // environment marker immediately before kill. This closes PID reuse and
    // guarantees that a same-named user Pi can never be selected.
    if current.start_ticks != process.start_ticks || !process_has_run_id(process.pid, run_id) {
        return;
    }
    // SAFETY: kill validates its arguments in the kernel. Selection was
    // revalidated above and failure merely means the process exited first.
    unsafe {
        libc::kill(process.pid as libc::pid_t, signal);
    }
}

#[cfg(not(unix))]
fn signal_owned_process(_process: &OwnedProcess, _run_id: &str, _signal: libc::c_int) {}

#[cfg(target_os = "linux")]
fn reap_adopted_children() {
    loop {
        let mut status = 0;
        // SAFETY: WNOHANG never blocks and writes only to `status`.
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if pid <= 0 {
            break;
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn reap_adopted_children() {}

fn write_cleanup_diagnostics(
    path: &Path,
    scenario: &str,
    run_id: &str,
    seen: &BTreeMap<(u32, u64), OwnedProcess>,
    survivors: &[OwnedProcess],
) {
    let mut body = format!(
        "scenario={}\nrun_id={}\nsupervisor_pid={}\nsurvivors={}\n",
        scenario.replace('\n', " "),
        run_id,
        std::process::id(),
        survivors.len()
    );
    for process in seen.values().take(128) {
        body.push_str(&format!(
            "pid={} start_ticks={} ppid={} process_group={} session={} state={} command={}\n",
            process.pid,
            process.start_ticks,
            process.ppid,
            process.process_group,
            process.session,
            process.state,
            process.command.replace('\n', " ")
        ));
    }
    let _ = std::fs::write(path, body);
}

fn terminate_owned_processes(
    run_id: &str,
    scenario: &str,
    diagnostics_file: &Path,
    reap_children: bool,
) -> std::result::Result<(), String> {
    let mut seen = BTreeMap::new();
    let term_deadline = Instant::now() + TERM_GRACE;
    loop {
        let processes = scan_owned_processes(run_id);
        for process in &processes {
            seen.entry((process.pid, process.start_ticks))
                .or_insert_with(|| process.clone());
            signal_owned_process(process, run_id, libc::SIGTERM);
        }
        if reap_children {
            reap_adopted_children();
        }
        if processes.is_empty() || Instant::now() >= term_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let kill_deadline = Instant::now() + KILL_GRACE;
    loop {
        let processes = scan_owned_processes(run_id);
        for process in &processes {
            seen.entry((process.pid, process.start_ticks))
                .or_insert_with(|| process.clone());
            signal_owned_process(process, run_id, libc::SIGKILL);
        }
        if reap_children {
            reap_adopted_children();
        }
        if processes.is_empty() || Instant::now() >= kill_deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if reap_children {
        reap_adopted_children();
    }
    let survivors = scan_owned_processes(run_id);
    if survivors.is_empty() {
        return Ok(());
    }
    for process in &survivors {
        seen.entry((process.pid, process.start_ticks))
            .or_insert_with(|| process.clone());
    }
    write_cleanup_diagnostics(diagnostics_file, scenario, run_id, &seen, &survivors);
    Err(format!(
        "smoke cleanup left {} process(es) owned by scenario '{}' alive; diagnostics: {}",
        survivors.len(),
        scenario,
        diagnostics_file.display()
    ))
}

fn stderr_tail(bytes: &[u8], n: usize) -> String {
    let s = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Aggregate result of running a collection of scenarios.
#[derive(Debug, Default, Clone)]
pub struct GateReport {
    pub results: Vec<ScenarioResult>,
}

impl GateReport {
    pub fn failures(&self) -> Vec<&ScenarioResult> {
        self.results
            .iter()
            .filter(|r| matches!(r.outcome, ScenarioOutcome::Fail { .. }))
            .collect()
    }

    pub fn errors(&self) -> Vec<&ScenarioResult> {
        self.results
            .iter()
            .filter(|r| matches!(r.outcome, ScenarioOutcome::Error { .. }))
            .collect()
    }

    pub fn skips(&self) -> Vec<&ScenarioResult> {
        self.results
            .iter()
            .filter(|r| matches!(r.outcome, ScenarioOutcome::Skip { .. }))
            .collect()
    }

    pub fn passes(&self) -> Vec<&ScenarioResult> {
        self.results
            .iter()
            .filter(|r| matches!(r.outcome, ScenarioOutcome::Pass))
            .collect()
    }

    /// Returns true when at least one scenario FAILED or had an Error.
    /// Skips never block the gate.
    pub fn blocks_done(&self) -> bool {
        !self.failures().is_empty() || !self.errors().is_empty()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for r in &self.results {
            match &r.outcome {
                ScenarioOutcome::Pass => out.push_str(&format!("  PASS  {}\n", r.name)),
                ScenarioOutcome::Skip { reason } => {
                    out.push_str(&format!("  SKIP  {} — {}\n", r.name, reason))
                }
                ScenarioOutcome::Fail {
                    exit_code,
                    stderr_tail,
                } => {
                    out.push_str(&format!(
                        "  FAIL  {} (exit {})\n        {}\n",
                        r.name,
                        exit_code,
                        stderr_tail.replace('\n', "\n        ")
                    ));
                }
                ScenarioOutcome::Error { message } => {
                    out.push_str(&format!("  ERROR {} — {}\n", r.name, message));
                }
            }
        }
        out
    }
}

/// Run every scenario in `scenarios`. The manifest_dir is needed to resolve
/// relative script paths.
///
/// Sweeps stale smoke-test daemons + scratch dirs both before AND after the
/// run. Pre-sweep prevents leaked state from a prior crashed run from
/// influencing the current run. Post-sweep guarantees that even if a
/// scenario crashes past its bash trap (SIGKILL, OOM, hard panic), the
/// system is left clean. See `tests/smoke/scenarios/_helpers.sh` for the
/// matching bash-side `wg_smoke_sweep`.
///
/// Exact live supervisor identity always protects a concurrent smoke run in
/// another worktree, regardless of age. The age cutoff applies only to legacy
/// records without PID-start metadata; a dead exact supervisor is reaped
/// immediately.
pub fn run_scenarios(scenarios: &[&Scenario], manifest_dir: &Path) -> GateReport {
    sweep_smoke_leaks_older_than(LEAK_REAP_MIN_AGE);
    let mut results = Vec::with_capacity(scenarios.len());
    for s in scenarios {
        results.push(run_scenario(s, manifest_dir));
    }
    sweep_smoke_leaks_older_than(LEAK_REAP_MIN_AGE);
    GateReport { results }
}

/// Daemons / scratch dirs younger than this are left alone by the gate
/// sweep so a concurrent smoke run doesn't cannibalise itself. Per-scenario
/// bash traps in `_helpers.sh` are the primary teardown; this sweep is
/// defence in depth for genuinely leaked fixtures.
const LEAK_REAP_MIN_AGE: Duration = Duration::from_secs(600);

/// Resolve the smoke fixture root (`${WG_SMOKE_ROOT:-${TMPDIR:-/tmp}/wgsmoke}`).
/// Mirrors the bash `wg_smoke_root` in `tests/smoke/scenarios/_helpers.sh`.
pub fn smoke_root() -> PathBuf {
    if let Ok(env_root) = std::env::var("WG_SMOKE_ROOT")
        && !env_root.is_empty()
    {
        return PathBuf::from(env_root);
    }
    let tmp = std::env::var("TMPDIR")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/tmp".to_string());
    PathBuf::from(tmp).join(DEFAULT_SMOKE_ROOT_NAME)
}

/// Find and terminate every process carrying an abandoned scenario ownership
/// record, then remove only scratch dirs listed by that record. Selection is by an
/// unguessable `WG_SMOKE_RUN_ID` recorded before launch — never by `pi`, `wg`,
/// a command-line substring, or a guessed PID alone.
///
/// `min_age` affects only legacy records without exact supervisor metadata;
/// concurrent live exact owners remain protected at every value.
pub fn sweep_smoke_leaks_older_than(min_age: Duration) {
    sweep_smoke_leaks_under(&smoke_root(), min_age)
}

/// Convenience: reap every explicitly-owned abandoned run regardless of age.
/// Unowned directories are never authority to signal or delete anything.
pub fn sweep_smoke_leaks() {
    sweep_smoke_leaks_under(&smoke_root(), Duration::ZERO);
}

/// Sweep against an explicit root (useful for tests, where pointing at a
/// shared global root would race with parallel test threads).
pub fn sweep_smoke_leaks_under(root: &Path, min_age: Duration) {
    // `waitpid(-1)` is process-global. Serialize sweeps with scenario runs so
    // one test thread can never reap another thread's scenario root child.
    let _process_guard = scenario_process_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let owners = root.join(OWNERS_DIR_NAME);
    if let Ok(entries) = std::fs::read_dir(&owners) {
        for entry in entries.flatten() {
            let owner_dir = entry.path();
            let owner_file = owner_dir.join("owner.env");
            let fields = read_owner_fields(&owner_file);
            // A live exact supervisor is never stale, regardless of age. A
            // dead PID+start identity is abandoned immediately; only legacy
            // records without that identity fall back to the age cutoff.
            match owner_record_supervisor_live(&fields) {
                Some(true) => continue,
                Some(false) => {}
                None if !path_age_exceeds(&owner_dir, min_age) => continue,
                None => {}
            }
            let Some(run_id) = fields.get("run_id") else {
                // An incomplete record is diagnostic evidence, not authority
                // to kill anything. Keep it for the operator.
                continue;
            };
            let scenario = fields
                .get("scenario")
                .map(String::as_str)
                .unwrap_or("unknown");
            let diagnostics = owner_dir.join("cleanup-diagnostics.log");
            if terminate_owned_processes(run_id, scenario, &diagnostics, true).is_ok()
                && remove_registered_scratch_dirs(&owner_dir).is_ok()
            {
                let _ = std::fs::remove_dir_all(&owner_dir);
            }
        }
    }
}

fn owner_record_supervisor_live(fields: &BTreeMap<String, String>) -> Option<bool> {
    let Some(pid) = fields
        .get("supervisor_pid")
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return None;
    };
    let Some(expected_start) = fields
        .get("supervisor_start_ticks")
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return None;
    };
    Some(
        process_info(pid)
            .map(|process| process.start_ticks == expected_start)
            .unwrap_or(false),
    )
}

fn read_owner_fields(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

fn path_age_exceeds(path: &Path, min_age: Duration) -> bool {
    if min_age.is_zero() {
        return true;
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };
    let modified = match metadata.modified() {
        Ok(modified) => modified,
        Err(_) => return false,
    };
    match std::time::SystemTime::now().duration_since(modified) {
        Ok(age) => age >= min_age,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, body).unwrap();
        let mut perm = fs::metadata(&p).unwrap().permissions();
        use std::os::unix::fs::PermissionsExt;
        perm.set_mode(0o755);
        fs::set_permissions(&p, perm).unwrap();
        p
    }

    #[test]
    fn pass_outcome_for_zero_exit_script() {
        let td = TempDir::new().unwrap();
        write_script(td.path(), "ok.sh", "#!/usr/bin/env bash\nexit 0\n");
        let scenario = Scenario {
            name: "ok".to_string(),
            script: "ok.sh".to_string(),
            owners: vec!["task-a".to_string()],
            description: String::new(),
            timeout_seconds: Some(10),
        };
        let r = run_scenario(&scenario, td.path());
        assert_eq!(r.outcome, ScenarioOutcome::Pass);
    }

    #[test]
    fn fail_outcome_for_nonzero_exit_script() {
        let td = TempDir::new().unwrap();
        write_script(
            td.path(),
            "bad.sh",
            "#!/usr/bin/env bash\necho 'broken thing' 1>&2\nexit 3\n",
        );
        let scenario = Scenario {
            name: "bad".to_string(),
            script: "bad.sh".to_string(),
            owners: vec!["task-a".to_string()],
            description: String::new(),
            timeout_seconds: Some(10),
        };
        let r = run_scenario(&scenario, td.path());
        match r.outcome {
            ScenarioOutcome::Fail {
                exit_code,
                stderr_tail,
            } => {
                assert_eq!(exit_code, 3);
                assert!(stderr_tail.contains("broken thing"));
            }
            other => panic!("expected Fail, got {:?}", other),
        }
    }

    #[test]
    fn skip_outcome_for_exit_77() {
        let td = TempDir::new().unwrap();
        write_script(
            td.path(),
            "skip.sh",
            "#!/usr/bin/env bash\necho 'endpoint unreachable' 1>&2\nexit 77\n",
        );
        let scenario = Scenario {
            name: "skipme".to_string(),
            script: "skip.sh".to_string(),
            owners: vec!["task-a".to_string()],
            description: String::new(),
            timeout_seconds: Some(10),
        };
        let r = run_scenario(&scenario, td.path());
        match r.outcome {
            ScenarioOutcome::Skip { reason } => {
                assert!(reason.contains("endpoint unreachable"));
            }
            other => panic!("expected Skip, got {:?}", other),
        }
    }

    #[test]
    fn manifest_loader_filters_owners() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("manifest.toml");
        fs::write(
            &path,
            r#"
[[scenario]]
name = "alpha"
script = "alpha.sh"
owners = ["task-a", "task-b"]

[[scenario]]
name = "beta"
script = "beta.sh"
owners = ["task-c"]
"#,
        )
        .unwrap();
        let m = Manifest::load_from(&path).unwrap();
        assert_eq!(m.scenarios_for_task("task-a").len(), 1);
        assert_eq!(m.scenarios_for_task("task-c").len(), 1);
        assert_eq!(m.scenarios_for_task("task-z").len(), 0);
    }

    #[test]
    fn missing_manifest_yields_empty_manifest() {
        let td = TempDir::new().unwrap();
        let m = Manifest::load_from(&td.path().join("nope.toml")).unwrap();
        assert!(m.scenarios.is_empty());
    }

    #[test]
    fn duplicate_scenario_name_is_rejected() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("manifest.toml");
        fs::write(
            &path,
            r#"
[[scenario]]
name = "dup"
script = "a.sh"

[[scenario]]
name = "dup"
script = "b.sh"
"#,
        )
        .unwrap();
        let err = Manifest::load_from(&path).unwrap_err().to_string();
        assert!(err.contains("duplicate scenario name 'dup'"));
    }

    #[test]
    fn gate_report_blocks_only_on_fail_or_error() {
        let r1 = ScenarioResult {
            name: "a".into(),
            outcome: ScenarioOutcome::Pass,
        };
        let r2 = ScenarioResult {
            name: "b".into(),
            outcome: ScenarioOutcome::Skip { reason: "x".into() },
        };
        let r3 = ScenarioResult {
            name: "c".into(),
            outcome: ScenarioOutcome::Fail {
                exit_code: 1,
                stderr_tail: "boom".into(),
            },
        };
        let report = GateReport {
            results: vec![r1.clone(), r2.clone()],
        };
        assert!(!report.blocks_done());
        let report2 = GateReport {
            results: vec![r1, r2, r3],
        };
        assert!(report2.blocks_done());
    }

    /// Combined into one test because env vars are process-global and
    /// cargo test runs tests in parallel — if we split the env-var case
    /// from the default case, the latter occasionally observes the former
    /// mid-flight and fails.
    #[test]
    fn smoke_root_resolution() {
        let prior = std::env::var("WG_SMOKE_ROOT").ok();

        unsafe { std::env::set_var("WG_SMOKE_ROOT", "/tmp/wgsmoke-test-override") };
        assert_eq!(smoke_root(), PathBuf::from("/tmp/wgsmoke-test-override"));

        unsafe { std::env::remove_var("WG_SMOKE_ROOT") };
        let r = smoke_root();
        assert!(
            r.ends_with(DEFAULT_SMOKE_ROOT_NAME),
            "expected default smoke root to end with '{}', got {}",
            DEFAULT_SMOKE_ROOT_NAME,
            r.display()
        );

        if let Some(p) = prior {
            unsafe { std::env::set_var("WG_SMOKE_ROOT", p) };
        }
    }

    #[test]
    fn ownership_authority_is_durable_before_any_scenario_spawn() {
        let _process_guard = scenario_process_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ownership = ScenarioOwnership::create("pre-spawn-owner").unwrap();
        let fields = read_owner_fields(&ownership.owner_file);
        assert_eq!(fields.get("version").map(String::as_str), Some("3"));
        assert_eq!(
            fields.get("run_id").map(String::as_str),
            Some(ownership.run_id.as_str())
        );
        assert_eq!(
            fields.get("scenario").map(String::as_str),
            Some("pre-spawn-owner")
        );
        assert_eq!(
            fields
                .get("supervisor_pid")
                .and_then(|pid| pid.parse().ok()),
            Some(std::process::id())
        );
        assert!(
            !fields.contains_key("root_pid"),
            "a pre-spawn record must not fabricate a child identity"
        );
        drop(ownership);
    }

    #[test]
    fn sweep_never_removes_unowned_subdirs() {
        // Age and location are not ownership proof. Even an old directory
        // under the smoke root must survive without an exact owner record.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("wgsmoke");
        std::fs::create_dir_all(&root).unwrap();
        let young = root.join("young.scenario.AAAAAA");
        let old = root.join("old.scenario.BBBBBB");
        std::fs::create_dir_all(&young).unwrap();
        std::fs::create_dir_all(&old).unwrap();

        // Backdate the "old" dir's mtime via `touch -d` so it counts as aged.
        // Avoids pulling in a new crate just for the test.
        let status = Command::new("touch")
            .arg("-d")
            .arg("2020-01-01")
            .arg(&old)
            .status()
            .expect("touch must be available for this test");
        assert!(status.success(), "touch -d failed");

        sweep_smoke_leaks_under(&root, Duration::from_secs(3600));

        assert!(young.is_dir(), "young unowned dir should be kept");
        assert!(old.is_dir(), "old unowned dir should be kept");
    }

    #[test]
    fn sweep_zero_age_still_requires_owner_record() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("wgsmoke");
        std::fs::create_dir_all(&root).unwrap();
        let a = root.join("a.scenario.XX");
        let b = root.join("b.scenario.YY");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();

        sweep_smoke_leaks_under(&root, Duration::ZERO);

        assert!(a.exists());
        assert!(b.exists());
        assert!(root.is_dir(), "root itself should remain");
    }

    #[test]
    fn registered_scratch_cleanup_is_bounded_to_own_root() {
        let td = TempDir::new().unwrap();
        let root = td.path().join("wgsmoke");
        let owner_dir = root.join(OWNERS_DIR_NAME).join("run");
        let registry = owner_dir.join("registry.test");
        let scratch = root.join("scenario.ABC123");
        let outside = td.path().join("outside");
        fs::create_dir_all(&registry).unwrap();
        fs::create_dir_all(&scratch).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(
            registry.join("scratches"),
            format!(
                "{}\n{}\n{}\n{}\n",
                scratch.display(),
                root.join("../outside").display(),
                root.display(),
                owner_dir.display()
            ),
        )
        .unwrap();

        remove_registered_scratch_dirs(&owner_dir).unwrap();

        assert!(!scratch.exists(), "owned scratch should be removed");
        assert!(
            outside.exists(),
            "path outside exact smoke root was removed"
        );
        assert!(root.exists(), "smoke root itself was removed");
        assert!(owner_dir.exists(), "owner evidence was removed too early");
    }

    #[test]
    fn missing_script_is_error_outcome() {
        let td = TempDir::new().unwrap();
        let scenario = Scenario {
            name: "ghost".to_string(),
            script: "does-not-exist.sh".to_string(),
            owners: vec!["task-a".to_string()],
            description: String::new(),
            timeout_seconds: Some(10),
        };
        let r = run_scenario(&scenario, td.path());
        assert!(matches!(r.outcome, ScenarioOutcome::Error { .. }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn scenario_backstop_reaps_term_ignoring_setsid_descendant_but_not_unrelated_pi() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let td = TempDir::new().unwrap();
        let script = td.path().join("escape.sh");
        let pid_file = td.path().join("owned.pid");
        let body = format!(
            r#"#!/usr/bin/env bash
set -u
setsid bash -c 'trap "" TERM; cd "$1"; exec 9>>held.log; echo $$ >owned.pid; while :; do sleep 1; done' _ '{}' &
sleep 0.2
exit 9
"#,
            td.path().display()
        );
        fs::write(&script, body).unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).unwrap();

        // A command whose executable is literally named `pi`, but which has
        // no run marker, must remain byte-for-byte the same process identity.
        let decoy_pi = td.path().join("pi");
        symlink("/bin/sleep", &decoy_pi).unwrap();
        let mut decoy = Command::new(&decoy_pi)
            .arg("30")
            .env_remove(SMOKE_RUN_ID_ENV)
            .spawn()
            .unwrap();
        let decoy_before = process_info(decoy.id()).unwrap();

        let scenario = Scenario {
            name: "owned-escape".to_string(),
            script: script.to_string_lossy().to_string(),
            owners: vec!["test".to_string()],
            description: String::new(),
            timeout_seconds: Some(10),
        };
        let result = run_scenario(&scenario, td.path());
        assert!(matches!(
            result.outcome,
            ScenarioOutcome::Fail { exit_code: 9, .. }
        ));

        let owned_pid: u32 = fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(
            process_info(owned_pid).is_none(),
            "owned escaped descendant {owned_pid} survived exact-marker cleanup"
        );
        let decoy_after = process_info(decoy.id()).expect("unrelated pi was killed");
        assert_eq!(decoy_before.start_ticks, decoy_after.start_ticks);
        assert_eq!(decoy_before.process_group, decoy_after.process_group);
        let _ = decoy.kill();
        let _ = decoy.wait();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stale_owner_record_sweep_uses_exact_run_identity() {
        let td = TempDir::new().unwrap();
        let root = td.path().join("wgsmoke");
        let owner_dir = root.join(OWNERS_DIR_NAME).join("stale");
        fs::create_dir_all(&owner_dir).unwrap();
        let run_id = format!("wg-smoke-v2:{}", uuid::Uuid::now_v7());
        fs::write(
            owner_dir.join("owner.env"),
            format!(
                "version=2\nrun_id={run_id}\nscenario=stale-owner-test\nsupervisor_pid=2147483000\nsupervisor_start_ticks=1\n"
            ),
        )
        .unwrap();
        let mut child = Command::new("bash")
            .args(["-c", "trap '' TERM; while :; do sleep 1; done"])
            .env(SMOKE_RUN_ID_ENV, &run_id)
            .spawn()
            .unwrap();
        let pid = child.id();

        // The record is brand-new, but its exact supervisor identity is
        // dead, so the sweep must not wait for the ordinary age cutoff.
        sweep_smoke_leaks_under(&root, Duration::from_secs(3600));

        assert!(
            process_info(pid).is_none(),
            "stale owned process survived sweep"
        );
        assert!(
            !owner_dir.exists(),
            "successful sweep retained owner record"
        );
        let _ = child.wait();
    }
}
