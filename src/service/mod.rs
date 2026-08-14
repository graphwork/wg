//! Agent service layer
//!
//! Provides agent registry and management for the WG agent service.
//!
//! This module includes:
//! - Executor configuration for spawning agents
//! - Agent registry for tracking running agents

pub mod chat_compactor;
pub mod convergence;
pub mod coordinator_prompt;
pub mod dispatch_boot;
pub mod executor;
pub mod graph_watcher;
pub mod llm;
pub mod planner;
pub mod provider_health;
pub mod registry;

pub use convergence::{
    Admission as ConvergenceAdmission, ConvergencePolicy, ConvergenceStage, ConvergenceState,
    FailedPrerequisiteConvergence, FinishConvergenceAction, FinishConvergenceDecision,
    FinishConvergenceRank, FinishConvergenceSnapshot, RouteAdmission, RouteBreaker,
    RouteBreakerState, WrapperChildCapability, converge_failed_prerequisites,
    reduce_exited_worker_finish,
};
pub use dispatch_boot::{
    ChatSupervisorBootSpec, enumerate_chat_supervisors_for_boot,
    enumerate_chat_supervisors_from_graph,
};
pub use executor::{
    ExecutorConfig, ExecutorRegistry, ExecutorSettings, PromptTemplate, TemplateVars,
};
pub use planner::{
    AckOutcome as PlannerAckOutcome, ActionKind as PlannerActionKind, DecisionTrace,
    DispatchAdmission as PlannerDispatchAdmission,
    DispatchEffectBinding as PlannerDispatchEffectBinding,
    DispatchObservation as PlannerDispatchObservation, DispatchPolicy as PlannerDispatchPolicy,
    DispatchReadiness as PlannerDispatchReadiness,
    DispatchRouteObservation as PlannerDispatchRouteObservation,
    EffectBinding as PlannerEffectBinding, EffectExecutionPhase as PlannerEffectExecutionPhase,
    EffectExecutionRecord as PlannerEffectExecutionRecord, EffectReplay as PlannerEffectReplay,
    EvidenceSlot as PlannerEvidenceSlot, ExternalWait as PlannerExternalWait,
    FailedPrerequisite as PlannerFailedPrerequisite,
    FailedPrerequisiteClass as PlannerFailedPrerequisiteClass,
    FailedPrerequisiteEvidence as PlannerFailedPrerequisiteEvidence,
    ImportedActionLease as PlannerImportedActionLease, ImportedBackoff as PlannerImportedBackoff,
    ImportedGoalSchedule as PlannerImportedGoalSchedule,
    ImportedRouteProbeLease as PlannerImportedRouteProbeLease,
    ImportedRouteSchedule as PlannerImportedRouteSchedule, IncidentCode as PlannerIncidentCode,
    LegacyConvergenceImport as PlannerLegacyConvergenceImport, Observation, ObservationEnvelope,
    OpaqueId as PlannerOpaqueId, OwnerEvidence as PlannerOwnerEvidence, PlannedEffect,
    PlannerRouteProbeLease, PlannerRouteProjection, PlannerRouteState, PlannerRuleset,
    PlannerState, PlannerStatusProjection, PlannerStep, PlannerStore, PlannerTimestamp,
    RedactionPolicy, ReplayReport, ResourceEvidence as PlannerResourceEvidence,
    RouteHealthEvidence as PlannerRouteHealthEvidence, ScheduledAction as PlannerScheduledAction,
    TaskKey as PlannerTaskKey, TaskObservation as PlannerTaskObservation,
    TypedObservation as PlannerTypedObservation, ViolationCode as PlannerViolationCode,
    WaitKind as PlannerWaitKind, WorktreeSpawnObservation as PlannerWorktreeSpawnObservation,
    ZeroOutputObservation as PlannerZeroOutputObservation, plan as plan_daemon_step, plan_guarded,
    replay as replay_daemon, replay_bytes,
};
pub use provider_health::{
    AgentExecutionOutcome, CompletionRefusalCode, ExecutionOutcome, ExecutorFailure,
    HealthRouteKey, ProviderErrorKind, ProviderHealth, ProviderHealthStatus, classify_error,
    classify_execution_outcome, completion_refusal_code, extract_provider_id, load_agent_outcome,
    record_done_outcome,
};
pub use registry::{AgentEntry, AgentRegistry, AgentStatus, LockedRegistry};

