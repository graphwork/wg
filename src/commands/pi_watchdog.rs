use anyhow::{Context, Result};
use chrono::Utc;
use std::path::{Path, PathBuf};
use worksgood::lifecycle::{
    ActorKind, FenceExpectation, LifecycleActor, TransitionKind, TransitionRequest,
    apply_transition,
};
use worksgood::parser::{load_graph, modify_graph};
use worksgood::pi_watchdog::{
    EffectAcknowledgement, ExitStatus, ManualGrant, Observation, PiWatchdog, ProcessIdentity,
    QosClass, RouteSnapshot, SessionProof, SourceTuple, TerminalDisposition, TerminalIntentReceipt,
    ToolContract, WatchdogPolicy,
};

use crate::cli::PiWatchdogCommands;

pub fn run(dir: &Path, command: PiWatchdogCommands, json: bool) -> Result<()> {
    match command {
        PiWatchdogCommands::Status { id } => status(dir, &id, json),
        PiWatchdogCommands::Resume {
            id,
            reason,
            grant_epochs,
            grant_elapsed_secs,
            ack_call,
            disposition,
            receipt,
        } => resume(
            dir,
            &id,
            reason,
            grant_epochs,
            grant_elapsed_secs,
            ack_call,
            disposition,
            receipt,
            json,
        ),
        PiWatchdogCommands::Abort { id, reason } => abort(dir, &id, &reason, json),
        PiWatchdogCommands::Bootstrap { id, agent_dir, pid } => {
            bootstrap(dir, &id, &agent_dir, pid)
        }
        PiWatchdogCommands::ProcessExit { id, exit_code } => process_exit(dir, &id, exit_code),
        PiWatchdogCommands::FixtureInit { id, worktree, now } => {
            fixture_init(dir, &id, &worktree, now)
        }
        PiWatchdogCommands::FixtureObserve { id, event, now } => {
            fixture_observe(dir, &id, &event, now)
        }
        PiWatchdogCommands::FixtureTick { id, now } => fixture_tick(dir, &id, now),
    }
}

fn state_path(dir: &Path, task_id: &str) -> Result<PathBuf> {
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(task_id)?;
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("task has no current attempt")?;
    let canonical = dir
        .join("attempts")
        .join(&attempt.id)
        .join("pi")
        .join("state.json");
    if canonical.exists() {
        return Ok(canonical);
    }
    // Compatibility for attempts started while the isolated-worktree observer
    // and watchdog roots were landing in separate commits.
    if let Some(agent) = task.assigned.as_deref() {
        let root = dir.parent().unwrap_or(dir);
        let compatibility = root
            .join(".wg-worktrees")
            .join(agent)
            .join(".wg-pi-watchdog/state.json");
        if compatibility.exists() {
            return Ok(compatibility);
        }
    }
    anyhow::bail!(
        "no Pi watchdog state for current attempt {} (expected {})",
        attempt.id,
        canonical.display()
    )
}

fn checked_open(dir: &Path, task_id: &str) -> Result<PiWatchdog> {
    let path = state_path(dir, task_id)?;
    let watchdog = PiWatchdog::open(&path).map_err(anyhow::Error::new)?;
    let graph = load_graph(dir.join("graph.jsonl"))?;
    let task = graph.get_task_or_err(task_id)?;
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("task has no current attempt")?;
    let source = &watchdog.state().source;
    if source.task_id != task.id
        || source.generation != task.lifecycle.generation
        || source.attempt_id != attempt.id
        || source.attempt_fence != task.lifecycle.fence
    {
        anyhow::bail!("stale_attempt: watchdog source tuple does not match the lifecycle kernel")
    }
    Ok(watchdog)
}

