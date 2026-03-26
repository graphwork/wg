//! Interactive configuration wizard for first-time workgraph setup.
//!
//! Creates/updates ~/.workgraph/config.toml via guided prompts using dialoguer.

use anyhow::{Context, Result, bail};
use dialoguer::{Confirm, Input, Select};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use workgraph::config::{Config, ModelRegistryEntry};
use workgraph::models::ModelRegistry;

/// Marker used to detect whether workgraph directives are already present in CLAUDE.md.
const CLAUDE_MD_MARKER: &str = "<!-- workgraph-managed -->";

/// The workgraph directives block appended to CLAUDE.md.
const CLAUDE_MD_DIRECTIVES: &str = r#"<!-- workgraph-managed -->
# Workgraph

Use workgraph for task management.

**At the start of each session, run `wg quickstart` in your terminal to orient yourself.**
Use `wg service start` to dispatch work — do not manually claim tasks.

## For All Agents (Including the Orchestrating Agent)

CRITICAL: Do NOT use built-in TaskCreate/TaskUpdate/TaskList/TaskGet tools.
These are a separate system that does NOT interact with workgraph.
Always use `wg` CLI commands for all task management.

CRITICAL: Do NOT use the built-in **Task tool** (subagents). NEVER spawn Explore, Plan,
general-purpose, or any other subagent type. The Task tool creates processes outside
workgraph, which defeats the entire system. If you need research, exploration, or planning
done — create a `wg add` task and let the coordinator dispatch it.

ALL tasks — including research, exploration, and planning — should be workgraph tasks.

### Orchestrating agent role

The orchestrating agent (the one the user interacts with directly) does ONLY:
- **Conversation** with the user
- **Inspection** via `wg show`, `wg viz`, `wg list`, `wg status`, and reading files
- **Task creation** via `wg add` with descriptions, dependencies, and context
- **Monitoring** via `wg agents`, `wg service status`, `wg watch`

It NEVER writes code, implements features, or does research itself.
Everything gets dispatched through `wg add` and `wg service start`.
"#;

/// Choices gathered from the interactive wizard.
#[derive(Debug, Clone)]
pub struct SetupChoices {
    pub provider: String,
    pub executor: String,
    pub model: String,
    pub agency_enabled: bool,
    pub evaluator_model: Option<String>,
    pub assigner_model: Option<String>,
    pub max_agents: usize,
    /// Endpoint config for non-Anthropic providers
    pub endpoint: Option<EndpointChoices>,
    /// Model registry entries to add
    pub model_registry_entries: Vec<ModelRegistryEntry>,
}

/// Endpoint configuration gathered from the wizard.
#[derive(Debug, Clone)]
pub struct EndpointChoices {
    pub name: String,
    pub provider: String,
    pub url: String,
    pub api_key_env: Option<String>,
    pub api_key_file: Option<String>,
}

/// Build a Config from wizard choices, optionally layered on top of an existing config.
pub fn build_config(choices: &SetupChoices, base: Option<&Config>) -> Config {
    use workgraph::config::{EndpointConfig, EndpointsConfig, RoleModelConfig};

    let mut config = base.cloned().unwrap_or_default();

    config.coordinator.executor = Some(choices.executor.clone());
    config.agent.executor = choices.executor.clone();

    config.agent.model = choices.model.clone();
    config.coordinator.model = Some(choices.model.clone());

    config.coordinator.max_agents = choices.max_agents;

    // Set coordinator provider for non-Anthropic providers
    if choices.provider != "anthropic" {
        config.coordinator.provider = Some(choices.provider.clone());
    }

    // Set models.default with provider
    if choices.provider != "anthropic" {
        config.models.default = Some(RoleModelConfig {
            provider: Some(choices.provider.clone()),
            model: Some(choices.model.clone()),
            tier: None,
            endpoint: None,
        });
    }

    // Configure endpoint
    if let Some(ref ep) = choices.endpoint {
        let endpoint = EndpointConfig {
            name: ep.name.clone(),
            provider: ep.provider.clone(),
            url: Some(ep.url.clone()),
            model: None,
            api_key: None,
            api_key_file: ep.api_key_file.clone(),
            api_key_env: ep.api_key_env.clone(),
            is_default: true,
        };
        config.llm_endpoints = EndpointsConfig {
            endpoints: vec![endpoint],
        };
    }

    // Add model registry entries
    if !choices.model_registry_entries.is_empty() {
        config.model_registry = choices.model_registry_entries.clone();
    }

    config.agency.auto_assign = choices.agency_enabled;
    config.agency.auto_evaluate = choices.agency_enabled;

    if let Some(ref eval_model) = choices.evaluator_model {
        config.agency.evaluator_model = Some(eval_model.clone());
    }
    if let Some(ref assign_model) = choices.assigner_model {
        config.agency.assigner_model = Some(assign_model.clone());
    }

    config
}

