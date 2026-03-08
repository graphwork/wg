//! OpenAI-compatible HTTP client for chat completions.
//!
//! Supports OpenRouter, direct OpenAI, and any API that implements the
//! OpenAI chat completions format (Ollama, vLLM, Together, etc.).
//!
//! Translates between the canonical Anthropic-style types used by the
//! agent loop and the OpenAI wire format.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};

use super::client::{
    ContentBlock, Message, MessagesRequest, MessagesResponse, Role, StopReason, ToolDefinition,
    Usage,
};

// ── OpenAI wire format types ────────────────────────────────────────────

/// OpenAI-format tool definition.
#[derive(Debug, Clone, Serialize)]
struct OaiToolDef {
    #[serde(rename = "type")]
    tool_type: String,
    function: OaiFunctionDef,
}

#[derive(Debug, Clone, Serialize)]
struct OaiFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// OpenAI-format message for the request.
#[derive(Debug, Clone, Serialize)]
struct OaiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OaiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

/// OpenAI-format request body.
#[derive(Debug, Clone, Serialize)]
struct OaiRequest {
    model: String,
    messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OaiToolDef>,
    stream: bool,
}

/// OpenAI-format response body.
#[derive(Debug, Clone, Deserialize)]
struct OaiResponse {
    #[allow(dead_code)]
    id: String,
    choices: Vec<OaiChoice>,
    #[serde(default)]
    usage: Option<OaiUsage>,
}

#[derive(Debug, Clone, Deserialize)]
struct OaiChoice {
    message: OaiResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OaiResponseMessage {
    #[allow(dead_code)]
    role: String,
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OaiToolCall>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OaiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OaiToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OaiToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Clone, Deserialize)]
struct OaiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

/// OpenAI-format error response.
#[derive(Debug, Clone, Deserialize)]
struct OaiErrorResponse {
    error: OaiErrorDetail,
}

#[derive(Debug, Clone, Deserialize)]
struct OaiErrorDetail {
    message: String,
    #[serde(default)]
    #[allow(dead_code)]
    code: Option<serde_json::Value>,
}

// ── Client ──────────────────────────────────────────────────────────────

const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";
const DEFAULT_MAX_TOKENS: u32 = 16384;

/// OpenAI-compatible chat completions client.
///
/// Works with OpenRouter, direct OpenAI API, and any compatible endpoint.
pub struct OpenAiClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    pub model: String,
    pub max_tokens: u32,
    /// Provider hint for provider-specific behavior (e.g. "openrouter", "openai", "local").
    provider_hint: Option<String>,
}

