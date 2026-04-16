//! Research tool: high-level web research primitive.
//!
//! One tool call, one answer. Internally composes:
//! 1. `web_search` (parallel fan-out across 9 backends)
//! 2. Headless Chrome fetch of the top N result URLs
//! 3. `summarize` (recursive map-reduce) on each fetched page
//! 4. Merge per-page summaries into a single research brief
//!
//! The agent gets a focused ~500-word answer with source citations
//! instead of having to orchestrate a 4-step pipeline manually.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::json;

use super::{Tool, ToolOutput};
use crate::executor::native::client::ToolDefinition;

/// How many top search results to fetch and summarize.
const MAX_PAGES_TO_FETCH: usize = 4;

/// Maximum chars of fetched page content to feed into summarize.
/// Pages longer than this are truncated before summarization.
const MAX_PAGE_CHARS: usize = 32_000;

/// Maximum chars for the final merged research brief.
const MAX_BRIEF_CHARS: usize = 8_000;

pub fn register_research_tool(registry: &mut super::ToolRegistry, workgraph_dir: PathBuf) {
    registry.register(Box::new(ResearchTool { workgraph_dir }));
}

struct ResearchTool {
    workgraph_dir: PathBuf,
}

#[async_trait]
impl Tool for ResearchTool {
    fn name(&self) -> &str {
        "research"
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "research".to_string(),
            description: "Research a topic on the web. Searches multiple backends, fetches \
                          the top results via headless Chrome, summarizes each page with your \
                          query as focus, and returns a merged research brief with source \
                          citations. Use this when you need factual information from the web \
                          — it handles the full search→fetch→read→synthesize pipeline in one \
                          call. Returns a focused answer, not a list of URLs."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The research question or topic to investigate"
                    },
                    "instruction": {
                        "type": "string",
                        "description": "Optional focus instruction for summarization. \
                                        E.g. 'extract publication dates and titles', \
                                        'focus on methodology', 'list key findings'. \
                                        Defaults to summarizing relevant information \
                                        matching the query."
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn execute(&self, input: &serde_json::Value) -> ToolOutput {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) if !q.trim().is_empty() => q.trim().to_string(),
            _ => {
                return ToolOutput::error("Missing or empty required parameter: query".to_string());
            }
        };

        let instruction = input
            .get("instruction")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| {
                format!(
                    "Extract information relevant to: {}. \
                     Include specific names, dates, numbers, and facts. \
                     Cite the source page title.",
                    query
                )
            });

        eprintln!(
            "\x1b[2m[research] searching: {:?}\x1b[0m",
            truncate(&query, 80)
        );

        // Step 1: Web search
        let search_input = json!({"query": query});
        let search_tool = super::web_search::WebSearchToolInternal;
        let search_result = search_tool.execute(&search_input).await;

        if search_result.is_error {
            return ToolOutput::error(format!(
                "Research failed at search step: {}",
                search_result.content
            ));
        }

        // Parse URLs from the search results (plain text format).
        let (mut urls, google_news_urls) = extract_urls_from_search_results(&search_result.content);

        // Resolve Google News RSS redirects to actual article URLs.
        if !google_news_urls.is_empty() {
            eprintln!(
                "\x1b[2m[research] resolving {} Google News redirect(s)\x1b[0m",
                google_news_urls.len()
            );
            for gnurl in &google_news_urls {
                if let Some(resolved) = resolve_google_news_redirect(gnurl).await {
                    urls.push(resolved);
                }
            }
        }

        if urls.is_empty() {
            return ToolOutput::error(format!(
                "Search returned no fetchable URLs for query: {:?}\n\n{}",
                query,
                &search_result.content[..search_result.content.len().min(500)]
            ));
        }

        let urls_to_fetch: Vec<_> = urls.into_iter().take(MAX_PAGES_TO_FETCH).collect();

        eprintln!(
            "\x1b[2m[research] fetching {} pages via Chrome\x1b[0m",
            urls_to_fetch.len()
        );

        // Step 2: Fetch pages via headless Chrome (primary, not rquest)
        // Chrome handles JS-rendered sites (PubMed, Scholar, Nature) natively.
        let mut page_contents: Vec<(String, String)> = Vec::new(); // (url, content)

        for url in &urls_to_fetch {
            let content = match fetch_page_content(url).await {
                Ok(c) if !c.trim().is_empty() && c.len() > 50 => c,
                _ => continue, // Skip failed/empty fetches
            };

            // Truncate before summarization
            let truncated = if content.len() > MAX_PAGE_CHARS {
                content[..content.floor_char_boundary(MAX_PAGE_CHARS)].to_string()
            } else {
                content
            };

            page_contents.push((url.clone(), truncated));
        }

        if page_contents.is_empty() {
            // Fall back to returning just the search snippets
            return ToolOutput::success(format!(
                "Research for: {}\n\n\
                 Could not fetch any of the result pages (JS-heavy sites or access denied). \
                 Here are the search snippets:\n\n{}",
                query,
                &search_result.content[..search_result.content.len().min(4000)]
            ));
        }

        eprintln!(
            "\x1b[2m[research] summarizing {} pages\x1b[0m",
            page_contents.len()
        );

        // Step 3: Summarize each page with the query as focus.
        // Resolve the model from config the same way the agent does —
        // WG_MODEL env var (set by the coordinator for task agents) or
        // the configured task_agent model from config.toml.
        let config = crate::config::Config::load_or_default(&self.workgraph_dir);
        let model = std::env::var("WG_MODEL")
            .ok()
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| {
                config
                    .resolve_model_for_role(crate::config::DispatchRole::TaskAgent)
                    .model
            });

        let provider =
            match crate::executor::native::provider::create_provider(&self.workgraph_dir, &model) {
                Ok(p) => p,
                Err(e) => {
                    return ToolOutput::error(format!(
                        "Failed to create provider for summarization (model: {}): {}",
                        model, e
                    ));
                }
            };

        let mut summaries: Vec<String> = Vec::new();

        for (url, content) in &page_contents {
            let page_instruction = format!("{}. This content is from: {}", instruction, url);

            match super::summarize::recursive_summarize(
                provider.as_ref(),
                content,
                &page_instruction,
                0,
            )
            .await
            {
                Ok(summary) if !summary.trim().is_empty() => {
                    summaries.push(format!("### Source: {}\n\n{}", url, summary.trim()));
                }
                Ok(_) => {
                    // Empty summary — page had no relevant content
                }
                Err(e) => {
                    eprintln!(
                        "\x1b[2m[research] summarize failed for {}: {}\x1b[0m",
                        truncate(url, 60),
                        e
                    );
                }
            }
        }

        // Step 4: Merge into a research brief
        if summaries.is_empty() {
            return ToolOutput::success(format!(
                "Research for: {}\n\n\
                 Fetched {} pages but none contained information relevant to the query. \
                 Search snippets:\n\n{}",
                query,
                page_contents.len(),
                &search_result.content[..search_result.content.len().min(4000)]
            ));
        }

        let merged = format!(
            "Research brief: {}\n\
             Sources consulted: {} pages\n\n\
             {}\n\n\
             ---\n\
             Note: This brief was synthesized from web search results. \
             For full page content, use `web_fetch` on the source URLs above.",
            query,
            summaries.len(),
            summaries.join("\n\n"),
        );

        // Truncate the final brief if needed
        let final_brief = if merged.len() > MAX_BRIEF_CHARS {
            let end = merged.floor_char_boundary(MAX_BRIEF_CHARS);
            format!(
                "{}\n\n[... truncated at {} chars ...]",
                &merged[..end],
                MAX_BRIEF_CHARS
            )
        } else {
            merged
        };

        eprintln!(
            "\x1b[2m[research] done: {} sources, {} chars\x1b[0m",
            summaries.len(),
            final_brief.len()
        );

        ToolOutput::success(final_brief)
    }
}

