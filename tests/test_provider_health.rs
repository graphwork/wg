use anyhow::Result;
use std::fs;
use tempfile::TempDir;
use workgraph::config::Config;
use workgraph::service::{
    ProviderErrorKind, ProviderHealth, ProviderHealthStatus, classify_error, extract_provider_id,
};

/// Test provider error classification
#[test]
fn test_provider_health_error_classification() {
    // Auth failures should be FatalProvider
    assert_eq!(
        classify_error(Some(1), "Authentication failed (HTTP 401): Invalid API key"),
        ProviderErrorKind::FatalProvider
    );
    assert_eq!(
        classify_error(Some(1), "Access denied (HTTP 403): Insufficient permissions"),
        ProviderErrorKind::FatalProvider
    );

    // CLI missing should be FatalProvider
    assert_eq!(
        classify_error(
            Some(1),
            "The 'claude' CLI is required but was not found in PATH"
        ),
        ProviderErrorKind::FatalProvider
    );

    // Quota exhaustion should be FatalProvider
    assert_eq!(
        classify_error(Some(1), "quota exceeded for this billing period"),
        ProviderErrorKind::FatalProvider
    );

    // Rate limiting should be Transient
    assert_eq!(
        classify_error(Some(1), "HTTP 429: Rate limit exceeded"),
        ProviderErrorKind::Transient
    );

    // Network timeouts should be Transient
    assert_eq!(
        classify_error(Some(1), "Native Anthropic call timed out"),
        ProviderErrorKind::Transient
    );

    // Context length should be FatalTask
    assert_eq!(
        classify_error(Some(1), "HTTP 413: Payload too large"),
        ProviderErrorKind::FatalTask
    );

    // Hard timeout should be FatalTask
    assert_eq!(
        classify_error(Some(124), "Agent exceeded hard timeout"),
        ProviderErrorKind::FatalTask
    );

    // Unknown errors default to Transient
    assert_eq!(
        classify_error(Some(1), "Some mysterious error"),
        ProviderErrorKind::Transient
    );
}

/// Test provider ID extraction
#[test]
fn test_provider_health_provider_id_extraction() {
    assert_eq!(extract_provider_id("claude", None), "claude");
    assert_eq!(
        extract_provider_id("native", Some("gpt-4-turbo")),
        "native:openai"
    );
    assert_eq!(
        extract_provider_id("native", Some("claude-3-sonnet")),
        "native:anthropic"
    );
    assert_eq!(
        extract_provider_id("native", Some("custom:model")),
        "native:custom"
    );
    assert_eq!(extract_provider_id("amplifier", None), "amplifier");
    assert_eq!(extract_provider_id("shell", None), "shell");
}

/// Test consecutive failures trigger pause
#[test]
fn test_provider_health_consecutive_failures_trigger_pause() {
    let mut health = ProviderHealth::default();
    let provider_id = "claude";

    // Record consecutive fatal provider errors
    for i in 1..=3 {
        health.record_failure(
            provider_id,
            ProviderErrorKind::FatalProvider,
            format!("Auth failure {}", i),
        );

        let provider = health.get_or_create_provider(provider_id);
        assert_eq!(provider.consecutive_failures, i);

        if i < 3 {
            assert!(!provider.should_pause(3)); // Below threshold
        } else {
            assert!(provider.should_pause(3)); // At threshold
        }
    }

    // Apply pause
    let paused = health.check_and_apply_pauses(3, "pause");
    assert_eq!(paused, vec![provider_id]);
    assert!(health.service_paused);

    let provider = health.get_or_create_provider(provider_id);
    assert!(provider.is_paused);
}

/// Test transient errors don't trigger pause
#[test]
fn test_provider_health_transient_errors_dont_trigger_pause() {
    let mut health = ProviderHealth::default();
    let provider_id = "claude";

    // Record many transient errors
    for i in 1..=10 {
        health.record_failure(
            provider_id,
            ProviderErrorKind::Transient,
            format!("Rate limit {}", i),
        );
    }

    // Transient errors should not count for provider health
    let provider = health.get_or_create_provider(provider_id);
    assert_eq!(provider.consecutive_failures, 0);
    assert!(!provider.should_pause(3));

    // Apply pause check - should not pause
    let paused = health.check_and_apply_pauses(3, "pause");
    assert!(paused.is_empty());
    assert!(!health.service_paused);
}

