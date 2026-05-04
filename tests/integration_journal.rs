//! Integration tests for conversation journal.
//!
//! Tests the journal through the native executor agent loop using a mock provider.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tempfile::TempDir;

use workgraph::executor::native::agent::AgentLoop;
use workgraph::executor::native::client::{
    ContentBlock, MessagesRequest, MessagesResponse, StopReason, Usage,
};
use workgraph::executor::native::journal::{self, EndReason, Journal, JournalEntryKind};
use workgraph::executor::native::provider::Provider;
use workgraph::executor::native::tools::ToolRegistry;

/// A mock provider that returns a pre-scripted response.
struct MockProvider {
    responses: Vec<MessagesResponse>,
    call_count: Arc<AtomicUsize>,
}

impl MockProvider {
    fn new(responses: Vec<MessagesResponse>) -> Self {
        Self {
            responses,
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Create a mock that returns a simple text response (end_turn) on first call.
    fn simple_text(text: &str) -> Self {
        Self::new(vec![MessagesResponse {
            id: "msg-test-001".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            stop_reason: Some(StopReason::EndTurn),
            usage: Usage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
                reasoning_tokens: None,
            },
        }])
    }

    /// Create a mock that first requests a tool call, then returns a final response.
    fn with_tool_call(tool_name: &str, tool_input: serde_json::Value, final_text: &str) -> Self {
        Self::new(vec![
            // First response: tool_use
            MessagesResponse {
                id: "msg-test-001".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "tu-mock-1".to_string(),
                    name: tool_name.to_string(),
                    input: tool_input,
                }],
                stop_reason: Some(StopReason::ToolUse),
                usage: Usage {
                    input_tokens: 100,
                    output_tokens: 30,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    reasoning_tokens: None,
                },
            },
            // Second response: final text
            MessagesResponse {
                id: "msg-test-002".to_string(),
                content: vec![ContentBlock::Text {
                    text: final_text.to_string(),
                }],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage {
                    input_tokens: 200,
                    output_tokens: 60,
                    cache_creation_input_tokens: None,
                    cache_read_input_tokens: None,
                    reasoning_tokens: None,
                },
            },
        ])
    }
}

#[async_trait::async_trait]
impl Provider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn model(&self) -> &str {
        "mock-model-v1"
    }

    fn max_tokens(&self) -> u32 {
        4096
    }

    async fn send(&self, _request: &MessagesRequest) -> anyhow::Result<MessagesResponse> {
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        if idx < self.responses.len() {
            Ok(self.responses[idx].clone())
        } else {
            // Fallback: return end_turn to stop the loop
            Ok(MessagesResponse {
                id: format!("msg-fallback-{}", idx),
                content: vec![ContentBlock::Text {
                    text: "[mock exhausted]".to_string(),
                }],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage::default(),
            })
        }
    }
}

fn setup_workgraph(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    let graph_path = dir.join("graph.jsonl");
    let graph = workgraph::graph::WorkGraph::new();
    workgraph::parser::save_graph(&graph, &graph_path).unwrap();
}

// ── Test: journal is created when agent loop runs ──────────────────────