impl OpenAiClient {
    /// Create a client with explicit configuration.
    pub fn new(api_key: String, model: &str, base_url: Option<&str>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self {
            http,
            api_key,
            base_url: base_url
                .unwrap_or(DEFAULT_BASE_URL)
                .trim_end_matches('/')
                .to_string(),
            model: model.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            provider_hint: None,
        })
    }

    /// Create from environment variables.
    ///
    /// Checks `OPENROUTER_API_KEY`, `OPENAI_API_KEY` in that order.
    /// Uses `OPENAI_BASE_URL` or `OPENROUTER_BASE_URL` for the endpoint.
    pub fn from_env(model: &str) -> Result<Self> {
        let api_key = resolve_openai_api_key()?;
        let base_url = std::env::var("OPENAI_BASE_URL")
            .or_else(|_| std::env::var("OPENROUTER_BASE_URL"))
            .ok();
        Self::new(api_key, model, base_url.as_deref())
    }

    /// Override the base URL.
    pub fn with_base_url(mut self, url: &str) -> Self {
        self.base_url = url.trim_end_matches('/').to_string();
        self
    }

    /// Override max tokens per response.
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Set a provider hint for provider-specific behavior.
    ///
    /// When set to `"openrouter"`, adds `HTTP-Referer` and `X-Title` attribution
    /// headers to requests.
    pub fn with_provider_hint(mut self, hint: &str) -> Self {
        self.provider_hint = Some(hint.to_string());
        self
    }

    /// Convert canonical tool definitions to OpenAI format.
    fn translate_tools(tools: &[ToolDefinition]) -> Vec<OaiToolDef> {
        tools
            .iter()
            .map(|t| OaiToolDef {
                tool_type: "function".to_string(),
                function: OaiFunctionDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.input_schema.clone(),
                },
            })
            .collect()
    }

    /// Convert canonical messages to OpenAI format.
    fn translate_messages(system: &Option<String>, messages: &[Message]) -> Vec<OaiMessage> {
        let mut oai_messages = Vec::new();

        // System message first
        if let Some(sys) = system {
            oai_messages.push(OaiMessage {
                role: "system".to_string(),
                content: Some(sys.clone()),
                tool_calls: None,
                tool_call_id: None,
            });
        }

        for msg in messages {
            match msg.role {
                Role::User => {
                    // User messages may contain text or tool results
                    let has_tool_results = msg
                        .content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolResult { .. }));

                    if has_tool_results {
                        // Each tool result becomes a separate message with role "tool"
                        for block in &msg.content {
                            match block {
                                ContentBlock::ToolResult {
                                    tool_use_id,
                                    content,
                                    ..
                                } => {
                                    oai_messages.push(OaiMessage {
                                        role: "tool".to_string(),
                                        content: Some(content.clone()),
                                        tool_calls: None,
                                        tool_call_id: Some(tool_use_id.clone()),
                                    });
                                }
                                ContentBlock::Text { text } => {
                                    oai_messages.push(OaiMessage {
                                        role: "user".to_string(),
                                        content: Some(text.clone()),
                                        tool_calls: None,
                                        tool_call_id: None,
                                    });
                                }
                                _ => {}
                            }
                        }
                    } else {
                        // Regular text message
                        let text: String = msg
                            .content
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::Text { text } => Some(text.as_str()),
                                _ => None,
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        oai_messages.push(OaiMessage {
                            role: "user".to_string(),
                            content: Some(text),
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                }
                Role::Assistant => {
                    // Collect text and tool_calls from content blocks
                    let mut text_parts = Vec::new();
                    let mut tool_calls = Vec::new();

                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text } => {
                                text_parts.push(text.clone());
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                tool_calls.push(OaiToolCall {
                                    id: id.clone(),
                                    call_type: "function".to_string(),
                                    function: OaiToolCallFunction {
                                        name: name.clone(),
                                        arguments: serde_json::to_string(input).unwrap_or_default(),
                                    },
                                });
                            }
                            _ => {}
                        }
                    }

                    let content = if text_parts.is_empty() {
                        None
                    } else {
                        Some(text_parts.join("\n"))
                    };

                    let tc = if tool_calls.is_empty() {
                        None
                    } else {
                        Some(tool_calls)
                    };

                    oai_messages.push(OaiMessage {
                        role: "assistant".to_string(),
                        content,
                        tool_calls: tc,
                        tool_call_id: None,
                    });
                }
            }
        }

        oai_messages
    }

    /// Convert an OpenAI response to canonical format.
    fn translate_response(oai: OaiResponse) -> Result<MessagesResponse> {
        let choice = oai
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Empty choices in API response"))?;

        let mut content_blocks = Vec::new();

        // Add text content if present
        if let Some(text) = choice.message.content
            && !text.is_empty()
        {
            content_blocks.push(ContentBlock::Text { text });
        }

        // Add tool calls if present
        if let Some(tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                let input: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or_default();
                content_blocks.push(ContentBlock::ToolUse {
                    id: tc.id,
                    name: tc.function.name,
                    input,
                });
            }
        }

        // If no content at all, add empty text
        if content_blocks.is_empty() {
            content_blocks.push(ContentBlock::Text {
                text: String::new(),
            });
        }

        let stop_reason = match choice.finish_reason.as_deref() {
            Some("stop") => Some(StopReason::EndTurn),
            Some("tool_calls") => Some(StopReason::ToolUse),
            Some("length") => Some(StopReason::MaxTokens),
            Some("content_filter") => Some(StopReason::StopSequence),
            _ => None,
        };

        let usage = oai
            .usage
            .map(|u| Usage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
                cache_creation_input_tokens: None,
                cache_read_input_tokens: None,
            })
            .unwrap_or_default();

        Ok(MessagesResponse {
            id: oai.id,
            content: content_blocks,
            stop_reason,
            usage,
        })
    }

    /// Send a non-streaming request.
    async fn chat_completion(&self, request: &MessagesRequest) -> Result<MessagesResponse> {
        let oai_request = OaiRequest {
            model: request.model.clone(),
            messages: Self::translate_messages(&request.system, &request.messages),
            max_tokens: Some(request.max_tokens),
            tools: Self::translate_tools(&request.tools),
            stream: false,
        };

        let url = format!("{}/chat/completions", self.base_url);
        self.send_with_retry(&url, &oai_request).await
    }

    /// Send a request with retry logic.
    async fn send_with_retry(&self, url: &str, request: &OaiRequest) -> Result<MessagesResponse> {
        let max_retries = 5;
        let mut retry_count = 0;
        let mut backoff_ms = 1000u64;

        loop {
            let headers = self.build_headers();
            let resp = self
                .http
                .post(url)
                .headers(headers)
                .json(request)
                .send()
                .await;

            match resp {
                Ok(response) => {
                    let status = response.status();

                    if status.is_success() {
                        let body = response
                            .text()
                            .await
                            .context("Failed to read response body")?;
                        let oai_resp: OaiResponse =
                            serde_json::from_str(&body).with_context(|| {
                                format!("Failed to parse API response: {}", truncate(&body, 500))
                            })?;
                        return Self::translate_response(oai_resp);
                    }

                    let status_code = status.as_u16();
                    let body = response.text().await.unwrap_or_default();

                    if is_retryable(status_code) && retry_count < max_retries {
                        retry_count += 1;
                        let wait = parse_retry_after_oai(&body).unwrap_or(backoff_ms);
                        eprintln!(
                            "[openai-client] Retryable error {} (attempt {}/{}), waiting {}ms",
                            status_code, retry_count, max_retries, wait
                        );
                        tokio::time::sleep(Duration::from_millis(wait)).await;
                        backoff_ms = (backoff_ms * 2).min(60_000);
                        continue;
                    }

                    return Err(oai_api_error(status_code, &body));
                }
                Err(e) => {
                    if retry_count < max_retries {
                        retry_count += 1;
                        eprintln!(
                            "[openai-client] Network error (attempt {}/{}): {}",
                            retry_count, max_retries, e
                        );
                        tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                        backoff_ms = (backoff_ms * 2).min(60_000);
                        continue;
                    }
                    return Err(e).context("Network error after retries");
                }
            }
        }
    }

    fn build_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                .expect("invalid api key header"),
        );
        headers.insert("content-type", HeaderValue::from_static("application/json"));

        // OpenRouter attribution headers
        if self.provider_hint.as_deref() == Some("openrouter") {
            headers.insert(
                "http-referer",
                HeaderValue::from_static("https://github.com/anthropics/workgraph"),
            );
            headers.insert("x-title", HeaderValue::from_static("workgraph"));
        }

        headers
    }
}