/// Check if a process with the given PID is alive.
///
/// Uses `kill(pid, 0)` on Unix to probe without sending a signal.
/// On Windows, uses `tasklist` to check for the PID.
#[cfg(unix)]
pub fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
pub fn is_process_alive(pid: u32) -> bool {
    // `tasklist /FI "PID eq <pid>" /NH /FO CSV` prints a row per matching
    // process and "INFO: No tasks are running which match the specified
    // criteria." on stderr when nothing matches. A quick sentinel check on
    // stdout is cheaper than parsing.
    use std::process::Command;
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {}", pid), "/NH", "/FO", "CSV"])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            // When there's a match the output contains the PID in a CSV cell;
            // when nothing matches tasklist prints the "No tasks..." line to
            // stderr and produces empty stdout.
            !s.trim().is_empty() && s.contains(&pid.to_string())
        }
        _ => false,
    }
}

/// Collect all descendant PIDs of `root_pid` by walking `/proc/*/stat`.
///
/// Returns an empty vec on platforms without a parent-map source. The root
/// PID itself is NOT included in the returned list.
///
/// This is load-bearing for tree-killing agents: a single `wg kill` used to
/// signal only the top-level `timeout` wrapper or bash script, leaving the
/// real `wg native-exec` subprocess alive and still writing to disk. Walking
/// the full parent-map catches every descendant regardless of whether the
/// intermediate processes called `setsid()` or changed process groups.
#[cfg(target_os = "linux")]
pub fn collect_process_descendants(root_pid: u32) -> Vec<u32> {
    use std::collections::HashMap;

    let entries = match std::fs::read_dir("/proc") {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    // Build the complete pid → ppid map from /proc.
    let mut parent_of: HashMap<u32, u32> = HashMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid: u32 = match name.to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let stat = match std::fs::read_to_string(format!("/proc/{}/stat", pid)) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // The `comm` field (2nd) may contain spaces and parentheses, so find
        // the final `)` and parse from there. Format after that:
        //   state(3) ppid(4) pgrp(5) ...
        let comm_end = match stat.rfind(')') {
            Some(i) => i,
            None => continue,
        };
        let rest = &stat[comm_end + 2..];
        let fields: Vec<&str> = rest.split_whitespace().collect();
        // fields[0]=state, fields[1]=ppid
        if let Some(ppid_str) = fields.get(1)
            && let Ok(ppid) = ppid_str.parse::<u32>()
        {
            parent_of.insert(pid, ppid);
        }
    }

    collect_descendants_from_parent_map(root_pid, &parent_of)
}

/// macOS implementation of the /proc parent-map walk above, via libproc:
/// `proc_listallpids` enumerates every visible pid and `proc_pidinfo`
/// (`PROC_PIDTBSDINFO`) returns a fixed-width record whose `pbi_pid`/
/// `pbi_ppid` fields carry everything the BFS needs. Each record is
/// validated (`pbi_pid == queried pid`) so an ABI drift degrades to an
/// empty map — the process-group signal in the kill helpers still covers
/// descendants that stayed in the wrapper's group.
#[cfg(target_os = "macos")]
pub fn collect_process_descendants(root_pid: u32) -> Vec<u32> {
    use std::collections::HashMap;

    // `struct proc_bsdinfo` from <libproc.h> (PROC_PIDTBSDINFO = 3).
    #[repr(C)]
    struct ProcBsdInfo {
        pbi_flags: u32,
        pbi_status: u32,
        pbi_rfu0: u32,
        pbi_pid: u32,
        pbi_ppid: u32,
        pbi_uid: i32,
        pbi_gid: i32,
        pbi_ruid: i32,
        pbi_rgid: i32,
        pbi_svuid: i32,
        pbi_svgid: i32,
        rfu_1: u32,
        pbi_comm: [u8; 16],
        pbi_pcomm: [u8; 32],
        pbi_nfiles: u32,
        pbi_pgid: u32,
        pbi_pjobc: u32,
        e_tdev: u32,
        e_tpgid: i32,
        pbi_nice: i32,
        pbi_start_tvsec: u64,
        pbi_start_tvusec: u64,
    }
    const PROC_PIDTBSDINFO: libc::c_int = 3;

    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_listallpids(buffer: *mut libc::c_void, buffersize: libc::c_int) -> libc::c_int;
        fn proc_pidinfo(
            pid: libc::c_int,
            flavor: libc::c_int,
            arg: libc::uint64_t,
            buffer: *mut libc::c_void,
            buffersize: libc::c_int,
        ) -> libc::c_int;
    }

    // First call with a null buffer returns the process count.
    let count = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Vec::new();
    }
    // Re-query with headroom: the process table can grow between calls.
    let mut pids: Vec<libc::c_int> = vec![0; count as usize + 32];
    let filled = unsafe {
        proc_listallpids(
            pids.as_mut_ptr().cast::<libc::c_void>(),
            (pids.len() * std::mem::size_of::<libc::c_int>()) as libc::c_int,
        )
    };
    if filled <= 0 {
        return Vec::new();
    }

    let mut parent_of: HashMap<u32, u32> = HashMap::new();
    for &pid in &pids[..filled as usize] {
        if pid <= 0 {
            continue;
        }
        let mut info: ProcBsdInfo = unsafe { std::mem::zeroed() };
        let got = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDTBSDINFO,
                0,
                &mut info as *mut ProcBsdInfo as *mut libc::c_void,
                std::mem::size_of::<ProcBsdInfo>() as libc::c_int,
            )
        };
        if got > 0 && info.pbi_pid == pid as u32 {
            parent_of.insert(info.pbi_pid, info.pbi_ppid);
        }
    }

    collect_descendants_from_parent_map(root_pid, &parent_of)
}

