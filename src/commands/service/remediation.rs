//! Self-healing remediation: diagnose failed tasks and create remediation tasks.
//!
//! When a task fails, the coordinator can automatically:
//! 1. Diagnose the failure via a lightweight LLM call
//! 2. Categorise the failure (transient, build, context overflow, etc.)
//! 3. Create a `.remediate-{task}` task to fix the underlying issue, OR
//! 4. Retry transient failures with exponential backoff, OR
//! 5. Escalate unfixable problems to a human

use anyhow::Result;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

use workgraph::config::{Config, DispatchRole};
use workgraph::graph::{is_system_task, LogEntry, Status, Task, WaitCondition, WaitSpec, WorkGraph};
use workgraph::parser::mutate_graph;

use super::triage::read_truncated_log;

// ── Failure taxonomy ────────────────────────────────────────────────────

/// Category of failure diagnosed by the LLM.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    /// Temporary issue (rate limit, network blip, OOM) — safe to retry
    Transient,
    /// Agent ran out of context window
    ContextOverflow,
    /// Code does not compile / tests fail
    BuildFailure,
    /// Missing dependency, tool, or environment setup
    MissingDep,
    /// Agent misunderstood the task or went off-track
    AgentConfusion,
    /// Fundamentally broken — needs human intervention
    Unfixable,
}

impl std::fmt::Display for FailureCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient => write!(f, "transient"),
            Self::ContextOverflow => write!(f, "context_overflow"),
            Self::BuildFailure => write!(f, "build_failure"),
            Self::MissingDep => write!(f, "missing_dep"),
            Self::AgentConfusion => write!(f, "agent_confusion"),
            Self::Unfixable => write!(f, "unfixable"),
        }
    }
}

/// Result of diagnosing a task failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnosis {
    pub category: FailureCategory,
    pub confidence: f64,
    pub summary: String,
    pub suggested_fix: Option<String>,
}

// ── Prompt & LLM call ───────────────────────────────────────────────────

fn build_diagnosis_prompt(task: &Task, log_tail: &str) -> String {
    let failure_reason = task
        .failure_reason
        .as_deref()
        .unwrap_or("(no failure reason recorded)");

    format!(
        r#"You are a failure diagnostician for an automated task system.

Analyze this failed task and classify the failure.

## Task
- ID: {id}
- Title: {title}
- Description: {desc}
- Failure reason: {reason}
- Retry count: {retries}
- Remediation count: {remediations}

## Agent output (tail)
```
{log}
```

## Instructions
Classify the failure into exactly ONE category:
- transient: temporary issue (rate limit, network error, OOM kill, timeout) — safe to retry
- context_overflow: agent ran out of context window
- build_failure: code doesn't compile or tests fail
- missing_dep: missing dependency, tool, or environment setup
- agent_confusion: agent misunderstood the task or went off-track
- unfixable: fundamentally broken, needs human intervention

Respond with ONLY this JSON (no markdown fences, no extra text):
{{
  "category": "<one of the above>",
  "confidence": <0.0 to 1.0>,
  "summary": "<one-sentence explanation>",
  "suggested_fix": "<brief fix suggestion or null>"
}}"#,
        id = task.id,
        title = task.title,
        desc = task.description.as_deref().unwrap_or("(none)"),
        reason = failure_reason,
        retries = task.retry_count,
        remediations = task.remediation_count,
        log = log_tail,
    )
}

/// Extract JSON from potentially noisy LLM output (may have markdown fences).
fn extract_json(text: &str) -> Option<&str> {
    if let Some(start) = text.find("```json") {
        let after_fence = &text[start + 7..];
        if let Some(end) = after_fence.find("```") {
            return Some(after_fence[..end].trim());
        }
    }
    if let Some(start) = text.find("```") {
        let after_fence = &text[start + 3..];
        if let Some(end) = after_fence.find("```") {
            let inner = after_fence[..end].trim();
            if inner.starts_with('{') {
                return Some(inner);
            }
        }
    }
    let trimmed = text.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Some(trimmed);
    }
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            if end > start {
                return Some(&text[start..=end]);
            }
        }
    }
    None
}

