//! Internal `wg classify-failure` subcommand.
//!
//! Reads the raw_stream.jsonl produced by the wrapper and prints the
//! FailureClass kebab string to stdout. Called by the wrapper script
//! before invoking `wg fail --class <CLASS>`. Hidden from user-facing help.

use anyhow::{Context, Result};
use std::path::Path;
use worksgood::dispatch::plan::ExecutorKind;
use worksgood::graph::{FailureClass, FailureReason, FailureSignal};

use super::spawn::raw_stream_classifier::{
    TerminalStreamState, classify_from_raw_stream, classify_no_operational_output,
    classify_signal_from_raw_stream, classify_terminal_from_raw_stream, infer_executor,
};

pub fn run(
    raw_stream: Option<&str>,
    exit_code: i32,
    executor: Option<&str>,
    route: Option<&str>,
    json: bool,
    terminal: bool,
) -> Result<()> {
    let executor = executor
        .and_then(ExecutorKind::from_str)
        .or_else(|| raw_stream.map(Path::new).map(infer_executor))
        .unwrap_or_default();
    if terminal {
        let projection = raw_stream
            .map(|path| {
                classify_terminal_from_raw_stream(
                    Path::new(path),
                    Path::new(path)
                        .parent()
                        .map(|parent| parent.join("output.log"))
                        .as_deref(),
                    exit_code,
                    executor,
                    route.map(str::to_string),
                )
            })
            .unwrap_or_else(|| {
                super::spawn::raw_stream_classifier::terminal_without_stream(
                    exit_code,
                    executor,
                    route.map(str::to_string),
                )
            });
        if json {
            println!("{}", serde_json::to_string(&projection)?);
        } else {
            println!("{}", projection.state.as_str());
        }
        return Ok(());
    }

    let class = raw_stream
        .map(|path| classify_from_raw_stream(Path::new(path), exit_code))
        .unwrap_or_else(|| {
            if exit_code == 124 {
                FailureClass::AgentHardTimeout
            } else {
                FailureClass::AgentExitNonzero
            }
        });
    if json {
        let signal = raw_stream
            .map(|path| {
                classify_signal_from_raw_stream(
                    Path::new(path),
                    Path::new(path)
                        .parent()
                        .map(|parent| parent.join("output.log"))
                        .as_deref(),
                    exit_code,
                    executor,
                    route.map(str::to_string),
                )
            })
            .unwrap_or_else(|| fallback_signal(class, executor, route));
        println!("{}", serde_json::to_string(&signal)?);
    } else {
        println!("{}", class);
    }
    Ok(())
}

fn fallback_signal(
    class: FailureClass,
    executor: ExecutorKind,
    route: Option<&str>,
) -> FailureSignal {
    let reason = match class {
        FailureClass::ApiError429RateLimit => FailureReason::RateLimit,
        FailureClass::ApiError5xxTransient => FailureReason::Transient5xx,
        FailureClass::ApiError400Document | FailureClass::ExecutorConfig => FailureReason::Hard,
        FailureClass::AgentHardTimeout => FailureReason::HardTimeout,
        FailureClass::ResourceExhaustedDisk => FailureReason::Disk,
        _ => FailureReason::Unknown,
    };
    FailureSignal {
        reason,
        confidence: 0.2,
        executor,
        route: route.map(str::to_string),
        detected_at_ms: chrono::Utc::now().timestamp_millis(),
        ..Default::default()
    }
}

