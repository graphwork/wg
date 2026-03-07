//! Question storage for agent-to-human input requests.
//!
//! Questions are stored as JSONL files in `.workgraph/questions/{task-id}.jsonl`.

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum QuestionStatus {
    Pending,
    Answered,
    Expired,
}

impl std::fmt::Display for QuestionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QuestionStatus::Pending => write!(f, "pending"),
            QuestionStatus::Answered => write!(f, "answered"),
            QuestionStatus::Expired => write!(f, "expired"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Question {
    pub id: String,
    pub task_id: String,
    pub agent_id: Option<String>,
    pub question: String,
    pub options: Vec<String>,
    pub allow_freeform: bool,
    pub created_at: String,
    pub status: QuestionStatus,
    pub answer: Option<String>,
    pub answered_by: Option<String>,
    pub answered_at: Option<String>,
}

fn questions_dir(workgraph_dir: &Path) -> PathBuf {
    workgraph_dir.join("questions")
}

fn question_file(workgraph_dir: &Path, task_id: &str) -> PathBuf {
    questions_dir(workgraph_dir).join(format!("{}.jsonl", task_id))
}

fn generate_question_id() -> String {
    let now = Utc::now().timestamp_nanos_opt().unwrap_or(0);
    let pid = std::process::id();
    let mut hasher = DefaultHasher::new();
    now.hash(&mut hasher);
    pid.hash(&mut hasher);
    let hash = hasher.finish();
    format!("q-{:08x}", hash as u32)
}

/// Create a new question for a task. Returns the question ID.
pub fn ask_question(
    workgraph_dir: &Path,
    task_id: &str,
    question_text: &str,
    options: &[String],
    agent_id: Option<&str>,
) -> Result<Question> {
    let dir = questions_dir(workgraph_dir);
    fs::create_dir_all(&dir).context("Failed to create questions directory")?;

    let q = Question {
        id: generate_question_id(),
        task_id: task_id.to_string(),
        agent_id: agent_id.map(|s| s.to_string()),
        question: question_text.to_string(),
        options: options.to_vec(),
        allow_freeform: true,
        created_at: Utc::now().to_rfc3339(),
        status: QuestionStatus::Pending,
        answer: None,
        answered_by: None,
        answered_at: None,
    };

    let file_path = question_file(workgraph_dir, task_id);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
        .context("Failed to open question file")?;

    let line = serde_json::to_string(&q).context("Failed to serialize question")?;
    writeln!(file, "{}", line)?;

    Ok(q)
}

/// Answer the most recent pending question for a task.
pub fn answer_question(
    workgraph_dir: &Path,
    task_id: &str,
    answer: &str,
    answered_by: Option<&str>,
) -> Result<Question> {
    let questions = load_questions(workgraph_dir, task_id)?;
    let pending = questions
        .iter()
        .rev()
        .find(|q| q.status == QuestionStatus::Pending);

    match pending {
        Some(q) => answer_question_by_id(workgraph_dir, task_id, &q.id, answer, answered_by),
        None => anyhow::bail!("No pending questions for task '{}'", task_id),
    }
}

/// Answer a specific question by its ID.
pub fn answer_question_by_id(
    workgraph_dir: &Path,
    task_id: &str,
    question_id: &str,
    answer: &str,
    answered_by: Option<&str>,
) -> Result<Question> {
    let mut questions = load_questions(workgraph_dir, task_id)?;
    let q = questions
        .iter_mut()
        .find(|q| q.id == question_id)
        .ok_or_else(|| anyhow::anyhow!("Question '{}' not found", question_id))?;

    if q.status != QuestionStatus::Pending {
        anyhow::bail!("Question '{}' is already {}", question_id, q.status);
    }

    q.status = QuestionStatus::Answered;
    q.answer = Some(answer.to_string());
    q.answered_by = Some(answered_by.unwrap_or("user").to_string());
    q.answered_at = Some(Utc::now().to_rfc3339());

    let updated = q.clone();
    save_questions(workgraph_dir, task_id, &questions)?;
    Ok(updated)
}

/// Load all questions for a task.
pub fn load_questions(workgraph_dir: &Path, task_id: &str) -> Result<Vec<Question>> {
    let file_path = question_file(workgraph_dir, task_id);
    if !file_path.exists() {
        return Ok(vec![]);
    }
    let file = fs::File::open(&file_path).context("Failed to open question file")?;
    let reader = BufReader::new(file);
    let mut questions = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let q: Question = serde_json::from_str(&line).context("Failed to parse question")?;
        questions.push(q);
    }
    Ok(questions)
}

/// Save all questions for a task (rewrite the file).
fn save_questions(workgraph_dir: &Path, task_id: &str, questions: &[Question]) -> Result<()> {
    let dir = questions_dir(workgraph_dir);
    fs::create_dir_all(&dir)?;
    let file_path = question_file(workgraph_dir, task_id);
    let mut file = fs::File::create(&file_path).context("Failed to create question file")?;
    for q in questions {
        let line = serde_json::to_string(q)?;
        writeln!(file, "{}", line)?;
    }
    Ok(())
}

/// List questions for a specific task.
pub fn list_questions_for_task(workgraph_dir: &Path, task_id: &str) -> Result<Vec<Question>> {
    load_questions(workgraph_dir, task_id)
}