#[async_trait::async_trait]
impl super::provider::Provider for OpenAiClient {
    fn name(&self) -> &str {
        self.provider_hint.as_deref().unwrap_or("openai")
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn max_tokens(&self) -> u32 {
        self.max_tokens
    }

    async fn send(&self, request: &MessagesRequest) -> Result<MessagesResponse> {
        self.chat_completion(request).await
    }
}

// ── API key resolution ──────────────────────────────────────────────────

/// Resolve an OpenAI-compatible API key.
///
/// Priority: OPENROUTER_API_KEY > OPENAI_API_KEY > config file
fn resolve_openai_api_key() -> Result<String> {
    for var in &["OPENROUTER_API_KEY", "OPENAI_API_KEY"] {
        if let Ok(key) = std::env::var(var)
            && !key.is_empty()
        {
            return Ok(key);
        }
    }

    // Try config file
    if let Ok(content) = std::fs::read_to_string(".workgraph/config.toml")
        && let Ok(val) = toml::from_str::<toml::Value>(&content)
        && let Some(key) = val
            .get("native_executor")
            .and_then(|v| v.get("api_key"))
            .and_then(|v| v.as_str())
        && !key.is_empty()
    {
        return Ok(key.to_string());
    }

    Err(anyhow!(
        "No OpenAI-compatible API key found. Set OPENROUTER_API_KEY or OPENAI_API_KEY \
         environment variable, or add [native_executor] api_key to .workgraph/config.toml"
    ))
}

/// Resolve API key from a specific workgraph directory.
pub fn resolve_openai_api_key_from_dir(workgraph_dir: &std::path::Path) -> Result<String> {
    for var in &["OPENROUTER_API_KEY", "OPENAI_API_KEY"] {
        if let Ok(key) = std::env::var(var)
            && !key.is_empty()
        {
            return Ok(key);
        }
    }

    let config_path = workgraph_dir.join("config.toml");
    if let Ok(content) = std::fs::read_to_string(&config_path)
        && let Ok(val) = toml::from_str::<toml::Value>(&content)
        && let Some(key) = val
            .get("native_executor")
            .and_then(|v| v.get("api_key"))
            .and_then(|v| v.as_str())
        && !key.is_empty()
    {
        return Ok(key.to_string());
    }

    Err(anyhow!(
        "No OpenAI-compatible API key found. Set OPENROUTER_API_KEY or OPENAI_API_KEY \
         environment variable, or add [native_executor] api_key to .workgraph/config.toml"
    ))
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn is_retryable(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503)
}

fn oai_api_error(status: u16, body: &str) -> anyhow::Error {
    if let Ok(err) = serde_json::from_str::<OaiErrorResponse>(body) {
        anyhow!("OpenAI API error {}: {}", status, err.error.message)
    } else {
        anyhow!("OpenAI API error {}: {}", status, truncate(body, 500))
    }
}

fn parse_retry_after_oai(body: &str) -> Option<u64> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(body)
        && let Some(secs) = val
            .get("error")
            .and_then(|e| e.get("metadata"))
            .and_then(|m| m.get("retry_after"))
            .and_then(|v| v.as_f64())
    {
        return Some((secs * 1000.0) as u64);
    }
    None
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..s.floor_char_boundary(max)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::native::client::{ContentBlock, Message, Role, ToolDefinition};
    use serde_json::json;

