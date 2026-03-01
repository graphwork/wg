//! Message queue CLI commands: send, list, read, poll.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

use workgraph::messages;

/// Send a message to a task's queue.
pub fn run_send(
    dir: &Path,
    task_id: &str,
    body: Option<&str>,
    sender: &str,
    priority: &str,
    stdin: bool,
) -> Result<()> {
    // Validate task exists in the graph
    let (graph, _path) = super::load_workgraph(dir)?;
    graph.get_task_or_err(task_id)?;

    let message_body = if stdin {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("Failed to read from stdin")?;
        buf.trim_end().to_string()
    } else {
        body.ok_or_else(|| anyhow::anyhow!("Message body required (or use --stdin)"))?
            .to_string()
    };

    if message_body.is_empty() {
        anyhow::bail!("Message body cannot be empty");
    }

    let id = messages::send_message(dir, task_id, &message_body, sender, priority)?;
    println!("Message #{} sent to '{}'", id, task_id);

    Ok(())
}

/// List all messages for a task.
pub fn run_list(dir: &Path, task_id: &str, json: bool) -> Result<()> {
    // Validate task exists
    let (graph, _path) = super::load_workgraph(dir)?;
    graph.get_task_or_err(task_id)?;

    let msgs = messages::list_messages(dir, task_id)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&msgs)?);
        return Ok(());
    }

    if msgs.is_empty() {
        println!("No messages for task '{}'", task_id);
        return Ok(());
    }

    println!("Messages for '{}' ({} total):", task_id, msgs.len());
    println!();

    for msg in &msgs {
        let priority_marker = if msg.priority == "urgent" {
            " [URGENT]"
        } else {
            ""
        };
        println!(
            "  #{} [{}] {}{}",
            msg.id, msg.timestamp, msg.sender, priority_marker
        );
        // Indent multi-line bodies
        for line in msg.body.lines() {
            println!("    {}", line);
        }
        println!();
    }

    Ok(())
}

/// Read unread messages for an agent (advances cursor).
pub fn run_read(dir: &Path, task_id: &str, agent_id: &str, json: bool) -> Result<()> {
    // Validate task exists
    let (graph, _path) = super::load_workgraph(dir)?;
    graph.get_task_or_err(task_id)?;

    let unread = messages::read_unread(dir, task_id, agent_id)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&unread)?);
        return Ok(());
    }

    if unread.is_empty() {
        println!(
            "No unread messages for task '{}' (agent: {})",
            task_id, agent_id
        );
        return Ok(());
    }

    println!(
        "{} unread message{} for '{}' (agent: {}):",
        unread.len(),
        if unread.len() == 1 { "" } else { "s" },
        task_id,
        agent_id
    );
    println!();

    for msg in &unread {
        let priority_marker = if msg.priority == "urgent" {
            " [URGENT]"
        } else {
            ""
        };
        println!(
            "  #{} [{}] {}{}",
            msg.id, msg.timestamp, msg.sender, priority_marker
        );
        for line in msg.body.lines() {
            println!("    {}", line);
        }
        println!();
    }

    Ok(())
}

/// Poll for new messages (exit code 0 = new messages, 1 = none).
///
/// Does NOT advance the cursor (messages remain "unread" for `read`).
pub fn run_poll(dir: &Path, task_id: &str, agent_id: &str, json: bool) -> Result<bool> {
    // Validate task exists
    let (graph, _path) = super::load_workgraph(dir)?;
    graph.get_task_or_err(task_id)?;

    let new_msgs = messages::poll_messages(dir, task_id, agent_id)?;

    if new_msgs.is_empty() {
        if !json {
            println!(
                "No new messages for task '{}' (agent: {})",
                task_id, agent_id
            );
        } else {
            println!("[]");
        }
        return Ok(false);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&new_msgs)?);
    } else {
        println!(
            "{} new message{} for '{}' (agent: {}):",
            new_msgs.len(),
            if new_msgs.len() == 1 { "" } else { "s" },
            task_id,
            agent_id
        );
        println!();
        for msg in &new_msgs {
            let priority_marker = if msg.priority == "urgent" {
                " [URGENT]"
            } else {
                ""
            };
            println!(
                "  #{} [{}] {}{}",
                msg.id, msg.timestamp, msg.sender, priority_marker
            );
            for line in msg.body.lines() {
                println!("    {}", line);
            }
            println!();
        }
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use workgraph::graph::{Node, Task, WorkGraph};
    use workgraph::parser::save_graph;

    fn make_task(id: &str, title: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            ..Task::default()
        }
    }

    fn setup_graph(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let mut graph = WorkGraph::new();
        graph.add_node(Node::Task(make_task("task-1", "Test Task")));
        let path = super::super::graph_path(dir);
        save_graph(&graph, &path).unwrap();
    }

    #[test]
    fn test_send_message() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".workgraph");
        setup_graph(&dir);

        run_send(&dir, "task-1", Some("Hello world"), "user", "normal", false).unwrap();

        let msgs = messages::list_messages(&dir, "task-1").unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].body, "Hello world");
    }

    #[test]
    fn test_send_to_nonexistent_task() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".workgraph");
        setup_graph(&dir);

        let result = run_send(&dir, "nonexistent", Some("Hello"), "user", "normal", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_send_empty_body_fails() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".workgraph");
        setup_graph(&dir);

        let result = run_send(&dir, "task-1", Some(""), "user", "normal", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn test_list_messages() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".workgraph");
        setup_graph(&dir);

        run_send(&dir, "task-1", Some("First"), "user", "normal", false).unwrap();
        run_send(
            &dir,
            "task-1",
            Some("Second"),
            "coordinator",
            "urgent",
            false,
        )
        .unwrap();

        // Should not error
        let result = run_list(&dir, "task-1", false);
        assert!(result.is_ok());

        let result = run_list(&dir, "task-1", true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_read_and_poll() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".workgraph");
        setup_graph(&dir);

        run_send(&dir, "task-1", Some("Message"), "user", "normal", false).unwrap();

        // Poll returns true (has messages)
        let has_new = run_poll(&dir, "task-1", "agent-1", false).unwrap();
        assert!(has_new);

        // Read advances cursor
        let result = run_read(&dir, "task-1", "agent-1", false);
        assert!(result.is_ok());

        // Poll returns false (no new messages)
        let has_new = run_poll(&dir, "task-1", "agent-1", false).unwrap();
        assert!(!has_new);
    }
}
