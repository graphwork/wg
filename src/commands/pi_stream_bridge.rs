//! Internal: translate a finished pi agent's NDJSON event stream into the
//! canonical `stream.jsonl` + a `session-summary.md`.
//!
//! Invoked by the spawn wrapper after `pi --mode json` exits. Reads the agent's
//! `raw_stream.jsonl` (pi's native NDJSON, falling back to `output.log`), sums
//! per-turn usage, and writes a canonical stream that carries REAL token/cost
//! figures — not the `usage:{input_tokens:0,output_tokens:0}` bookend the
//! generic wrapper used to emit — so the TUI, `wg show`, `wg spend`, and
//! `wg stats` reflect the pi task. Also writes the agent's `session-summary.md`
//! from the final assistant turn so `wg show <pi task>` isn't bare.
//!
//! Token-cost accounting (`task.token_usage`) is handled independently by
//! `graph::parse_token_usage`, which learned the same pi `turn_end` summation;
//! this command exists to populate the canonical event channel.

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

#[cfg(test)]
use worksgood::stream_event::StreamEvent;
use worksgood::stream_event::{self, StreamWriter};

use crate::commands::pi_watchdog::{
    SilenceAction, SilencePolicy, classify_silence, enforce_silence_hardcap,
};

/// Maximum characters of final assistant text to persist as the session
/// summary — a guard against a pathologically long final message.
const MAX_SUMMARY_CHARS: usize = 4000;

/// How often the native follower evaluates the watchdog clocks (soft warning /
/// hard-cap enforcement). The byte-follow loop itself runs far faster.
const ENFORCEMENT_INTERVAL: Duration = Duration::from_secs(1);

/// Bounded no-progress exit (FIX C of the tool-hang hotfix): the forever-spinning
/// 6%-CPU observer must not outlive its purpose. If NO bytes have been appended
/// to the native capture for this long, the follower exits non-zero with a clear
/// line instead of spinning forever.
///
/// Default 900s deliberately sits ABOVE the 600s watchdog hard cap, so the
/// hard-cap enforcement (kill + fail) is always the first to act; this is the
/// backstop for the case where enforcement could not run (no watchdog state,
/// unreadable graph). `0` disables it.
pub const DEFAULT_OBSERVE_NO_PROGRESS_SECS: u64 = 900;

/// Env override for {@link DEFAULT_OBSERVE_NO_PROGRESS_SECS}.
pub const OBSERVE_NO_PROGRESS_ENV: &str = "WG_PI_OBSERVE_NO_PROGRESS_SECS";

/// Follower timing knobs (env-resolved in production, explicit in tests).
#[derive(Debug, Clone, Copy)]
pub struct ObserveOptions {
    pub poll_interval: Duration,
    pub no_progress_limit_secs: u64,
}

impl ObserveOptions {
    pub fn from_env() -> Self {
        Self {
            poll_interval: Duration::from_millis(50),
            no_progress_limit_secs: no_progress_limit_from_env(),
        }
    }
}

/// Resolve the no-progress limit: env override, else the 900s default. A
/// non-numeric/absurd value degrades to the default rather than disabling the
/// bound by accident.
pub fn no_progress_limit_from_env() -> u64 {
    std::env::var(OBSERVE_NO_PROGRESS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value <= 86_400)
        .unwrap_or(DEFAULT_OBSERVE_NO_PROGRESS_SECS)
}

/// The watchdog verdict produced inside the follower loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SilenceEnforcement {
    /// No watchdog state to judge, or proof is fresh.
    None,
    /// Soft threshold crossed: LOG ONLY (never touch the process).
    SoftWarn { silence_secs: u64, soft_secs: u64 },
    /// Hard cap breached and enforced (process killed, attempt failed).
    HardCap { silence_secs: u64 },
}