/// Test fallback mode switches provider
#[test]
fn test_provider_health_fallback_mode() {
    let mut health = ProviderHealth::default();
    let provider_id = "claude";

    // Record consecutive fatal provider errors
    for i in 1..=3 {
        health.record_failure(
            provider_id,
            ProviderErrorKind::FatalProvider,
            format!("Auth failure {}", i),
        );
    }

    // Apply pause with fallback behavior
    let paused = health.check_and_apply_pauses(3, "fallback");
    assert_eq!(paused, vec![provider_id]);

    // Service should not be globally paused in fallback mode
    assert!(!health.service_paused);

    // But the specific provider should be paused
    let provider = health.get_or_create_provider(provider_id);
    assert!(provider.is_paused);
}

/// Test continue mode doesn't pause
#[test]
fn test_provider_health_continue_mode() {
    let mut health = ProviderHealth::default();
    let provider_id = "claude";

    // Record consecutive fatal provider errors
    for i in 1..=3 {
        health.record_failure(
            provider_id,
            ProviderErrorKind::FatalProvider,
            format!("Auth failure {}", i),
        );
    }

    // Apply pause with continue behavior
    let paused = health.check_and_apply_pauses(3, "continue");
    assert_eq!(paused, vec![provider_id]); // Still returned as would-be paused

    // But nothing should actually be paused
    assert!(!health.service_paused);
    let provider = health.get_or_create_provider(provider_id);
    assert!(!provider.is_paused); // Immediately unpaused in continue mode
}

/// Test success resets failure count
#[test]
fn test_provider_health_success_resets_failure_count() {
    let mut health = ProviderHealth::default();
    let provider_id = "claude";

    // Build up some failures
    health.record_failure(
        provider_id,
        ProviderErrorKind::FatalProvider,
        "Auth failure 1".to_string(),
    );
    health.record_failure(
        provider_id,
        ProviderErrorKind::FatalProvider,
        "Auth failure 2".to_string(),
    );

    let provider = health.get_or_create_provider(provider_id);
    assert_eq!(provider.consecutive_failures, 2);

    // Success should reset count
    health.record_success(provider_id);
    let provider = health.get_or_create_provider(provider_id);
    assert_eq!(provider.consecutive_failures, 0);
    assert!(!provider.should_pause(3));
}

/// Test resume clears all pause state
#[test]
fn test_provider_health_resume_clears_pause_state() {
    let mut health = ProviderHealth::default();

    // Simulate a paused state
    health.service_paused = true;
    health.pause_reason = Some("Provider failures".to_string());
    health.paused_at = Some("2024-01-01T00:00:00Z".to_string());

    let provider_id = "claude";
    let mut provider = ProviderHealthStatus::new(provider_id.to_string());
    provider.pause("Too many failures".to_string());
    health.providers.insert(provider_id.to_string(), provider);

    // Resume should clear everything
    health.resume_service();

    assert!(!health.service_paused);
    assert!(health.pause_reason.is_none());
    assert!(health.paused_at.is_none());

    let provider = health.get_or_create_provider(provider_id);
    assert!(!provider.is_paused);
    assert!(provider.pause_reason.is_none());
    assert!(provider.paused_at.is_none());
    assert_eq!(provider.consecutive_failures, 0);
}

/// Test provider health persistence
#[test]
fn test_provider_health_persistence() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let service_dir = temp_dir.path().join("service");
    fs::create_dir_all(&service_dir)?;

    // Create initial health state
    let mut health = ProviderHealth::default();
    health.record_failure(
        "claude",
        ProviderErrorKind::FatalProvider,
        "Test failure".to_string(),
    );
    health.save(temp_dir.path())?;

    // Load and verify persistence
    let loaded_health = ProviderHealth::load(temp_dir.path())?;
    assert_eq!(loaded_health.providers.len(), 1);

    let provider = loaded_health.providers.get("claude").unwrap();
    assert_eq!(provider.consecutive_failures, 1);
    assert_eq!(provider.last_error, Some("Test failure".to_string()));

    Ok(())
}

/// Integration test with config.toml settings
#[test]
fn test_provider_health_config_integration() {
    let mut config = Config::default();

    // Test default values
    assert_eq!(config.coordinator.on_provider_failure, "pause");
    assert_eq!(config.coordinator.provider_failure_threshold, 3);
    assert_eq!(config.coordinator.provider_failure_cooldown, "");

    // Test setting different values
    config.coordinator.on_provider_failure = "fallback".to_string();
    config.coordinator.provider_failure_threshold = 5;
    config.coordinator.provider_failure_cooldown = "10m".to_string();

    assert_eq!(config.coordinator.on_provider_failure, "fallback");
    assert_eq!(config.coordinator.provider_failure_threshold, 5);
    assert_eq!(config.coordinator.provider_failure_cooldown, "10m");
}