/// List all pending questions across all tasks.
pub fn list_all_pending(workgraph_dir: &Path) -> Result<Vec<Question>> {
    let dir = questions_dir(workgraph_dir);
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut all_pending = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            let task_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            let questions = load_questions(workgraph_dir, task_id)?;
            for q in questions {
                if q.status == QuestionStatus::Pending {
                    all_pending.push(q);
                }
            }
        }
    }
    Ok(all_pending)
}

/// Check if a specific question has been answered. Returns the question if found.
pub fn check_answer(workgraph_dir: &Path, question_id: &str) -> Result<Option<Question>> {
    let dir = questions_dir(workgraph_dir);
    if !dir.exists() {
        return Ok(None);
    }
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            let task_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            let questions = load_questions(workgraph_dir, task_id)?;
            if let Some(q) = questions.into_iter().find(|q| q.id == question_id) {
                return Ok(Some(q));
            }
        }
    }
    Ok(None)
}

/// Get the latest answer for a task (most recent answered question).
pub fn get_latest_answer(workgraph_dir: &Path, task_id: &str) -> Result<Option<Question>> {
    let questions = load_questions(workgraph_dir, task_id)?;
    Ok(questions
        .into_iter()
        .rev()
        .find(|q| q.status == QuestionStatus::Answered))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let wg_dir = tmp.path().join(".workgraph");
        fs::create_dir_all(&wg_dir).unwrap();
        (tmp, wg_dir)
    }

    #[test]
    fn test_ask_creates_question() {
        let (_tmp, wg_dir) = setup();
        let q = ask_question(&wg_dir, "task-1", "What color?", &["red".into(), "blue".into()], Some("agent-1")).unwrap();
        assert!(q.id.starts_with("q-"));
        assert_eq!(q.task_id, "task-1");
        assert_eq!(q.question, "What color?");
        assert_eq!(q.options, vec!["red", "blue"]);
        assert_eq!(q.status, QuestionStatus::Pending);
        assert!(q.answer.is_none());
    }

    #[test]
    fn test_answer_question() {
        let (_tmp, wg_dir) = setup();
        let q = ask_question(&wg_dir, "task-1", "What color?", &[], None).unwrap();
        let answered = answer_question_by_id(&wg_dir, "task-1", &q.id, "blue", Some("user")).unwrap();
        assert_eq!(answered.status, QuestionStatus::Answered);
        assert_eq!(answered.answer.as_deref(), Some("blue"));
        assert_eq!(answered.answered_by.as_deref(), Some("user"));
    }

    #[test]
    fn test_answer_most_recent_pending() {
        let (_tmp, wg_dir) = setup();
        ask_question(&wg_dir, "task-1", "First question?", &[], None).unwrap();
        let q2 = ask_question(&wg_dir, "task-1", "Second question?", &[], None).unwrap();
        let answered = answer_question(&wg_dir, "task-1", "yes", None).unwrap();
        assert_eq!(answered.id, q2.id);
    }

    #[test]
    fn test_no_pending_questions_error() {
        let (_tmp, wg_dir) = setup();
        let result = answer_question(&wg_dir, "task-1", "answer", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_list_questions_for_task() {
        let (_tmp, wg_dir) = setup();
        ask_question(&wg_dir, "task-1", "Q1?", &[], None).unwrap();
        ask_question(&wg_dir, "task-1", "Q2?", &[], None).unwrap();
        let questions = list_questions_for_task(&wg_dir, "task-1").unwrap();
        assert_eq!(questions.len(), 2);
    }

    #[test]
    fn test_list_all_pending() {
        let (_tmp, wg_dir) = setup();
        ask_question(&wg_dir, "task-1", "Q1?", &[], None).unwrap();
        ask_question(&wg_dir, "task-2", "Q2?", &[], None).unwrap();
        let q3 = ask_question(&wg_dir, "task-1", "Q3?", &[], None).unwrap();
        answer_question_by_id(&wg_dir, "task-1", &q3.id, "done", None).unwrap();
        let pending = list_all_pending(&wg_dir).unwrap();
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn test_check_answer_found() {
        let (_tmp, wg_dir) = setup();
        let q = ask_question(&wg_dir, "task-1", "Q?", &[], None).unwrap();
        let found = check_answer(&wg_dir, &q.id).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().status, QuestionStatus::Pending);
    }

    #[test]
    fn test_check_answer_not_found() {
        let (_tmp, wg_dir) = setup();
        let found = check_answer(&wg_dir, "q-nonexistent").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn test_get_latest_answer() {
        let (_tmp, wg_dir) = setup();
        let q1 = ask_question(&wg_dir, "task-1", "Q1?", &[], None).unwrap();
        ask_question(&wg_dir, "task-1", "Q2?", &[], None).unwrap();
        answer_question_by_id(&wg_dir, "task-1", &q1.id, "a1", None).unwrap();
        let latest = get_latest_answer(&wg_dir, "task-1").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().answer.as_deref(), Some("a1"));
    }

    #[test]
    fn test_double_answer_fails() {
        let (_tmp, wg_dir) = setup();
        let q = ask_question(&wg_dir, "task-1", "Q?", &[], None).unwrap();
        answer_question_by_id(&wg_dir, "task-1", &q.id, "a1", None).unwrap();
        let result = answer_question_by_id(&wg_dir, "task-1", &q.id, "a2", None);
        assert!(result.is_err());
    }
}