/// Evaluate the watchdog silence clocks for this agent and, on a hard-cap
/// breach, EXECUTE the enforcement (kill the exact wedged process + fail the
/// attempt with `watchdog-tool-silence-hardcap`). The soft rung only warns.
fn enforce_watchdog_silence(agent_dir: &Path) -> Result<SilenceEnforcement> {
    let Some(watchdog) = open_watchdog_for_agent(agent_dir) else {
        return Ok(SilenceEnforcement::None);
    };
    let state = watchdog.state();
    let now = chrono::Utc::now().timestamp();
    let silence_secs = now.saturating_sub(state.last_meaningful_at).max(0) as u64;
    let policy = SilencePolicy::from_soft(watchdog.policy().meaningful_silence_secs);
    let task_id = state.source.task_id.clone();
    if state.terminal {
        return Ok(SilenceEnforcement::None);
    }
    match classify_silence(silence_secs, policy) {
        SilenceAction::None => Ok(SilenceEnforcement::None),
        SilenceAction::SoftWarn => Ok(SilenceEnforcement::SoftWarn {
            silence_secs,
            soft_secs: policy.soft_secs,
        }),
        SilenceAction::HardCap => {
            let graph_dir = agent_dir
                .parent()
                .and_then(Path::parent)
                .context("Pi agent dir has no graph root")?;
            let acted = match enforce_silence_hardcap(graph_dir, &task_id, silence_secs) {
                Ok(acted) => acted,
                Err(error) => {
                    // Never let an enforcement error take the follower down; the
                    // no-progress backstop still bounds this process.
                    eprintln!(
                        "[pi-stream-observe] WARNING: watchdog hard-cap enforcement failed for {}: {}",
                        task_id, error
                    );
                    false
                }
            };
            if acted {
                return Ok(SilenceEnforcement::HardCap { silence_secs });
            }
            // Not actionable (already terminal / stale fence): keep following
            // so the bounded no-progress exit remains the last resort.
            Ok(SilenceEnforcement::None)
        }
    }
}

/// Follow the append-only capture while the exact Pi child is alive. Only
/// bounded projections enter watchdog state; raw bytes remain exclusively in
/// raw_stream.jsonl/output.log.
///
/// The loop is BOUNDED twice over: it exits when the followed pid is gone and
/// quiet, it evaluates the watchdog hard cap every second (soft = log only,
/// hard = kill + fail), and it exits non-zero when no bytes have arrived for
/// {@link ObserveOptions::no_progress_limit_secs}.
pub fn observe_live(agent_dir: &Path, follow_pid: u32) -> Result<()> {
    observe_live_with(agent_dir, follow_pid, ObserveOptions::from_env())
}