/// BFS over a pid → ppid map collecting every descendant of `root_pid`
/// (excluding the root itself). Shared by the platform-specific collectors.
fn collect_descendants_from_parent_map(
    root_pid: u32,
    parent_of: &std::collections::HashMap<u32, u32>,
) -> Vec<u32> {
    let mut descendants: Vec<u32> = Vec::new();
    let mut frontier: Vec<u32> = vec![root_pid];
    while let Some(current) = frontier.pop() {
        for (&child, &parent) in parent_of {
            if parent == current && child != root_pid && !descendants.contains(&child) {
                descendants.push(child);
                frontier.push(child);
            }
        }
    }
    descendants
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn collect_process_descendants(_root_pid: u32) -> Vec<u32> {
    // No /proc parent-map or sysctl kinfo_proc walk on this platform. The
    // process-group signal in the kill helpers still covers descendants
    // that stayed in the wrapper's group.
    Vec::new()
}

/// Send `signal` to `pid`, swallowing ESRCH (process already gone).
#[cfg(unix)]
fn signal_pid(pid: u32, signal: libc::c_int) -> anyhow::Result<()> {
    use anyhow::Context;
    let pid_i32 = pid as i32;
    if unsafe { libc::kill(pid_i32, signal) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        return Err(err).context(format!("Failed to signal PID {}", pid));
    }
    Ok(())
}

/// Signal the whole process group led by `pid` (kill(-pid)).
///
/// Agent wrappers are spawned via `setsid()` (see `spawn/execution.rs`), so
/// the registered wrapper PID names a process group that contains every
/// descendant which did not itself detach. Signaling the group reaches them
/// even when a parent-map walk cannot: on platforms without /proc the walk
/// is empty, and once the wrapper exits its children reparent to init and
/// become unreachable by ppid — but they stay in the group.
///
/// Best-effort: silently does nothing when `pid` does not lead a group
/// (kill returns ESRCH), which is expected for non-leader targets. Guards
/// against the pid<=1 and self special meanings of kill(-1)/kill(-pid).
#[cfg(unix)]
fn signal_process_group(pid: u32, signal: libc::c_int) {
    if pid <= 1 || pid == std::process::id() {
        return;
    }
    unsafe {
        libc::kill(-(pid as libc::pid_t), signal);
    }
}

/// Send SIGTERM to `pid` and all its descendants, wait up to `wait_secs`
/// seconds for the root to exit, then SIGKILL the tree if anything remains.
///
/// Tree-killing is essential for agent processes: the top-level PID is
/// usually a shell or `timeout` wrapper, and killing just that leaves the
/// real worker running as an orphan. See `collect_process_descendants`.
#[cfg(unix)]
pub fn kill_process_graceful(pid: u32, wait_secs: u64) -> anyhow::Result<()> {
    use std::thread;
    use std::time::Duration;

    if !is_process_alive(pid) {
        return Ok(());
    }

    // Snapshot the tree before we start signaling — once children start
    // exiting, their ppid can reparent to 1 (init) and we lose them.
    let descendants = collect_process_descendants(pid);

    // SIGTERM the whole tree (root first so the shell can propagate its own).
    // Group-signal covers in-group descendants (the wrapper is spawned via
    // setsid(), so its pid names its group), the parent-map walk adds any
    // descendant that detached into its own session.
    signal_process_group(pid, libc::SIGTERM);
    signal_pid(pid, libc::SIGTERM)?;
    for child in &descendants {
        let _ = signal_pid(*child, libc::SIGTERM);
    }

    // Wait for root to exit.
    for _ in 0..wait_secs {
        thread::sleep(Duration::from_secs(1));
        if !is_process_alive(pid) {
            break;
        }
    }

    // Re-walk descendants in case new ones spawned, then SIGKILL anything
    // still alive in the tree.
    let mut remaining: Vec<u32> = Vec::new();
    if is_process_alive(pid) {
        remaining.push(pid);
    }
    let late_descendants = collect_process_descendants(pid);
    for child in descendants.iter().chain(late_descendants.iter()).copied() {
        if is_process_alive(child) && !remaining.contains(&child) {
            remaining.push(child);
        }
    }
    for p in &remaining {
        let _ = signal_pid(*p, libc::SIGKILL);
    }
    // Final group sweep: anything still in the wrapper's process group that
    // the parent-map walk missed (e.g. reparented to init) dies here too.
    signal_process_group(pid, libc::SIGKILL);

    Ok(())
}

#[cfg(windows)]
pub fn kill_process_graceful(pid: u32, wait_secs: u64) -> anyhow::Result<()> {
    use std::process::Command;
    use std::thread;
    use std::time::Duration;

    if !is_process_alive(pid) {
        return Ok(());
    }

    // `taskkill /T` without /F requests a graceful WM_CLOSE + tree kill on
    // the process and its descendants. This is the closest Windows analogue
    // to SIGTERM-on-a-process-group. Console-only processes often ignore it
    // since they have no window, but it's still worth trying first.
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T"])
        .output();

    for _ in 0..wait_secs {
        thread::sleep(Duration::from_secs(1));
        if !is_process_alive(pid) {
            return Ok(());
        }
    }

    // Still alive — force kill the tree.
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();

    Ok(())
}

/// Send SIGKILL to `pid` and all its descendants.
#[cfg(unix)]
pub fn kill_process_force(pid: u32) -> anyhow::Result<()> {
    // Group-signal first: the wrapper leads its own session/group, so this
    // reaches every in-group descendant — including ones a parent-map walk
    // would miss after the root exits and they reparent to init.
    signal_process_group(pid, libc::SIGKILL);
    if !is_process_alive(pid) {
        // Still walk the tree — the root is gone but descendants may remain
        // (orphaned to init) and need explicit cleanup.
        for child in collect_process_descendants(pid) {
            let _ = signal_pid(child, libc::SIGKILL);
        }
        return Ok(());
    }

    let descendants = collect_process_descendants(pid);
    signal_pid(pid, libc::SIGKILL)?;
    for child in &descendants {
        let _ = signal_pid(*child, libc::SIGKILL);
    }
    Ok(())
}

#[cfg(windows)]
pub fn kill_process_force(pid: u32) -> anyhow::Result<()> {
    use std::process::Command;
    // `taskkill /F /T /PID` hard-terminates the whole tree.
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
    Ok(())
}

/// SIGKILL every descendant of `root_pid` *without* touching `root_pid`
/// itself. Use this when the caller IS the root (e.g. a native-executor
/// session hard-cancelling its own spawned subprocess tree — we want
/// bash/chrome/curl children dead, but not ourselves).
///
/// Processes detached with `setsid` / `nohup` / `disown` are still
/// descendants in `/proc` terms and will be killed. Genuinely
/// reparented-to-init processes that the kernel no longer associates
/// with our pid are outside our reach — that's the standard Unix
/// semantics we respect (see `docs/design/native-executor-run-loop.md`).
#[cfg(unix)]
pub fn kill_descendants(root_pid: u32) {
    for child in collect_process_descendants(root_pid) {
        let _ = signal_pid(child, libc::SIGKILL);
    }
}

#[cfg(windows)]
pub fn kill_descendants(root_pid: u32) {
    // `taskkill /T` without listing the parent PID target isn't possible;
    // `taskkill /F /T /PID <root>` kills the root AND its tree. We want the
    // tree only. Walk the tree via `wmic` and kill each child.
    use std::process::Command;
    let Ok(out) = Command::new("wmic")
        .args([
            "process",
            "where",
            &format!("ParentProcessId={}", root_pid),
            "get",
            "ProcessId",
            "/format:value",
        ])
        .output()
    else {
        return;
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    for line in stdout.lines() {
        if let Some(pid_str) = line.strip_prefix("ProcessId=")
            && let Ok(pid) = pid_str.trim().parse::<u32>()
            && pid != 0
        {
            let _ = Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .output();
        }
    }
}

/// Read the kernel process-start identity from `/proc/<pid>/stat`.
///
/// Linux field 22 is the start time in clock ticks since boot. Unlike an
/// epoch timestamp rounded to seconds, this token is precise enough for a
/// heartbeat watcher to reject even rapid PID reuse.
#[cfg(target_os = "linux")]
pub fn read_proc_start_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Field 2 (comm) can contain spaces and parentheses; find the last ')'.
    let comm_end = stat.rfind(')')?;
    let fields: Vec<&str> = stat[comm_end + 2..].split_whitespace().collect();
    // starttime is field 22 overall; after stripping pid + comm (fields 1-2),
    // the remaining fields start at field 3, so starttime is at index 19.
    fields.get(19)?.parse().ok()
}

#[cfg(not(target_os = "linux"))]
pub fn read_proc_start_ticks(_pid: u32) -> Option<u64> {
    None
}

/// Read a process's start time from `/proc/<pid>/stat` as seconds since epoch.
///
/// Returns `None` if the process doesn't exist, `/proc` is unavailable, or
/// the stat file can't be parsed. Used to detect PID reuse: if the process
/// at a given PID started much later than expected, the PID was recycled.
#[cfg(target_os = "linux")]
pub fn read_proc_start_time_secs(pid: u32) -> Option<i64> {
    let starttime_ticks = read_proc_start_ticks(pid)?;
    let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;
    if clk_tck == 0 {
        return None;
    }

    let boot_time = read_boot_time()?;
    Some(boot_time + (starttime_ticks / clk_tck) as i64)
}

#[cfg(not(target_os = "linux"))]
pub fn read_proc_start_time_secs(_pid: u32) -> Option<i64> {
    None
}

/// Read system boot time from `/proc/stat` (btime line).
#[cfg(target_os = "linux")]
fn read_boot_time() -> Option<i64> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    for line in stat.lines() {
        if let Some(rest) = line.strip_prefix("btime ") {
            return rest.trim().parse().ok();
        }
    }
    None
}