/// Format a summary of what will be written.
pub fn format_summary(choices: &SetupChoices) -> String {
    let mut lines = Vec::new();
    lines.push("[coordinator]".to_string());
    lines.push(format!("  executor = \"{}\"", choices.executor));
    lines.push(format!("  model = \"{}\"", choices.model));
    lines.push(format!("  max_agents = {}", choices.max_agents));
    if choices.provider != "anthropic" {
        lines.push(format!("  provider = \"{}\"", choices.provider));
    }
    lines.push(String::new());
    lines.push("[agent]".to_string());
    lines.push(format!("  executor = \"{}\"", choices.executor));
    lines.push(format!("  model = \"{}\"", choices.model));
    if choices.provider != "anthropic" {
        lines.push(String::new());
        lines.push("[models.default]".to_string());
        lines.push(format!("  provider = \"{}\"", choices.provider));
        lines.push(format!("  model = \"{}\"", choices.model));
    }
    if let Some(ref ep) = choices.endpoint {
        lines.push(String::new());
        lines.push("[[llm_endpoints.endpoints]]".to_string());
        lines.push(format!("  name = \"{}\"", ep.name));
        lines.push(format!("  provider = \"{}\"", ep.provider));
        lines.push(format!("  url = \"{}\"", ep.url));
        if let Some(ref env) = ep.api_key_env {
            lines.push(format!("  api_key_env = \"{}\"", env));
        }
        if let Some(ref file) = ep.api_key_file {
            lines.push(format!("  api_key_file = \"{}\"", file));
        }
        lines.push("  is_default = true".to_string());
    }
    if !choices.model_registry_entries.is_empty() {
        for entry in &choices.model_registry_entries {
            lines.push(String::new());
            lines.push("[[model_registry]]".to_string());
            lines.push(format!("  id = \"{}\"", entry.id));
            lines.push(format!("  provider = \"{}\"", entry.provider));
            lines.push(format!("  model = \"{}\"", entry.model));
        }
    }
    lines.push(String::new());
    lines.push("[agency]".to_string());
    lines.push(format!("  auto_assign = {}", choices.agency_enabled));
    lines.push(format!("  auto_evaluate = {}", choices.agency_enabled));
    if let Some(ref m) = choices.evaluator_model {
        lines.push(format!("  evaluator_model = \"{}\"", m));
    }
    if let Some(ref m) = choices.assigner_model {
        lines.push(format!("  assigner_model = \"{}\"", m));
    }
    lines.join("\n")
}

/// Check whether a CLAUDE.md file already contains workgraph directives.
pub fn has_workgraph_directives(path: &Path) -> bool {
    if let Ok(content) = std::fs::read_to_string(path) {
        content.contains(CLAUDE_MD_MARKER)
    } else {
        false
    }
}

/// Configure ~/.claude/CLAUDE.md with workgraph directives.
///
/// - If ~/.claude/ doesn't exist, it is created.
/// - If CLAUDE.md doesn't exist, it is created with the directives.
/// - If CLAUDE.md exists but has no workgraph marker, directives are appended.
/// - If CLAUDE.md already contains the marker, it is left unchanged (idempotent).
///
/// Returns a status string for display and whether changes were made.
pub fn configure_claude_md() -> Result<(String, bool)> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    let claude_dir = PathBuf::from(&home).join(".claude");
    let claude_md = claude_dir.join("CLAUDE.md");

    configure_claude_md_at(&claude_md)
}

/// Configure a CLAUDE.md at the given project directory.
///
/// Creates or updates `<project_dir>/CLAUDE.md` with workgraph directives.
/// Same idempotency rules as `configure_claude_md`.
pub fn configure_project_claude_md(project_dir: &Path) -> Result<(String, bool)> {
    let claude_md = project_dir.join("CLAUDE.md");
    configure_claude_md_at(&claude_md)
}

/// Shared implementation for configuring a CLAUDE.md at a specific path.
fn configure_claude_md_at(claude_md: &Path) -> Result<(String, bool)> {
    if has_workgraph_directives(claude_md) {
        return Ok((format!("{} already configured", claude_md.display()), false));
    }

    // Ensure parent directory exists
    if let Some(parent) = claude_md.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    if claude_md.exists() {
        // Append to existing file
        let existing = std::fs::read_to_string(claude_md)
            .with_context(|| format!("Failed to read {}", claude_md.display()))?;
        let separator = if existing.ends_with('\n') || existing.is_empty() {
            "\n"
        } else {
            "\n\n"
        };
        let new_content = format!("{}{}{}", existing, separator, CLAUDE_MD_DIRECTIVES);
        std::fs::write(claude_md, new_content)
            .with_context(|| format!("Failed to write {}", claude_md.display()))?;
        Ok((
            format!("Updated {} with workgraph directives", claude_md.display()),
            true,
        ))
    } else {
        // Create new file
        std::fs::write(claude_md, CLAUDE_MD_DIRECTIVES)
            .with_context(|| format!("Failed to create {}", claude_md.display()))?;
        Ok((
            format!("Created {} with workgraph directives", claude_md.display()),
            true,
        ))
    }
}

