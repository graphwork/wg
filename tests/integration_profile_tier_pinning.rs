//! Integration tests for profile tier pinning (--fast/--standard/--premium flags).
//!
//! These tests verify that tier pins are correctly written to config and
//! that they override profile defaults in the effective tier resolution.
//!
//! Note: We use Config::save + Config::load_local (not load_merged) to avoid
//! interference from the user's global config during tests.

use tempfile::TempDir;
use workgraph::config::Config;

/// Create a temp WG dir with a minimal config.
fn setup_workgraph_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    let config = Config::default();
    config.save(dir.path()).unwrap();
    dir
}

/// Load config from just the local file (no global merge) to avoid test pollution.
fn load_local_config(dir: &std::path::Path) -> Config {
    let config_path = dir.join("config.toml");
    let content = std::fs::read_to_string(&config_path).unwrap();
    toml::from_str(&content).unwrap()
}

#[test]
fn test_tier_pins_written_to_config() {
    let dir = setup_workgraph_dir();

    // Simulate what `profile set anthropic --fast X --standard Y --premium Z` does:
    let mut config = load_local_config(dir.path());
    config.profile = Some("anthropic".to_string());
    config.tiers.fast = Some("openrouter:vendor/fast-model".to_string());
    config.tiers.standard = Some("openrouter:vendor/standard-model".to_string());
    config.tiers.premium = Some("openrouter:vendor/premium-model".to_string());
    config.save(dir.path()).unwrap();

    // Reload and verify
    let config = load_local_config(dir.path());
    assert_eq!(config.profile.as_deref(), Some("anthropic"));
    assert_eq!(
        config.tiers.fast.as_deref(),
        Some("openrouter:vendor/fast-model")
    );
    assert_eq!(
        config.tiers.standard.as_deref(),
        Some("openrouter:vendor/standard-model")
    );
    assert_eq!(
        config.tiers.premium.as_deref(),
        Some("openrouter:vendor/premium-model")
    );
}

#[test]
fn test_single_tier_pin_leaves_others_unset() {
    let dir = setup_workgraph_dir();

    let mut config = load_local_config(dir.path());
    config.profile = Some("anthropic".to_string());
    config.tiers.fast = Some("openrouter:vendor/my-fast".to_string());
    // standard and premium left as None
    config.save(dir.path()).unwrap();

    let config = load_local_config(dir.path());
    assert_eq!(
        config.tiers.fast.as_deref(),
        Some("openrouter:vendor/my-fast")
    );
    assert!(config.tiers.standard.is_none());
    assert!(config.tiers.premium.is_none());
}

#[test]
fn test_tier_pins_override_profile_defaults_in_effective() {
    let dir = setup_workgraph_dir();

    // Set anthropic profile with a fast tier pin
    let mut config = load_local_config(dir.path());
    config.profile = Some("anthropic".to_string());
    config.tiers.fast = Some("openrouter:custom/fast".to_string());
    config.save(dir.path()).unwrap();

    let config = load_local_config(dir.path());
    let effective = config.effective_tiers_public();
    // Pin overrides anthropic's default fast=claude:haiku
    assert_eq!(effective.fast.as_deref(), Some("openrouter:custom/fast"));
    // Others fall through to anthropic profile defaults
    assert_eq!(effective.standard.as_deref(), Some("claude:sonnet"));
    assert_eq!(effective.premium.as_deref(), Some("claude:opus"));
}

#[test]
fn test_no_pins_uses_profile_defaults() {
    let dir = setup_workgraph_dir();

    let mut config = load_local_config(dir.path());
    config.profile = Some("anthropic".to_string());
    config.save(dir.path()).unwrap();

    let config = load_local_config(dir.path());
    assert!(config.tiers.fast.is_none());
    assert!(config.tiers.standard.is_none());
    assert!(config.tiers.premium.is_none());

    let effective = config.effective_tiers_public();
    assert_eq!(effective.fast.as_deref(), Some("claude:haiku"));
    assert_eq!(effective.standard.as_deref(), Some("claude:sonnet"));
    assert_eq!(effective.premium.as_deref(), Some("claude:opus"));
}

#[test]
fn test_tier_pins_persist_across_reloads() {
    let dir = setup_workgraph_dir();

    let mut config = load_local_config(dir.path());
    config.profile = Some("openai".to_string());
    config.tiers.standard = Some("openrouter:custom/standard".to_string());
    config.tiers.premium = Some("openrouter:custom/premium".to_string());
    config.save(dir.path()).unwrap();

    let config = load_local_config(dir.path());
    assert_eq!(config.profile.as_deref(), Some("openai"));
    assert_eq!(
        config.tiers.standard.as_deref(),
        Some("openrouter:custom/standard")
    );
    assert_eq!(
        config.tiers.premium.as_deref(),
        Some("openrouter:custom/premium")
    );
    // fast was not pinned
    assert!(config.tiers.fast.is_none());

    // Effective tiers: fast from openai profile, others from pins
    let effective = config.effective_tiers_public();
    assert_eq!(
        effective.fast.as_deref(),
        Some("openrouter:openai/gpt-4o-mini")
    );
    assert_eq!(
        effective.standard.as_deref(),
        Some("openrouter:custom/standard")
    );
    assert_eq!(
        effective.premium.as_deref(),
        Some("openrouter:custom/premium")
    );
}

#[test]
fn test_all_tier_pins_override_all_profile_defaults() {
    let dir = setup_workgraph_dir();

    let mut config = load_local_config(dir.path());
    config.profile = Some("openai".to_string());
    config.tiers.fast = Some("openrouter:qwen/qwen3-coder".to_string());
    config.tiers.standard = Some("openrouter:deepseek/deepseek-r1".to_string());
    config.tiers.premium = Some("openrouter:qwen/qwen3-max".to_string());
    config.save(dir.path()).unwrap();

    let config = load_local_config(dir.path());
    let effective = config.effective_tiers_public();
    assert_eq!(
        effective.fast.as_deref(),
        Some("openrouter:qwen/qwen3-coder")
    );
    assert_eq!(
        effective.standard.as_deref(),
        Some("openrouter:deepseek/deepseek-r1")
    );
    assert_eq!(
        effective.premium.as_deref(),
        Some("openrouter:qwen/qwen3-max")
    );
}