pub fn observe_live_with(agent_dir: &Path, follow_pid: u32, options: ObserveOptions) -> Result<()> {
    let raw_path = agent_dir.join(stream_event::RAW_STREAM_FILE_NAME);
    let stream_id = raw_path.to_string_lossy().into_owned();
    let mut cursor = open_watchdog_for_agent(agent_dir)
        .map(|watchdog| watchdog.native_stream_offset(&stream_id))
        .unwrap_or(0);
    let mut dead_stable_rounds = 0u8;
    let mut reconcile_needed = true;
    let mut last_progress = Instant::now();
    let mut last_enforcement = Instant::now();
    let mut soft_warned = false;
    loop {
        let before = cursor;
        if raw_path.is_file() {
            let mut file = std::fs::File::open(&raw_path)?;
            let length = file.metadata()?.len();
            if length < cursor {
                anyhow::bail!(
                    "Pi native capture shrank from {} to {}; append-only proof failed",
                    cursor,
                    length
                );
            }
            file.seek(SeekFrom::Start(cursor))?;
            let mut reader = BufReader::new(file);
            loop {
                let line_start = cursor;
                let mut line = String::new();
                let read = reader.read_line(&mut line)?;
                if read == 0 {
                    break;
                }
                if !line.ends_with('\n') {
                    cursor = line_start;
                    break;
                }
                cursor = cursor.saturating_add(read as u64);
                if let Some(mut watchdog) = open_watchdog_for_agent(agent_dir) {
                    if watchdog.state().process.nonce != "fixture-nonce"
                        && watchdog.state().process.pid != follow_pid
                    {
                        anyhow::bail!(
                            "stale_process_identity: native follower PID {} is not current epoch {} PID {}",
                            follow_pid,
                            watchdog.state().process_epoch,
                            watchdog.state().process.pid
                        );
                    }
                    watchdog
                        .ingest_native_line(
                            line.trim_end_matches(['\r', '\n']),
                            &stream_id,
                            cursor,
                            chrono::Utc::now().timestamp(),
                        )
                        .map_err(anyhow::Error::new)?;
                    if let Some(graph_dir) = agent_dir.parent().and_then(Path::parent) {
                        crate::commands::pi_watchdog::sync_lifecycle_continuation_authority(
                            graph_dir,
                            &watchdog.state().source.task_id,
                            &watchdog,
                        )?;
                    }
                    if reconcile_needed {
                        match watchdog.reconcile_session_journal(chrono::Utc::now().timestamp()) {
                            Ok(changed) => reconcile_needed = !changed,
                            Err(error) if error.code == "session_journal_missing" => {}
                            Err(error) => {
                                let _ = watchdog.observe(
                                    worksgood::pi_watchdog::Observation::GuardFailure(
                                        worksgood::pi_watchdog::GuardFailure::Session,
                                    ),
                                    chrono::Utc::now().timestamp(),
                                );
                                return Err(anyhow::Error::new(error));
                            }
                        }
                    }
                }
            }
        }
        if cursor > before {
            last_progress = Instant::now();
            soft_warned = false;
        }
        if last_enforcement.elapsed() >= ENFORCEMENT_INTERVAL {
            last_enforcement = Instant::now();
            match enforce_watchdog_silence(agent_dir) {
                Ok(SilenceEnforcement::HardCap { silence_secs }) => {
                    eprintln!(
                        "[pi-stream-observe] watchdog hard-cap breach: {}s without proven progress; killed the wedged Pi process and failed the attempt with 'watchdog-tool-silence-hardcap'",
                        silence_secs
                    );
                    return Ok(());
                }
                Ok(SilenceEnforcement::SoftWarn {
                    silence_secs,
                    soft_secs,
                }) => {
                    if !soft_warned {
                        soft_warned = true;
                        eprintln!(
                            "[pi-stream-observe] soft watchdog warning: {}s without proven progress (soft={}s); observation only, process left intact",
                            silence_secs, soft_secs
                        );
                    }
                }
                Ok(SilenceEnforcement::None) => {}
                Err(error) => {
                    if !soft_warned {
                        soft_warned = true;
                        eprintln!(
                            "[pi-stream-observe] WARNING: watchdog evaluation failed: {error}"
                        );
                    }
                }
            }
        }
        if options.no_progress_limit_secs > 0
            && last_progress.elapsed() >= Duration::from_secs(options.no_progress_limit_secs)
        {
            anyhow::bail!(
                "pi-stream-observe: no stream progress for {}s (limit {}s, env {}); exiting rather than spinning forever",
                last_progress.elapsed().as_secs(),
                options.no_progress_limit_secs,
                OBSERVE_NO_PROGRESS_ENV
            );
        }
        let alive = process_alive(follow_pid);
        if !alive && cursor == before {
            dead_stable_rounds = dead_stable_rounds.saturating_add(1);
            if dead_stable_rounds >= 4 {
                break;
            }
        } else {
            dead_stable_rounds = 0;
        }
        std::thread::sleep(options.poll_interval);
    }
    if let Some(mut watchdog) = open_watchdog_for_agent(agent_dir)
        && (watchdog.state().process.nonce == "fixture-nonce"
            || watchdog.state().process.pid == follow_pid)
        && let Err(error) = watchdog.reconcile_session_journal(chrono::Utc::now().timestamp())
    {
        let _ = watchdog.observe(
            worksgood::pi_watchdog::Observation::GuardFailure(
                worksgood::pi_watchdog::GuardFailure::Session,
            ),
            chrono::Utc::now().timestamp(),
        );
        return Err(anyhow::Error::new(error));
    }
    Ok(())
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // Signal 0 performs identity-free liveness only; the watchdog separately
    // owns the exact PID/start-ticks proof used for any disposition.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}

pub fn run(agent_dir: &Path, exit_code: i32, follow_pid: Option<u32>) -> Result<()> {
    let success = exit_code == 0;

    // Prefer pi's native NDJSON capture; fall back to the combined log (which
    // also contains the NDJSON when `raw_stream.jsonl` is absent).
    let raw_path = agent_dir.join(stream_event::RAW_STREAM_FILE_NAME);
    let log_path = agent_dir.join("output.log");
    let content = std::fs::read_to_string(&raw_path)
        .or_else(|_| std::fs::read_to_string(&log_path))
        .unwrap_or_default();

    let model_override = read_metadata_model(agent_dir);

    // Project native progress/provider/settled evidence into the durable,
    // process-epoch watchdog journal as well as the canonical UI stream. This
    // is evidence only; the lifecycle kernel remains the sole task-state
    // writer. Replay is idempotent at the watchdog action/receipt layer.
    if let Some(mut watchdog) = open_watchdog_for_agent(agent_dir) {
        if let Some(pid) = follow_pid
            && watchdog.state().process.nonce != "fixture-nonce"
            && watchdog.state().process.pid != pid
        {
            anyhow::bail!(
                "stale_process_identity: stream bridge PID {pid} is not current epoch {} PID {}",
                watchdog.state().process_epoch,
                watchdog.state().process.pid
            );
        }
        let stream_id = raw_path.to_string_lossy();
        let mut offset = 0u64;
        for line in content.split_inclusive('\n') {
            offset = offset.saturating_add(line.len() as u64);
            let line = line.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                continue;
            }
            let at = chrono::Utc::now().timestamp();
            let _ = watchdog.ingest_native_line(line, &stream_id, offset, at);
        }
        if let Some(graph_dir) = agent_dir.parent().and_then(Path::parent) {
            crate::commands::pi_watchdog::sync_lifecycle_continuation_authority(
                graph_dir,
                &watchdog.state().source.task_id,
                &watchdog,
            )?;
        }
        watchdog
            .reconcile_session_journal(chrono::Utc::now().timestamp())
            .map_err(anyhow::Error::new)?;
    }

    // Pi is the accounting authority. Missing cost remains zero/unknown; WG
    // never substitutes registry pricing for Pi events.
    let tr = stream_event::translate_pi_stream(&content, model_override.as_deref(), success);

    // Write the canonical stream fresh, overwriting the 0/0 bookend the wrapper
    // may have written on a path where this command did not yet run.
    let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);
    std::fs::write(&stream_path, "")
        .with_context(|| format!("truncate {}", stream_path.display()))?;
    let writer = StreamWriter::new(&stream_path);
    for event in &tr.events {
        writer.write_event(event);
    }

    // Session summary so `wg show` isn't bare.
    if let Some(text) = tr.final_text.as_deref() {
        let summary = if text.chars().count() > MAX_SUMMARY_CHARS {
            let cut = text
                .char_indices()
                .nth(MAX_SUMMARY_CHARS)
                .map(|(i, _)| i)
                .unwrap_or(text.len());
            format!("{}\n[...truncated]", &text[..cut])
        } else {
            text.to_string()
        };
        let summary_path = agent_dir.join("session-summary.md");
        if let Err(e) =
            worksgood::executor::native::resume::store_session_summary(&summary_path, &summary)
        {
            eprintln!("[pi-stream-bridge] warning: failed to write session summary: {e}");
        }
    }

    Ok(())
}

