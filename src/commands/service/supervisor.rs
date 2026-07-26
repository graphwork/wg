//! Auto-restart supervisor for the service daemon (`fix-coordinator-daemon`).
//!
//! See `docs/studies/daemon-crash-supervisor-design.md` for the full rationale.
//! Summary: the daemon can still die from a truly fatal cause (`SIGKILL`,
//! segfault, an unhandled abort) even with signal handlers + the strengthened
//! panic hook. This module is the external process that brings it back.
//!
//! `wg service start` (default-on; `--no-supervise` opts out) forks
//! [`run_supervisor`] instead of the daemon directly. The supervisor forks the
//! daemon (`wg service daemon …`) as its child, records the daemon PID in
//! `state.json` (unchanged contract + a new optional `supervisor_pid`), and
//! monitors it. On an unexpected exit (no `.clean_shutdown` sentinel) it
//! re-spawns the daemon with exponential backoff and a restart budget. A clean
//! `wg service stop` (IPC `Shutdown` → daemon writes the sentinel → exits) makes
//! the supervisor exit too, so a real stop is not re-spawned.
//!
//! The supervisor itself installs `SIGTERM`/`SIGINT` handlers that set a
//! stopping flag (no restart) so a system shutdown or `wg service stop --force`
//! (which kills the supervisor tree via `supervisor_pid`) terminates cleanly.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::{
    DaemonLogger, ServiceState, clean_shutdown_sentinel_path, consume_clean_shutdown_sentinel,
    log_file_path,
};

#[cfg(unix)]
static SUPERVISOR_STOPPING: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn supervisor_signal_handler(_sig: libc::c_int) {
    SUPERVISOR_STOPPING.store(true, Ordering::SeqCst);
}

/// Maximum restart attempts within the rolling window before the supervisor
/// gives up (so a persistently-crashing daemon does not loop forever).
const RESTART_BUDGET_PER_WINDOW: u32 = 8;
/// Window length for the restart budget.
const RESTART_WINDOW: Duration = Duration::from_secs(5 * 60);
/// A run that lasts longer than this is considered "healthy" and resets the
/// rapid-fail + window counters.
const HEALTHY_RUN: Duration = Duration::from_secs(60);
/// Tight budget for back-to-back instant-exit failures (e.g. a persistent
/// config error): give up fast instead of slow-backoff-looping.
const RAPID_FAIL_BUDGET: u32 = 3;
const RAPID_FAIL_RUN: Duration = Duration::from_secs(5);
/// Backoff floor / ceiling.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// `try_wait` poll cadence.
const POLL_SLICE: Duration = Duration::from_millis(100);

