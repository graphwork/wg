//! Worker-side CLI adapter for the attempt-scoped daemon capability channel.
//!
//! Presence of `WG_WORKER_CAPABILITY` is a hard mode switch: commands are
//! either translated to a typed worker operation or refused. There is no
//! filesystem graph fallback, even when cwd happens to contain a guessed `.wg`.

use crate::cli::{Commands, MsgCommands, PiCompactionKickCommands, PiWatchdogCommands};
use crate::commands;
use anyhow::{Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use worksgood::worker_control::{WORKER_CONTROL_PROTOCOL, WorkerOperation, WorkerRequestEnvelope};

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
    match operation {
        WorkerOperation::PiCompactionKickAuthorize {
            compaction_entry_id,
            ..
        } => format!("pi-kick-authorize:{}", short_digest(compaction_entry_id)),
        WorkerOperation::PiCompactionKickPermit { action_id } => {
            format!("pi-kick-permit:{}", short_digest(action_id))
        }
        WorkerOperation::PiCompactionKickAck { action_id, .. } => {
            format!("pi-kick-ack:{}", short_digest(action_id))
        }
        WorkerOperation::PiCompactionKickCancel { action_id, reason } => format!(
            "pi-kick-cancel:{}:{}",
            short_digest(action_id),
            short_digest(reason)
        ),
        WorkerOperation::PiCompactionKickSettle { action_id } => {
            format!("pi-kick-settle:{}", short_digest(action_id))
        }
        WorkerOperation::PiCompactionKickAbortAck { action_id } => {
            format!("pi-kick-abort-ack:{}", short_digest(action_id))
        }
        WorkerOperation::PiCompactionKickEffectBegin {
            action_id,
            tool_call_id,
        } => format!(
            "pi-kick-effect-begin:{}:{}",
            short_digest(action_id),
            short_digest(tool_call_id)
        ),
        WorkerOperation::PiCompactionKickEffectEnd {
            action_id,
            tool_call_id,
        } => format!(
            "pi-kick-effect-end:{}:{}",
            short_digest(action_id),
            short_digest(tool_call_id)
        ),
        _ => format!("worker:{}", uuid::Uuid::now_v7()),
    }
}

fn short_digest(value: &str) -> String {
    let digest = blake3::hash(value.as_bytes()).to_hex().to_string();
    digest[..24].to_string()
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
                println!("{}", serde_json::to_string_pretty(&data)?);
            } else {
                let messages = data.as_array().cloned().unwrap_or_default();
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
        _ => {
            if !data.is_null() {
                println!("{}", serde_json::to_string(&data)?);
            }
        }
    }
    Ok(())
}

fn send(operation: WorkerOperation) -> Result<()> {
    let endpoint = PathBuf::from(
        std::env::var_os("WG_WORKER_IPC")
            .context("worker capability missing WG_WORKER_IPC endpoint")?,
    );
    let capability =
        std::env::var("WG_WORKER_CAPABILITY").context("worker capability token is unavailable")?;
    let envelope = WorkerRequestEnvelope {
        protocol: WORKER_CONTROL_PROTOCOL.to_string(),
        request_id: request_id(&operation, &capability),
        capability,
        operation: operation.clone(),
    };
    let response = commands::service::send_worker_request_endpoint(&endpoint, &envelope)?;
    render_response(&operation, response)
}

/// Return `Ok(None)` outside worker mode, `Ok(Some(()))` when handled.
pub fn maybe_run(command: &Commands, json: bool) -> Result<Option<()>> {
    if std::env::var_os("WG_WORKER_CAPABILITY").is_none() {
        return Ok(None);
    }
    if std::env::var_os("WG_DIR").is_some() {
        anyhow::bail!("worker_control.raw_graph_environment_refused: WG_DIR must not be present");
    }

    let operation = match command {
        Commands::Show { id } => {
            task_matches(id)?;
            Some(WorkerOperation::Show { json })
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
            PiWatchdogCommands::CompactionKick { command } => {
                let operation = match command {
                    PiCompactionKickCommands::Authorize {
                        id,
                        reason,
                        will_retry,
                        compaction_entry_id,
                        compaction_parent_id,
                        session_id,
                        session_file,
                        session_leaf_id,
                        pid,
                        provider,
                        model,
                        reasoning,
                        plugin_compat,
                        quiescent,
                        host_idle,
                        queue_empty,
                        tool_clear,
                    } => {
                        task_matches(id)?;
                        WorkerOperation::PiCompactionKickAuthorize {
                            reason: reason.clone(),
                            will_retry: *will_retry,
                            compaction_entry_id: compaction_entry_id.clone(),
                            compaction_parent_id: compaction_parent_id.clone(),
                            session_id: session_id.clone(),
                            session_file: session_file.clone(),
                            session_leaf_id: session_leaf_id.clone(),
                            pid: *pid,
                            provider: provider.clone(),
                            model: model.clone(),
                            reasoning: reasoning.clone(),
                            plugin_compat: plugin_compat.clone(),
                            quiescent: *quiescent,
                            host_idle: *host_idle,
                            queue_empty: *queue_empty,
                            tool_clear: *tool_clear,
                        }
                    }
                    PiCompactionKickCommands::Permit { id, action } => {
                        task_matches(id)?;
                        WorkerOperation::PiCompactionKickPermit {
                            action_id: action.clone(),
                        }
                    }
                    PiCompactionKickCommands::Ack {
                        id,
                        action,
                        prompt_version,
                        prompt_digest,
                    } => {
                        task_matches(id)?;
                        WorkerOperation::PiCompactionKickAck {
                            action_id: action.clone(),
                            prompt_version: prompt_version.clone(),
                            prompt_digest: prompt_digest.clone(),
                        }
                    }
                    PiCompactionKickCommands::Cancel { id, action, reason } => {
                        task_matches(id)?;
                        WorkerOperation::PiCompactionKickCancel {
                            action_id: action.clone(),
                            reason: reason.clone(),
                        }
                    }
                    PiCompactionKickCommands::Settle { id, action } => {
                        task_matches(id)?;
                        WorkerOperation::PiCompactionKickSettle {
                            action_id: action.clone(),
                        }
                    }
                    PiCompactionKickCommands::AbortAck { id, action } => {
                        task_matches(id)?;
                        WorkerOperation::PiCompactionKickAbortAck {
                            action_id: action.clone(),
                        }
                    }
                    PiCompactionKickCommands::EffectBegin {
                        id,
                        action,
                        tool_call,
                    } => {
                        task_matches(id)?;
                        WorkerOperation::PiCompactionKickEffectBegin {
                            action_id: action.clone(),
                            tool_call_id: tool_call.clone(),
                        }
                    }
                    PiCompactionKickCommands::EffectEnd {
                        id,
                        action,
                        tool_call,
                    } => {
                        task_matches(id)?;
                        WorkerOperation::PiCompactionKickEffectEnd {
                            action_id: action.clone(),
                            tool_call_id: tool_call.clone(),
                        }
                    }
                };
                Some(operation)
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
        _ => anyhow::bail!(
            "worker_control.operation_refused: this command requires operator/graph authority"
        ),
    };

    if let Some(operation) = operation {
        send(operation)?;
    }
    Ok(Some(()))
}