#[tokio::test]
async fn test_journal_created_on_agent_run() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    setup_workgraph(&wg_dir);

    let task_id = "test-journal-task";
    let j_path = journal::journal_path(&wg_dir, task_id);

    // Journal file should not exist yet
    assert!(!j_path.exists());

    let provider = MockProvider::simple_text("Task complete.");
    let registry = ToolRegistry::default_all(&wg_dir, tmp.path());
    let output_log = wg_dir.join("test.ndjson");

    let mut agent = AgentLoop::new(
        Box::new(provider),
        registry,
        "You are a test agent.".to_string(),
        10,
        output_log,
    )
    .with_journal(j_path.clone(), task_id.to_string());

    let result = agent.run("Do the task.").await.unwrap();
    assert_eq!(result.turns, 1);

    // Journal file should now exist
    assert!(j_path.exists(), "Journal file should be created");

    let entries = Journal::read_all(&j_path).unwrap();
    // Expect: Init, User message, Assistant message, End
    assert_eq!(
        entries.len(),
        4,
        "Expected 4 journal entries, got {}",
        entries.len()
    );

    // Check Init entry
    assert_eq!(entries[0].seq, 1);
    match &entries[0].kind {
        JournalEntryKind::Init {
            model,
            provider,
            task_id: tid,
            ..
        } => {
            assert_eq!(model, "mock-model-v1");
            assert_eq!(provider, "mock");
            assert_eq!(tid.as_deref(), Some("test-journal-task"));
        }
        _ => panic!("Expected Init entry at seq 1"),
    }

    // Check User message
    match &entries[1].kind {
        JournalEntryKind::Message { role, content, .. } => {
            assert_eq!(*role, workgraph::executor::native::client::Role::User);
            assert_eq!(content.len(), 1);
        }
        _ => panic!("Expected User Message at seq 2"),
    }

    // Check Assistant message
    match &entries[2].kind {
        JournalEntryKind::Message {
            role,
            usage,
            response_id,
            stop_reason,
            ..
        } => {
            assert_eq!(*role, workgraph::executor::native::client::Role::Assistant);
            assert!(usage.is_some());
            assert_eq!(response_id.as_deref(), Some("msg-test-001"));
            assert_eq!(*stop_reason, Some(StopReason::EndTurn));
        }
        _ => panic!("Expected Assistant Message at seq 3"),
    }

    // Check End entry
    match &entries[3].kind {
        JournalEntryKind::End {
            reason,
            total_usage,
            turns,
        } => {
            assert!(matches!(reason, EndReason::Complete));
            assert_eq!(total_usage.input_tokens, 100);
            assert_eq!(total_usage.output_tokens, 50);
            assert_eq!(*turns, 1);
        }
        _ => panic!("Expected End entry at seq 4"),
    }
}

// ── Test: journal entries have correct provider-agnostic format ─────────

#[tokio::test]
async fn test_journal_entries_provider_agnostic_format() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    setup_workgraph(&wg_dir);

    let task_id = "format-test";
    let j_path = journal::journal_path(&wg_dir, task_id);

    let provider = MockProvider::simple_text("Done.");
    let registry = ToolRegistry::default_all(&wg_dir, tmp.path());
    let output_log = wg_dir.join("test.ndjson");

    let mut agent = AgentLoop::new(
        Box::new(provider),
        registry,
        "Test prompt.".to_string(),
        10,
        output_log,
    )
    .with_journal(j_path.clone(), task_id.to_string());

    agent.run("Hello").await.unwrap();

    // Read raw JSONL and verify format
    let raw = fs::read_to_string(&j_path).unwrap();
    let lines: Vec<&str> = raw.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 4);

    // Each line should be valid JSON
    for (i, line) in lines.iter().enumerate() {
        let val: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("Line {} is not valid JSON: {}", i + 1, e));

        // Every entry has seq and timestamp
        assert!(val["seq"].is_number(), "Line {} missing seq", i + 1);
        assert!(
            val["timestamp"].is_string(),
            "Line {} missing timestamp",
            i + 1
        );
        assert!(
            val["entry_type"].is_string(),
            "Line {} missing entry_type",
            i + 1
        );
    }

    // Verify Init entry uses canonical types (no provider-specific fields)
    let init: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(init["entry_type"], "init");
    assert!(init["model"].is_string());
    assert!(init["provider"].is_string());
    assert!(init["system_prompt"].is_string());

    // Verify Message entry uses canonical Role and ContentBlock
    let msg: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
    assert_eq!(msg["entry_type"], "message");
    assert_eq!(msg["role"], "assistant");
    assert_eq!(msg["content"][0]["type"], "text");
}

// ── Test: tool calls are journaled correctly ────────────────────────────