/// Arguments needed to (re)spawn the daemon. Mirrors the `wg service start`
/// launch args; the supervisor passes them through to `wg service daemon`.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn run_supervisor(
    dir: &Path,
    socket_path: &str,
    max_agents: Option<usize>,
    executor: Option<&str>,
    interval: Option<u64>,
    model: Option<&str>,
    no_chat_agent: bool,
    no_pin: bool,
) -> Result<()> {
    let dir = dir.to_path_buf();
    let logger = DaemonLogger::open(&dir).context("supervisor: failed to open daemon log")?;
    install_supervisor_signal_handlers();

    let exe = std::env::current_exe().context("supervisor: current_exe")?;
    let log_path = log_file_path(&dir);
    let service_dir = dir.join("service");
    if !service_dir.exists() {
        fs::create_dir_all(&service_dir)?;
    }
    // Open the log in append mode so the daemon child's stderr lands here too.
    let stderr_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("supervisor: open log {:?}", log_path))?;

    // Compute the authenticated service identity once (config/exe don't change
    // across restarts). If it fails we still supervise — identity is for
    // lifecycle clients, not for restart correctness.
    let identity = {
        let config = worksgood::config::Config::load_merged(&dir).ok();
        config
            .as_ref()
            .and_then(|c| worksgood::service_identity::expected_identity(&dir, &exe, c).ok())
    };

    logger.info(&format!(
        "Supervisor starting (PID {}, will manage `wg service daemon`); restart budget {} per {}s, healthy-run reset {}s",
        std::process::id(),
        RESTART_BUDGET_PER_WINDOW,
        RESTART_WINDOW.as_secs(),
        HEALTHY_RUN.as_secs()
    ));

    let dir_str = dir.to_string_lossy().to_string();
    let mut backoff = BACKOFF_MIN;
    let mut window_start = Instant::now();
    let mut restarts_in_window: u32 = 0;
    let mut rapid_fails: u32 = 0;

    let daemon_args = build_daemon_args(
        &dir_str,
        socket_path,
        max_agents,
        executor,
        interval,
        model,
        no_chat_agent,
        no_pin,
    );

    loop {
        // If we were asked to stop (signal / system shutdown), don't (re)spawn.
        if stopping() {
            logger.info("Supervisor stopping (signal received); not (re)starting daemon");
            break;
        }

        // Roll the restart window.
        if window_start.elapsed() >= RESTART_WINDOW {
            window_start = Instant::now();
            restarts_in_window = 0;
        }

        let spawn_start = Instant::now();
        let mut child = match spawn_daemon(&exe, &daemon_args, &dir, stderr_file.try_clone()?) {
            Ok(c) => c,
            Err(e) => {
                logger.error(&format!(
                    "Supervisor: failed to spawn daemon ({}); retrying in {:?}",
                    e, backoff
                ));
                sleep_or_stop(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX);
                restarts_in_window += 1;
                if restarts_in_window > RESTART_BUDGET_PER_WINDOW {
                    logger
                        .error("Supervisor: restart budget exhausted (spawn failures); giving up");
                    let _ = ServiceState::remove(&dir);
                    break;
                }
                continue;
            }
        };
        let daemon_pid = child.id();

        // Record state.json with the daemon pid (the unchanged contract) plus
        // our own pid so `wg service stop --force` can kill the whole tree.
        let state = ServiceState {
            pid: daemon_pid,
            socket_path: socket_path.to_string(),
            started_at: chrono::Utc::now().to_rfc3339(),
            pid_start_identity: worksgood::service_identity::pid_start_identity(daemon_pid),
            identity: identity.clone(),
            supervisor_pid: Some(std::process::id()),
        };
        if let Err(e) = state.save(&dir) {
            logger.warn(&format!("Supervisor: failed to write state.json: {}", e));
        }

        logger.info(&format!(
            "Supervisor: daemon spawned (PID {}); monitoring",
            daemon_pid
        ));

        // Poll for child exit, waking on the stopping flag.
        let exit_status = poll_wait(&mut child);
        let ran = spawn_start.elapsed();

        if stopping() {
            // We were asked to stop. Forward SIGTERM to the daemon so it exits
            // promptly, then reap it. Do NOT restart.
            logger.info("Supervisor: stop requested; signalling daemon to exit");
            signal_child(daemon_pid, true);
            let _ = child.wait();
            let _ = ServiceState::remove(&dir);
            break;
        }

        // Clean sentinel present → the daemon exited via IPC `Shutdown`
        // (`wg service stop`). Consume it and exit without restarting.
        if consume_clean_shutdown_sentinel(&dir) {
            logger.info(
                "Supervisor: daemon exited cleanly (clean-shutdown sentinel); supervisor exiting",
            );
            let _ = ServiceState::remove(&dir);
            break;
        }

        // Successful long run resets the rapid/window fail counters.
        if ran >= HEALTHY_RUN {
            if rapid_fails != 0 || restarts_in_window != 0 {
                logger.info(&format!(
                    "Supervisor: daemon ran {:?} before exit (healthy); resetting fail counters",
                    ran
                ));
            }
            restarts_in_window = 0;
            rapid_fails = 0;
            backoff = BACKOFF_MIN;
        }

        let status_str = match exit_status {
            Some(code) => format!("exit code {}", code),
            None => "signal/lost".to_string(),
        };
        logger.error(&format!(
            "Supervisor: daemon (PID {}) exited unexpectedly after {:?} ({}). No clean sentinel — treating as crash.",
            daemon_pid, ran, status_str
        ));

        // Rapid-fail guard: persistent startup error (bad config, missing
        // route). Give up fast instead of slow-backoff-looping.
        if ran < RAPID_FAIL_RUN {
            rapid_fails += 1;
            if rapid_fails > RAPID_FAIL_BUDGET {
                logger.error(&format!(
                    "Supervisor: daemon failed to start {} times within {:?} (likely a persistent startup error); giving up. Check the daemon log: {}",
                    rapid_fails, RAPID_FAIL_RUN, log_path.display()
                ));
                let _ = ServiceState::remove(&dir);
                break;
            }
        }

        restarts_in_window += 1;
        if restarts_in_window > RESTART_BUDGET_PER_WINDOW {
            logger.error(&format!(
                "Supervisor: restart budget ({} per {}s) exhausted; giving up. Last daemon exit: {}. Log: {}",
                RESTART_BUDGET_PER_WINDOW,
                RESTART_WINDOW.as_secs(),
                status_str,
                log_path.display()
            ));
            let _ = ServiceState::remove(&dir);
            break;
        }

        logger.warn(&format!(
            "Supervisor: restarting daemon in {:?} (attempt {}, backoff)",
            backoff, restarts_in_window
        ));
        sleep_or_stop(backoff);
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }

    // Best-effort: ensure no stale sentinel lingers across a fresh `start`.
    let _ = fs::remove_file(clean_shutdown_sentinel_path(&dir));
    logger.info("Supervisor exiting");
    Ok(())
}