/// Check whether a process has any active child processes.
///
/// On Linux, reads `/proc/<pid>/task/<tid>/children` for each thread of the
/// process. Falls back to scanning `/proc/*/stat` for ppid matches if the
/// `children` file is unavailable (requires CONFIG_PROC_CHILDREN).
///
/// Returns `true` if at least one live child process is found.
/// Returns `false` on non-Linux platforms or if `/proc` is unavailable.
#[cfg(target_os = "linux")]
pub fn has_active_children(pid: u32) -> bool {
    // Fast path: try /proc/<pid>/task/<pid>/children (available with CONFIG_PROC_CHILDREN)
    let children_path = format!("/proc/{}/task/{}/children", pid, pid);
    if let Ok(content) = std::fs::read_to_string(&children_path) {
        let has_children = content
            .split_whitespace()
            .any(|tok| tok.parse::<u32>().map(is_process_alive).unwrap_or(false));
        if has_children {
            return true;
        }
        // File existed but was empty — no children via this thread. Check other
        // threads below only if there are multiple, otherwise we're done.
    }

    // Check all threads of this process for children
    let task_dir = format!("/proc/{}/task", pid);
    if let Ok(entries) = std::fs::read_dir(&task_dir) {
        for entry in entries.flatten() {
            let tid = entry.file_name();
            let tid_str = tid.to_string_lossy();
            // Skip the main thread we already checked
            if tid_str == pid.to_string() {
                continue;
            }
            let thread_children = format!("/proc/{}/task/{}/children", pid, tid_str);
            if let Ok(content) = std::fs::read_to_string(&thread_children)
                && content
                    .split_whitespace()
                    .any(|tok| tok.parse::<u32>().map(is_process_alive).unwrap_or(false))
            {
                return true;
            }
        }
    }

    // Slow fallback: scan /proc/*/stat for ppid == our pid.
    // This works even without CONFIG_PROC_CHILDREN.
    if let Ok(entries) = std::fs::read_dir("/proc") {
        let pid_str = pid.to_string();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Only look at numeric directories (PIDs)
            if !name_str.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                continue;
            }
            // Skip self
            if name_str == pid_str {
                continue;
            }
            let stat_path = format!("/proc/{}/stat", name_str);
            if let Ok(stat) = std::fs::read_to_string(&stat_path) {
                // ppid is field 4; field 2 (comm) can contain ')' so find the last one
                if let Some(comm_end) = stat.rfind(')') {
                    let after_comm = &stat[comm_end + 2..];
                    let fields: Vec<&str> = after_comm.split_whitespace().collect();
                    // fields[0] = state, fields[1] = ppid
                    if let Some(ppid_str) = fields.get(1)
                        && *ppid_str == pid_str
                    {
                        return true;
                    }
                }
            }
        }
    }

    false
}

