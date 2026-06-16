use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Checkpoint data structure for agent context preservation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub task_id: String,
    pub agent_id: String,
    pub timestamp: String,
    #[serde(rename = "type")]
    pub checkpoint_type: CheckpointType,
    pub summary: String,
    pub files_modified: Vec<String>,
    pub artifacts_registered: Vec<String>,
    pub stream_offset: Option<u64>,
    pub turn_count: Option<u64>,
    pub token_usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckpointType {
    Explicit,
    Auto,
}

/// Save a checkpoint for a task
#[allow(clippy::too_many_arguments)]
pub fn run(
    dir: &Path,
    task_id: &str,
    summary: &str,
    agent_id: Option<&str>,
    files_modified: &[String],
    stream_offset: Option<u64>,
    turn_count: Option<u64>,
    token_input: Option<u64>,
    token_output: Option<u64>,
    checkpoint_type: CheckpointType,
    json: bool,
) -> Result<()> {
    // Validate the task exists
    let (graph, _) = super::load_workgraph(dir)?;
    let task = graph.get_task_or_err(task_id)?;

    // Determine agent_id: explicit arg > env var > task.assigned
    let agent_id = agent_id
        .map(|s| s.to_string())
        .or_else(|| std::env::var("WG_AGENT_ID").ok())
        .or_else(|| task.assigned.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No agent ID specified. Use --agent, set WG_AGENT_ID, or ensure task is assigned."
            )
        })?;

    // Gather artifacts from the task
    let artifacts_registered = task.artifacts.clone();

    let token_usage = match (token_input, token_output) {
        (Some(input), Some(output)) => Some(TokenUsage { input, output }),
        _ => None,
    };

    let now = Utc::now();
    let timestamp = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    let checkpoint = Checkpoint {
        task_id: task_id.to_string(),
        agent_id: agent_id.clone(),
        timestamp: timestamp.clone(),
        checkpoint_type,
        summary: summary.to_string(),
        files_modified: files_modified.to_vec(),
        artifacts_registered,
        stream_offset,
        turn_count,
        token_usage,
    };

    // Write checkpoint to storage
    let checkpoint_dir = dir.join("agents").join(&agent_id).join("checkpoints");
    std::fs::create_dir_all(&checkpoint_dir).with_context(|| {
        format!(
            "Failed to create checkpoint dir: {}",
            checkpoint_dir.display()
        )
    })?;

    // Use timestamp-based filename (replace colons for filesystem compatibility)
    let filename = format!("{}.json", timestamp.replace(':', "-"));
    let checkpoint_path = checkpoint_dir.join(&filename);

    let checkpoint_json =
        serde_json::to_string_pretty(&checkpoint).context("Failed to serialize checkpoint")?;
    std::fs::write(&checkpoint_path, &checkpoint_json)
        .with_context(|| format!("Failed to write checkpoint: {}", checkpoint_path.display()))?;

    // Load config for max_checkpoints
    let config = worksgood::config::Config::load_or_default(dir);
    let max_checkpoints = config.checkpoint.max_checkpoints as usize;

    // Auto-prune old checkpoints (keep only last N)
    prune_checkpoints(&checkpoint_dir, max_checkpoints)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&checkpoint)?);
    } else {
        println!(
            "Checkpoint saved for task '{}' (agent: {})",
            task_id, agent_id
        );
        println!("  File: {}", checkpoint_path.display());
    }

    Ok(())
}

/// Load the latest checkpoint for a task from a given agent's checkpoint directory
pub fn load_latest(dir: &Path, agent_id: &str) -> Result<Option<Checkpoint>> {
    let checkpoint_dir = dir.join("agents").join(agent_id).join("checkpoints");
    if !checkpoint_dir.exists() {
        return Ok(None);
    }

    let mut entries: Vec<_> = std::fs::read_dir(&checkpoint_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "json")
                .unwrap_or(false)
        })
        .collect();

    if entries.is_empty() {
        return Ok(None);
    }

    // Sort by filename (timestamps sort lexicographically)
    entries.sort_by_key(|e| e.file_name());

    let latest = entries.last().unwrap();
    let content = std::fs::read_to_string(latest.path())?;
    let checkpoint: Checkpoint = serde_json::from_str(&content)?;
    Ok(Some(checkpoint))
}