fn status(dir: &Path, id: &str, json: bool) -> Result<()> {
    let watchdog = checked_open(dir, id)?;
    let state = watchdog.state();
    let now = Utc::now().timestamp();
    let silence = now.saturating_sub(state.last_meaningful_at);
    if json {
        let value = serde_json::json!({
            "state": state,
            "policy": watchdog.policy(),
            "silence_secs": silence,
            "soft_suspect_secs": watchdog.policy().meaningful_silence_secs,
            "hard_resume_after_secs": state.hard_resume_after_secs,
            "next_safe_operator_action": next_action(state),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!(
        "Pi watchdog: {:?}{}",
        state.classification,
        if state.classification == worksgood::pi_watchdog::Classification::Suspect {
            " (soft observation; process intact)"
        } else {
            ""
        }
    );
    println!(
        "  source: task={} gen={} attempt={} fence={} worktree-lease={}",
        state.source.task_id,
        state.source.generation,
        state.source.attempt_id,
        state.source.attempt_fence,
        state.source.worktree_lease_epoch
    );
    println!(
        "  session: id={} leaf={} proof={} route=pi:{}:{}@{} qos={:?}",
        state.session.session_id,
        state.session.branch_leaf,
        state.session.digest(),
        state.route.provider,
        state.route.model,
        state.route.reasoning.as_deref().unwrap_or("default"),
        state.route.qos
    );
    println!(
        "  process: continuation-epoch={} epoch={} pid={} pgid={} start={} boot={} nonce={} exact={}",
        state.continuation_epoch,
        state.process_epoch,
        state.process.pid,
        state.process.pgid,
        state.process.start_ticks,
        state.process.boot_id,
        state.process.nonce,
        state.exact_guards.pid_identity
    );
    println!(
        "  progress: seq={} {} at {}; silence={}s / soft-suspect={}s",
        state.progress_seq,
        state.last_meaningful_kind,
        state.last_meaningful_at,
        silence,
        watchdog.policy().meaningful_silence_secs
    );
    println!(
        "  probe: action={:?} observed={:?}; progress-reset=no",
        state.probe_action_id, state.probe_observed_at
    );
    println!(
        "  hard-resume: phase={:?} threshold={} eligible={:?} grace-deadline={:?}",
        state.phase,
        state
            .hard_resume_after_secs
            .map(|v| format!("{v}s"))
            .unwrap_or_else(|| "none".into()),
        state.hard_eligible_at,
        state.hard_grace_deadline
    );
    println!(
        "  tool: {:?}; wait: {:?}; prompt-marker: {:?}",
        state.tool, state.wait_correlation, state.prompt_marker
    );
    println!(
        "  budget: epochs={}/{}+{} elapsed-reserved={}/{}+{}s (recovery only)",
        state.epochs_used,
        watchdog.policy().max_continuation_epochs,
        state.manual_epochs_granted,
        state.elapsed_reserved_secs,
        watchdog.policy().max_continuation_elapsed_secs,
        state.manual_elapsed_granted_secs
    );
    println!(
        "  reason: {}; pending={:?}; exact-route-error={:?}",
        state.reason_code.as_deref().unwrap_or("none"),
        state.pending_actions,
        state.exact_route_error
    );
    println!("  next: {}", next_action(state));
    Ok(())
}

fn next_action(state: &worksgood::pi_watchdog::PiWatchdogState) -> String {
    use worksgood::pi_watchdog::Classification::*;
    match state.classification {
        Active => "continued observation; total runtime is not a deadline".into(),
        Suspect => "read-only probe/observe; no signal before hard policy + grace + proof".into(),
        HardResumeEligible => "await hard grace and fresh unchanged proof CAS".into(),
        NeedsFinalization => {
            "inspect same-session completion action; no completion is inferred".into()
        }
        StalledOperatorRequired => format!(
            "wg pi-watchdog resume {} --reason '<audited reason>'",
            state.source.task_id
        ),
        WaitingUser => "await the accepted correlation through normal lifecycle".into(),
        LongTool => "protect the valid long-tool lease; reconcile effects at expiry".into(),
        Fencing | Resuming => "allow the durable outbox to converge; do not launch manually".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn resume(
    dir: &Path,
    id: &str,
    reason: String,
    epochs: u32,
    elapsed: u64,
    ack_call: Option<String>,
    disposition: Option<String>,
    receipt: Option<String>,
    json: bool,
) -> Result<()> {
    let mut watchdog = checked_open(dir, id)?;
    let effect_ack = match (ack_call, disposition, receipt) {
        (None, None, None) => None,
        (Some(tool_call_id), Some(disposition), Some(receipt)) => Some(EffectAcknowledgement {
            tool_call_id,
            disposition,
            receipt,
        }),
        _ => anyhow::bail!("--ack-call, --disposition, and --receipt must be supplied together"),
    };
    let action_id = format!(
        "manual:{}:{}:{}:{}",
        id,
        watchdog.state().source.attempt_fence,
        watchdog.state().process_epoch,
        blake3::hash(reason.as_bytes()).to_hex()
    );
    watchdog
        .manual_resume(
            ManualGrant {
                action_id,
                reason,
                epochs,
                elapsed_secs: elapsed,
                effect_ack,
            },
            Utc::now().timestamp(),
        )
        .map_err(anyhow::Error::new)?;
    if json {
        println!("{}", serde_json::to_string_pretty(watchdog.state())?);
    } else {
        println!(
            "Manual same-session grant recorded for '{}'; route/session/attempt/worktree remain frozen",
            id
        );
    }
    Ok(())
}

fn bootstrap(dir: &Path, id: &str, agent_dir: &Path, pid: u32) -> Result<()> {
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let task = graph.get_task_or_err(id)?;
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("Pi bootstrap requires current attempt")?
        .clone();
    let state_path = dir.join("attempts").join(&attempt.id).join("pi/state.json");
    if state_path.exists() {
        return Ok(());
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&std::fs::read(agent_dir.join("metadata.json"))?)?;
    let plan: serde_json::Value =
        serde_json::from_slice(&std::fs::read(agent_dir.join("pi-session-plan.json"))?)?;
    let worktree = metadata
        .get("worktree_path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            metadata
                .get("effective_cwd")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        });
    let model = metadata
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("pi:unknown:unknown");
    let inner = model.strip_prefix("pi:").unwrap_or(model);
    let (provider, model_id) = inner.split_once(':').unwrap_or(("unknown", inner));
    let session_file = PathBuf::from(
        plan["session_file"]
            .as_str()
            .context("session file missing")?,
    );
    let session_bytes = std::fs::read(&session_file)?;
    let source = SourceTuple {
        task_id: id.into(),
        generation: task.lifecycle.generation,
        attempt_id: attempt.id.clone(),
        attempt_fence: task.lifecycle.fence,
        worktree_lease_epoch: task.lifecycle.fence,
        worktree_path: worktree,
    };
    let route = RouteSnapshot {
        handler: "pi".into(),
        provider: provider.into(),
        model: model_id.into(),
        reasoning: metadata
            .get("reasoning")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        endpoint_redacted: "pi-owned".into(),
        endpoint_hmac: format!("b3:{}", blake3::hash(model.as_bytes()).to_hex()),
        qos: QosClass::Low,
        pi_binary_digest: "pi-path-owned".into(),
        plugin_digest: worksgood::pi_plugin::WG_PI_PLUGIN_COMPAT_VERSION.into(),
    };
    let session = SessionProof {
        session_id: plan["session_id"]
            .as_str()
            .context("session id missing")?
            .into(),
        branch_leaf: "header".into(),
        session_dir: PathBuf::from(
            plan["session_dir"]
                .as_str()
                .context("session dir missing")?,
        ),
        session_file,
        header_digest: plan["header_digest"]
            .as_str()
            .context("header digest missing")?
            .into(),
        append_prefix_digest: format!("b3:{}", blake3::hash(&session_bytes).to_hex()),
        append_prefix_len: 1,
    };
    let process = capture_process(pid)?;
    PiWatchdog::new_at(
        state_path,
        source.clone(),
        route.clone(),
        session.clone(),
        process,
        WatchdogPolicy::default(),
        Utc::now().timestamp(),
    )
    .map_err(anyhow::Error::new)?;
    let authorization = worksgood::lifecycle::PiContinuationAuthorization {
        authorization_id: format!("pi-auth:{}", attempt.id),
        task_id: id.into(),
        generation: source.generation,
        attempt_id: source.attempt_id,
        attempt_fence: source.attempt_fence,
        worktree_lease_epoch: source.worktree_lease_epoch,
        session_proof_digest: session.digest(),
        route_snapshot_digest: route.digest(),
        state: worksgood::lifecycle::PiAuthorizationState::Active,
        max_replacement_epochs: 3,
        max_reserved_elapsed_secs: 1800,
        epochs_used: 0,
        elapsed_reserved_secs: 0,
        issued_by_policy: "pi-watchdog-static-v1".into(),
    };
    let expected = FenceExpectation::current(task);
    let mut rejection = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        if let Err(e) = apply_transition(
            task,
            TransitionRequest::new(
                TransitionKind::PiContinuationAuthorized {
                    authorization: authorization.clone(),
                    initial_process_epoch: 1,
                },
                LifecycleActor {
                    kind: ActorKind::Dispatcher,
                    id: "pi-spawn-bootstrap".into(),
                },
                "pi_authorized",
                format!("pi-auth:{}", attempt.id),
            )
            .expecting(expected.clone()),
        ) {
            rejection = Some(e);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    Ok(())
}

fn capture_process(pid: u32) -> Result<ProcessIdentity> {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let close = stat.rfind(')').context("invalid proc stat")?;
        let fields: Vec<&str> = stat[close + 2..].split_whitespace().collect();
        let pgid = fields
            .get(2)
            .context("proc process group missing")?
            .parse()?;
        let start_ticks = fields
            .get(19)
            .context("proc start ticks missing")?
            .parse()?;
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
            .trim()
            .to_string();
        Ok(ProcessIdentity {
            pid,
            pgid,
            start_ticks,
            boot_id,
            nonce: uuid::Uuid::now_v7().to_string(),
        })
    }
    #[cfg(not(target_os = "linux"))]
    Ok(ProcessIdentity {
        pid,
        pgid: pid,
        start_ticks: 0,
        boot_id: "platform".into(),
        nonce: uuid::Uuid::now_v7().to_string(),
    })
}

/// Reserve a worker terminal intent in the lifecycle/watchdog first-terminal
/// CAS. Candidate finalization consumes this receipt only after process exit;
/// this function never checkpoints, merges, or resumes Pi.
pub fn reserve_worker_terminal(
    dir: &Path,
    id: &str,
    disposition: TerminalDisposition,
    tool_call_id: &str,
) -> Result<()> {
    let mut watchdog = checked_open(dir, id)?;
    let receipt = TerminalIntentReceipt::new(
        &watchdog,
        watchdog.state().process_epoch,
        tool_call_id,
        disposition,
    );
    let graph_path = dir.join("graph.jsonl");
    let mut rejection = None;
    let receipt_for_graph = receipt.clone();
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        let request = TransitionRequest::new(
            TransitionKind::PiTerminalIntent {
                receipt: receipt_for_graph.clone(),
            },
            LifecycleActor {
                kind: ActorKind::Worker,
                id: task
                    .lifecycle
                    .current_attempt
                    .as_ref()
                    .map(|a| a.actor_id.clone())
                    .unwrap_or_else(|| "pi-worker".into()),
            },
            "worker_terminal_intent",
            receipt_for_graph.idempotency_key.clone(),
        )
        .expecting(FenceExpectation::current(task));
        if let Err(error) = apply_transition(task, request) {
            // Exact duplicate is idempotent at the lifecycle layer. A
            // contradictory receipt remains evidence and cannot replace it.
            rejection = Some(error);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    watchdog
        .observe(Observation::TerminalIntent(receipt), Utc::now().timestamp())
        .map_err(anyhow::Error::new)?;
    Ok(())
}

fn process_exit(dir: &Path, id: &str, exit_code: i32) -> Result<()> {
    let mut watchdog = checked_open(dir, id)?;
    let state = watchdog.state().clone();
    let graph_path = dir.join("graph.jsonl");
    let mut rejection = None;
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        let request = TransitionRequest::new(
            TransitionKind::PiProcessEpochExited {
                process_epoch: state.process_epoch,
                exact_reap_proof: true,
                effect_safe: state.exact_guards.effect,
            },
            LifecycleActor {
                kind: ActorKind::ProcessObserver,
                id: "pi-watchdog".into(),
            },
            "needs_finalization_exit",
            format!(
                "pi-exit:{}:{}",
                state.source.attempt_id, state.process_epoch
            ),
        )
        .expecting(FenceExpectation::current(task));
        if let Err(e) = apply_transition(task, request) {
            rejection = Some(e);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    watchdog
        .observe(
            Observation::ProcessExited {
                status: ExitStatus::Code(exit_code),
                reaped: true,
            },
            Utc::now().timestamp(),
        )
        .map_err(anyhow::Error::new)?;
    Ok(())
}

fn fixture_init(dir: &Path, id: &str, worktree: &Path, now: i64) -> Result<()> {
    let graph_path = dir.join("graph.jsonl");
    let graph = load_graph(&graph_path)?;
    let task = graph.get_task_or_err(id)?;
    let attempt = task
        .lifecycle
        .current_attempt
        .as_ref()
        .context("fixture task must be claimed")?
        .clone();
    let state_dir = dir.join("attempts").join(&attempt.id).join("pi");
    let session_dir = state_dir.join("session");
    std::fs::create_dir_all(&session_dir)?;
    let session_file = session_dir.join("fake-session.jsonl");
    std::fs::write(
        &session_file,
        "{\"type\":\"session\",\"version\":3,\"id\":\"fake-session\"}\n",
    )?;
    let source = SourceTuple {
        task_id: id.into(),
        generation: task.lifecycle.generation,
        attempt_id: attempt.id.clone(),
        attempt_fence: task.lifecycle.fence,
        worktree_lease_epoch: task.lifecycle.fence,
        worktree_path: worktree.to_owned(),
    };
    let route = RouteSnapshot {
        handler: "pi".into(),
        provider: "fake-free".into(),
        model: "fake-slow".into(),
        reasoning: Some("high".into()),
        endpoint_redacted: "fake://local".into(),
        endpoint_hmac: "fixture-endpoint".into(),
        qos: QosClass::Free,
        pi_binary_digest: "fake-pi-v1".into(),
        plugin_digest: "fake-plugin-v1".into(),
    };
    let session = SessionProof {
        session_id: "fake-session".into(),
        branch_leaf: "leaf-0".into(),
        session_dir,
        session_file,
        header_digest: "fixture-header".into(),
        append_prefix_digest: "fixture-prefix".into(),
        append_prefix_len: 1,
    };
    let process = ProcessIdentity {
        pid: std::process::id(),
        pgid: std::process::id(),
        start_ticks: 1,
        boot_id: "fixture-boot".into(),
        nonce: "fixture-nonce".into(),
    };
    let state_path = state_dir.join("state.json");
    PiWatchdog::new_at(
        state_path,
        source.clone(),
        route.clone(),
        session.clone(),
        process,
        WatchdogPolicy::default(),
        now,
    )
    .map_err(anyhow::Error::new)?;
    let authorization = worksgood::lifecycle::PiContinuationAuthorization {
        authorization_id: format!("fixture-auth:{}", attempt.id),
        task_id: id.into(),
        generation: source.generation,
        attempt_id: source.attempt_id,
        attempt_fence: source.attempt_fence,
        worktree_lease_epoch: source.worktree_lease_epoch,
        session_proof_digest: session.digest(),
        route_snapshot_digest: route.digest(),
        state: worksgood::lifecycle::PiAuthorizationState::Active,
        max_replacement_epochs: 3,
        max_reserved_elapsed_secs: 1800,
        epochs_used: 0,
        elapsed_reserved_secs: 0,
        issued_by_policy: "pi-watchdog-static-v1".into(),
    };
    let expected = FenceExpectation::current(task);
    let task_id = id.to_string();
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(&task_id) else {
            return false;
        };
        apply_transition(
            task,
            TransitionRequest::new(
                TransitionKind::PiContinuationAuthorized {
                    authorization: authorization.clone(),
                    initial_process_epoch: 1,
                },
                LifecycleActor {
                    kind: ActorKind::Dispatcher,
                    id: "fake-pi-fixture".into(),
                },
                "pi_authorized",
                format!("fixture-auth:{}", attempt.id),
            )
            .expecting(expected.clone()),
        )
        .is_ok()
    })?;
    println!(
        "Fake-Pi initialized: production soft=300s free/low hard>=900s grace=60s session=fake-session attempt={}",
        attempt.id
    );
    Ok(())
}

fn fixture_observe(dir: &Path, id: &str, event: &str, now: i64) -> Result<()> {
    let mut watchdog = checked_open(dir, id)?;
    let mut terminal_receipt = None;
    let observation = match event {
        "provider-start" => Observation::ProviderRequestStarted {
            call_id: "provider-1".into(),
        },
        "provider-retry" => Observation::ProviderRetry,
        "token" => Observation::TokenDelta { tokens: 1 },
        "thinking" => Observation::ThinkingDelta,
        "unknown" => Observation::PhaseUnknown,
        "settled" => Observation::AgentSettled,
        "exit-zero" => Observation::ProcessExited {
            status: ExitStatus::Code(0),
            reaped: true,
        },
        "exit-nonzero" => Observation::ProcessExited {
            status: ExitStatus::Code(9),
            reaped: true,
        },
        "eof" => Observation::PipeEof { reaped: true },
        "wait" => Observation::WaitAccepted {
            correlation: "fixture-answer".into(),
        },
        "long-tool" => Observation::ToolIntent {
            contract: ToolContract::read_only("fixture-tool", now + 10_000),
        },
        "unsafe-tool" => Observation::ToolIntent {
            contract: ToolContract::non_idempotent("fixture-danger"),
        },
        "probe" => Observation::ProbeObserved {
            progress_seq: watchdog.state().progress_seq,
            session_leaf: watchdog.state().session.branch_leaf.clone(),
            alive: true,
        },
        "launched" => Observation::ContinuationLaunched,
        "permit" => Observation::ExecutionPermitted,
        "done" | "fail" | "park" => {
            let disposition = match event {
                "done" => TerminalDisposition::SuccessIntent,
                "fail" => TerminalDisposition::Failure,
                _ => TerminalDisposition::Park,
            };
            let receipt = TerminalIntentReceipt::new(
                &watchdog,
                watchdog.state().process_epoch,
                format!("fixture-{event}"),
                disposition,
            );
            terminal_receipt = Some(receipt.clone());
            Observation::TerminalIntent(receipt)
        }
        _ => anyhow::bail!("unknown Fake-Pi event {event}"),
    };
    if let Some(receipt) = terminal_receipt.as_ref() {
        let graph_path = dir.join("graph.jsonl");
        let mut rejection = None;
        let receipt_for_graph = receipt.clone();
        modify_graph(&graph_path, |graph| {
            let Some(task) = graph.get_task_mut(id) else {
                return false;
            };
            let request = TransitionRequest::new(
                TransitionKind::PiTerminalIntent {
                    receipt: receipt_for_graph.clone(),
                },
                LifecycleActor {
                    kind: ActorKind::Worker,
                    id: task
                        .lifecycle
                        .current_attempt
                        .as_ref()
                        .map(|a| a.actor_id.clone())
                        .unwrap_or_else(|| "fake-pi-fixture".into()),
                },
                "fixture_terminal_intent",
                receipt_for_graph.idempotency_key.clone(),
            )
            .expecting(FenceExpectation::current(task));
            if let Err(error) = apply_transition(task, request) {
                rejection = Some(error);
                return false;
            }
            true
        })?;
        if let Some(error) = rejection {
            return Err(anyhow::Error::new(error));
        }
    }
    let actions = watchdog
        .observe(observation, now)
        .map_err(anyhow::Error::new)?;
    println!(
        "event={event} classification={:?} actions={actions:?} process_epoch={} continuation_epoch={} prompts={} terminal={}",
        watchdog.state().classification,
        watchdog.state().process_epoch,
        watchdog.state().continuation_epoch,
        watchdog.state().prompt_count,
        watchdog.state().terminal
    );
    Ok(())
}

fn fixture_tick(dir: &Path, id: &str, now: i64) -> Result<()> {
    let mut watchdog = checked_open(dir, id)?;
    let actions = watchdog.tick(now).map_err(anyhow::Error::new)?;
    println!(
        "tick={now} classification={:?} actions={actions:?} process_epoch={} continuation_epoch={} prompts={} budget={}/{}s",
        watchdog.state().classification,
        watchdog.state().process_epoch,
        watchdog.state().continuation_epoch,
        watchdog.state().prompt_count,
        watchdog.state().epochs_used,
        watchdog.state().elapsed_reserved_secs
    );
    Ok(())
}

fn abort(dir: &Path, id: &str, reason: &str, json: bool) -> Result<()> {
    if reason.trim().is_empty() {
        anyhow::bail!("operator abort requires --reason");
    }
    let mut watchdog = checked_open(dir, id)?;
    let state = watchdog.state().clone();
    let receipt = TerminalIntentReceipt {
        task_id: id.into(),
        generation: state.source.generation,
        attempt_id: state.source.attempt_id.clone(),
        attempt_fence: state.source.attempt_fence,
        process_epoch: state.process_epoch,
        tool_call_id: format!(
            "operator-abort:{}:{}",
            state.process_epoch,
            blake3::hash(reason.as_bytes()).to_hex()
        ),
        disposition: TerminalDisposition::Abort,
        idempotency_key: format!(
            "pi-operator-abort:{}:{}",
            state.source.attempt_id, state.process_epoch
        ),
    };
    let graph_path = dir.join("graph.jsonl");
    let mut rejection = None;
    let receipt_for_graph = receipt.clone();
    modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(id) else {
            return false;
        };
        let request = TransitionRequest::new(
            TransitionKind::PiTerminalIntent {
                receipt: receipt_for_graph.clone(),
            },
            LifecycleActor {
                kind: ActorKind::Operator,
                id: worksgood::current_user(),
            },
            "operator_abort",
            receipt_for_graph.idempotency_key.clone(),
        )
        .expecting(FenceExpectation::current(task))
        .with_evidence(format!(
            "operator-reason:b3:{}",
            blake3::hash(reason.as_bytes()).to_hex()
        ));
        if let Err(error) = apply_transition(task, request) {
            rejection = Some(error);
            return false;
        }
        true
    })?;
    if let Some(error) = rejection {
        return Err(anyhow::Error::new(error));
    }
    watchdog
        .observe(Observation::TerminalIntent(receipt), Utc::now().timestamp())
        .map_err(anyhow::Error::new)?;
    if json {
        println!(
            "{{\"task\":{},\"reason_code\":\"operator_abort\"}}",
            serde_json::to_string(id)?
        );
    } else {
        println!(
            "Operator abort accepted for '{}' by first-terminal-wins lifecycle CAS",
            id
        );
    }
    Ok(())
}
