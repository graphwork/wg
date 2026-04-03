//! Tests for heartbeat signal during long-running tool execution.
//!
//! NOTE: Heartbeat functionality (periodic signals during long tool execution)
//! is not yet implemented. These tests verify that the bash tool basic functionality
//! works correctly.

use tempfile::TempDir;
use workgraph::executor::native::tools::ToolRegistry;

/// Test that bash tool executes a command and returns output.
#[tokio::test]
async fn test_bash_tool_basic_execution() {
    let tmp = TempDir::new().unwrap();
    let working_dir = tmp.path().to_path_buf();

    let mut registry = ToolRegistry::new();
    workgraph::executor::native::tools::bash::register_bash_tool(&mut registry, working_dir);

    let input = serde_json::json!({
        "command": "echo 'hello world'",
        "timeout": 5000
    });

    let output = registry.execute("bash", &input).await;
    assert!(!output.is_error, "Command should succeed: {}", output.content);
    assert!(output.content.contains("hello world"));
}

/// Test that bash tool handles errors correctly.
#[tokio::test]
async fn test_bash_tool_error_handling() {
    let tmp = TempDir::new().unwrap();
    let working_dir = tmp.path().to_path_buf();

    let mut registry = ToolRegistry::new();
    workgraph::executor::native::tools::bash::register_bash_tool(&mut registry, working_dir);

    // Run a command that fails
    let input = serde_json::json!({
        "command": "exit 1",
        "timeout": 5000
    });

    let output = registry.execute("bash", &input).await;
    assert!(output.is_error, "Command should fail with non-zero exit");
}

/// Test that bash tool respects timeout.
#[tokio::test]
async fn test_bash_tool_timeout() {
    let tmp = TempDir::new().unwrap();
    let working_dir = tmp.path().to_path_buf();

    let mut registry = ToolRegistry::new();
    workgraph::executor::native::tools::bash::register_bash_tool(&mut registry, working_dir);

    // Run a command that times out
    let input = serde_json::json!({
        "command": "sleep 10",
        "timeout": 100  // 100ms timeout
    });

    let output = registry.execute("bash", &input).await;
    assert!(output.is_error, "Command should timeout");
    assert!(output.content.contains("timed out"));
}