pub fn run_record(
    dir: &Path,
    task_id: &str,
    raw_stream: Option<&str>,
    exit_code: i32,
    executor: Option<&str>,
    route: Option<&str>,
    json: bool,
) -> Result<()> {
    let registry = worksgood::service::registry::AgentRegistry::load(dir).ok();
    let agent = registry
        .as_ref()
        .and_then(|registry| registry.get_agent_by_task(task_id));
    let raw_path = raw_stream.map(std::path::PathBuf::from).or_else(|| {
        agent.map(|agent| {
            Path::new(&agent.output_file)
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("raw_stream.jsonl")
        })
    });
    let executor = executor
        .and_then(ExecutorKind::from_str)
        .or_else(|| agent.and_then(|agent| ExecutorKind::from_str(&agent.executor)))
        .or_else(|| raw_path.as_deref().map(infer_executor))
        .unwrap_or_default();
    let route = route
        .map(str::to_string)
        .or_else(|| agent.and_then(|agent| agent.model.clone()));
    let terminal = raw_path.as_deref().map(|raw| {
        classify_terminal_from_raw_stream(
            raw,
            raw.parent()
                .map(|parent| parent.join("output.log"))
                .as_deref(),
            exit_code,
            executor,
            route.clone(),
        )
    });
    let signal = raw_path
        .as_deref()
        .map(|raw| {
            classify_signal_from_raw_stream(
                raw,
                raw.parent()
                    .map(|parent| parent.join("output.log"))
                    .as_deref(),
                exit_code,
                executor,
                route.clone(),
            )
        })
        .unwrap_or_else(|| {
            fallback_signal(
                if exit_code == 124 {
                    FailureClass::AgentHardTimeout
                } else {
                    FailureClass::AgentExitNonzero
                },
                executor,
                route.as_deref(),
            )
        });

    // Completed, finalization-blocked, and ambiguous receipts are not failed
    // attempts. In particular a non-zero wrapper exit after finalization must
    // not manufacture provider telemetry from incidental timeout prose.
    if terminal.as_ref().is_some_and(|projection| {
        matches!(
            projection.state,
            TerminalStreamState::Completed
                | TerminalStreamState::FinalizationBlocked
                | TerminalStreamState::Ambiguous
        )
    }) || (exit_code == 0 && signal.reason == FailureReason::Unknown)
    {
        if json {
            println!("{}", serde_json::to_string(&signal)?);
        }
        return Ok(());
    }

    let graph_path = super::graph_path(dir);
    let mut attempt = 1;
    let persisted = signal.clone();
    worksgood::parser::modify_graph(&graph_path, |graph| {
        let Some(task) = graph.get_task_mut(task_id) else {
            return false;
        };
        attempt = if task.status == worksgood::graph::Status::Failed {
            task.retry_count.max(1)
        } else {
            task.retry_count.saturating_add(1)
        };
        task.failure_signal = Some(persisted.clone());
        true
    })
    .with_context(|| format!("persist telemetry signal for task '{task_id}'"))?;
    worksgood::telemetry::append_record(
        dir,
        worksgood::telemetry::TelemetryRecord::new(task_id, attempt, signal.clone()),
    )?;
    if json {
        println!("{}", serde_json::to_string(&signal)?);
    }
    Ok(())
}

/// Classify a NoOperationalOutput (guardrail G4) run from the observable
/// signals gathered by the wrapper. Reads the agent's output.log to derive
/// `output_log_nonempty` AND scans it for filesystem-mutation tokens (the
/// `output_log_has_mutations` signal) which is OR'd into the wrapper-supplied
/// `has_file_writes`. Prints `no-operational-output` when the signature
/// matches, or `none` otherwise.
pub fn run_no_op(
    output_log: &str,
    clean_exit: bool,
    artifacts_empty: bool,
    has_file_writes: bool,
) -> Result<()> {
    use super::spawn::raw_stream_classifier::output_log_has_mutations;
    let content = std::fs::read_to_string(output_log).unwrap_or_default();
    let output_log_nonempty = !content.trim().is_empty();
    // Either the wrapper's git-status signal OR an output.log mutation token
    // counts as "the agent acted" — both satisfy G4's has_file_writes.
    let effective_has_file_writes = has_file_writes || output_log_has_mutations(&content);
    let class = classify_no_operational_output(
        clean_exit,
        artifacts_empty,
        effective_has_file_writes,
        output_log_nonempty,
    );
    match class {
        Some(c) => println!("{}", c),
        None => println!("none"),
    }
    Ok(())
}
