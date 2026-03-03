//! Native executor CLI entry point.
//!
//! `wg native-exec` runs the Rust-native LLM agent loop for a task.
//! It is called by the spawn wrapper script when the executor type is "native".
//!
//! This command:
//! 1. Reads the prompt from a file
//! 2. Resolves the bundle for the exec_mode (tool filtering)
//! 3. Initializes the appropriate LLM client (Anthropic or OpenAI-compatible)
//! 4. Runs the agent loop to completion
//! 5. Exits with 0 on success, non-zero on failure

use std::path::Path;

use anyhow::{Context, Result};

use workgraph::executor::native::agent::AgentLoop;
use workgraph::executor::native::bundle::resolve_bundle;
use workgraph::executor::native::client::{AnthropicClient, LlmClient};
use workgraph::executor::native::openai_client::OpenAiClient;
use workgraph::executor::native::tools::ToolRegistry;

const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250514";

/// Resolve which LLM provider to use and create the appropriate client.
///
/// Resolution order:
/// 1. `[native_executor] provider` in config.toml ("anthropic" or "openai")
/// 2. `WG_LLM_PROVIDER` environment variable
/// 3. Heuristic: if model contains "/" (e.g., "openai/gpt-4o"), use OpenAI
/// 4. Default to "anthropic"
fn create_client(workgraph_dir: &Path, model: &str) -> Result<Box<dyn LlmClient>> {
    // Read config for provider and endpoint settings
    let config_path = workgraph_dir.join("config.toml");
    let config_val: Option<toml::Value> = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|c| toml::from_str(&c).ok());

    let native_cfg = config_val.as_ref().and_then(|v| v.get("native_executor"));

    // Resolve provider
    let provider = native_cfg
        .and_then(|c| c.get("provider"))
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| std::env::var("WG_LLM_PROVIDER").ok())
        .unwrap_or_else(|| {
            // Heuristic: models with "/" are likely OpenRouter format
            if model.contains('/') {
                "openai".to_string()
            } else {
                "anthropic".to_string()
            }
        });

    // Resolve optional base URL and max_tokens from config
    let api_base = native_cfg
        .and_then(|c| c.get("api_base"))
        .and_then(|v| v.as_str())
        .map(String::from);

    let max_tokens = native_cfg
        .and_then(|c| c.get("max_tokens"))
        .and_then(|v| v.as_integer())
        .map(|v| v as u32);

    match provider.as_str() {
        "openai" | "openrouter" => {
            let mut client = OpenAiClient::from_env(model)
                .or_else(|_| {
                    // Fall back to Anthropic key for OpenRouter (it accepts both)
                    let key = workgraph::executor::native::client::resolve_api_key_from_dir(
                        workgraph_dir,
                    )?;
                    OpenAiClient::new(key, model, None)
                })
                .context("Failed to initialize OpenAI-compatible client")?;
            if let Some(base) = api_base {
                client = client.with_base_url(&base);
            }
            if let Some(mt) = max_tokens {
                client = client.with_max_tokens(mt);
            }
            eprintln!(
                "[native-exec] Using OpenAI-compatible provider ({})",
                client.model
            );
            Ok(Box::new(client))
        }
        _ => {
            // Default: Anthropic
            let mut client = AnthropicClient::from_env(model)
                .context("Failed to initialize Anthropic client")?;
            if let Some(base) = api_base {
                client = client.with_base_url(&base);
            }
            if let Some(mt) = max_tokens {
                client = client.with_max_tokens(mt);
            }
            eprintln!("[native-exec] Using Anthropic provider ({})", client.model);
            Ok(Box::new(client))
        }
    }
}

/// Run the native executor agent loop.
pub fn run(
    workgraph_dir: &Path,
    prompt_file: &str,
    exec_mode: &str,
    task_id: &str,
    model: Option<&str>,
    max_turns: usize,
) -> Result<()> {
    let prompt = std::fs::read_to_string(prompt_file)
        .with_context(|| format!("Failed to read prompt file: {}", prompt_file))?;

    let effective_model = model
        .map(String::from)
        .or_else(|| std::env::var("WG_MODEL").ok())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    // Resolve the working directory (parent of .workgraph/)
    let working_dir = workgraph_dir
        .canonicalize()
        .ok()
        .and_then(|p| p.parent().map(|pp| pp.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Build the tool registry
    let mut registry = ToolRegistry::default_all(workgraph_dir, &working_dir);

    // Resolve bundle and filter tools
    let system_suffix = if let Some(bundle) = resolve_bundle(exec_mode, workgraph_dir) {
        let suffix = bundle.system_prompt_suffix.clone();
        registry = bundle.filter_registry(registry);
        suffix
    } else {
        String::new()
    };

    // Build full system prompt
    let system_prompt = if system_suffix.is_empty() {
        prompt
    } else {
        format!("{}\n\n{}", prompt, system_suffix)
    };

    // Build output log path
    let output_log = if let Ok(agent_id) = std::env::var("WG_AGENT_ID") {
        workgraph_dir
            .join("agents")
            .join(&agent_id)
            .join("agent.ndjson")
    } else {
        workgraph_dir.join("native-exec.ndjson")
    };

    eprintln!(
        "[native-exec] Starting agent loop for task '{}' with model '{}', exec_mode '{}', max_turns {}",
        task_id, effective_model, exec_mode, max_turns
    );

    // Create the API client (auto-selects provider)
    let client = create_client(workgraph_dir, &effective_model)?;

    // Create and run the agent loop
    let agent = AgentLoop::new(client, registry, system_prompt, max_turns, output_log);

    // Run the async agent loop
    let rt = tokio::runtime::Runtime::new().context("Failed to create tokio runtime")?;
    let result = rt.block_on(agent.run(&format!(
        "You are working on task '{}'. Complete the task as described in your system prompt. \
         When done, use the wg_done tool with task_id '{}'. \
         If you cannot complete the task, use the wg_fail tool with a reason.",
        task_id, task_id
    )))?;

    eprintln!(
        "[native-exec] Agent completed: {} turns, {}+{} tokens",
        result.turns, result.total_usage.input_tokens, result.total_usage.output_tokens
    );

    Ok(())
}
