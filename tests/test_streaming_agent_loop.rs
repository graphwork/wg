//! Tests for streaming agent loop (TDD).
//!
//! Verifies that the agent loop uses `send_streaming()` instead of `send()`,
//! writes text chunks to `stream.jsonl`, and updates the `.streaming` file
//! for TUI live display.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tempfile::TempDir;

use workgraph::executor::native::agent::AgentLoop;
use workgraph::executor::native::client::{
    ContentBlock, MessagesRequest, MessagesResponse, StopReason, Usage,
};
use workgraph::executor::native::provider::Provider;
use workgraph::executor::native::tools::ToolRegistry;
use workgraph::stream_event::{StreamEvent, read_stream_events};

// ── Mock provider ───────────────────────────────────────────────────────

struct MockStreamingProvider {
    streaming_called: Arc<AtomicBool>,
}

#[async_trait]
impl Provider for MockStreamingProvider {
    fn name(&self) -> &str {
        "mock"
    }

    fn model(&self) -> &str {
        "mock-model"
    }

    fn max_tokens(&self) -> u32 {
        1024
    }

    async fn send(&self, _: &MessagesRequest) -> anyhow::Result<MessagesResponse> {
        panic!("send() must not be called — streaming agent loop should use send_streaming()");
    }

    async fn send_streaming(
        &self,
        _: &MessagesRequest,
        on_text: &(dyn Fn(String) + Send + Sync),
    ) -> anyhow::Result<MessagesResponse> {
        self.streaming_called.store(true, Ordering::SeqCst);
        on_text("Hello ".to_string());
        on_text("streaming!".to_string());
        Ok(MessagesResponse {
            id: "msg_stream_test".to_string(),
            content: vec![ContentBlock::Text {
                text: "Hello streaming!".to_string(),
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

// ── Tests ────────────────────────────────────────────────────────────────

/// Agent loop must call `send_streaming()` not `send()`, and must write
/// `TextChunk` events to `stream.jsonl` as chunks arrive.
#[tokio::test]
async fn test_streaming_agent_loop() {
    let dir = TempDir::new().unwrap();
    let output_log = dir.path().join("output.log");
    let stream_path = dir.path().join("stream.jsonl");

    let streaming_called = Arc::new(AtomicBool::new(false));
    let provider = MockStreamingProvider {
        streaming_called: Arc::clone(&streaming_called),
    };

    let mut agent = AgentLoop::new(
        Box::new(provider),
        ToolRegistry::new(),
        "Test system prompt".to_string(),
        10,
        output_log,
    );

    let result = agent.run("Test initial message").await.unwrap();

    // send_streaming() must have been called instead of send()
    assert!(
        streaming_called.load(Ordering::SeqCst),
        "send_streaming() was not called — agent loop still uses send()"
    );

    // Final result must be correct
    assert_eq!(result.final_text, "Hello streaming!");
    assert_eq!(result.turns, 1);

    // TextChunk events must be present in stream.jsonl
    assert!(stream_path.exists(), "stream.jsonl was not created");
    let (events, _) = read_stream_events(&stream_path, 0).unwrap();
    let text_chunks: Vec<String> = events
        .into_iter()
        .filter_map(|e| match e {
            StreamEvent::TextChunk { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert!(
        !text_chunks.is_empty(),
        "No TextChunk events written to stream.jsonl"
    );
    assert_eq!(
        text_chunks,
        vec!["Hello ".to_string(), "streaming!".to_string()],
        "Text chunks don't match expected values"
    );
}

/// Agent loop must write accumulated text to .streaming file during streaming.
#[tokio::test]
async fn test_streaming_file_written_during_streaming() {
    use std::sync::Mutex;

    let dir = TempDir::new().unwrap();
    let output_log = dir.path().join("output.log");
    let streaming_path = dir.path().join(".streaming");

    // Capture streaming file contents at callback time
    let captured_streaming: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    struct CaptureProvider {
        streaming_path: std::path::PathBuf,
        captured: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl Provider for CaptureProvider {
        fn name(&self) -> &str {
            "capture"
        }
        fn model(&self) -> &str {
            "capture-model"
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
            on_text: &(dyn Fn(String) + Send + Sync),
        ) -> anyhow::Result<MessagesResponse> {
            on_text("chunk1 ".to_string());
            // After chunk1 arrives, .streaming should have "chunk1 "
            if let Ok(contents) = std::fs::read_to_string(&self.streaming_path) {
                let mut cap = self.captured.lock().unwrap();
                cap.push(contents);
            }

            on_text("chunk2".to_string());
            // After chunk2 arrives, .streaming should have "chunk1 chunk2"
            if let Ok(contents) = std::fs::read_to_string(&self.streaming_path) {
                let mut cap = self.captured.lock().unwrap();
                cap.push(contents);
            }

            Ok(MessagesResponse {
                id: "msg_capture".to_string(),
                content: vec![ContentBlock::Text {
                    text: "chunk1 chunk2".to_string(),
                }],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage {
                    input_tokens: 5,
                    output_tokens: 2,
                    ..Default::default()
                },
            })
        }
    }

    let provider = CaptureProvider {
        streaming_path: streaming_path.clone(),
        captured: Arc::clone(&captured_streaming),
    };

    let mut agent = AgentLoop::new(
        Box::new(provider),
        ToolRegistry::new(),
        "Test system prompt".to_string(),
        10,
        output_log,
    );

    let _result = agent.run("Test message").await.unwrap();

    let captured = captured_streaming.lock().unwrap();
    assert_eq!(captured.len(), 2, "Expected 2 streaming captures");
    assert_eq!(captured[0], "chunk1 ", "First capture should be 'chunk1 '");
    assert_eq!(
        captured[1], "chunk1 chunk2",
        "Second capture should be 'chunk1 chunk2'"
    );
}

/// Non-streaming fallback: if `send_streaming()` is not overridden,
/// the default implementation falls back to `send()` — agent loop
/// should handle this gracefully.
#[tokio::test]
async fn test_streaming_fallback_to_send() {
    let dir = TempDir::new().unwrap();
    let output_log = dir.path().join("output.log");

    struct FallbackProvider;

    #[async_trait]
    impl Provider for FallbackProvider {
        fn name(&self) -> &str {
            "fallback"
        }
        fn model(&self) -> &str {
            "fallback-model"
        }
        fn max_tokens(&self) -> u32 {
            1024
        }

        async fn send(&self, _: &MessagesRequest) -> anyhow::Result<MessagesResponse> {
            Ok(MessagesResponse {
                id: "msg_fallback".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Fallback works".to_string(),
                }],
                stop_reason: Some(StopReason::EndTurn),
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 5,
                    ..Default::default()
                },
            })
        }
        // send_streaming not overridden — uses default which calls send()
    }

    let mut agent = AgentLoop::new(
        Box::new(FallbackProvider),
        ToolRegistry::new(),
        "Test system prompt".to_string(),
        10,
        output_log,
    );

    let result = agent.run("Test message").await.unwrap();
    assert_eq!(result.final_text, "Fallback works");
    assert_eq!(result.turns, 1);
}