#[tokio::test]
async fn test_journal_with_tool_calls() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    setup_workgraph(&wg_dir);

    // Create a file for the read_file tool to read
    let test_file = tmp.path().join("test.txt");
    fs::write(&test_file, "hello world").unwrap();

    let task_id = "tool-test";
    let j_path = journal::journal_path(&wg_dir, task_id);

    let provider = MockProvider::with_tool_call(
        "read_file",
        serde_json::json!({"path": test_file.to_str().unwrap()}),
        "I read the file.",
    );
    let registry = ToolRegistry::default_all(&wg_dir, tmp.path());
    let output_log = wg_dir.join("test.ndjson");

    let mut agent = AgentLoop::new(
        Box::new(provider),
        registry,
        "Test agent.".to_string(),
        10,
        output_log,
    )
    .with_journal(j_path.clone(), task_id.to_string());

    let result = agent.run("Read the file.").await.unwrap();
    assert_eq!(result.turns, 2);

    let entries = Journal::read_all(&j_path).unwrap();

    // Expected: Init, User msg, Assistant msg (tool_use), ToolExecution, User msg (tool_result), Assistant msg (final), End
    assert_eq!(
        entries.len(),
        7,
        "Expected 7 entries, got {}: {:?}",
        entries.len(),
        entries
            .iter()
            .map(|e| match &e.kind {
                JournalEntryKind::Init { .. } => "Init",
                JournalEntryKind::Message { role, .. } => match role {
                    workgraph::executor::native::client::Role::User => "User",
                    workgraph::executor::native::client::Role::Assistant => "Assistant",
                },
                JournalEntryKind::ToolExecution { .. } => "ToolExecution",
                JournalEntryKind::Compaction { .. } => "Compaction",
                JournalEntryKind::End { .. } => "End",
            })
            .collect::<Vec<_>>()
    );

    // Verify ToolExecution entry
    let tool_exec = entries
        .iter()
        .find(|e| matches!(e.kind, JournalEntryKind::ToolExecution { .. }));
    assert!(tool_exec.is_some(), "Should have a ToolExecution entry");
    match &tool_exec.unwrap().kind {
        JournalEntryKind::ToolExecution {
            tool_use_id,
            name,
            duration_ms,
            ..
        } => {
            assert_eq!(tool_use_id, "tu-mock-1");
            assert_eq!(name, "read_file");
            assert!(*duration_ms < 5000, "Tool execution should be fast");
        }
        _ => unreachable!(),
    }

    // Verify ordering: ToolExecution comes after assistant's tool_use and before user's tool_result
    let tool_exec_idx = entries
        .iter()
        .position(|e| matches!(e.kind, JournalEntryKind::ToolExecution { .. }))
        .unwrap();
    // The entry before ToolExecution should be the assistant's tool_use message
    match &entries[tool_exec_idx - 1].kind {
        JournalEntryKind::Message { role, content, .. } => {
            assert_eq!(*role, workgraph::executor::native::client::Role::Assistant);
            assert!(
                content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolUse { .. }))
            );
        }
        _ => panic!("Entry before ToolExecution should be assistant message with tool_use"),
    }
    // The entry after ToolExecution should be the user's tool_result message
    match &entries[tool_exec_idx + 1].kind {
        JournalEntryKind::Message { role, content, .. } => {
            assert_eq!(*role, workgraph::executor::native::client::Role::User);
            assert!(
                content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
            );
        }
        _ => panic!("Entry after ToolExecution should be user message with tool_result"),
    }
}

// ── Test: journal survives simulated crash ──────────────────────────────