/// Run the interactive setup wizard.
pub fn run() -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("wg setup requires an interactive terminal");
    }

    // Load existing global config for defaults
    let existing = Config::load_global()?.unwrap_or_default();
    let global_path = Config::global_config_path()?;

    println!("Welcome to workgraph setup.");
    println!(
        "This will configure your global defaults at {}",
        global_path.display()
    );
    println!();

    // 1. Provider selection (primary decision point)
    let provider_options = &[
        "Anthropic (direct)",
        "OpenRouter",
        "OpenAI",
        "Local (Ollama/vLLM)",
        "Custom",
    ];
    let provider_keys = &["anthropic", "openrouter", "openai", "local", "custom"];

    let current_provider = existing
        .coordinator
        .provider
        .as_deref()
        .unwrap_or("anthropic");
    let current_provider_idx = provider_keys
        .iter()
        .position(|&p| p == current_provider)
        .unwrap_or(0);

    let provider_idx = Select::new()
        .with_prompt("Which LLM provider?")
        .items(provider_options)
        .default(current_provider_idx)
        .interact()?;

    let provider = provider_keys[provider_idx].to_string();

    // 2. Auto-set executor based on provider, with override option
    let default_executor = match provider.as_str() {
        "anthropic" => "claude",
        "openrouter" | "openai" | "local" => "native",
        _ => "native",
    };

    println!();
    println!(
        "  Executor auto-set to '{}' for {} provider.",
        default_executor, provider
    );

    let override_executor = Confirm::new()
        .with_prompt("Override executor?")
        .default(false)
        .interact()?;

    let executor = if override_executor {
        let executor_options = &["claude", "native", "amplifier", "custom"];
        let current_idx = executor_options
            .iter()
            .position(|&e| e == default_executor)
            .unwrap_or(0);
        let idx = Select::new()
            .with_prompt("Which executor backend?")
            .items(executor_options)
            .default(current_idx)
            .interact()?;
        if idx == 3 {
            let custom: String = Input::new()
                .with_prompt("Custom executor name")
                .interact_text()?;
            custom
        } else {
            executor_options[idx].to_string()
        }
    } else {
        default_executor.to_string()
    };

    // 3. Provider-specific configuration
    let (endpoint, model_registry_entries, model) = match provider.as_str() {
        "openrouter" => configure_openrouter(&existing)?,
        "openai" => configure_openai(&existing)?,
        "local" => configure_local(&existing)?,
        "custom" => configure_custom_provider(&existing)?,
        _ => configure_anthropic(&existing)?,
    };

    // 4. Agency
    println!();
    let agency_enabled = Confirm::new()
        .with_prompt("Enable agency (auto-assign agents to tasks, auto-evaluate completed work)?")
        .default(existing.agency.auto_assign || existing.agency.auto_evaluate)
        .interact()?;

    let (evaluator_model, assigner_model) = if agency_enabled {
        let eval_options = &[
            "haiku (recommended, lightweight)",
            "sonnet",
            "same as default",
        ];
        let current_eval_idx = match existing.agency.evaluator_model.as_deref() {
            Some("sonnet") => 1,
            Some(m) if m == model => 2,
            _ => 0,
        };
        let eval_idx = Select::new()
            .with_prompt("Evaluator model?")
            .items(eval_options)
            .default(current_eval_idx)
            .interact()?;
        let eval_model = match eval_idx {
            0 => Some("haiku".to_string()),
            1 => Some("sonnet".to_string()),
            _ => None,
        };

        let assign_options = &["haiku (recommended, cheap)", "sonnet", "same as default"];
        let current_assign_idx = match existing.agency.assigner_model.as_deref() {
            Some("sonnet") => 1,
            Some(m) if m == model => 2,
            _ => 0,
        };
        let assign_idx = Select::new()
            .with_prompt("Assigner model?")
            .items(assign_options)
            .default(current_assign_idx)
            .interact()?;
        let assign_model = match assign_idx {
            0 => Some("haiku".to_string()),
            1 => Some("sonnet".to_string()),
            _ => None,
        };

        (eval_model, assign_model)
    } else {
        (None, None)
    };

    // 5. Max agents
    let max_agents: usize = Input::new()
        .with_prompt("Max parallel agents?")
        .default(existing.coordinator.max_agents)
        .interact_text()?;

    let choices = SetupChoices {
        provider: provider.clone(),
        executor,
        model,
        agency_enabled,
        evaluator_model,
        assigner_model,
        max_agents,
        endpoint,
        model_registry_entries,
    };

    // 6. Summary and confirmation
    println!();
    println!("Configuration to write:");
    println!("───────────────────────");
    println!("{}", format_summary(&choices));
    println!("───────────────────────");
    println!();

    let confirm = Confirm::new()
        .with_prompt(format!("Write to {}?", global_path.display()))
        .default(true)
        .interact()?;

    if !confirm {
        println!("Setup cancelled.");
        return Ok(());
    }

    // Build and save
    let config = build_config(&choices, Some(&existing));
    config.save_global()?;

    // Post-save: guide skill/bundle installation based on executor
    println!();
    let skill_status = guide_skill_bundle_install(&choices.executor)?;

    // Configure ~/.claude/CLAUDE.md for Claude Code executor
    let claude_md_status = if choices.executor == "claude" {
        println!();
        guide_claude_md_install()?
    } else {
        "N/A (non-Claude executor)".to_string()
    };

    println!();
    println!("Setup complete.");
    println!();
    println!("Summary:");
    println!("  Provider:  {}", choices.provider);
    println!("  Executor:  {}", choices.executor);
    println!("  Model:     {}", choices.model);
    println!("  Agents:    {} max parallel", choices.max_agents);
    if choices.endpoint.is_some() {
        println!("  Endpoint:  configured");
    }
    println!("  Skill:     {}", skill_status);
    println!("  CLAUDE.md: {}", claude_md_status);
    println!();
    println!("Run `wg init` in a project directory to get started.");

    Ok(())
}

/// Configure OpenRouter provider: API key, model selection, endpoint.
fn configure_openrouter(
    existing: &Config,
) -> Result<(Option<EndpointChoices>, Vec<ModelRegistryEntry>, String)> {
    println!();
    println!("OpenRouter configuration");
    println!("────────────────────────");

    // API key setup
    let api_key_options = &[
        "Environment variable (OPENROUTER_API_KEY)",
        "Key file (e.g., ~/.config/openrouter/key)",
    ];
    let key_idx = Select::new()
        .with_prompt("How should the API key be provided?")
        .items(api_key_options)
        .default(0)
        .interact()?;

    let (api_key_env, api_key_file) = if key_idx == 0 {
        // Check if the env var is already set
        if std::env::var("OPENROUTER_API_KEY").is_ok() {
            println!("  OPENROUTER_API_KEY is set in your environment.");
        } else {
            println!("  Set OPENROUTER_API_KEY in your shell profile before running agents.");
            println!("  Example: export OPENROUTER_API_KEY=sk-or-...");
        }
        (Some("OPENROUTER_API_KEY".to_string()), None)
    } else {
        let default_path = "~/.config/openrouter/key".to_string();
        let key_path: String = Input::new()
            .with_prompt("Path to API key file")
            .default(default_path)
            .interact_text()?;
        println!("  Make sure the key file exists and contains your OpenRouter API key.");
        (None, Some(key_path))
    };

    // Model selection
    println!();
    let model_method_options = &[
        "Enter model ID manually",
        "Use popular defaults (Claude via OpenRouter)",
    ];
    let method_idx = Select::new()
        .with_prompt("How would you like to select models?")
        .items(model_method_options)
        .default(1)
        .interact()?;

    let (model, registry_entries) = if method_idx == 0 {
        // Manual model entry
        let current_model = existing
            .coordinator
            .model
            .as_deref()
            .unwrap_or("anthropic/claude-sonnet-4");
        let model_id: String = Input::new()
            .with_prompt("Default model ID (OpenRouter format, e.g., anthropic/claude-sonnet-4)")
            .default(current_model.to_string())
            .interact_text()?;

        let entry = ModelRegistryEntry {
            id: model_id.clone(),
            provider: "openrouter".to_string(),
            model: model_id.clone(),
            tier: workgraph::config::Tier::Standard,
            ..Default::default()
        };

        (model_id, vec![entry])
    } else {
        // Popular defaults
        let entries = default_openrouter_registry();
        let model_labels: Vec<String> = entries
            .iter()
            .map(|e| format!("{} — {}", e.id, e.model))
            .collect();

        let default_idx = entries.iter().position(|e| e.id == "sonnet").unwrap_or(0);

        let idx = Select::new()
            .with_prompt("Default model?")
            .items(&model_labels)
            .default(default_idx)
            .interact()?;

        let model = entries[idx].id.clone();
        (model, entries)
    };

    let endpoint = EndpointChoices {
        name: "openrouter".to_string(),
        provider: "openrouter".to_string(),
        url: "https://openrouter.ai/api/v1".to_string(),
        api_key_env,
        api_key_file,
    };

    Ok((Some(endpoint), registry_entries, model))
}