/// Extract URLs from the plain-text web_search output format.
/// Looks for lines matching `    URL: https://...`
/// Google News RSS URLs are collected separately for redirect resolution.
fn extract_urls_from_search_results(text: &str) -> (Vec<String>, Vec<String>) {
    let mut direct = Vec::new();
    let mut google_news = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(url) = trimmed.strip_prefix("URL: ").map(|u| u.trim().to_string()) {
            if url.contains("news.google.com/rss/articles/") {
                google_news.push(url);
            } else {
                direct.push(url);
            }
        }
    }
    (direct, google_news)
}

/// Resolve a Google News RSS redirect URL to the actual article URL.
/// Uses rquest with redirect disabled to capture the Location header.
async fn resolve_google_news_redirect(url: &str) -> Option<String> {
    let client = rquest::Client::builder()
        .emulation(rquest_util::Emulation::Chrome136)
        .redirect(rquest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    resp.headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
}

/// Fetch a page via headless Chrome (primary) with rquest fallback.
/// Returns extracted markdown text.
async fn fetch_page_content(url: &str) -> Result<String, String> {
    use std::io::Cursor;

    // Try headless Chrome first — handles JS-rendered sites
    let html = match super::web_search::get_or_launch_browser_for_fetch().await {
        Ok(cell) => {
            let page = {
                let guard = cell.lock().await;
                let handle = guard
                    .as_ref()
                    .ok_or_else(|| "browser not ready".to_string())?;
                handle
                    .browser
                    .new_page(url)
                    .await
                    .map_err(|e| format!("new_page: {}", e))?
            };
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let content = match page.content().await {
                Ok(c) => c,
                Err(e) => {
                    let _ = page.close().await;
                    return Err(format!("content: {}", e));
                }
            };
            let _ = page.close().await;
            content
        }
        Err(_) => {
            // Chrome not available — fall back to rquest
            let client = rquest::Client::builder()
                .emulation(rquest_util::Emulation::Chrome136)
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .map_err(|e| format!("client: {}", e))?;
            let resp = client
                .get(url)
                .send()
                .await
                .map_err(|e| format!("fetch: {}", e))?;
            if !resp.status().is_success() {
                return Err(format!("HTTP {}", resp.status()));
            }
            resp.text().await.map_err(|e| format!("body: {}", e))?
        }
    };

    // Extract readable content via readability + html2md
    let parsed_url = url::Url::parse(url).map_err(|e| format!("parse url: {}", e))?;
    let mut cursor = Cursor::new(html.as_bytes());
    let markdown = match readability::extractor::extract(&mut cursor, &parsed_url) {
        Ok(product) => {
            let md = html2md::parse_html(&product.content);
            if !product.title.is_empty() {
                format!("# {}\n\n{}", product.title, md)
            } else {
                md
            }
        }
        Err(_) => html2md::parse_html(&html),
    };

    Ok(markdown)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        &s[..s.floor_char_boundary(max)]
    }
}
