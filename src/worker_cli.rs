//! Worker-side CLI adapter for the attempt-scoped daemon capability channel.
//!
//! Presence of `WG_WORKER_CAPABILITY` is a hard mode switch. Scoped/read-only
//! workers use narrow typed daemon operations. Trusted local workers run normal
//! positively-bounded coordination commands directly against the canonical
//! graph; every graph commit revalidates the exact capability/fence under lock.
//! Own-task completion remains typed and receipt-backed.

use crate::cli::{Commands, MsgCommands, PiWatchdogCommands};
use crate::commands;
use anyhow::{Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use worksgood::worker_control::{
    WORKER_CONTROL_PROTOCOL, WorkerControlMode, WorkerOperation, WorkerRequestEnvelope,
};

fn control_mode() -> WorkerControlMode {
    std::env::var("WG_WORKER_CONTROL_MODE")
        .ok()
        .and_then(|value| value.parse().ok())
        // Capabilities minted before the visible-mode rollout stay narrow.
        .unwrap_or(WorkerControlMode::Scoped)
}

fn task_is_own(task: &str) -> bool {
    std::env::var("WG_TASK_ID").as_deref() == Ok(task)
}

fn task_matches(task: &str) -> Result<()> {
    let own = std::env::var("WG_TASK_ID").context("worker capability missing WG_TASK_ID")?;
    if task != own {
        anyhow::bail!("worker_control.cross_task_refused: requested={task} capability_task={own}");
    }
    Ok(())
}

fn request_id(operation: &WorkerOperation, capability: &str) -> String {
    if let Some(value) = std::env::var("WG_WORKER_REQUEST_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return value;
    }
    if matches!(operation, WorkerOperation::DoneHandoff { .. }) {
        // Done is a terminal intent, not an ordinary RPC. Reinvoking the CLI
        // after a lost response must reuse the same key; changed flags then
        // conflict against the broker's full-operation CID under that key.
        return format!(
            "intent:{}",
            worksgood::worker_control::token_digest(capability)
        );
    }
    format!("worker:{}", uuid::Uuid::now_v7())
}

fn render_response(
    operation: &WorkerOperation,
    response: commands::service::IpcResponse,
) -> Result<()> {
    if !response.ok {
        anyhow::bail!(
            "{}",
            response
                .error
                .unwrap_or_else(|| "worker control request refused".to_string())
        );
    }
    let data = response.data.unwrap_or(serde_json::Value::Null);
    match operation {
        WorkerOperation::MessageRead { json } | WorkerOperation::MessagePoll { json } => {
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(data.get("messages").unwrap_or(&data))?
                );
            } else {
                let messages = data
                    .get("messages")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if messages.is_empty() {
                    println!(
                        "No unread messages for task '{}'.",
                        std::env::var("WG_TASK_ID").unwrap_or_default()
                    );
                } else {
                    for message in messages {
                        let sender = message
                            .get("sender")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let body = message.get("body").and_then(|v| v.as_str()).unwrap_or("");
                        println!("[{sender}] {body}");
                    }
                }
            }
        }
        WorkerOperation::Show { json }
        | WorkerOperation::Context { json }
        | WorkerOperation::ArtifactList { json } => {
            if *json {
                println!("{}", serde_json::to_string_pretty(&data)?);
            } else {
                // The daemon intentionally returns a scoped value, never a raw
                // graph. A stable structured rendering avoids re-loading the
                // graph merely to reproduce the legacy pretty-printer.
                println!("{}", serde_json::to_string_pretty(&data)?);
            }
        }
        WorkerOperation::DependencyArtifactRead { .. } => {
            if let Some(content) = data.get("content").and_then(|value| value.as_str()) {
                print!("{content}");
            }
        }
        WorkerOperation::Capabilities => {
            if data.get("mode").is_some() {
                if std::env::args().any(|arg| arg == "--json") {
                    println!("{}", serde_json::to_string_pretty(&data)?);
                } else {
                    println!(
                        "Worker control mode: {}",
                        data.get("mode")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                    );
                    println!(
                        "Restrictions: {}",
                        data.get("restrictions")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                    );
                }
            }
        }
        _ => {
            if !data.is_null() {
                println!("{}", serde_json::to_string(&data)?);
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustedCliClass {
    Coordination,
    Privileged,
}

/// Exhaustive compile-time classification of every top-level CLI family.
/// Adding a command cannot silently widen or narrow worker authority: Rust
/// requires this match to be updated. Coordination uses the normal CLI;
/// service/admin, execution management, federation/review, and immutable
/// completion families remain protected.
fn trusted_cli_class(command: &Commands) -> TrustedCliClass {
    match command {
        Commands::Msg {
            command:
                MsgCommands::Send {
                    to: None,
                    body: None,
                    kind,
                    seal: false,
                    store: None,
                    ..
                },
        } if kind == "msg" => TrustedCliClass::Coordination,
        Commands::Msg {
            command: MsgCommands::List { .. } | MsgCommands::Read { .. },
        } => TrustedCliClass::Coordination,
        Commands::Msg {
            command:
                MsgCommands::Poll {
                    as_identity: None,
                    store: None,
                    require_fresh: None,
                    review: false,
                    no_review: false,
                    ..
                },
        } => TrustedCliClass::Coordination,
        Commands::Msg { .. } => TrustedCliClass::Privileged,

        Commands::Reset { .. }
        | Commands::Rescue { .. }
        | Commands::Insert { .. }
        | Commands::Add { .. }
        | Commands::Edit { .. }
        | Commands::Abandon { .. }
        | Commands::Retry { .. }
        | Commands::Recover { .. }
        | Commands::Requeue { .. }
        | Commands::Approve { .. }
        | Commands::Reject { .. }
        | Commands::Claim { .. }
        | Commands::Unclaim { .. }
        | Commands::Pause { .. }
        | Commands::Resume { .. }
        | Commands::Publish { .. }
        | Commands::Wait { .. }
        | Commands::AddDep { .. }
        | Commands::RmDep { .. }
        | Commands::Reclaim { .. }
        | Commands::Ready
        | Commands::Discover { .. }
        | Commands::Blocked { .. }
        | Commands::WhyBlocked { .. }
        | Commands::Check
        | Commands::Cycles
        | Commands::Cron { .. }
        | Commands::List { .. }
        | Commands::Viz { .. }
        | Commands::GraphExport { .. }
        | Commands::Cost { .. }
        | Commands::Coordinate { .. }
        | Commands::Plan { .. }
        | Commands::Reschedule { .. }
        | Commands::Reprioritize { .. }
        | Commands::Impact { .. }
        | Commands::Structure
        | Commands::Bottlenecks
        | Commands::Velocity { .. }
        | Commands::Aging
        | Commands::Forecast
        | Commands::Workload
        | Commands::Resources
        | Commands::CriticalPath
        | Commands::Analyze
        | Commands::Show { .. }
        | Commands::Capabilities
        | Commands::Runs { .. }
        | Commands::Log { .. }
        | Commands::Tokens { .. }
        | Commands::Spend { .. }
        | Commands::Checkpoint { .. }
        | Commands::Assign { .. }
        | Commands::Match { .. }
        | Commands::Heartbeat { .. }
        | Commands::HeartbeatWatch { .. }
        | Commands::Artifact { .. }
        | Commands::Context { .. }
        | Commands::Next { .. }
        | Commands::Trajectory { .. }
        | Commands::Html { .. }
        | Commands::Which { .. }
        | Commands::Status { .. }
        | Commands::Stats
        | Commands::Metrics { .. }
        | Commands::Matrix { .. } => TrustedCliClass::Coordination,

        Commands::Init { .. }
        | Commands::Done { .. }
        | Commands::CompletionObject { .. }
        | Commands::CompletionManifest { .. }
        | Commands::Submit { .. }
        | Commands::Land { .. }
        | Commands::Contract { .. }
        | Commands::Finalize { .. }
        | Commands::Candidate { .. }
        | Commands::MergeResolution { .. }
        | Commands::Fail { .. }
        | Commands::ClassifyFailure { .. }
        | Commands::RecordTelemetry { .. }
        | Commands::ClassifyNoOp { .. }
        | Commands::PiStreamBridge { .. }
        | Commands::PiStreamObserve { .. }
        | Commands::ChatRuntimeWrapper { .. }
        | Commands::Incomplete { .. }
        | Commands::Doctor
        | Commands::Cleanup { .. }
        | Commands::Worktree(..)
        | Commands::Disk(..)
        | Commands::Archive { .. }
        | Commands::Coordinator(..)
        | Commands::Gc { .. }
        | Commands::Trace { .. }
        | Commands::Func { .. }
        | Commands::Replay { .. }
        | Commands::Openrouter { .. }
        | Commands::User { .. }
        | Commands::Chat { .. }
        | Commands::Resource { .. }
        | Commands::Session { .. }
        | Commands::Skill { .. }
        | Commands::PiPlugin { .. }
        | Commands::PiWatchdog { .. }
        | Commands::Agency { .. }
        | Commands::Peer { .. }
        | Commands::Role { .. }
        | Commands::Tradeoff { .. }
        | Commands::Exec { .. }
        | Commands::Agent { .. }
        | Commands::Spawn { .. }
        | Commands::Evaluate { .. }
        | Commands::Evolve { .. }
        | Commands::Profile { .. }
        | Commands::Config { .. }
        | Commands::DeadAgents { .. }
        | Commands::Sweep { .. }
        | Commands::Migrate { .. }
        | Commands::Upgrade { .. }
        | Commands::Agents { .. }
        | Commands::Kill { .. }
        | Commands::Reap { .. }
        | Commands::Service { .. }
        | Commands::Tui { .. }
        | Commands::TuiDump { .. }
        | Commands::Screencast { .. }
        | Commands::Server { .. }
        | Commands::Setup { .. }
        | Commands::Quickstart
        | Commands::DevCheck
        | Commands::AgentGuide
        | Commands::Notify { .. }
        | Commands::Watch { .. }
        | Commands::Telegram { .. }
        | Commands::Endpoints { .. }
        | Commands::Endpoint { .. }
        | Commands::Models { .. }
        | Commands::ModelScout { .. }
        | Commands::Model { .. }
        | Commands::Key { .. }
        | Commands::Login { .. }
        | Commands::Secret { .. }
        | Commands::Identity { .. }
        | Commands::FedNode { .. }
        | Commands::Review { .. }
        | Commands::Provider { .. }
        | Commands::Pilot { .. }
        | Commands::Nex(..)
        | Commands::TuiNex { .. }
        | Commands::TuiPty { .. }
        | Commands::SpawnTask { .. }
        | Commands::WorktreeObserverRun { .. }
        | Commands::WorktreeObserverReconcile { .. }
        | Commands::ClaudeHandler { .. }
        | Commands::CodexHandler { .. }
        | Commands::OpenCodeHandler { .. }
        | Commands::PiHandler { .. }
        | Commands::Executors { .. }
        | Commands::NativeExec { .. }
        | Commands::ApplyPlacement { .. } => TrustedCliClass::Privileged,
    }
}

fn trusted_cli_passthrough(parsed_command: &Commands) -> Result<Option<()>> {
    if trusted_cli_class(parsed_command) != TrustedCliClass::Coordination {
        anyhow::bail!("worker_control.operation_refused: privileged CLI family");
    }
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv
        .iter()
        .any(|arg| arg == "--dir" || arg.starts_with("--dir="))
    {
        anyhow::bail!("worker_control.graph_cli_cross_graph_refused");
    }
    let command = argv
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .unwrap_or("");
    // SAFETY: CLI dispatch is single-threaded at this point. The marker is
    // consumed only by this process's graph commit path.
    unsafe { std::env::set_var("WG_TRUSTED_DIRECT_CLI", command) };
    Ok(None)
}

fn send(operation: WorkerOperation) -> Result<()> {
    let endpoint = PathBuf::from(
        std::env::var_os("WG_WORKER_IPC")
            .context("worker capability missing WG_WORKER_IPC endpoint")?,
    );
    let capability =
        std::env::var("WG_WORKER_CAPABILITY").context("worker capability token is unavailable")?;
    let protocol = std::env::var("WG_WORKER_CONTROL_PROTOCOL")
        .ok()
        .filter(|protocol| worksgood::worker_control::is_supported_worker_protocol(protocol))
        .unwrap_or_else(|| WORKER_CONTROL_PROTOCOL.to_string());
    let envelope = WorkerRequestEnvelope {
        protocol,
        request_id: request_id(&operation, &capability),
        capability,
        operation: operation.clone(),
    };
    let response = commands::service::send_worker_request_endpoint(&endpoint, &envelope)?;
    render_response(&operation, response)
}

/// Return `Ok(None)` outside worker mode, `Ok(Some(()))` when handled.
pub fn maybe_run(command: &Commands, json: bool, resolved_dir: &Path) -> Result<Option<()>> {
    if std::env::var_os("WG_WORKER_CAPABILITY").is_none() {
        if worksgood::worker_control::is_managed_worker_process(resolved_dir)? {
            anyhow::bail!(
                "worker_control.capability_required_for_managed_process: stripping WG_* does not grant operator authority"
            );
        }
        return Ok(None);
    }
    let mode = control_mode();
    if mode != WorkerControlMode::Trusted && std::env::var_os("WG_DIR").is_some() {
        anyhow::bail!("worker_control.raw_graph_environment_refused: WG_DIR must not be present");
    }
    if mode == WorkerControlMode::Trusted && std::env::var_os("WG_DIR").is_none() {
        anyhow::bail!("worker_control.trusted_graph_environment_missing");
    }
    let operation = match command {
        Commands::Capabilities => Some(WorkerOperation::Capabilities),
        Commands::Show { id } if mode == WorkerControlMode::Trusted && !task_is_own(id) => {
            return trusted_cli_passthrough(command);
        }
        Commands::Show { id } => {
            task_matches(id)?;
            Some(WorkerOperation::Show { json })
        }
        Commands::Context { task, dependents }
            if mode == WorkerControlMode::Trusted && (!task_is_own(task) || *dependents) =>
        {
            return trusted_cli_passthrough(command);
        }
        Commands::Context { task, dependents } => {
            task_matches(task)?;
            if *dependents {
                anyhow::bail!("worker_control.graph_enumeration_refused");
            }
            Some(WorkerOperation::Context { json })
        }
        Commands::Log {
            id,
            message,
            actor,
            list,
            agent,
            operations,
        } if mode == WorkerControlMode::Trusted
            && (id.as_deref().is_some_and(|id| !task_is_own(id))
                || *operations
                || *agent
                || *list
                || actor.is_some()) =>
        {
            return trusted_cli_passthrough(command);
        }
        Commands::Log {
            id,
            message,
            actor,
            list,
            agent,
            operations,
        } => {
            if *operations || *agent || *list || actor.is_some() {
                anyhow::bail!("worker_control.log_scope_refused");
            }
            let id = id.as_deref().context("worker log requires own task id")?;
            task_matches(id)?;
            Some(WorkerOperation::Log {
                message: message.clone().context("worker log requires a message")?,
            })
        }
        Commands::Artifact { task, path, remove } => {
            task_matches(task)?;
            match (path, remove) {
                (None, _) => Some(WorkerOperation::ArtifactList { json }),
                (Some(path), false) => Some(WorkerOperation::ArtifactAdd { path: path.clone() }),
                (Some(path), true) => Some(WorkerOperation::ArtifactRemove { path: path.clone() }),
            }
        }
        Commands::Msg {
            command: msg_command,
        } if mode == WorkerControlMode::Trusted
            && match msg_command {
                MsgCommands::Read { task_id, .. } | MsgCommands::List { task_id } => {
                    !task_is_own(task_id)
                }
                MsgCommands::Poll {
                    task_id,
                    as_identity,
                    ..
                } => as_identity.is_none() && task_id.as_deref().is_some_and(|id| !task_is_own(id)),
                MsgCommands::Send { task_id, to, .. } => {
                    to.is_none() && task_id.as_deref().is_some_and(|id| !task_is_own(id))
                }
            } =>
        {
            return trusted_cli_passthrough(command);
        }
        Commands::Msg { command } => match command {
            MsgCommands::Read { task_id, .. } => {
                task_matches(task_id)?;
                Some(WorkerOperation::MessageRead { json })
            }
            MsgCommands::List { task_id } => {
                task_matches(task_id)?;
                Some(WorkerOperation::MessageRead { json })
            }
            MsgCommands::Poll {
                task_id,
                as_identity,
                ..
            } => {
                if as_identity.is_some() {
                    anyhow::bail!("worker_control.federated_poll_refused");
                }
                let task_id = task_id
                    .as_deref()
                    .context("worker poll requires own task id")?;
                task_matches(task_id)?;
                Some(WorkerOperation::MessagePoll { json })
            }
            MsgCommands::Send {
                task_id,
                message,
                stdin,
                to,
                priority,
                ..
            } => {
                if to.is_some() {
                    anyhow::bail!("worker_control.cross_graph_send_refused");
                }
                let task_id = task_id
                    .as_deref()
                    .context("worker send requires own task id")?;
                task_matches(task_id)?;
                let body = if *stdin {
                    let mut body = String::new();
                    std::io::stdin().read_to_string(&mut body)?;
                    body.trim_end().to_string()
                } else {
                    message.clone().context("worker send requires a message")?
                };
                Some(WorkerOperation::MessageSend {
                    body,
                    priority: priority.clone(),
                })
            }
        },
        Commands::Heartbeat { agent, check, .. } => {
            if *check {
                anyhow::bail!("worker_control.agent_enumeration_refused");
            }
            let own =
                std::env::var("WG_AGENT_ID").context("worker capability missing WG_AGENT_ID")?;
            if agent.as_deref() != Some(own.as_str()) {
                anyhow::bail!("worker_control.cross_agent_refused");
            }
            Some(WorkerOperation::Heartbeat)
        }
        Commands::HeartbeatWatch {
            agent,
            interval_seconds,
            supervised_pid,
        } => {
            let own =
                std::env::var("WG_AGENT_ID").context("worker capability missing WG_AGENT_ID")?;
            if agent != &own {
                anyhow::bail!("worker_control.cross_agent_refused");
            }
            loop {
                send(WorkerOperation::Heartbeat)?;
                if let Some(pid) = supervised_pid {
                    #[cfg(unix)]
                    if unsafe { libc::kill(*pid as i32, 0) } != 0 {
                        break;
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs((*interval_seconds).max(1)));
            }
            return Ok(Some(()));
        }
        Commands::Checkpoint {
            task,
            summary,
            files,
            list,
            ..
        } => {
            task_matches(task)?;
            if *list {
                anyhow::bail!("worker_control.checkpoint_enumeration_refused");
            }
            Some(WorkerOperation::Checkpoint {
                summary: summary.clone(),
                files: files.clone(),
            })
        }
        Commands::Wait {
            id,
            until,
            checkpoint,
        } => {
            task_matches(id)?;
            Some(WorkerOperation::Wait {
                until: until.clone(),
                checkpoint: checkpoint.clone(),
            })
        }
        Commands::CompletionObject {
            path,
            media_type,
            evidence_kind,
        } => Some(WorkerOperation::CompletionObject {
            path: path.to_string_lossy().into_owned(),
            media_type: media_type.clone(),
            evidence_kind: evidence_kind.clone(),
        }),
        Commands::CompletionManifest {
            id,
            summary,
            output_refs,
            evidence_refs,
            git,
            source_revision,
        } => {
            task_matches(id)?;
            let summary = std::fs::read_to_string(summary)
                .with_context(|| format!("read worker summary {}", summary.display()))?;
            let outputs = output_refs
                .iter()
                .map(|path| {
                    serde_json::from_slice(
                        &std::fs::read(path)
                            .with_context(|| format!("read output reference {}", path.display()))?,
                    )
                    .with_context(|| format!("parse output reference {}", path.display()))
                })
                .collect::<Result<Vec<_>>>()?;
            let evidence =
                evidence_refs
                    .iter()
                    .map(|path| {
                        serde_json::from_slice(&std::fs::read(path).with_context(|| {
                            format!("read evidence reference {}", path.display())
                        })?)
                        .with_context(|| format!("parse evidence reference {}", path.display()))
                    })
                    .collect::<Result<Vec<_>>>()?;
            Some(WorkerOperation::CompletionManifest {
                summary,
                outputs,
                evidence,
                git: *git,
                source_revision: source_revision.clone(),
            })
        }
        Commands::Submit {
            id,
            manifest,
            summary,
        } => {
            task_matches(id)?;
            Some(WorkerOperation::SubmitCompletion {
                manifest: manifest.to_string_lossy().into_owned(),
                summary: summary.to_string_lossy().into_owned(),
            })
        }
        Commands::Land {
            id,
            integration_ref,
        } => {
            task_matches(id)?;
            Some(WorkerOperation::Land {
                integration_ref: integration_ref.clone(),
            })
        }
        Commands::Done {
            id,
            converged,
            full_smoke,
            skip_verify,
            ignore_unmerged_worktree,
            skip_smoke,
        } => {
            task_matches(id)?;
            if *skip_verify || *ignore_unmerged_worktree || *skip_smoke {
                anyhow::bail!("worker_control.done_bypass_refused");
            }
            Some(WorkerOperation::DoneHandoff {
                converged: *converged,
                full_smoke: *full_smoke,
            })
        }
        Commands::Fail {
            id,
            reason,
            class,
            eval_reject,
        } => {
            task_matches(id)?;
            if *eval_reject {
                anyhow::bail!("worker_control.eval_terminalization_refused");
            }
            Some(WorkerOperation::FailHandoff {
                reason: reason
                    .clone()
                    .unwrap_or_else(|| "Worker reported failure".to_string()),
                class: class.clone(),
            })
        }
        Commands::Finalize { .. } => {
            anyhow::bail!("worker_control.legacy_finalization_retired")
        }
        Commands::PiWatchdog { command } => match command {
            PiWatchdogCommands::Bootstrap {
                id,
                agent_dir,
                pid,
                wrapper_pid,
            } => {
                task_matches(id)?;
                Some(WorkerOperation::PiWatchdogBootstrap {
                    agent_dir: agent_dir.to_string_lossy().into_owned(),
                    pid: *pid,
                    wrapper_pid: *wrapper_pid,
                })
            }
            PiWatchdogCommands::ProcessExit { id, exit_code, pid } => {
                task_matches(id)?;
                Some(WorkerOperation::PiWatchdogProcessExit {
                    exit_code: *exit_code,
                    pid: *pid,
                })
            }
            PiWatchdogCommands::Status { id } => {
                task_matches(id)?;
                Some(WorkerOperation::Show { json })
            }
            _ => anyhow::bail!("worker_control.operator_watchdog_action_refused"),
        },
        Commands::RecordTelemetry {
            task,
            raw_stream,
            exit_code,
            executor,
            route,
            ..
        } => {
            task_matches(task)?;
            Some(WorkerOperation::RecordTelemetry {
                raw_stream: raw_stream.clone(),
                exit_code: *exit_code,
                executor: executor.clone(),
                route: route.clone(),
            })
        }
        // These commands operate only on explicitly named files inside the
        // worker's own runtime directory and do not resolve a graph. Execute
        // them before main's graph resolver/usage logger.
        Commands::PiStreamBridge {
            agent_dir,
            exit_code,
            follow_pid,
        } => {
            commands::pi_stream_bridge::run(Path::new(agent_dir), *exit_code, *follow_pid)?;
            return Ok(Some(()));
        }
        Commands::PiStreamObserve {
            agent_dir,
            follow_pid,
        } => {
            commands::pi_stream_bridge::observe_live(Path::new(agent_dir), *follow_pid)?;
            return Ok(Some(()));
        }
        Commands::ClassifyFailure {
            raw_stream,
            exit_code,
            executor,
            route,
            json,
        } => {
            commands::classify_failure::run(
                raw_stream.as_deref(),
                *exit_code,
                executor.as_deref(),
                route.as_deref(),
                *json,
            )?;
            return Ok(Some(()));
        }
        Commands::ClassifyNoOp {
            output_log,
            clean_exit,
            artifacts_empty,
            has_file_writes,
        } => {
            commands::classify_failure::run_no_op(
                output_log,
                *clean_exit,
                *artifacts_empty,
                *has_file_writes,
            )?;
            return Ok(Some(()));
        }
        _ if mode == WorkerControlMode::Trusted => return trusted_cli_passthrough(command),
        _ => anyhow::bail!(
            "worker_control.operation_refused: this command requires operator/graph authority; effective mode={mode}; run `wg capabilities`"
        ),
    };

    if let Some(operation) = operation {
        send(operation)?;
    }
    Ok(Some(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_cli_class_is_exhaustive_and_protects_management() {
        assert_eq!(
            trusted_cli_class(&Commands::Check),
            TrustedCliClass::Coordination
        );
        assert_eq!(
            trusted_cli_class(&Commands::Quickstart),
            TrustedCliClass::Privileged
        );
        assert_eq!(
            trusted_cli_class(&Commands::Stats),
            TrustedCliClass::Coordination
        );
    }
}
