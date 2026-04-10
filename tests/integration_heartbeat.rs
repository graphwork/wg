//! Tests for heartbeat signal during long-running tool execution.
//!
//! Verifies that the agent loop emits periodic `Heartbeat` stream events
//! while tools are running, enabling the coordinator to detect agent liveness.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tempfile::TempDir;

use workgraph::executor::native::agent::AgentLoop;
use workgraph::executor::native::client::{
    ContentBlock, MessagesRequest, MessagesResponse, StopReason, Usage,
};
use workgraph::executor::native::provider::Provider;
use workgraph::executor::native::tools::ToolRegistry;
use workgraph::executor::native::tools::bash::register_bash_tool;
use workgraph::stream_event::{StreamEvent, read_stream_events};

// ── Mock provider that triggers a bash tool call ────────────────────────

struct HeartbeatTestProvider {
    call_count: Arc<AtomicUsize>,
}

#[async_trait]
impl Provider for HeartbeatTestProvider {
    fn name(&self) -> &str {
        "heartbeat-test"
    }

    fn model(&self) -> &str {
        "test-model"
    }

    fn max_tokens(&self) -> u32 {
        1024
    }

    async fn send(&self, _: &MessagesRequest) -> anyhow::Result<MessagesResponse> {
        panic!("send() must not be called");
    }

    async fn send_streaming(
        &self,
        _: &MessagesRequest,
        _on_text: &(dyn Fn(String) + Send + Sync),
    ) -> anyhow::Result<MessagesResponse> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst);

        if n == 0 {
            // First call: request a bash tool that sleeps for 3 seconds
            Ok(MessagesResponse {
                id: "msg_tool".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "tu_1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({
                        "command": "sleep 3",
                        "timeout": 10000
                    }),
                }],
                stop_reason: Some(StopReason::ToolUse),
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
            })
        } else {
            // Second call: return final text
            Ok(MessagesResponse {
                id: "msg_done".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Done".to_string(),
                }],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
            })
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

/// Heartbeat events must be written to stream.jsonl during a long-running
/// bash command. With a 1-second heartbeat interval and a 3-second sleep,
/// we expect at least 2 heartbeat events.
#[tokio::test]
async fn test_heartbeat_during_long_bash() {
    let dir = TempDir::new().unwrap();
    let output_log = dir.path().join("output.log");
    let stream_path = dir.path().join("stream.jsonl");

    let provider = HeartbeatTestProvider {
        call_count: Arc::new(AtomicUsize::new(0)),
    };

    // Register bash tool with the temp dir as working directory
    let mut registry = ToolRegistry::new();
    register_bash_tool(&mut registry, dir.path().to_path_buf());

    let mut agent = AgentLoop::new(
        Box::new(provider),
        registry,
        "Test system prompt".to_string(),
        10,
        output_log,
    )
    .with_heartbeat_interval(Duration::from_secs(1));

    let result = agent.run("Run a sleep command").await.unwrap();
    assert_eq!(result.final_text, "Done");

    // Read stream events and count heartbeats
    let (events, _) = read_stream_events(&stream_path, 0).unwrap();

    let heartbeats: Vec<&StreamEvent> = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Heartbeat { .. }))
        .collect();

    assert!(
        heartbeats.len() >= 2,
        "Expected at least 2 heartbeats during 3s sleep with 1s interval, got {}. Events: {:?}",
        heartbeats.len(),
        events
            .iter()
            .map(|e| format!("{:?}", e))
            .collect::<Vec<_>>()
    );

    // Verify heartbeats have valid timestamps
    for hb in &heartbeats {
        if let StreamEvent::Heartbeat { timestamp_ms } = hb {
            assert!(*timestamp_ms > 0, "Heartbeat timestamp should be positive");
        }
    }

    // Verify heartbeats appear between ToolStart and ToolEnd
    let tool_start_idx = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolStart { .. }))
        .expect("Should have ToolStart event");
    let tool_end_idx = events
        .iter()
        .position(|e| matches!(e, StreamEvent::ToolEnd { .. }))
        .expect("Should have ToolEnd event");
    let first_heartbeat_idx = events
        .iter()
        .position(|e| matches!(e, StreamEvent::Heartbeat { .. }))
        .expect("Should have Heartbeat event");

    assert!(
        first_heartbeat_idx > tool_start_idx,
        "First heartbeat should come after ToolStart"
    );
    assert!(
        first_heartbeat_idx < tool_end_idx,
        "First heartbeat should come before ToolEnd"
    );
}