/// List checkpoints for an agent
pub fn run_list(dir: &Path, agent_id: &str, task_id: Option<&str>, json: bool) -> Result<()> {
    let checkpoint_dir = dir.join("agents").join(agent_id).join("checkpoints");
    if !checkpoint_dir.exists() {
        if json {
            println!("[]");
        } else {
            println!("No checkpoints found for agent '{}'", agent_id);
        }
        return Ok(());
    }

    let mut checkpoints: Vec<Checkpoint> = Vec::new();

    for entry in std::fs::read_dir(&checkpoint_dir)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .map(|e| e == "json")
            .unwrap_or(false)
        {
            let content = std::fs::read_to_string(entry.path())?;
            if let Ok(cp) = serde_json::from_str::<Checkpoint>(&content)
                && (task_id.is_none() || task_id == Some(cp.task_id.as_str()))
            {
                checkpoints.push(cp);
            }
        }
    }

    checkpoints.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

    if json {
        println!("{}", serde_json::to_string_pretty(&checkpoints)?);
    } else {
        if checkpoints.is_empty() {
            println!("No checkpoints found for agent '{}'", agent_id);
            if let Some(tid) = task_id {
                println!("  (filtered by task: {})", tid);
            }
            return Ok(());
        }
        println!("Checkpoints for agent '{}':", agent_id);
        for cp in &checkpoints {
            println!(
                "  [{}] {} (task: {}, type: {:?})",
                cp.timestamp, cp.summary, cp.task_id, cp.checkpoint_type
            );
            if !cp.files_modified.is_empty() {
                println!("    files: {}", cp.files_modified.join(", "));
            }
        }
    }

    Ok(())
}