fn diagnose_failure(config: &Config, task: &Task, log_tail: &str) -> Option<Diagnosis> {
    let prompt = build_diagnosis_prompt(task, log_tail);
    let timeout_secs = 30;

    let result = workgraph::service::llm::run_lightweight_llm_call(
        config,
        DispatchRole::Diagnostician,
        &prompt,
        timeout_secs,
    );

    match result {
        Ok(llm_result) => {
            let json_str = extract_json(&llm_result.text)?;
            serde_json::from_str::<Diagnosis>(json_str).ok()
        }
        Err(e) => {
            eprintln!("[remediation] LLM diagnosis call failed: {e}");
            None
        }
    }
}

// ── Eligibility checks ─────────────────────────────────────────────────

fn has_pending_remediation(graph: &WorkGraph, task_id: &str) -> bool {
    let remediation_id = format!(".remediate-{task_id}");
    if let Some(t) = graph.get_task(&remediation_id) {
        matches!(t.status, Status::Open | Status::InProgress | Status::Waiting)
    } else {
        false
    }
}

fn check_remediation_eligibility(
    graph: &WorkGraph,
    task: &Task,
    config: &Config,
) -> Result<(), String> {
    if !config.coordinator.auto_remediate {
        return Err("auto_remediate is disabled".into());
    }
    if is_system_task(&task.id) {
        return Err("system tasks are never remediated".into());
    }
    if task.status != Status::Failed {
        return Err(format!("task is {:?}, not Failed", task.status));
    }
    if task.paused {
        return Err("task is paused".into());
    }
    if has_pending_remediation(graph, &task.id) {
        return Err("remediation task already pending".into());
    }
    let max = config.coordinator.max_remediation_attempts;
    if task.remediation_count >= max {
        return Err(format!(
            "max remediation attempts reached ({}/{})",
            task.remediation_count, max
        ));
    }
    if let Some(ref usage) = task.token_usage {
        let original_tokens = usage.total_input() + usage.output_tokens;
        let budget =
            (original_tokens as f64 * config.coordinator.remediation_budget_multiplier) as u64;
        let spent = task.remediation_count as u64 * original_tokens;
        if spent >= budget {
            return Err(format!(
                "token budget exhausted ({} spent of {} budget)",
                spent, budget
            ));
        }
    }
    Ok(())
}

// ── Remediation actions ─────────────────────────────────────────────────

fn remediation_description(task: &Task, diagnosis: &Diagnosis) -> String {
    let base = format!(
        "Remediate failure in task '{}': {}",
        task.id, diagnosis.summary
    );
    let fix_hint = diagnosis
        .suggested_fix
        .as_deref()
        .unwrap_or("Review the failure and fix the root cause.");

    match diagnosis.category {
        FailureCategory::BuildFailure => format!(
            "{base}\n\n## Instructions\nThe task failed because the code doesn't compile or tests fail.\n\
            Suggested fix: {fix_hint}\n\n\
            Fix the build/test issue. Do NOT re-implement the original task — only fix the compilation or test failure."
        ),
        FailureCategory::MissingDep => format!(
            "{base}\n\n## Instructions\nThe task failed due to a missing dependency or tool.\n\
            Suggested fix: {fix_hint}\n\n\
            Install or configure the missing dependency. Do NOT re-implement the original task."
        ),
        FailureCategory::ContextOverflow => format!(
            "{base}\n\n## Instructions\nThe agent ran out of context window.\n\
            Suggested fix: {fix_hint}\n\n\
            Break the problem into smaller pieces or simplify the approach. \
            Focus on the most critical part first."
        ),
        FailureCategory::AgentConfusion => format!(
            "{base}\n\n## Instructions\nThe agent misunderstood the task.\n\
            Suggested fix: {fix_hint}\n\n\
            Clarify the task requirements and try a different approach."
        ),
        FailureCategory::Transient | FailureCategory::Unfixable => {
            format!("{base}\n\n## Instructions\n{fix_hint}")
        }
    }
}