/// Configure OpenAI provider.
fn configure_openai(
    existing: &Config,
) -> Result<(Option<EndpointChoices>, Vec<ModelRegistryEntry>, String)> {
    println!();
    println!("OpenAI configuration");
    println!("────────────────────");

    let api_key_options = &["Environment variable (OPENAI_API_KEY)", "Key file"];
    let key_idx = Select::new()
        .with_prompt("How should the API key be provided?")
        .items(api_key_options)
        .default(0)
        .interact()?;

    let (api_key_env, api_key_file) = if key_idx == 0 {
        if std::env::var("OPENAI_API_KEY").is_ok() {
            println!("  OPENAI_API_KEY is set in your environment.");
        } else {
            println!("  Set OPENAI_API_KEY in your shell profile before running agents.");
        }
        (Some("OPENAI_API_KEY".to_string()), None)
    } else {
        let key_path: String = Input::new()
            .with_prompt("Path to API key file")
            .default("~/.config/openai/key".to_string())
            .interact_text()?;
        (None, Some(key_path))
    };

    let current_model = existing.coordinator.model.as_deref().unwrap_or("gpt-4o");
    let model_id: String = Input::new()
        .with_prompt("Default model ID")
        .default(current_model.to_string())
        .interact_text()?;

    let entry = ModelRegistryEntry {
        id: model_id.clone(),
        provider: "openai".to_string(),
        model: model_id.clone(),
        tier: workgraph::config::Tier::Standard,
        ..Default::default()
    };

    let endpoint = EndpointChoices {
        name: "openai".to_string(),
        provider: "openai".to_string(),
        url: "https://api.openai.com/v1".to_string(),
        api_key_env,
        api_key_file,
    };

    Ok((Some(endpoint), vec![entry], model_id))
}

/// Configure local provider (Ollama/vLLM).
fn configure_local(
    existing: &Config,
) -> Result<(Option<EndpointChoices>, Vec<ModelRegistryEntry>, String)> {
    println!();
    println!("Local LLM configuration (Ollama/vLLM)");
    println!("──────────────────────────────────────");

    let url: String = Input::new()
        .with_prompt("API endpoint URL")
        .default("http://localhost:11434/v1".to_string())
        .interact_text()?;

    let current_model = existing.coordinator.model.as_deref().unwrap_or("llama3");
    let model_id: String = Input::new()
        .with_prompt("Default model ID")
        .default(current_model.to_string())
        .interact_text()?;

    let entry = ModelRegistryEntry {
        id: model_id.clone(),
        provider: "local".to_string(),
        model: model_id.clone(),
        tier: workgraph::config::Tier::Standard,
        ..Default::default()
    };

    let endpoint = EndpointChoices {
        name: "local".to_string(),
        provider: "local".to_string(),
        url,
        api_key_env: None,
        api_key_file: None,
    };

    Ok((Some(endpoint), vec![entry], model_id))
}

/// Configure custom provider.
fn configure_custom_provider(
    existing: &Config,
) -> Result<(Option<EndpointChoices>, Vec<ModelRegistryEntry>, String)> {
    println!();
    println!("Custom provider configuration");
    println!("─────────────────────────────");

    let provider_name: String = Input::new()
        .with_prompt("Provider name")
        .default("custom".to_string())
        .interact_text()?;

    let url: String = Input::new()
        .with_prompt("API endpoint URL")
        .interact_text()?;

    let api_key_env: String = Input::new()
        .with_prompt("Environment variable for API key (leave empty for none)")
        .default(String::new())
        .interact_text()?;

    let current_model = existing.coordinator.model.as_deref().unwrap_or("default");
    let model_id: String = Input::new()
        .with_prompt("Default model ID")
        .default(current_model.to_string())
        .interact_text()?;

    let entry = ModelRegistryEntry {
        id: model_id.clone(),
        provider: provider_name.clone(),
        model: model_id.clone(),
        tier: workgraph::config::Tier::Standard,
        ..Default::default()
    };

    let endpoint = EndpointChoices {
        name: provider_name.clone(),
        provider: provider_name,
        url,
        api_key_env: if api_key_env.is_empty() {
            None
        } else {
            Some(api_key_env)
        },
        api_key_file: None,
    };

    Ok((Some(endpoint), vec![entry], model_id))
}

/// Configure Anthropic (direct) provider — uses existing model registry flow.
fn configure_anthropic(
    existing: &Config,
) -> Result<(Option<EndpointChoices>, Vec<ModelRegistryEntry>, String)> {
    println!();
    let registry = ModelRegistry::with_defaults();
    let model_options = registry.model_choices_with_descriptions();
    let model_labels: Vec<String> = model_options
        .iter()
        .map(|(name, desc)| format!("{} — {}", name, desc))
        .collect();

    let current_model = existing
        .coordinator
        .model
        .as_deref()
        .unwrap_or(&existing.agent.model);
    let current_model_idx = model_options
        .iter()
        .position(|(name, _)| name == current_model)
        .unwrap_or(0);

    let model_idx = Select::new()
        .with_prompt("Default model for agents?")
        .items(&model_labels)
        .default(current_model_idx)
        .interact()?;

    let model = model_options[model_idx].0.clone();

    Ok((None, vec![], model))
}