#[cfg(not(target_os = "linux"))]
pub fn has_active_children(_pid: u32) -> bool {
    false
}

/// Check whether the process at `pid` is the same one that was started at
/// `expected_start_epoch` (Unix timestamp). Returns `true` if we can confirm
/// the process identity matches, or if the check is inconclusive (non-Linux,
/// missing `/proc`, etc.). Returns `false` only when we can positively
/// determine that the PID has been reused by a different process.
pub fn verify_process_identity(pid: u32, expected_start_epoch: i64) -> bool {
    match read_proc_start_time_secs(pid) {
        Some(actual_start) => {
            // Allow 120 seconds of slack: the wrapper script may take a
            // moment to start after the spawn timestamp is recorded, and
            // clock granularity in /proc/stat is 1 second.
            actual_start <= expected_start_epoch + 120
        }
        None => {
            // Can't read /proc — process might be gone, or we're not on
            // Linux. Fall back to conservative "assume same process".
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn collect_process_descendants_finds_grandchildren() {
        // Spawn: sleep → (bash → sleep). collect_process_descendants(bash's
        // parent) should include the inner sleep.
        let outer = std::process::Command::new("bash")
            .arg("-c")
            .arg("sleep 30 & wait")
            .spawn()
            .expect("spawn bash");
        let outer_pid = outer.id();

        // Give the inner sleep a moment to fork.
        std::thread::sleep(std::time::Duration::from_millis(200));

        let descendants = collect_process_descendants(outer_pid);
        let mut outer = outer;
        let _ = kill_process_force(outer_pid);
        outer.wait().ok();
        assert!(
            !descendants.is_empty(),
            "expected at least one descendant of the bash wrapper; got {:?}",
            descendants
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kill_process_force_tree_reaps_descendants() {
        // Spawn bash that backgrounds a long sleep. Kill the root — the
        // inner sleep must be gone too. Descendants are *not* this test
        // process's direct children (they're bash's), so they become
        // zombies that init reaps automatically — we can check them
        // directly. The bash child *is* our direct child so we must
        // wait() on it to reap the zombie before checking liveness.
        let mut outer = std::process::Command::new("bash")
            .arg("-c")
            .arg("sleep 300 & wait")
            .spawn()
            .expect("spawn bash");
        let outer_pid = outer.id();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let descendants_before = collect_process_descendants(outer_pid);
        assert!(!descendants_before.is_empty(), "needs a descendant to test");

        kill_process_force(outer_pid).expect("force kill root");

        // Reap our direct child so its PID becomes invalid (otherwise
        // kill(pid, 0) succeeds on the zombie PID and is_process_alive
        // returns a false positive).
        outer.wait().ok();

        // Give the kernel a beat for init to reap the grandchildren.
        std::thread::sleep(std::time::Duration::from_millis(200));

        for child in &descendants_before {
            assert!(
                !is_process_alive(*child),
                "descendant PID {} should be dead after tree kill",
                child
            );
        }
        assert!(
            !is_process_alive(outer_pid),
            "root PID {} should be dead after tree kill",
            outer_pid
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn collect_process_descendants_finds_grandchildren_macos() {
        // Same shape as the Linux test: spawn bash that backgrounds a sleep,
        // then collect descendants. Exercises the sysctl(KERN_PROC_ALL) walk.
        let outer = std::process::Command::new("bash")
            .arg("-c")
            .arg("sleep 30 & sleep 30 & wait")
            .spawn()
            .expect("spawn bash");
        let outer_pid = outer.id();

        std::thread::sleep(std::time::Duration::from_millis(200));

        let descendants = collect_process_descendants(outer_pid);
        let mut outer = outer;
        let _ = kill_process_force(outer_pid);
        outer.wait().ok();
        assert!(
            !descendants.is_empty(),
            "expected at least one descendant of the bash wrapper; got {:?}",
            descendants
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn kill_process_force_tree_reaps_descendants_macos() {
        // Reproduction of the orphaned-agent bug: a wrapper-like bash that
        // backgrounded a long sleep is killed; the inner sleep must die too.
        // The wrapper is NOT a process-group leader here (plain spawn), so
        // the group-signal path misses and the sysctl descendant walk must
        // carry the kill.
        let mut outer = std::process::Command::new("bash")
            .arg("-c")
            .arg("sleep 300 & wait")
            .spawn()
            .expect("spawn bash");
        let outer_pid = outer.id();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let descendants_before = collect_process_descendants(outer_pid);
        assert!(!descendants_before.is_empty(), "needs a descendant to test");

        kill_process_force(outer_pid).expect("force kill root");

        outer.wait().ok();
        std::thread::sleep(std::time::Duration::from_millis(200));

        for child in &descendants_before {
            assert!(
                !is_process_alive(*child),
                "descendant PID {} should be dead after tree kill",
                child
            );
        }
        assert!(
            !is_process_alive(outer_pid),
            "root PID {} should be dead after tree kill",
            outer_pid
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn kill_process_force_reaps_session_leader_tree_macos() {
        // The real agent-wrapper spawn path calls setsid() (see
        // spawn/execution.rs), so the wrapper is a session/process-group
        // leader. This test mimics that: the group-signal path must reach
        // descendants even without any /proc-style walk.
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new("bash");
        cmd.arg("-c").arg("sleep 300 & wait");
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut outer = cmd.spawn().expect("spawn bash");
        let outer_pid = outer.id();
        std::thread::sleep(std::time::Duration::from_millis(200));

        let descendants_before = collect_process_descendants(outer_pid);
        assert!(!descendants_before.is_empty(), "needs a descendant to test");

        kill_process_force(outer_pid).expect("force kill root");

        outer.wait().ok();
        std::thread::sleep(std::time::Duration::from_millis(200));

        for child in &descendants_before {
            assert!(
                !is_process_alive(*child),
                "descendant PID {} should be dead after tree kill",
                child
            );
        }
        assert!(
            !is_process_alive(outer_pid),
            "root PID {} should be dead after tree kill",
            outer_pid
        );
    }

    #[test]
    fn has_active_children_with_child_process() {
        // Spawn a child process that sleeps, then verify has_active_children returns true
        let child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("failed to spawn sleep");
        let our_pid = std::process::id();

        // Our process should now have at least one child
        assert!(
            has_active_children(our_pid),
            "expected has_active_children to return true when a child process is running"
        );

        // Clean up
        let mut child = child;
        child.kill().ok();
        child.wait().ok();
    }

    #[test]
    fn has_active_children_without_child_process() {
        // PID 1 (init/systemd) always exists but its children aren't ours.
        // Use a nonsense PID that doesn't exist.
        assert!(
            !has_active_children(u32::MAX - 1),
            "expected has_active_children to return false for a nonexistent PID"
        );
    }

    #[test]
    fn has_active_children_after_child_exits() {
        // Spawn a child that exits immediately, then verify no active children
        let child = std::process::Command::new("true")
            .spawn()
            .expect("failed to spawn true");
        let mut child = child;
        child.wait().ok(); // Wait for it to exit

        // After the child exits, check with a fresh spawn to avoid flakiness
        // from other test children. Use a dedicated subprocess as the parent.
        let parent = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("failed to spawn sleep");
        let parent_pid = parent.id();

        // The sleep process has no children of its own
        assert!(
            !has_active_children(parent_pid),
            "expected has_active_children to return false for a process with no children"
        );

        // Clean up
        let mut parent = parent;
        parent.kill().ok();
        parent.wait().ok();
    }
}