    #[test]
    fn test_translate_tools() {
        let tools = vec![ToolDefinition {
            name: "bash".to_string(),
            description: "Execute a shell command".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                },
                "required": ["command"]
            }),
        }];

        let oai_tools = OpenAiClient::translate_tools(&tools);
        assert_eq!(oai_tools.len(), 1);
        assert_eq!(oai_tools[0].tool_type, "function");
        assert_eq!(oai_tools[0].function.name, "bash");
    }

    #[test]
    fn test_translate_messages_with_system() {
        let system = Some("You are a helpful assistant.".to_string());
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        }];

        let oai_msgs = OpenAiClient::translate_messages(&system, &messages);
        assert_eq!(oai_msgs.len(), 2);
        assert_eq!(oai_msgs[0].role, "system");
        assert_eq!(
            oai_msgs[0].content.as_deref(),
            Some("You are a helpful assistant.")
        );
        assert_eq!(oai_msgs[1].role, "user");
        assert_eq!(oai_msgs[1].content.as_deref(), Some("Hello"));
    }

    #[test]
    fn test_translate_messages_with_tool_results() {
        let messages = vec![Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call_123".to_string(),
                content: "result data".to_string(),
                is_error: false,
            }],
        }];

        let oai_msgs = OpenAiClient::translate_messages(&None, &messages);
        assert_eq!(oai_msgs.len(), 1);
        assert_eq!(oai_msgs[0].role, "tool");
        assert_eq!(oai_msgs[0].tool_call_id.as_deref(), Some("call_123"));
        assert_eq!(oai_msgs[0].content.as_deref(), Some("result data"));
    }

    #[test]
    fn test_translate_messages_with_assistant_tool_calls() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "Let me run that.".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "call_456".to_string(),
                    name: "bash".to_string(),
                    input: json!({"command": "ls"}),
                },
            ],
        }];

        let oai_msgs = OpenAiClient::translate_messages(&None, &messages);
        assert_eq!(oai_msgs.len(), 1);
        assert_eq!(oai_msgs[0].role, "assistant");
        assert_eq!(oai_msgs[0].content.as_deref(), Some("Let me run that."));
        let tc = oai_msgs[0].tool_calls.as_ref().unwrap();
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].id, "call_456");
        assert_eq!(tc[0].function.name, "bash");
    }

    #[test]
    fn test_translate_response_text_only() {
        let oai = OaiResponse {
            id: "chatcmpl-123".to_string(),
            choices: vec![OaiChoice {
                message: OaiResponseMessage {
                    role: "assistant".to_string(),
                    content: Some("Hello!".to_string()),
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(OaiUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
            }),
        };

        let resp = OpenAiClient::translate_response(oai).unwrap();
        assert_eq!(resp.content.len(), 1);
        assert!(matches!(&resp.content[0], ContentBlock::Text { text } if text == "Hello!"));
        assert_eq!(resp.stop_reason, Some(StopReason::EndTurn));
        assert_eq!(resp.usage.input_tokens, 10);
        assert_eq!(resp.usage.output_tokens, 5);
    }

    #[test]
    fn test_translate_response_with_tool_calls() {
        let oai = OaiResponse {
            id: "chatcmpl-456".to_string(),
            choices: vec![OaiChoice {
                message: OaiResponseMessage {
                    role: "assistant".to_string(),
                    content: None,
                    tool_calls: Some(vec![OaiToolCall {
                        id: "call_789".to_string(),
                        call_type: "function".to_string(),
                        function: OaiToolCallFunction {
                            name: "bash".to_string(),
                            arguments: r#"{"command":"ls -la"}"#.to_string(),
                        },
                    }]),
                },
                finish_reason: Some("tool_calls".to_string()),
            }],
            usage: Some(OaiUsage {
                prompt_tokens: 20,
                completion_tokens: 15,
            }),
        };

        let resp = OpenAiClient::translate_response(oai).unwrap();
        assert_eq!(resp.content.len(), 1);
        assert!(matches!(
            &resp.content[0],
            ContentBlock::ToolUse { id, name, input }
            if id == "call_789" && name == "bash" && input.get("command").is_some()
        ));
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
    }

    #[test]
    fn test_translate_response_max_tokens() {
        let oai = OaiResponse {
            id: "chatcmpl-max".to_string(),
            choices: vec![OaiChoice {
                message: OaiResponseMessage {
                    role: "assistant".to_string(),
                    content: Some("partial...".to_string()),
                    tool_calls: None,
                },
                finish_reason: Some("length".to_string()),
            }],
            usage: None,
        };

        let resp = OpenAiClient::translate_response(oai).unwrap();
        assert_eq!(resp.stop_reason, Some(StopReason::MaxTokens));
    }

    // ── OpenRouter-specific tests ───────────────────────────────────────

    #[test]
    fn test_openrouter_headers_included() {
        let client = OpenAiClient::new("test-key".into(), "test/model", None)
            .unwrap()
            .with_provider_hint("openrouter");
        let headers = client.build_headers();
        assert_eq!(
            headers.get("http-referer").unwrap(),
            "https://github.com/anthropics/workgraph"
        );
        assert_eq!(headers.get("x-title").unwrap(), "workgraph");
    }

    #[test]
    fn test_non_openrouter_no_extra_headers() {
        let client = OpenAiClient::new("test-key".into(), "gpt-4o", None)
            .unwrap()
            .with_provider_hint("openai");
        let headers = client.build_headers();
        assert!(headers.get("http-referer").is_none());
        assert!(headers.get("x-title").is_none());
    }

    #[test]
    fn test_no_hint_no_extra_headers() {
        let client = OpenAiClient::new("test-key".into(), "gpt-4o", None).unwrap();
        let headers = client.build_headers();
        assert!(headers.get("http-referer").is_none());
        assert!(headers.get("x-title").is_none());
    }

    #[test]
    fn test_openrouter_url_construction() {
        let client =
            OpenAiClient::new("test-key".into(), "minimax/minimax-m2.5", None).unwrap();
        assert!(client.base_url.ends_with("/v1"));
        let expected = format!("{}/chat/completions", client.base_url);
        assert_eq!(expected, "https://openrouter.ai/api/v1/chat/completions");
    }

    #[test]
    fn test_model_id_passthrough() {
        let c1 = OpenAiClient::new("k".into(), "minimax/minimax-m2.5", None).unwrap();
        assert_eq!(c1.model, "minimax/minimax-m2.5");
        let c2 = OpenAiClient::new("k".into(), "openai/gpt-4o-mini", None).unwrap();
        assert_eq!(c2.model, "openai/gpt-4o-mini");
    }

    #[test]
    fn test_provider_hint_sets_name() {
        use super::super::provider::Provider;
        let client = OpenAiClient::new("test-key".into(), "test/model", None)
            .unwrap()
            .with_provider_hint("openrouter");
        assert_eq!(client.name(), "openrouter");
        let client2 = OpenAiClient::new("test-key".into(), "test/model", None)
            .unwrap()
            .with_provider_hint("local");
        assert_eq!(client2.name(), "local");
        let client3 = OpenAiClient::new("test-key".into(), "gpt-4o", None).unwrap();
        assert_eq!(client3.name(), "openai");
    }

    #[test]
    fn test_local_provider_placeholder_key() {
        use super::super::provider::Provider;
        let client = OpenAiClient::new("local".into(), "llama3.1", None)
            .unwrap()
            .with_provider_hint("local");
        assert_eq!(client.name(), "local");
        let headers = client.build_headers();
        assert_eq!(headers.get("authorization").unwrap(), "Bearer local");
        assert!(headers.get("http-referer").is_none());
    }

    #[test]
    fn test_openrouter_response_gen_prefix_id() {
        let oai = OaiResponse {
            id: "gen-abc123".to_string(),
            choices: vec![OaiChoice {
                message: OaiResponseMessage {
                    role: "assistant".to_string(),
                    content: Some("Hello from OpenRouter".to_string()),
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(OaiUsage {
                prompt_tokens: 5,
                completion_tokens: 4,
            }),
        };
        let resp = OpenAiClient::translate_response(oai).unwrap();
        assert_eq!(resp.id, "gen-abc123");
        assert!(
            matches!(&resp.content[0], ContentBlock::Text { text } if text == "Hello from OpenRouter")
        );
    }
}