#[tokio::test]
async fn test_journal_survives_crash() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    setup_workgraph(&wg_dir);

    let task_id = "crash-test";
    let j_path = journal::journal_path(&wg_dir, task_id);

    // Run agent — writes Init, User, Assistant, End
    let provider = MockProvider::simple_text("First run.");
    let registry = ToolRegistry::default_all(&wg_dir, tmp.path());
    let output_log = wg_dir.join("test.ndjson");

    let mut agent = AgentLoop::new(
        Box::new(provider),
        registry,
        "Prompt.".to_string(),
        10,
        output_log,
    )
    .with_journal(j_path.clone(), task_id.to_string());

    agent.run("Start.").await.unwrap();

    let entries_before_crash = Journal::read_all(&j_path).unwrap();
    assert_eq!(entries_before_crash.len(), 4);

    // Simulate crash: append a corrupted line (partial JSON write)
    {
        use std::io::Write;
        let mut file = fs::OpenOptions::new().append(true).open(&j_path).unwrap();
        write!(
            file,
            "{{\"seq\":5,\"timestamp\":\"2026-01-01T00:00:00Z\",\"entry_type\":\"message\""
        )
        .unwrap();
        // No closing brace, no newline — simulates a crash mid-write
    }

    // Read back — should recover all 4 good entries, skip the corrupted one
    let entries_after_crash = Journal::read_all(&j_path).unwrap();
    assert_eq!(
        entries_after_crash.len(),
        4,
        "Should recover all entries written before crash"
    );

    // Verify the entries are intact
    assert!(matches!(
        entries_after_crash[0].kind,
        JournalEntryKind::Init { .. }
    ));
    assert!(matches!(
        entries_after_crash[3].kind,
        JournalEntryKind::End { .. }
    ));

    // Can reopen and continue writing
    let mut journal = Journal::open(&j_path).unwrap();
    assert_eq!(journal.seq(), 4, "Should resume from last valid seq");

    journal
        .append(JournalEntryKind::Message {
            role: workgraph::executor::native::client::Role::User,
            content: vec![ContentBlock::Text {
                text: "Resumed after crash.".to_string(),
            }],
            usage: None,
            response_id: None,
            stop_reason: None,
        })
        .unwrap();

    let final_entries = Journal::read_all(&j_path).unwrap();
    assert_eq!(
        final_entries.len(),
        5,
        "Should have 4 original + 1 new entry"
    );
    assert_eq!(final_entries[4].seq, 5);
}

// ── Test: agent without journal still works ─────────────────────────────

#[tokio::test]
async fn test_agent_without_journal() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    setup_workgraph(&wg_dir);

    let provider = MockProvider::simple_text("No journal.");
    let registry = ToolRegistry::default_all(&wg_dir, tmp.path());
    let output_log = wg_dir.join("test.ndjson");

    // No .with_journal() — should work fine
    let mut agent = AgentLoop::new(
        Box::new(provider),
        registry,
        "Test.".to_string(),
        10,
        output_log,
    );

    let result = agent.run("Hello.").await.unwrap();
    assert_eq!(result.turns, 1);
    assert_eq!(result.final_text, "No journal.");
}

// ── Test: max turns produces correct End entry ──────────────────────────

#[tokio::test]
async fn test_journal_max_turns_end_reason() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    setup_workgraph(&wg_dir);

    let task_id = "max-turns-test";
    let j_path = journal::journal_path(&wg_dir, task_id);

    // Provider that always asks for tool use (never ends)
    let responses: Vec<MessagesResponse> = (0..5)
        .map(|i| MessagesResponse {
            id: format!("msg-{}", i),
            content: vec![ContentBlock::ToolUse {
                id: format!("tu-{}", i),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "/dev/null"}),
            }],
            stop_reason: Some(StopReason::ToolUse),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Usage::default()
            },
        })
        .collect();

    let provider = MockProvider::new(responses);
    let registry = ToolRegistry::default_all(&wg_dir, tmp.path());
    let output_log = wg_dir.join("test.ndjson");

    let mut agent = AgentLoop::new(
        Box::new(provider),
        registry,
        "Test.".to_string(),
        2, // Max 2 turns
        output_log,
    )
    .with_journal(j_path.clone(), task_id.to_string());

    let result = agent.run("Go.").await.unwrap();
    assert_eq!(result.final_text, "");
    assert_eq!(result.exit_reason, "max_turns");
    assert!(!result.terminated_cleanly());

    let entries = Journal::read_all(&j_path).unwrap();
    let end_entry = entries.last().unwrap();
    match &end_entry.kind {
        JournalEntryKind::End { reason, turns, .. } => {
            assert!(matches!(reason, EndReason::MaxTurns));
            assert_eq!(*turns, 2);
        }
        _ => panic!("Last entry should be End with MaxTurns reason"),
    }
}