/// Default OpenRouter model registry entries for Claude models.
fn default_openrouter_registry() -> Vec<ModelRegistryEntry> {
    vec![
        ModelRegistryEntry {
            id: "opus".to_string(),
            provider: "openrouter".to_string(),
            model: "anthropic/claude-opus-4".to_string(),
            tier: workgraph::config::Tier::Premium,
            context_window: 200_000,
            max_output_tokens: 32_000,
            ..Default::default()
        },
        ModelRegistryEntry {
            id: "sonnet".to_string(),
            provider: "openrouter".to_string(),
            model: "anthropic/claude-sonnet-4".to_string(),
            tier: workgraph::config::Tier::Standard,
            context_window: 200_000,
            max_output_tokens: 64_000,
            ..Default::default()
        },
        ModelRegistryEntry {
            id: "haiku".to_string(),
            provider: "openrouter".to_string(),
            model: "anthropic/claude-haiku-4".to_string(),
            tier: workgraph::config::Tier::Fast,
            context_window: 200_000,
            max_output_tokens: 8_192,
            ..Default::default()
        },
    ]
}

/// Check if the wg Claude Code skill is installed.
pub fn is_claude_skill_installed() -> bool {
    if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home)
            .join(".claude/skills/wg/SKILL.md")
            .exists()
    } else {
        false
    }
}

/// Check if the amplifier-bundle-workgraph setup script exists in common locations.
fn find_amplifier_bundle_setup() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME") {
        let candidate = PathBuf::from(&home).join("amplifier-bundle-workgraph/setup.sh");
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// After executor selection, guide the user to install the appropriate skill or bundle.
/// Returns a status string for the summary.
fn guide_skill_bundle_install(executor: &str) -> Result<String> {
    match executor {
        "claude" => {
            if is_claude_skill_installed() {
                Ok("wg skill installed ✓".to_string())
            } else {
                println!(
                    "Spawned Claude Code agents need the wg skill to understand workgraph commands."
                );
                let install = Confirm::new()
                    .with_prompt("Install wg skill for Claude Code? (recommended)")
                    .default(true)
                    .interact()?;
                if install {
                    super::skills::run_install()?;
                    Ok("wg skill installed ✓".to_string())
                } else {
                    println!("  You can install it later with: wg skill install");
                    Ok("wg skill NOT installed — run `wg skill install`".to_string())
                }
            }
        }
        "amplifier" => {
            if let Some(setup_path) = find_amplifier_bundle_setup() {
                println!(
                    "Found amplifier-bundle-workgraph at: {}",
                    setup_path.parent().unwrap().display()
                );
                println!("  Run the setup script to install the executor and bundle:");
                println!("    {}", setup_path.display());
                println!();
                println!("  Then start sessions with: amplifier run -B workgraph");
            } else {
                println!(
                    "Spawned Amplifier agents need the workgraph bundle to understand wg commands."
                );
                println!();
                println!("  Install the bundle:");
                println!(
                    "    git clone https://github.com/graphwork/amplifier-bundle-workgraph ~/amplifier-bundle-workgraph"
                );
                println!("    cd ~/amplifier-bundle-workgraph && ./setup.sh");
                println!();
                println!("  Or add it directly:");
                println!(
                    "    amplifier bundle add git+https://github.com/graphwork/amplifier-bundle-workgraph"
                );
                println!();
                println!("  Then start sessions with: amplifier run -B workgraph");
            }
            Ok("amplifier bundle — see instructions above".to_string())
        }
        _ => {
            println!("Custom executor selected. Make sure your agents know about wg commands.");
            println!("  For reference, see: wg quickstart");
            Ok(format!(
                "custom executor '{}' — manual setup needed",
                executor
            ))
        }
    }
}

