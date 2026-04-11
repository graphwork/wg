//! Web search tool using DuckDuckGo.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::{Tool, ToolOutput, truncate_for_tool};
use crate::executor::native::client::ToolDefinition;

/// Maximum snippet length before truncation.
const MAX_SNIPPET_LEN: usize = 300;

/// Register the web_search tool.
pub fn register_web_search_tool(registry: &mut super::ToolRegistry) {
    registry.register(Box::new(WebSearchTool));
}

struct WebSearchTool;

#[derive(Debug, Deserialize)]
struct DuckDuckGoResponse {
    #[serde(default)]
    RelatedTopics: Vec<RelatedTopic>,
    #[serde(default)]
    Answer: String,
    #[serde(default)]
    AbstractText: String,
    #[serde(default)]
    AbstractURL: String,
}

#[derive(Debug, Deserialize)]
struct RelatedTopic {
    #[serde(default)]
    Text: String,
    #[serde(default)]
    FirstURL: String,
    #[serde(default)]
    Result: String,
    // Some topics are nested (e.g., "Topics" field)
    #[serde(default)]
    Topics: Vec<RelatedTopic>,
}

#[derive(Debug, serde::Serialize)]
struct SearchResult {
    title: String,
    snippet: String,
    url: String,
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "web_search".to_string(),
            description: "Search the web using DuckDuckGo and return structured results."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, input: &serde_json::Value) -> ToolOutput {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return ToolOutput::error("Missing required parameter: query".to_string()),
        };

        let url = format!(
            "https://api.duckduckgo.com/?q={}&format=json&no_redirect=1",
            urlencoding::encode(query)
        );

        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("Failed to create HTTP client: {}", e)),
        };

        let response = match client.get(&url).send().await {
            Ok(resp) => resp,
            Err(_e) => {
                // Fallback to lite version if main API fails
                return self.search_lite(query).await;
            }
        };

        if !response.status().is_success() {
            return self.search_lite(query).await;
        }

        let body = match response.text().await {
            Ok(text) => text,
            Err(e) => return ToolOutput::error(format!("Failed to read response: {}", e)),
        };

        let ddg_response: DuckDuckGoResponse = match serde_json::from_str(&body) {
            Ok(r) => r,
            Err(_) => return self.search_lite(query).await,
        };

        let results = self.parse_ddg_response(ddg_response);

        if results.is_empty() {
            return self.search_lite(query).await;
        }

        let output = json!({
            "query": query,
            "source": "duckduckgo",
            "results": results
        });

        ToolOutput::success(truncate_for_tool(
            &serde_json::to_string_pretty(&output).unwrap_or_default(),
            "web_search",
        ))
    }
}

impl WebSearchTool {
    fn parse_ddg_response(&self, response: DuckDuckGoResponse) -> Vec<SearchResult> {
        let mut results = Vec::new();

        // First check for an instant answer
        if !response.Answer.is_empty() {
            results.push(SearchResult {
                title: "Instant Answer".to_string(),
                snippet: truncate_snippet(&response.Answer),
                url: response.AbstractURL.clone(),
            });
        }

        // Check abstract/definition
        if !response.AbstractText.is_empty() && results.is_empty() {
            results.push(SearchResult {
                title: response.AbstractText.chars().take(100).collect(),
                snippet: truncate_snippet(&response.AbstractText),
                url: response.AbstractURL.clone(),
            });
        }

        self.extract_results(&response.RelatedTopics, &mut results);

        results.truncate(20);
        results
    }

    fn extract_results(&self, topics: &[RelatedTopic], results: &mut Vec<SearchResult>) {
        for topic in topics {
            // Skip empty entries
            if topic.Text.is_empty() && topic.FirstURL.is_empty() {
                continue;
            }

            // Some topics have nested topics
            if !topic.Topics.is_empty() {
                self.extract_results(&topic.Topics, results);
                continue;
            }

            // Use Text as snippet, but also check Result field which may have better content
            let snippet = if !topic.Result.is_empty() {
                topic.Result.clone()
            } else {
                topic.Text.clone()
            };

            // Skip if snippet is just an empty heredoc marker or too short
            if snippet.is_empty() || snippet == "<ud>" || snippet.len() < 10 {
                continue;
            }

            let url = topic.FirstURL.clone();

            // Build title from snippet or URL
            let title = if !topic.Text.is_empty() {
                topic.Text.chars().take(100).collect()
            } else if !url.is_empty() {
                url.split('/')
                    .next_back()
                    .unwrap_or(&url)
                    .chars()
                    .take(60)
                    .collect::<String>()
                    .replace(['-', '_'], " ")
            } else {
                continue;
            };

            results.push(SearchResult {
                title,
                snippet: truncate_snippet(&snippet),
                url,
            });
        }
    }