fn open_watchdog_for_agent(agent_dir: &Path) -> Option<worksgood::pi_watchdog::PiWatchdog> {
    let content = std::fs::read_to_string(agent_dir.join("metadata.json")).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    let graph_dir = agent_dir.parent()?.parent()?;
    let task_id = val
        .get("task_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .or_else(|| std::env::var("WG_TASK_ID").ok())?;
    let graph = worksgood::parser::load_graph(graph_dir.join("graph.jsonl")).ok()?;
    let task = graph.get_task(&task_id)?;
    let attempt = task.lifecycle.current_attempt.as_ref()?;
    if val.get("attempt_id")?.as_str()? != attempt.id {
        return None;
    }
    let key = worksgood::attempt_runtime::AttemptRuntimeKey::for_attempt(task, attempt);
    let state_path = worksgood::attempt_runtime::component_for_update(graph_dir, &key, "pi")
        .ok()?
        .join("state.json");
    if !state_path.is_file() {
        return None;
    }
    let mut watchdog = worksgood::pi_watchdog::PiWatchdog::open(&state_path).ok()?;
    if watchdog.state().source.task_id != key.task_id
        || watchdog.state().source.generation != key.generation
        || watchdog.state().source.attempt_id != key.attempt_id
        || watchdog.state().source.attempt_fence != key.attempt_fence
        || watchdog.state().source.worktree_lease_epoch != key.worktree_lease_epoch
    {
        return None;
    }
    // The native follower is not an independent epoch authority. Refuse all
    // progress unless lifecycle and watchdog agree on the exact current
    // process fence/identity; this also performs the one-time schema-v1 repair.
    crate::commands::pi_watchdog::sync_lifecycle_process_authority(
        graph_dir,
        &watchdog.state().source.task_id.clone(),
        &mut watchdog,
    )
    .ok()?;
    watchdog
        .reconcile_pending_same_process_prompt(chrono::Utc::now().timestamp())
        .ok()?;
    crate::commands::pi_watchdog::sync_lifecycle_continuation_authority(
        graph_dir,
        &watchdog.state().source.task_id,
        &watchdog,
    )
    .ok()?;
    Some(watchdog)
}

fn read_metadata_model(agent_dir: &Path) -> Option<String> {
    let content = std::fs::read_to_string(agent_dir.join("metadata.json")).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get("model").and_then(|v| v.as_str()).map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    #[test]
    fn test_bridge_writes_nonzero_stream_and_summary() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path();
        write(
            agent_dir,
            "metadata.json",
            r#"{"executor":"pi","model":"openrouter:z-ai/glm-5.2"}"#,
        );
        let raw = [
            r#"{"type":"session","id":"sess-1","cwd":"/tmp"}"#,
            r#"{"type":"turn_end","message":{"role":"assistant","provider":"openrouter","model":"z-ai/glm-5.2","content":[{"type":"toolCall","name":"bash"}],"usage":{"input":200,"output":10,"cacheRead":50,"cacheWrite":0,"totalTokens":260,"cost":{"total":0.02}}}}"#,
            r#"{"type":"turn_end","message":{"role":"assistant","provider":"openrouter","model":"z-ai/glm-5.2","content":[{"type":"text","text":"all done, task complete"}],"usage":{"input":5,"output":7,"cacheRead":260,"cacheWrite":0,"totalTokens":272,"cost":{"total":0.03}}}}"#,
        ]
        .join("\n");
        write(agent_dir, "raw_stream.jsonl", &raw);

        run(agent_dir, 0, None).unwrap();

        // stream.jsonl now carries a NONZERO summed result.usage.
        let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);
        let (events, _) = stream_event::read_stream_events(&stream_path, 0).unwrap();
        let result = events
            .iter()
            .find_map(|e| match e {
                StreamEvent::Result { usage, .. } => Some(usage.clone()),
                _ => None,
            })
            .expect("a result event");
        assert_eq!(result.input_tokens, 205);
        assert_eq!(result.output_tokens, 17);
        assert!((result.cost_usd.unwrap() - 0.05).abs() < 1e-9);
        // Per-step events are present between init and result.
        assert!(events.iter().any(|e| matches!(e, StreamEvent::Turn { .. })));

        // Session summary written from the final assistant text.
        let summary = std::fs::read_to_string(agent_dir.join("session-summary.md")).unwrap();
        assert!(summary.contains("all done, task complete"));
    }

    #[test]
    fn test_no_progress_limit_env_resolution() {
        // SAFETY: these env mutations are confined to this test.
        unsafe { std::env::remove_var(OBSERVE_NO_PROGRESS_ENV) };
        assert_eq!(
            no_progress_limit_from_env(),
            DEFAULT_OBSERVE_NO_PROGRESS_SECS
        );
        // The bounded exit must never fire BEFORE the watchdog hard cap.
        assert!(DEFAULT_OBSERVE_NO_PROGRESS_SECS > 600);
        unsafe { std::env::set_var(OBSERVE_NO_PROGRESS_ENV, "1") };
        assert_eq!(no_progress_limit_from_env(), 1);
        unsafe { std::env::set_var(OBSERVE_NO_PROGRESS_ENV, "0") };
        assert_eq!(no_progress_limit_from_env(), 0); // explicit disable
        unsafe { std::env::set_var(OBSERVE_NO_PROGRESS_ENV, "junk") };
        assert_eq!(
            no_progress_limit_from_env(),
            DEFAULT_OBSERVE_NO_PROGRESS_SECS
        );
        unsafe { std::env::remove_var(OBSERVE_NO_PROGRESS_ENV) };
    }

    /// FIX C: a silent followed process cannot make the observer outlive its
    /// purpose. The follower exits non-zero with a clear line instead of
    /// spinning at ~6% CPU forever.
    #[cfg(unix)]
    #[test]
    fn test_observer_exits_on_prolonged_no_progress() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("agent-1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // No metadata.json => no watchdog state; the no-progress bound alone
        // must still terminate the follower.
        let mut child = std::process::Command::new("sleep")
            .arg("999999")
            .spawn()
            .expect("spawn silent child");
        let started = std::time::Instant::now();
        let result = observe_live_with(
            &agent_dir,
            child.id(),
            ObserveOptions {
                poll_interval: Duration::from_millis(10),
                no_progress_limit_secs: 1,
            },
        );
        let _ = child.kill();
        let _ = child.wait();
        let error = result.expect_err("observer must exit non-zero on no progress");
        let message = error.to_string();
        assert!(
            message.contains("no stream progress"),
            "unexpected observer exit: {message}"
        );
        assert!(message.contains(OBSERVE_NO_PROGRESS_ENV));
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "bounded exit took too long: {:?}",
            started.elapsed()
        );
    }

    /// A live, quiet child with the bound disabled still exits promptly once the
    /// pid dies (the pre-existing behaviour must not regress).
    #[test]
    fn test_observer_exits_when_pid_is_gone() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agents").join("agent-1");
        std::fs::create_dir_all(&agent_dir).unwrap();
        // A PID that cannot be alive.
        let dead_pid = 4_000_000u32;
        let result = observe_live_with(
            &agent_dir,
            dead_pid,
            ObserveOptions {
                poll_interval: Duration::from_millis(5),
                no_progress_limit_secs: 0,
            },
        );
        assert!(
            result.is_ok(),
            "dead-pid follower should exit cleanly: {result:?}"
        );
    }

    #[test]
    fn test_bridge_handles_missing_stream_gracefully() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path();
        // No raw_stream.jsonl, no output.log.
        run(agent_dir, 1, None).unwrap();
        let stream_path = agent_dir.join(stream_event::STREAM_FILE_NAME);
        let (events, _) = stream_event::read_stream_events(&stream_path, 0).unwrap();
        // Still emits init + result bookends (result success=false).
        assert!(matches!(events.first(), Some(StreamEvent::Init { .. })));
        assert!(matches!(
            events.last(),
            Some(StreamEvent::Result { success: false, .. })
        ));
    }
}