/// Guide the user through configuring ~/.claude/CLAUDE.md.
/// Returns a status string for the summary.
fn guide_claude_md_install() -> Result<String> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    let claude_md = PathBuf::from(&home).join(".claude/CLAUDE.md");

    if has_workgraph_directives(&claude_md) {
        return Ok("already configured ✓".to_string());
    }

    println!("Claude Code's built-in task and agent tools conflict with workgraph.");
    println!(
        "Configuring ~/.claude/CLAUDE.md suppresses them so Claude uses `wg` commands instead."
    );

    let action = if claude_md.exists() {
        "Append workgraph directives to"
    } else {
        "Create"
    };

    let install = Confirm::new()
        .with_prompt(format!("{} ~/.claude/CLAUDE.md? (recommended)", action))
        .default(true)
        .interact()?;

    if install {
        let (status, _changed) = configure_claude_md()?;
        println!("  {}", status);
        Ok("configured ✓".to_string())
    } else {
        println!("  You can configure it later with: wg setup");
        Ok("NOT configured — Claude may use its own task tools".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workgraph::config::Config;

    #[test]
    fn test_build_config_defaults() {
        let choices = SetupChoices {
            provider: "anthropic".to_string(),
            executor: "claude".to_string(),
            model: "opus".to_string(),
            agency_enabled: true,
            evaluator_model: Some("sonnet".to_string()),
            assigner_model: Some("haiku".to_string()),
            max_agents: 4,
            endpoint: None,
            model_registry_entries: vec![],
        };

        let config = build_config(&choices, None);
        assert_eq!(config.coordinator.executor, Some("claude".to_string()));
        assert_eq!(config.agent.executor, "claude");
        assert_eq!(config.agent.model, "opus");
        assert_eq!(config.coordinator.model, Some("opus".to_string()));
        assert_eq!(config.coordinator.max_agents, 4);
        assert!(config.agency.auto_assign);
        assert!(config.agency.auto_evaluate);
        assert_eq!(config.agency.evaluator_model, Some("sonnet".to_string()));
        assert_eq!(config.agency.assigner_model, Some("haiku".to_string()));
    }

    #[test]
    fn test_build_config_amplifier() {
        let choices = SetupChoices {
            provider: "anthropic".to_string(),
            executor: "amplifier".to_string(),
            model: "sonnet".to_string(),
            agency_enabled: false,
            evaluator_model: None,
            assigner_model: None,
            max_agents: 8,
            endpoint: None,
            model_registry_entries: vec![],
        };

        let config = build_config(&choices, None);
        assert_eq!(config.coordinator.executor, Some("amplifier".to_string()));
        assert_eq!(config.agent.executor, "amplifier");
        assert_eq!(config.agent.model, "sonnet");
        assert_eq!(config.coordinator.max_agents, 8);
        assert!(!config.agency.auto_assign);
        assert!(!config.agency.auto_evaluate);
        assert!(config.agency.evaluator_model.is_none());
        assert!(config.agency.assigner_model.is_none());
    }

    #[test]
    fn test_build_config_preserves_base() {
        let mut base = Config::default();
        base.project.name = Some("my-project".to_string());
        base.agency.retention_heuristics = Some("keep good ones".to_string());
        base.log.rotation_threshold = 5_000_000;

        let choices = SetupChoices {
            provider: "anthropic".to_string(),
            executor: "claude".to_string(),
            model: "haiku".to_string(),
            agency_enabled: true,
            evaluator_model: Some("sonnet".to_string()),
            assigner_model: None,
            max_agents: 2,
            endpoint: None,
            model_registry_entries: vec![],
        };

        let config = build_config(&choices, Some(&base));
        // Wizard-set values
        assert_eq!(config.agent.model, "haiku");
        assert_eq!(config.coordinator.max_agents, 2);
        assert!(config.agency.auto_assign);
        assert_eq!(config.agency.evaluator_model, Some("sonnet".to_string()));

        // Preserved from base
        assert_eq!(config.project.name, Some("my-project".to_string()));
        assert_eq!(
            config.agency.retention_heuristics,
            Some("keep good ones".to_string())
        );
        assert_eq!(config.log.rotation_threshold, 5_000_000);
    }

    #[test]
    fn test_build_config_agency_disabled() {
        let choices = SetupChoices {
            provider: "anthropic".to_string(),
            executor: "claude".to_string(),
            model: "opus".to_string(),
            agency_enabled: false,
            evaluator_model: None,
            assigner_model: None,
            max_agents: 4,
            endpoint: None,
            model_registry_entries: vec![],
        };

        let config = build_config(&choices, None);
        assert!(!config.agency.auto_assign);
        assert!(!config.agency.auto_evaluate);
        assert!(config.agency.evaluator_model.is_none());
        assert!(config.agency.assigner_model.is_none());
    }

    #[test]
    fn test_build_config_same_as_default_models() {
        // When user picks "same as default", evaluator/assigner models are None
        let choices = SetupChoices {
            provider: "anthropic".to_string(),
            executor: "claude".to_string(),
            model: "sonnet".to_string(),
            agency_enabled: true,
            evaluator_model: None,
            assigner_model: None,
            max_agents: 4,
            endpoint: None,
            model_registry_entries: vec![],
        };

        let config = build_config(&choices, None);
        assert!(config.agency.auto_assign);
        assert!(config.agency.auto_evaluate);
        assert!(config.agency.evaluator_model.is_none());
        assert!(config.agency.assigner_model.is_none());
    }

    #[test]
    fn test_format_summary_basic() {
        let choices = SetupChoices {
            provider: "anthropic".to_string(),
            executor: "claude".to_string(),
            model: "opus".to_string(),
            agency_enabled: true,
            evaluator_model: Some("sonnet".to_string()),
            assigner_model: Some("haiku".to_string()),
            max_agents: 4,
            endpoint: None,
            model_registry_entries: vec![],
        };

        let summary = format_summary(&choices);
        assert!(summary.contains("executor = \"claude\""));
        assert!(summary.contains("model = \"opus\""));
        assert!(summary.contains("max_agents = 4"));
        assert!(summary.contains("auto_assign = true"));
        assert!(summary.contains("auto_evaluate = true"));
        assert!(summary.contains("evaluator_model = \"sonnet\""));
        assert!(summary.contains("assigner_model = \"haiku\""));
    }

    #[test]
    fn test_format_summary_agency_disabled() {
        let choices = SetupChoices {
            provider: "anthropic".to_string(),
            executor: "amplifier".to_string(),
            model: "sonnet".to_string(),
            agency_enabled: false,
            evaluator_model: None,
            assigner_model: None,
            max_agents: 8,
            endpoint: None,
            model_registry_entries: vec![],
        };

        let summary = format_summary(&choices);
        assert!(summary.contains("executor = \"amplifier\""));
        assert!(summary.contains("auto_assign = false"));
        assert!(summary.contains("auto_evaluate = false"));
        assert!(!summary.contains("evaluator_model"));
        assert!(!summary.contains("assigner_model"));
    }

    #[test]
    fn test_build_config_roundtrip_through_toml() {
        let choices = SetupChoices {
            provider: "anthropic".to_string(),
            executor: "claude".to_string(),
            model: "opus".to_string(),
            agency_enabled: true,
            evaluator_model: Some("sonnet".to_string()),
            assigner_model: Some("haiku".to_string()),
            max_agents: 6,
            endpoint: None,
            model_registry_entries: vec![],
        };

        let config = build_config(&choices, None);
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let reloaded: Config = toml::from_str(&toml_str).unwrap();

        assert_eq!(reloaded.coordinator.executor, Some("claude".to_string()));
        assert_eq!(reloaded.agent.model, "opus");
        assert_eq!(reloaded.coordinator.max_agents, 6);
        assert!(reloaded.agency.auto_assign);
        assert!(reloaded.agency.auto_evaluate);
        assert_eq!(reloaded.agency.evaluator_model, Some("sonnet".to_string()));
        assert_eq!(reloaded.agency.assigner_model, Some("haiku".to_string()));
    }

    #[test]
    fn test_format_summary_includes_executor_and_model() {
        let choices = SetupChoices {
            provider: "anthropic".to_string(),
            executor: "claude".to_string(),
            model: "sonnet".to_string(),
            agency_enabled: false,
            evaluator_model: None,
            assigner_model: None,
            max_agents: 3,
            endpoint: None,
            model_registry_entries: vec![],
        };
        let summary = format_summary(&choices);
        assert!(summary.contains("executor = \"claude\""));
        assert!(summary.contains("model = \"sonnet\""));
        assert!(summary.contains("max_agents = 3"));
    }

    #[test]
    fn test_is_claude_skill_installed_returns_bool() {
        // Just verify the function runs without panicking.
        // Actual result depends on the test environment.
        let _installed = super::is_claude_skill_installed();
    }

    #[test]
    fn test_build_config_custom_executor() {
        let choices = SetupChoices {
            provider: "anthropic".to_string(),
            executor: "my-custom-executor".to_string(),
            model: "haiku".to_string(),
            agency_enabled: false,
            evaluator_model: None,
            assigner_model: None,
            max_agents: 1,
            endpoint: None,
            model_registry_entries: vec![],
        };

        let config = build_config(&choices, None);
        assert_eq!(
            config.coordinator.executor,
            Some("my-custom-executor".to_string())
        );
        assert_eq!(config.agent.executor, "my-custom-executor");
    }

    #[test]
    fn test_build_config_openrouter_provider() {
        let choices = SetupChoices {
            provider: "openrouter".to_string(),
            executor: "native".to_string(),
            model: "sonnet".to_string(),
            agency_enabled: false,
            evaluator_model: None,
            assigner_model: None,
            max_agents: 4,
            endpoint: Some(EndpointChoices {
                name: "openrouter".to_string(),
                provider: "openrouter".to_string(),
                url: "https://openrouter.ai/api/v1".to_string(),
                api_key_env: Some("OPENROUTER_API_KEY".to_string()),
                api_key_file: None,
            }),
            model_registry_entries: default_openrouter_registry(),
        };

        let config = build_config(&choices, None);

        // Executor and provider
        assert_eq!(config.coordinator.executor, Some("native".to_string()));
        assert_eq!(config.agent.executor, "native");
        assert_eq!(config.coordinator.provider, Some("openrouter".to_string()));

        // models.default
        let models_default = config.models.default.as_ref().unwrap();
        assert_eq!(models_default.provider, Some("openrouter".to_string()));
        assert_eq!(models_default.model, Some("sonnet".to_string()));

        // Endpoint
        assert_eq!(config.llm_endpoints.endpoints.len(), 1);
        let ep = &config.llm_endpoints.endpoints[0];
        assert_eq!(ep.name, "openrouter");
        assert_eq!(ep.provider, "openrouter");
        assert_eq!(ep.url, Some("https://openrouter.ai/api/v1".to_string()));
        assert_eq!(ep.api_key_env, Some("OPENROUTER_API_KEY".to_string()));
        assert!(ep.is_default);

        // Model registry
        assert!(!config.model_registry.is_empty());
        let sonnet = config
            .model_registry
            .iter()
            .find(|e| e.id == "sonnet")
            .unwrap();
        assert_eq!(sonnet.provider, "openrouter");
        assert_eq!(sonnet.model, "anthropic/claude-sonnet-4");
    }

    #[test]
    fn test_build_config_openrouter_roundtrip_toml() {
        let choices = SetupChoices {
            provider: "openrouter".to_string(),
            executor: "native".to_string(),
            model: "sonnet".to_string(),
            agency_enabled: true,
            evaluator_model: Some("haiku".to_string()),
            assigner_model: Some("haiku".to_string()),
            max_agents: 2,
            endpoint: Some(EndpointChoices {
                name: "openrouter".to_string(),
                provider: "openrouter".to_string(),
                url: "https://openrouter.ai/api/v1".to_string(),
                api_key_env: Some("OPENROUTER_API_KEY".to_string()),
                api_key_file: None,
            }),
            model_registry_entries: default_openrouter_registry(),
        };

        let config = build_config(&choices, None);
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let reloaded: Config = toml::from_str(&toml_str).unwrap();

        // Verify everything survives round-trip
        assert_eq!(
            reloaded.coordinator.provider,
            Some("openrouter".to_string())
        );
        assert_eq!(reloaded.coordinator.effective_executor(), "native");
        assert_eq!(reloaded.llm_endpoints.endpoints.len(), 1);
        assert!(!reloaded.model_registry.is_empty());
        let models_default = reloaded.models.default.as_ref().unwrap();
        assert_eq!(models_default.provider, Some("openrouter".to_string()));
    }

    #[test]
    fn test_format_summary_openrouter() {
        let choices = SetupChoices {
            provider: "openrouter".to_string(),
            executor: "native".to_string(),
            model: "sonnet".to_string(),
            agency_enabled: false,
            evaluator_model: None,
            assigner_model: None,
            max_agents: 4,
            endpoint: Some(EndpointChoices {
                name: "openrouter".to_string(),
                provider: "openrouter".to_string(),
                url: "https://openrouter.ai/api/v1".to_string(),
                api_key_env: Some("OPENROUTER_API_KEY".to_string()),
                api_key_file: None,
            }),
            model_registry_entries: default_openrouter_registry(),
        };

        let summary = format_summary(&choices);
        assert!(summary.contains("executor = \"native\""));
        assert!(summary.contains("provider = \"openrouter\""));
        assert!(summary.contains("[models.default]"));
        assert!(summary.contains("[[llm_endpoints.endpoints]]"));
        assert!(summary.contains("api_key_env = \"OPENROUTER_API_KEY\""));
        assert!(summary.contains("[[model_registry]]"));
    }

    #[test]
    fn test_format_summary_anthropic_no_extra_sections() {
        let choices = SetupChoices {
            provider: "anthropic".to_string(),
            executor: "claude".to_string(),
            model: "opus".to_string(),
            agency_enabled: false,
            evaluator_model: None,
            assigner_model: None,
            max_agents: 4,
            endpoint: None,
            model_registry_entries: vec![],
        };

        let summary = format_summary(&choices);
        // Anthropic provider should NOT include extra sections
        assert!(!summary.contains("[models.default]"));
        assert!(!summary.contains("[[llm_endpoints.endpoints]]"));
        assert!(!summary.contains("[[model_registry]]"));
        assert!(!summary.contains("provider = "));
    }

    #[test]
    fn test_default_openrouter_registry() {
        let entries = default_openrouter_registry();
        assert_eq!(entries.len(), 3);

        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert!(ids.contains(&"opus"));
        assert!(ids.contains(&"sonnet"));
        assert!(ids.contains(&"haiku"));

        for entry in &entries {
            assert_eq!(entry.provider, "openrouter");
            assert!(entry.model.starts_with("anthropic/"));
        }
    }

    #[test]
    fn test_configure_claude_md_creates_new_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let claude_md = tmp.path().join("CLAUDE.md");

        let (status, changed) = configure_claude_md_at(&claude_md).unwrap();
        assert!(changed);
        assert!(status.contains("Created"));

        let content = std::fs::read_to_string(&claude_md).unwrap();
        assert!(content.contains(CLAUDE_MD_MARKER));
        assert!(content.contains("Do NOT use built-in TaskCreate"));
        assert!(content.contains("Do NOT use the built-in **Task tool**"));
        assert!(content.contains("wg quickstart"));
    }

    #[test]
    fn test_configure_claude_md_appends_to_existing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let claude_md = tmp.path().join("CLAUDE.md");

        let existing_content = "# My Existing Config\n\nSome custom rules here.\n";
        std::fs::write(&claude_md, existing_content).unwrap();

        let (status, changed) = configure_claude_md_at(&claude_md).unwrap();
        assert!(changed);
        assert!(status.contains("Updated"));

        let content = std::fs::read_to_string(&claude_md).unwrap();
        // Original content preserved
        assert!(content.contains("# My Existing Config"));
        assert!(content.contains("Some custom rules here."));
        // Workgraph directives appended
        assert!(content.contains(CLAUDE_MD_MARKER));
        assert!(content.contains("Do NOT use built-in TaskCreate"));
    }

    #[test]
    fn test_configure_claude_md_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let claude_md = tmp.path().join("CLAUDE.md");

        // First call creates
        let (_status, changed1) = configure_claude_md_at(&claude_md).unwrap();
        assert!(changed1);

        let content_after_first = std::fs::read_to_string(&claude_md).unwrap();

        // Second call is a no-op
        let (status, changed2) = configure_claude_md_at(&claude_md).unwrap();
        assert!(!changed2);
        assert!(status.contains("already configured"));

        let content_after_second = std::fs::read_to_string(&claude_md).unwrap();
        assert_eq!(content_after_first, content_after_second);
    }

    #[test]
    fn test_configure_claude_md_idempotent_with_existing_content() {
        let tmp = tempfile::TempDir::new().unwrap();
        let claude_md = tmp.path().join("CLAUDE.md");

        std::fs::write(&claude_md, "# Pre-existing\n").unwrap();

        let (_status, changed1) = configure_claude_md_at(&claude_md).unwrap();
        assert!(changed1);

        let content_after_first = std::fs::read_to_string(&claude_md).unwrap();

        // Second call doesn't duplicate
        let (_status, changed2) = configure_claude_md_at(&claude_md).unwrap();
        assert!(!changed2);

        let content_after_second = std::fs::read_to_string(&claude_md).unwrap();
        assert_eq!(content_after_first, content_after_second);
        assert_eq!(
            content_after_second.matches(CLAUDE_MD_MARKER).count(),
            1,
            "marker should appear exactly once"
        );
    }

    #[test]
    fn test_configure_claude_md_creates_parent_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let claude_md = tmp.path().join("nested").join("dir").join("CLAUDE.md");

        let (_, changed) = configure_claude_md_at(&claude_md).unwrap();
        assert!(changed);
        assert!(claude_md.exists());
    }

    #[test]
    fn test_has_workgraph_directives_false_for_missing_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let claude_md = tmp.path().join("CLAUDE.md");
        assert!(!has_workgraph_directives(&claude_md));
    }

    #[test]
    fn test_has_workgraph_directives_false_for_plain_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let claude_md = tmp.path().join("CLAUDE.md");
        std::fs::write(&claude_md, "# Just some markdown\n").unwrap();
        assert!(!has_workgraph_directives(&claude_md));
    }

    #[test]
    fn test_has_workgraph_directives_true_after_configure() {
        let tmp = tempfile::TempDir::new().unwrap();
        let claude_md = tmp.path().join("CLAUDE.md");
        configure_claude_md_at(&claude_md).unwrap();
        assert!(has_workgraph_directives(&claude_md));
    }

    #[test]
    fn test_configure_project_claude_md() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_dir = tmp.path();

        let (status, changed) = configure_project_claude_md(project_dir).unwrap();
        assert!(changed);
        assert!(status.contains("Created"));

        let claude_md = project_dir.join("CLAUDE.md");
        let content = std::fs::read_to_string(&claude_md).unwrap();
        assert!(content.contains(CLAUDE_MD_MARKER));
        assert!(content.contains("wg quickstart"));
    }

    #[test]
    fn test_claude_md_directives_contain_critical_rules() {
        // Verify the template contains all the critical rules from the task description
        assert!(CLAUDE_MD_DIRECTIVES.contains("TaskCreate"));
        assert!(CLAUDE_MD_DIRECTIVES.contains("TaskUpdate"));
        assert!(CLAUDE_MD_DIRECTIVES.contains("TaskList"));
        assert!(CLAUDE_MD_DIRECTIVES.contains("TaskGet"));
        assert!(CLAUDE_MD_DIRECTIVES.contains("Task tool"));
        assert!(CLAUDE_MD_DIRECTIVES.contains("subagent"));
        assert!(CLAUDE_MD_DIRECTIVES.contains("wg quickstart"));
        assert!(CLAUDE_MD_DIRECTIVES.contains("Orchestrating agent"));
        assert!(CLAUDE_MD_DIRECTIVES.contains("wg service start"));
    }
}
