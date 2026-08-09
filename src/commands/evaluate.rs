//! Read-only compatibility views for historical evaluation records.
//!
//! Evaluation mutation, rollout mutation, FLIP execution, gate decisions, and
//! evaluator-driven source transitions are retired from the public command
//! surface (`src/main.rs` rejects them deliberately). Ordinary completion owns
//! source-bound review receipts instead. Keep this module only while supported
//! graphs may carry historical evaluation records and rollout status.
//!
//! Removal condition: historical evaluation records have a versioned export or
//! migration and the CLI no longer promises `evaluate show`/rollout status.

use anyhow::Result;
use std::path::Path;
use worksgood::agency::load_all_evaluations_or_warn;

fn byte_prefix(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let boundary = value
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(value.len()))
        .take_while(|index| *index <= max_bytes)
        .last()
        .unwrap_or(0);
    &value[..boundary]
}

fn print_rollout_status(
    status: &worksgood::evaluation::rollout::RolloutStatus,
    json: bool,
) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(status)?);
    } else {
        println!("Pi evaluation rollout: {}", status.stage);
        println!("  mode: {}", status.mode);
        println!("  auto_evaluate: {}", status.auto_evaluate);
        println!(
            "  eval_gate_all: {} (historical compatibility only)",
            status.eval_gate_all
        );
        println!(
            "  global deep-readonly FLIP selection: {}",
            status.global_flip_enabled
        );
        println!("  canary/observation evidence: {}", status.evidence.len());
        println!("  rollbacks: {}", status.rollback_count);
        println!("  evidence: {}", status.state_path);
    }
    Ok(())
}

pub fn rollout_status(dir: &Path, json: bool) -> Result<()> {
    let status = worksgood::evaluation::rollout::status(dir)?;
    print_rollout_status(&status, json)
}

/// Show historical evaluation records with optional filters.
///
/// This is observation only. It cannot create a verdict, mutate a source task,
/// or synthesize an evaluator graph row.
pub fn run_show(
    dir: &Path,
    task_filter: Option<&str>,
    agent_filter: Option<&str>,
    source_filter: Option<&str>,
    limit: Option<usize>,
    json: bool,
    task_detail: Option<&str>,
) -> Result<()> {
    let evals_dir = dir.join("agency").join("evaluations");

    if let Some(tid) = task_detail {
        let mut task_evals = load_all_evaluations_or_warn(&evals_dir);
        task_evals
            .retain(|evaluation| evaluation.task_id == tid || evaluation.task_id.starts_with(tid));
        task_evals.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        if json {
            let out = serde_json::json!({
                "task_id": tid,
                "evaluations": task_evals,
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else {
            println!("=== Evaluations for task '{tid}' ===\n");
            println!("Evaluations ({}):", task_evals.len());
            if task_evals.is_empty() {
                println!("  (none)");
            } else {
                for evaluation in &task_evals {
                    println!(
                        "  Score: {:.3}  Source: {}  Agent: {}  {}",
                        evaluation.score,
                        evaluation.source,
                        if evaluation.agent_id.is_empty() {
                            "-"
                        } else {
                            byte_prefix(&evaluation.agent_id, 10)
                        },
                        evaluation.timestamp
                    );
                    for (dimension, value) in &evaluation.dimensions {
                        println!("    {dimension}: {value:.3}");
                    }
                }
            }
        }
        return Ok(());
    }

    let mut evaluations = load_all_evaluations_or_warn(&evals_dir);
    if let Some(task_prefix) = task_filter {
        evaluations.retain(|evaluation| evaluation.task_id.starts_with(task_prefix));
    }
    if let Some(agent_prefix) = agent_filter {
        evaluations.retain(|evaluation| evaluation.agent_id.starts_with(agent_prefix));
    }
    if let Some(source_pattern) = source_filter {
        if let Some((prefix, suffix)) = source_pattern.split_once('*') {
            evaluations.retain(|evaluation| {
                evaluation.source.starts_with(prefix) && evaluation.source.ends_with(suffix)
            });
        } else {
            evaluations.retain(|evaluation| evaluation.source == source_pattern);
        }
    }
    evaluations.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    if let Some(limit) = limit {
        evaluations.truncate(limit);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&evaluations)?);
    } else if evaluations.is_empty() {
        println!("No evaluations found.");
    } else {
        println!(
            "{:<20} {:>5}  {:<16} {:<12} Timestamp",
            "Task", "Score", "Source", "Agent"
        );
        println!("{}", "─".repeat(75));
        for evaluation in &evaluations {
            let agent = if evaluation.agent_id.is_empty() {
                "-"
            } else {
                byte_prefix(&evaluation.agent_id, 10)
            };
            let task = byte_prefix(&evaluation.task_id, 18);
            let source = byte_prefix(&evaluation.source, 14);
            println!(
                "{task:<20} {:>5.2}  {source:<16} {agent:<12} {}",
                evaluation.score, evaluation.timestamp
            );
        }
        println!("\n{} evaluation(s)", evaluations.len());
    }

    Ok(())
}