/// Verify that the coordinator's AgentStreamState correctly processes
/// heartbeat events for liveness detection.
#[test]
fn test_coordinator_reads_heartbeat_for_liveness() {
    use workgraph::stream_event::{AgentStreamState, now_ms};

    let mut state = AgentStreamState::default();

    // Simulate ingesting a heartbeat event
    let events = vec![StreamEvent::Heartbeat {
        timestamp_ms: now_ms(),
    }];
    state.ingest(&events, 100);

    // After ingesting heartbeat, last_event_ms should be set
    assert!(
        state.last_event_ms.is_some(),
        "last_event_ms should be set after heartbeat"
    );

    // Stream should not be stale (heartbeat was just now)
    assert!(
        !state.is_stale(5000),
        "Stream should not be stale right after a heartbeat"
    );
}

/// Fast tool execution should not produce heartbeats (interval not reached).
#[tokio::test]
async fn test_no_heartbeat_for_fast_tools() {
    let dir = TempDir::new().unwrap();
    let output_log = dir.path().join("output.log");
    let stream_path = dir.path().join("stream.jsonl");

    let provider = HeartbeatTestProvider {
        call_count: Arc::new(AtomicUsize::new(0)),
    };

    // Use a provider that runs "echo fast" instead of sleep
    struct FastToolProvider {
        call_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Provider for FastToolProvider {
        fn name(&self) -> &str {
            "fast-test"
        }
        fn model(&self) -> &str {
            "test-model"
        }
        fn max_tokens(&self) -> u32 {
            1024
        }

        async fn send(&self, _: &MessagesRequest) -> anyhow::Result<MessagesResponse> {
            panic!("send() should not be called");
        }

        async fn send_streaming(
            &self,
            _: &MessagesRequest,
            _on_text: &(dyn Fn(String) + Send + Sync),
        ) -> anyhow::Result<MessagesResponse> {
            let n = self.call_count.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Ok(MessagesResponse {
                    id: "msg_tool".to_string(),
                    content: vec![ContentBlock::ToolUse {
                        id: "tu_1".to_string(),
                        name: "bash".to_string(),
                        input: serde_json::json!({
                            "command": "echo fast",
                            "timeout": 5000
                        }),
                    }],
                    stop_reason: Some(StopReason::ToolUse),
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        ..Default::default()
                    },
                })
            } else {
                Ok(MessagesResponse {
                    id: "msg_done".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "Done".to_string(),
                    }],
                    stop_reason: Some(StopReason::EndTurn),
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 5,
                        ..Default::default()
                    },
                })
            }
        }
    }

    let fast_provider = FastToolProvider {
        call_count: Arc::new(AtomicUsize::new(0)),
    };

    let mut registry = ToolRegistry::new();
    register_bash_tool(&mut registry, dir.path().to_path_buf());

    // Drop the unused provider for the test
    drop(provider);

    let mut agent = AgentLoop::new(
        Box::new(fast_provider),
        registry,
        "Test".to_string(),
        10,
        output_log,
    )
    .with_heartbeat_interval(Duration::from_secs(30));

    let result = agent.run("Run a fast command").await.unwrap();
    assert_eq!(result.final_text, "Done");

    let (events, _) = read_stream_events(&stream_path, 0).unwrap();
    let heartbeats = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::Heartbeat { .. }))
        .count();

    assert_eq!(
        heartbeats, 0,
        "Fast tool execution should not produce heartbeats"
    );
}