fn create_remediation_task(graph: &mut WorkGraph, task_id: &str, diagnosis: &Diagnosis) {
    let task = match graph.get_task(task_id) {
        Some(t) => t.clone(),
        None => return,
    };

    let remediation_id = format!(".remediate-{task_id}");
    let description = remediation_description(&task, diagnosis);

    let remediation = Task {
        id: remediation_id.clone(),
        title: format!("Remediate: {}", task.title),
        description: Some(description),
        status: Status::Open,
        assigned: None,
        estimate: None,
        before: vec![task_id.to_string()],
        after: vec![],
        requires: vec![],
        tags: vec!["remediation".to_string()],
        skills: vec![],
        inputs: vec![],
        deliverables: vec![],
        artifacts: vec![],
        exec: None,
        not_before: None,
        created_at: Some(Utc::now().to_rfc3339()),
        started_at: None,
        completed_at: None,
        log: vec![LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("coordinator".to_string()),
            message: format!(
                "Auto-created remediation task (category: {}, confidence: {:.2})",
                diagnosis.category, diagnosis.confidence
            ),
            ..Default::default()
        }],
        retry_count: 0,
        max_retries: Some(1),
        failure_reason: None,
        model: task.model.clone(),
        provider: task.provider.clone(),
        verify_cmd: None,
        verify_prompt: None,
        agent: None,
        loop_iteration: 0,
        cycle_failure_restarts: 0,
        cycle_config: None,
        ready_after: None,
        paused: false,
        visibility: "normal".to_string(),
        context_scope: None,
        exec_mode: None,
        token_usage: None,
        session_id: None,
        wait_condition: None,
        checkpoint: None,
        resurrection_count: 0,
        last_resurrected_at: None,
        iteration_snapshots: vec![],
        remediation_count: 0,
    };

    graph.add_node(workgraph::graph::Node::Task(remediation));

    if let Some(failed_task) = graph.get_task_mut(task_id) {
        failed_task.status = Status::Open;
        failed_task.assigned = None;
        failed_task.failure_reason = None;
        failed_task.remediation_count += 1;
        failed_task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("coordinator".to_string()),
            message: format!(
                "Remediation task '{}' created (attempt {}, category: {})",
                remediation_id, failed_task.remediation_count, diagnosis.category
            ),
            ..Default::default()
        });
        if !failed_task.after.contains(&remediation_id) {
            failed_task.after.push(remediation_id);
        }
    }
}

fn create_transient_retry(graph: &mut WorkGraph, task_id: &str, diagnosis: &Diagnosis) {
    if let Some(task) = graph.get_task_mut(task_id) {
        let backoff_secs = std::cmp::min(30 * 2_i64.pow(task.remediation_count), 300);
        let resume_at = Utc::now() + Duration::seconds(backoff_secs);

        task.status = Status::Waiting;
        task.assigned = None;
        task.failure_reason = None;
        task.remediation_count += 1;
        task.wait_condition = Some(WaitSpec::All(vec![WaitCondition::Timer {
            resume_after: resume_at.to_rfc3339(),
        }]));
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("coordinator".to_string()),
            message: format!(
                "Transient failure — scheduling retry in {}s (attempt {}, reason: {})",
                backoff_secs, task.remediation_count, diagnosis.summary
            ),
            ..Default::default()
        });
    }
}

fn escalate(graph: &mut WorkGraph, task_id: &str, reason: &str) {
    if let Some(task) = graph.get_task_mut(task_id) {
        task.paused = true;
        task.log.push(LogEntry {
            timestamp: Utc::now().to_rfc3339(),
            actor: Some("coordinator".to_string()),
            message: format!("Escalated to human: {reason}"),
            ..Default::default()
        });
    }
}

// ── Main entry point ────────────────────────────────────────────────────