    /// Fallback: parse HTML from lite.duckduckgo.com
    async fn search_lite(&self, query: &str) -> ToolOutput {
        let url = format!(
            "https://lite.duckduckgo.com/lite/?q={}",
            urlencoding::encode(query)
        );

        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => return ToolOutput::error(format!("Failed to create HTTP client: {}", e)),
        };

        let response = match client.get(&url).send().await {
            Ok(resp) => resp,
            Err(e) => return ToolOutput::error(format!("Web search failed: {}", e)),
        };

        if !response.status().is_success() {
            return ToolOutput::error(format!(
                "Web search failed with status: {}",
                response.status()
            ));
        }

        let body = match response.text().await {
            Ok(text) => text,
            Err(e) => return ToolOutput::error(format!("Failed to read response: {}", e)),
        };

        let results = self.parse_lite_html(&body);

        let output = json!({
            "query": query,
            "source": "duckduckgo",
            "results": results
        });

        ToolOutput::success(truncate_for_tool(
            &serde_json::to_string_pretty(&output).unwrap_or_default(),
            "web_search",
        ))
    }

    fn parse_lite_html(&self, html: &str) -> Vec<SearchResult> {
        use regex::Regex;

        let mut results = Vec::new();

        // Pattern: <a href="URL">TITLE</a> ... SNIPPET
        let re = Regex::new(r#"<a\s+href="([^"]+)"[^>]*>([^<]+)</a>"#).ok();

        if let Some(re) = re {
            // Simple approach: extract text between <a> tags as titles
            // and nearby text as snippets
            for cap in re.captures_iter(html) {
                let url = cap.get(1).map(|m| m.as_str()).unwrap_or_default();
                let title = cap.get(2).map(|m| m.as_str().trim()).unwrap_or_default();

                // Filter out navigation/ads links
                if url.contains("duckduckgo") || url.contains("yandex") || title.is_empty() {
                    continue;
                }

                results.push(SearchResult {
                    title: title.chars().take(100).collect(),
                    snippet: format!("Result from: {}", url),
                    url: url.to_string(),
                });

                if results.len() >= 20 {
                    break;
                }
            }
        }

        results
    }
}

fn truncate_snippet(snippet: &str) -> String {
    // Remove HTML tags if any
    let cleaned = regex::Regex::new(r"<[^>]+>")
        .ok()
        .map(|re| re.replace_all(snippet, "").to_string())
        .unwrap_or_else(|| snippet.to_string());

    // Decode HTML entities
    let decoded = html_escape::decode_html_entities(&cleaned).to_string();

    if decoded.len() > MAX_SNIPPET_LEN {
        let end = decoded.floor_char_boundary(MAX_SNIPPET_LEN);
        format!("{}...", &decoded[..end])
    } else {
        decoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_snippet_short() {
        let short = "This is a short snippet";
        assert_eq!(truncate_snippet(short), short);
    }

    #[test]
    fn test_truncate_snippet_long() {
        let long = "a".repeat(500);
        let result = truncate_snippet(&long);
        assert!(result.ends_with("..."));
        assert!(result.len() <= MAX_SNIPPET_LEN + 3);
    }

    #[test]
    fn test_truncate_snippet_html() {
        let with_html = "<p>Hello <b>world</b></p>";
        let result = truncate_snippet(with_html);
        assert!(!result.contains("<p>"));
        assert!(result.contains("Hello"));
        assert!(result.contains("world"));
    }

    #[tokio::test]
    async fn test_web_search_empty_query() {
        let tool = WebSearchTool;
        let input = serde_json::json!({});
        let output = tool.execute(&input).await;
        assert!(output.is_error);
        assert!(output.content.contains("Missing required parameter"));
    }

    #[tokio::test]
    async fn test_web_search_read_only() {
        let tool = WebSearchTool;
        assert!(tool.is_read_only());
    }
}
