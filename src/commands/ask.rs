//! Ask CLI commands: question, answer, list, check.
//!
//! Enables agents to request human input during execution.

use anyhow::Result;
use std::path::Path;

use workgraph::questions;

/// Create a question for a task. Optionally block until answered.
pub fn run_ask(
    dir: &Path,
    task_id: &str,
    question: &str,
    options: &[String],
    agent_id: Option<&str>,
    wait: bool,
    timeout_secs: Option<u64>,
    json: bool,
) -> Result<()> {
    let (graph, _path) = super::load_workgraph(dir)?;
    graph.get_task_or_err(task_id)?;

    let q = questions::ask_question(dir, task_id, question, options, agent_id)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&q)?);
    } else {
        println!("Question {} created for task '{}'", q.id, task_id);
        if !q.options.is_empty() {
            println!("Options: {}", q.options.join(", "));
        }
    }

    if wait {
        return poll_for_answer(dir, &q.id, timeout_secs, json);
    }

    Ok(())
}

/// Answer the most recent pending question for a task.
pub fn run_answer(
    dir: &Path,
    task_id: &str,
    answer: &str,
    answered_by: Option<&str>,
    json: bool,
) -> Result<()> {
    let (graph, _path) = super::load_workgraph(dir)?;
    graph.get_task_or_err(task_id)?;

    let q = questions::answer_question(dir, task_id, answer, answered_by)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&q)?);
    } else {
        println!("Answered question {} for task '{}': {}", q.id, task_id, answer);
    }

    Ok(())
}

/// List questions (all pending, or for a specific task).
pub fn run_list(dir: &Path, task_id: Option<&str>, json: bool) -> Result<()> {
    let questions = if let Some(tid) = task_id {
        questions::list_questions_for_task(dir, tid)?
    } else {
        questions::list_all_pending(dir)?
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&questions)?);
        return Ok(());
    }

    if questions.is_empty() {
        if task_id.is_some() {
            println!("No questions for task '{}'", task_id.unwrap());
        } else {
            println!("No pending questions");
        }
        return Ok(());
    }

    for q in &questions {
        let status = match q.status {
            questions::QuestionStatus::Pending => "PENDING",
            questions::QuestionStatus::Answered => "ANSWERED",
            questions::QuestionStatus::Expired => "EXPIRED",
        };
        println!("[{}] {} (task: {}) - {}", status, q.id, q.task_id, q.question);
        if !q.options.is_empty() {
            println!("  Options: {}", q.options.join(", "));
        }
        if let Some(ref ans) = q.answer {
            println!("  Answer: {}", ans);
        }
    }

    Ok(())
}

/// Check if a question has been answered. Returns true if answered.
pub fn run_check(dir: &Path, question_id: &str, json: bool) -> Result<bool> {
    let q = questions::check_answer(dir, question_id)?;

    match q {
        Some(q) => {
            let answered = q.status == questions::QuestionStatus::Answered;
            if json {
                println!("{}", serde_json::to_string_pretty(&q)?);
            } else if answered {
                println!("Question {} is answered: {}", q.id, q.answer.as_deref().unwrap_or(""));
            } else {
                println!("Question {} is {}", q.id, q.status);
            }
            Ok(answered)
        }
        None => {
            if json {
                println!("null");
            } else {
                println!("Question '{}' not found", question_id);
            }
            Ok(false)
        }
    }
}

/// Poll for an answer to a question, blocking until answered or timeout.
fn poll_for_answer(wg_dir: &Path, question_id: &str, timeout_secs: Option<u64>, json: bool) -> Result<()> {
    let start = std::time::Instant::now();
    let poll_interval = std::time::Duration::from_secs(2);

    loop {
        let q = questions::check_answer(wg_dir, question_id)?;
        if let Some(ref q) = q {
            if q.status == questions::QuestionStatus::Answered {
                if json {
                    println!("{}", serde_json::to_string_pretty(&q)?);
                } else {
                    println!("Answer: {}", q.answer.as_deref().unwrap_or(""));
                }
                return Ok(());
            }
        }

        if let Some(timeout) = timeout_secs {
            if start.elapsed().as_secs() >= timeout {
                anyhow::bail!("Timeout waiting for answer to question '{}'", question_id);
            }
        }

        std::thread::sleep(poll_interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_workgraph() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        fs::create_dir_all(&wg_dir).unwrap();
        // Create a minimal graph.json so load_workgraph won't fail
        let graph = serde_json::json!({
            "tasks": {
                "test-task": {
                    "id": "test-task",
                    "title": "Test Task",
                    "status": "open"
                }
            }
        });
        fs::write(wg_dir.join("graph.json"), serde_json::to_string_pretty(&graph).unwrap()).unwrap();
        (tmp, wg_dir)
    }

    #[test]
    fn test_run_ask_creates_question() {
        let (_tmp, wg_dir) = setup_workgraph();
        let q = questions::ask_question(&wg_dir, "test-task", "What?", &[], None).unwrap();
        assert!(q.id.starts_with("q-"));
        assert_eq!(q.status, questions::QuestionStatus::Pending);
    }

    #[test]
    fn test_run_answer_answers_question() {
        let (_tmp, wg_dir) = setup_workgraph();
        questions::ask_question(&wg_dir, "test-task", "What?", &[], None).unwrap();
        let answered = questions::answer_question(&wg_dir, "test-task", "that", Some("user")).unwrap();
        assert_eq!(answered.status, questions::QuestionStatus::Answered);
        assert_eq!(answered.answer.as_deref(), Some("that"));
    }

    #[test]
    fn test_run_check_finds_question() {
        let (_tmp, wg_dir) = setup_workgraph();
        let q = questions::ask_question(&wg_dir, "test-task", "What?", &[], None).unwrap();
        let found = questions::check_answer(&wg_dir, &q.id).unwrap();
        assert!(found.is_some());
    }

    #[test]
    fn test_list_pending_across_tasks() {
        let (_tmp, wg_dir) = setup_workgraph();
        questions::ask_question(&wg_dir, "test-task", "Q1?", &[], None).unwrap();
        // Create another task file directly
        questions::ask_question(&wg_dir, "task-2", "Q2?", &[], None).unwrap();
        let pending = questions::list_all_pending(&wg_dir).unwrap();
        assert_eq!(pending.len(), 2);
    }
}