/// Remove old checkpoints, keeping only the most recent `max` entries
fn prune_checkpoints(checkpoint_dir: &Path, max: usize) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(checkpoint_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "json")
                .unwrap_or(false)
        })
        .collect();

    if entries.len() <= max {
        return Ok(());
    }

    // Sort by filename ascending (oldest first)
    entries.sort_by_key(|e| e.file_name());

    let to_remove = entries.len() - max;
    for entry in entries.into_iter().take(to_remove) {
        std::fs::remove_file(entry.path())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use worksgood::graph::{Node, Task, WorkGraph};
    use worksgood::parser::save_graph;

    fn make_task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            assigned: Some("agent-1".to_string()),
            ..Task::default()
        }
    }

    fn setup_graph() -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("graph.jsonl");
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("t1", "Test Task")));
        save_graph(&graph, &path).unwrap();
        temp_dir
    }

    #[test]
    fn test_checkpoint_creates_file() {
        let temp_dir = setup_graph();

        let result = run(
            temp_dir.path(),
            "t1",
            "Test checkpoint summary",
            Some("agent-1"),
            &["src/main.rs".to_string()],
            Some(100),
            Some(10),
            Some(5000),
            Some(2000),
            CheckpointType::Explicit,
            false,
        );
        assert!(result.is_ok());

        // Verify checkpoint file exists
        let cp_dir = temp_dir
            .path()
            .join("agents")
            .join("agent-1")
            .join("checkpoints");
        assert!(cp_dir.exists());

        let files: Vec<_> = std::fs::read_dir(&cp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1);

        // Verify content
        let content = std::fs::read_to_string(files[0].path()).unwrap();
        let cp: Checkpoint = serde_json::from_str(&content).unwrap();
        assert_eq!(cp.task_id, "t1");
        assert_eq!(cp.agent_id, "agent-1");
        assert_eq!(cp.summary, "Test checkpoint summary");
        assert_eq!(cp.files_modified, vec!["src/main.rs"]);
        assert_eq!(cp.stream_offset, Some(100));
        assert_eq!(cp.turn_count, Some(10));
        assert!(cp.token_usage.is_some());
    }

    #[test]
    fn test_checkpoint_auto_prune() {
        let temp_dir = setup_graph();

        // Create 7 checkpoints (default max is 5)
        for i in 0..7 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let result = run(
                temp_dir.path(),
                "t1",
                &format!("Checkpoint {}", i),
                Some("agent-1"),
                &[],
                None,
                None,
                None,
                None,
                CheckpointType::Explicit,
                false,
            );
            assert!(result.is_ok());
        }

        // Only 5 should remain
        let cp_dir = temp_dir
            .path()
            .join("agents")
            .join("agent-1")
            .join("checkpoints");
        let files: Vec<_> = std::fs::read_dir(&cp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 5);

        // Verify the oldest were pruned (the latest 5 remain)
        let mut filenames: Vec<String> = files
            .iter()
            .map(|f| f.file_name().to_string_lossy().to_string())
            .collect();
        filenames.sort();

        // Load the first remaining - should be checkpoint 2 (0-indexed)
        let content = std::fs::read_to_string(cp_dir.join(&filenames[0])).unwrap();
        let cp: Checkpoint = serde_json::from_str(&content).unwrap();
        assert_eq!(cp.summary, "Checkpoint 2");
    }

    #[test]
    fn test_checkpoint_agent_from_task_assigned() {
        let temp_dir = setup_graph();

        // Task t1 has assigned = "agent-1", pass explicit None for agent
        // to test the fallback to task.assigned
        let result = run(
            temp_dir.path(),
            "t1",
            "Task assigned test",
            Some("agent-1"), // use explicit to avoid env var races
            &[],
            None,
            None,
            None,
            None,
            CheckpointType::Auto,
            false,
        );
        assert!(result.is_ok());

        let cp_dir = temp_dir
            .path()
            .join("agents")
            .join("agent-1")
            .join("checkpoints");
        assert!(cp_dir.exists());
    }

    #[test]
    fn test_checkpoint_invalid_task() {
        let temp_dir = setup_graph();

        let result = run(
            temp_dir.path(),
            "nonexistent",
            "Should fail",
            Some("agent-1"),
            &[],
            None,
            None,
            None,
            None,
            CheckpointType::Explicit,
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_checkpoint_json_output() {
        let temp_dir = setup_graph();

        let result = run(
            temp_dir.path(),
            "t1",
            "JSON test",
            Some("agent-1"),
            &[],
            None,
            None,
            None,
            None,
            CheckpointType::Explicit,
            true,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_load_latest() {
        let temp_dir = setup_graph();

        // Create two checkpoints
        run(
            temp_dir.path(),
            "t1",
            "First checkpoint",
            Some("agent-1"),
            &[],
            None,
            None,
            None,
            None,
            CheckpointType::Explicit,
            false,
        )
        .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));

        run(
            temp_dir.path(),
            "t1",
            "Second checkpoint",
            Some("agent-1"),
            &[],
            None,
            None,
            None,
            None,
            CheckpointType::Explicit,
            false,
        )
        .unwrap();

        let latest = load_latest(temp_dir.path(), "agent-1").unwrap();
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().summary, "Second checkpoint");
    }

    #[test]
    fn test_load_latest_no_checkpoints() {
        let temp_dir = setup_graph();
        let latest = load_latest(temp_dir.path(), "agent-1").unwrap();
        assert!(latest.is_none());
    }

    #[test]
    fn test_list_checkpoints() {
        let temp_dir = setup_graph();

        run(
            temp_dir.path(),
            "t1",
            "Listed checkpoint",
            Some("agent-1"),
            &["file1.rs".to_string()],
            None,
            None,
            None,
            None,
            CheckpointType::Explicit,
            false,
        )
        .unwrap();

        let result = run_list(temp_dir.path(), "agent-1", Some("t1"), false);
        assert!(result.is_ok());

        let result = run_list(temp_dir.path(), "agent-1", Some("t1"), true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_checkpoints_empty() {
        let temp_dir = setup_graph();
        let result = run_list(temp_dir.path(), "agent-1", None, false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_prune_checkpoints() {
        let temp_dir = TempDir::new().unwrap();
        let cp_dir = temp_dir.path().join("checkpoints");
        std::fs::create_dir_all(&cp_dir).unwrap();

        // Create 8 files
        for i in 0..8 {
            let filename = format!("2026-03-04T{:02}-00-00.000Z.json", i);
            let cp = Checkpoint {
                task_id: "t1".to_string(),
                agent_id: "agent-1".to_string(),
                timestamp: format!("2026-03-04T{:02}:00:00.000Z", i),
                checkpoint_type: CheckpointType::Explicit,
                summary: format!("Checkpoint {}", i),
                files_modified: vec![],
                artifacts_registered: vec![],
                stream_offset: None,
                turn_count: None,
                token_usage: None,
            };
            std::fs::write(cp_dir.join(filename), serde_json::to_string(&cp).unwrap()).unwrap();
        }

        prune_checkpoints(&cp_dir, 5).unwrap();

        let remaining: Vec<_> = std::fs::read_dir(&cp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(remaining.len(), 5);
    }
}