/// Process all failed tasks: diagnose and remediate.
/// Called from the coordinator tick loop (Phase 2.9).
pub fn process_failed_tasks(graph: &mut WorkGraph, config: &Config, dir: &Path) {
    if !config.coordinator.auto_remediate {
        return;
    }

    let graph_path = dir.join("graph.jsonl");
    let max_log_bytes = 20_000;

    let failed_ids: Vec<String> = graph
        .tasks()
        .filter(|t| t.status == Status::Failed && !is_system_task(&t.id) && !t.paused)
        .map(|t| t.id.clone())
        .collect();

    for task_id in &failed_ids {
        let task = match graph.get_task(task_id) {
            Some(t) => t.clone(),
            None => continue,
        };

        if let Err(reason) = check_remediation_eligibility(graph, &task, config) {
            eprintln!("[remediation] skip {task_id}: {reason}");
            continue;
        }

        let output_dir = dir.join("agents").join(task_id);
        let output_file = output_dir.join("output.log");
        let log_tail = if output_file.exists() {
            read_truncated_log(output_file.to_str().unwrap_or(""), max_log_bytes)
        } else {
            "(no output log found)".to_string()
        };

        let diagnosis = match diagnose_failure(config, &task, &log_tail) {
            Some(d) => d,
            None => {
                eprintln!("[remediation] diagnosis failed for {task_id}, escalating");
                escalate(graph, task_id, "LLM diagnosis failed");
                continue;
            }
        };

        if diagnosis.confidence < 0.6 {
            eprintln!(
                "[remediation] low confidence ({:.2}) for {task_id}, escalating",
                diagnosis.confidence
            );
            escalate(
                graph,
                task_id,
                &format!(
                    "Low diagnosis confidence ({:.2}): {}",
                    diagnosis.confidence, diagnosis.summary
                ),
            );
            continue;
        }

        eprintln!(
            "[remediation] {task_id}: {} (confidence: {:.2})",
            diagnosis.category, diagnosis.confidence
        );

        match diagnosis.category {
            FailureCategory::Transient => {
                create_transient_retry(graph, task_id, &diagnosis);
            }
            FailureCategory::Unfixable => {
                escalate(graph, task_id, &diagnosis.summary);
            }
            FailureCategory::BuildFailure
            | FailureCategory::MissingDep
            | FailureCategory::ContextOverflow
            | FailureCategory::AgentConfusion => {
                create_remediation_task(graph, task_id, &diagnosis);
            }
        }
    }

    if !failed_ids.is_empty() {
        if let Err(e) = mutate_graph(&graph_path, |g| -> Result<(), anyhow::Error> {
            for task_id in &failed_ids {
                if let Some(src) = graph.get_task(task_id) {
                    if let Some(dst) = g.get_task_mut(task_id) {
                        *dst = src.clone();
                    }
                }
                let rem_id = format!(".remediate-{task_id}");
                if let Some(src) = graph.get_task(&rem_id) {
                    if g.get_task(&rem_id).is_none() {
                        g.add_node(workgraph::graph::Node::Task(src.clone()));
                    }
                }
            }
            Ok(())
        }) {
            eprintln!("[remediation] failed to persist changes: {e}");
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_failed_task(id: &str) -> Task {
        Task {
            id: id.to_string(),
            title: format!("Test task {id}"),
            description: Some("A test task".to_string()),
            status: Status::Failed,
            failure_reason: Some("compilation error".to_string()),
            remediation_count: 0,
            ..Default::default()
        }
    }

    fn default_config() -> Config {
        let mut config = Config::default();
        config.coordinator.auto_remediate = true;
        config.coordinator.max_remediation_attempts = 3;
        config.coordinator.remediation_budget_multiplier = 2.0;
        config
    }

    #[test]
    fn test_extract_json_raw() {
        let input = r#"{"category": "transient", "confidence": 0.9, "summary": "timeout", "suggested_fix": null}"#;
        assert_eq!(extract_json(input), Some(input));
    }

    #[test]
    fn test_extract_json_fenced() {
        let input = "Here is my analysis:\n```json\n{\"category\": \"build_failure\"}\n```\nDone.";
        assert_eq!(
            extract_json(input),
            Some("{\"category\": \"build_failure\"}")
        );
    }

    #[test]
    fn test_extract_json_generic_fence() {
        let input = "```\n{\"category\": \"transient\"}\n```";
        assert_eq!(extract_json(input), Some("{\"category\": \"transient\"}"));
    }

    #[test]
    fn test_extract_json_noisy() {
        let input = "The failure is: {\"category\": \"missing_dep\", \"confidence\": 0.8, \"summary\": \"missing lib\", \"suggested_fix\": \"install it\"} end";
        let extracted = extract_json(input).unwrap();
        let d: Diagnosis = serde_json::from_str(extracted).unwrap();
        assert_eq!(d.category, FailureCategory::MissingDep);
    }

    #[test]
    fn test_extract_json_none() {
        assert_eq!(extract_json("no json here"), None);
    }

    #[test]
    fn test_diagnosis_deserialization() {
        let json = r#"{"category": "build_failure", "confidence": 0.85, "summary": "cargo build failed", "suggested_fix": "fix the syntax error"}"#;
        let d: Diagnosis = serde_json::from_str(json).unwrap();
        assert_eq!(d.category, FailureCategory::BuildFailure);
        assert!((d.confidence - 0.85).abs() < f64::EPSILON);
        assert_eq!(d.summary, "cargo build failed");
        assert_eq!(d.suggested_fix.as_deref(), Some("fix the syntax error"));
    }

    #[test]
    fn test_diagnosis_null_fix() {
        let json = r#"{"category": "transient", "confidence": 0.95, "summary": "rate limited", "suggested_fix": null}"#;
        let d: Diagnosis = serde_json::from_str(json).unwrap();
        assert_eq!(d.category, FailureCategory::Transient);
        assert!(d.suggested_fix.is_none());
    }

    #[test]
    fn test_eligibility_disabled() {
        let graph = WorkGraph::default();
        let task = make_failed_task("t1");
        let mut config = default_config();
        config.coordinator.auto_remediate = false;
        assert!(check_remediation_eligibility(&graph, &task, &config).is_err());
    }

    #[test]
    fn test_eligibility_system_task() {
        let graph = WorkGraph::default();
        let task = make_failed_task(".system-task");
        let config = default_config();
        assert!(check_remediation_eligibility(&graph, &task, &config).is_err());
    }

    #[test]
    fn test_eligibility_not_failed() {
        let graph = WorkGraph::default();
        let mut task = make_failed_task("t1");
        task.status = Status::Open;
        let config = default_config();
        assert!(check_remediation_eligibility(&graph, &task, &config).is_err());
    }

    #[test]
    fn test_eligibility_paused() {
        let graph = WorkGraph::default();
        let mut task = make_failed_task("t1");
        task.paused = true;
        let config = default_config();
        assert!(check_remediation_eligibility(&graph, &task, &config).is_err());
    }

    #[test]
    fn test_eligibility_max_attempts() {
        let graph = WorkGraph::default();
        let mut task = make_failed_task("t1");
        task.remediation_count = 3;
        let config = default_config();
        assert!(check_remediation_eligibility(&graph, &task, &config).is_err());
    }

    #[test]
    fn test_eligibility_ok() {
        let graph = WorkGraph::default();
        let task = make_failed_task("t1");
        let config = default_config();
        assert!(check_remediation_eligibility(&graph, &task, &config).is_ok());
    }

    #[test]
    fn test_eligibility_pending_remediation() {
        let mut graph = WorkGraph::default();
        let task = make_failed_task("t1");
        let rem = Task {
            id: ".remediate-t1".to_string(),
            title: "Remediate: t1".to_string(),
            status: Status::Open,
            ..Default::default()
        };
        graph.add_node(workgraph::graph::Node::Task(rem));
        let config = default_config();
        assert!(check_remediation_eligibility(&graph, &task, &config).is_err());
    }

    #[test]
    fn test_failure_category_display() {
        assert_eq!(format!("{}", FailureCategory::Transient), "transient");
        assert_eq!(format!("{}", FailureCategory::ContextOverflow), "context_overflow");
        assert_eq!(format!("{}", FailureCategory::BuildFailure), "build_failure");
        assert_eq!(format!("{}", FailureCategory::MissingDep), "missing_dep");
        assert_eq!(format!("{}", FailureCategory::AgentConfusion), "agent_confusion");
        assert_eq!(format!("{}", FailureCategory::Unfixable), "unfixable");
    }

    #[test]
    fn test_build_diagnosis_prompt_contains_task_info() {
        let task = make_failed_task("my-task");
        let prompt = build_diagnosis_prompt(&task, "some log output");
        assert!(prompt.contains("my-task"));
        assert!(prompt.contains("compilation error"));
        assert!(prompt.contains("some log output"));
    }

    #[test]
    fn test_remediation_description_build_failure() {
        let task = make_failed_task("t1");
        let diagnosis = Diagnosis {
            category: FailureCategory::BuildFailure,
            confidence: 0.9,
            summary: "syntax error in main.rs".to_string(),
            suggested_fix: Some("fix the missing semicolon".to_string()),
        };
        let desc = remediation_description(&task, &diagnosis);
        assert!(desc.contains("doesn't compile"));
        assert!(desc.contains("missing semicolon"));
    }

    #[test]
    fn test_remediation_description_missing_dep() {
        let task = make_failed_task("t1");
        let diagnosis = Diagnosis {
            category: FailureCategory::MissingDep,
            confidence: 0.85,
            summary: "missing libssl-dev".to_string(),
            suggested_fix: Some("apt install libssl-dev".to_string()),
        };
        let desc = remediation_description(&task, &diagnosis);
        assert!(desc.contains("missing dependency"));
        assert!(desc.contains("apt install"));
    }

    #[test]
    fn test_create_transient_retry() {
        let mut graph = WorkGraph::default();
        let task = make_failed_task("t1");
        graph.add_node(workgraph::graph::Node::Task(task));
        let diagnosis = Diagnosis {
            category: FailureCategory::Transient,
            confidence: 0.95,
            summary: "rate limited".to_string(),
            suggested_fix: None,
        };
        create_transient_retry(&mut graph, "t1", &diagnosis);
        let t = graph.get_task("t1").unwrap();
        assert_eq!(t.status, Status::Waiting);
        assert_eq!(t.remediation_count, 1);
        assert!(t.wait_condition.is_some());
    }

    #[test]
    fn test_create_transient_retry_backoff() {
        let mut graph = WorkGraph::default();
        let mut task = make_failed_task("t1");
        task.remediation_count = 2;
        graph.add_node(workgraph::graph::Node::Task(task));
        let diagnosis = Diagnosis {
            category: FailureCategory::Transient,
            confidence: 0.9,
            summary: "timeout".to_string(),
            suggested_fix: None,
        };
        create_transient_retry(&mut graph, "t1", &diagnosis);
        let t = graph.get_task("t1").unwrap();
        assert_eq!(t.remediation_count, 3);
    }

    #[test]
    fn test_escalate() {
        let mut graph = WorkGraph::default();
        let task = make_failed_task("t1");
        graph.add_node(workgraph::graph::Node::Task(task));
        escalate(&mut graph, "t1", "cannot fix this");
        let t = graph.get_task("t1").unwrap();
        assert!(t.paused);
        assert!(t.log.last().unwrap().message.contains("cannot fix this"));
    }

    #[test]
    fn test_create_remediation_task() {
        let mut graph = WorkGraph::default();
        let task = make_failed_task("my-task");
        graph.add_node(workgraph::graph::Node::Task(task));
        let diagnosis = Diagnosis {
            category: FailureCategory::BuildFailure,
            confidence: 0.9,
            summary: "cargo build failed".to_string(),
            suggested_fix: Some("fix syntax".to_string()),
        };
        create_remediation_task(&mut graph, "my-task", &diagnosis);
        let rem = graph.get_task(".remediate-my-task").unwrap();
        assert_eq!(rem.status, Status::Open);
        assert!(rem.before.contains(&"my-task".to_string()));
        assert!(rem.tags.contains(&"remediation".to_string()));
        let orig = graph.get_task("my-task").unwrap();
        assert_eq!(orig.status, Status::Open);
        assert_eq!(orig.remediation_count, 1);
        assert!(orig.after.contains(&".remediate-my-task".to_string()));
    }
}