/// Build the argv for `wg service daemon …`, mirroring `run_start`'s legacy
/// daemon-spawn args.
#[allow(clippy::too_many_arguments)]
fn build_daemon_args(
    dir_str: &str,
    socket_path: &str,
    max_agents: Option<usize>,
    executor: Option<&str>,
    interval: Option<u64>,
    model: Option<&str>,
    no_chat_agent: bool,
    no_pin: bool,
) -> Vec<String> {
    let mut args = vec![
        "--dir".to_string(),
        dir_str.to_string(),
        "service".to_string(),
        "daemon".to_string(),
        "--socket".to_string(),
        socket_path.to_string(),
    ];
    if let Some(n) = max_agents {
        args.push("--max-agents".to_string());
        args.push(n.to_string());
    }
    if let Some(e) = executor {
        args.push("--executor".to_string());
        args.push(e.to_string());
    }
    if let Some(i) = interval {
        args.push("--interval".to_string());
        args.push(i.to_string());
    }
    if let Some(m) = model {
        args.push("--model".to_string());
        args.push(m.to_string());
    }
    if no_chat_agent {
        args.push("--no-chat-agent".to_string());
    }
    if no_pin {
        args.push("--no-pin".to_string());
    }
    args
}

/// Spawn `wg service daemon` with null stdio + stderr to the log. Inherit the
/// supervisor's session (do NOT setsid) so `kill_process_*(supervisor_pid)`
/// reaches the daemon through the descendant tree.
fn spawn_daemon(
    exe: &Path,
    args: &[String],
    dir: &Path,
    stderr: fs::File,
) -> Result<std::process::Child> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env("WG_DIR", dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(stderr));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Put the daemon in its own process group so a signal sent to the
        // supervisor's group (e.g. Ctrl-C at a terminal that somehow still
        // owns us) does not fan out to agent grandchildren, while keeping it
        // a child of the supervisor for waitpid + tree-kill.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    cmd.spawn()
        .with_context(|| format!("supervisor: spawn {:?} {}", exe.display(), args.join(" ")))
}

/// Poll the child with `try_wait` until it exits or the supervisor is asked to
/// stop. Returns `Some(exit_code)` on normal exit, or `None` if the child was
/// killed by a signal / status unavailable.
fn poll_wait(child: &mut std::process::Child) -> Option<i32> {
    loop {
        if stopping() {
            return None;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return status.code();
            }
            Ok(None) => {
                std::thread::sleep(POLL_SLICE);
            }
            Err(_) => return None,
        }
    }
}

fn stopping() -> bool {
    #[cfg(unix)]
    {
        SUPERVISOR_STOPPING.load(Ordering::SeqCst)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Sleep for `d`, but bail out early if the supervisor is asked to stop.
fn sleep_or_stop(d: Duration) {
    let end = Instant::now() + d;
    while Instant::now() < end {
        if stopping() {
            return;
        }
        std::thread::sleep(POLL_SLICE.min(end.saturating_duration_since(Instant::now())));
    }
}

/// Send SIGTERM (graceful) or SIGKILL to a child pid.
fn signal_child(pid: u32, graceful: bool) {
    #[cfg(unix)]
    {
        let sig = if graceful {
            libc::SIGTERM
        } else {
            libc::SIGKILL
        };
        unsafe {
            libc::kill(pid as libc::pid_t, sig);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, graceful);
    }
}

fn install_supervisor_signal_handlers() {
    #[cfg(unix)]
    {
        let install = |sig: libc::c_int| unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            let handler: extern "C" fn(libc::c_int) = supervisor_signal_handler;
            sa.sa_sigaction = handler as usize;
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        };
        install(libc::SIGTERM);
        install(libc::SIGINT);
        install(libc::SIGHUP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_daemon_args_passes_through_overrides() {
        let args = build_daemon_args(
            "/tmp/wg",
            "/tmp/wg/service/daemon.sock",
            Some(4),
            Some("pi"),
            Some(7),
            Some("pi:openrouter:x"),
            true,
            true,
        );
        let joined = args.join(" ");
        assert!(joined.contains("--dir /tmp/wg"));
        assert!(joined.contains("service daemon"));
        assert!(joined.contains("--socket /tmp/wg/service/daemon.sock"));
        assert!(joined.contains("--max-agents 4"));
        assert!(joined.contains("--executor pi"));
        assert!(joined.contains("--interval 7"));
        assert!(joined.contains("--model pi:openrouter:x"));
        assert!(joined.contains("--no-chat-agent"));
        assert!(joined.contains("--no-pin"));
    }

    #[test]
    fn build_daemon_args_minimal() {
        let args = build_daemon_args(
            "/tmp/wg",
            "/tmp/wg/service/daemon.sock",
            None,
            None,
            None,
            None,
            false,
            false,
        );
        // No optional flags when None.
        assert!(!args.iter().any(|a| a == "--max-agents"));
        assert!(!args.iter().any(|a| a == "--no-pin"));
        assert!(
            args.windows(2)
                .any(|w| w[0] == "service" && w[1] == "daemon")
        );
    }
}
