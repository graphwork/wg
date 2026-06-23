//! Integration coverage for the two-tier Pi profile setter (`wg profile pi`).
//!
//! Exercises the full **list → select → apply** pipeline at the library level
//! (deterministic; no global HOME, no daemon): parse the baked-in Pi starter,
//! list its configured models (the picker source), apply a tier change through
//! the same surgical TOML patcher the CLI uses, and confirm the new tiers
//! survive a re-parse (the "reflected next turn / persists across reload"
//! contract). A scripted PTY smoke (`tests/smoke/scenarios/pi_two_tier_profile.sh`)
//! covers the real CLI human flow on top of this.

use worksgood::config::Config;
use worksgood::profile::named::{self, STARTER_PI};

/// Apply the `(strong, weak)` key-set to a TOML string exactly as
/// `patch_pi_tiers` does (without touching the global HOME-based profile file),
/// so the test is hermetic.
fn patch_in_memory(content: &str, strong: Option<&str>, weak: Option<&str>) -> String {
    let mut out = content.to_string();
    if let Some(s) = strong {
        for key in Config::PI_STRONG_TOML_KEYS {
            out = named::set_toml_string_value(&out, key, s);
        }
    }
    if let Some(w) = weak {
        for key in Config::PI_WEAK_TOML_KEYS {
            out = named::set_toml_string_value(&out, key, w);
        }
    }
    out
}

#[test]
fn test_pi_list_reports_configured_models_not_hardcoded() {
    // LIST: the starter's configured tiers are surfaced from the profile, not a
    // hardcoded constant.
    let cfg: Config = toml::from_str(STARTER_PI).expect("pi starter parses");
    let (strong, weak) = cfg.pi_tiers();
    assert_eq!(strong.as_deref(), Some("pi:openrouter/z-ai/glm-5.2"));
    assert_eq!(weak.as_deref(), Some("openrouter:deepseek/deepseek-chat"));
}

#[test]
fn test_pi_select_and_apply_updates_both_tiers_and_persists() {
    // SELECT + APPLY: pick new strong/weak and patch the profile TOML.
    let patched = patch_in_memory(
        STARTER_PI,
        Some("openrouter:qwen/qwen3-max"),
        Some("openrouter:deepseek/deepseek-v3.1"),
    );

    // PERSISTS ACROSS RELOAD: re-parse the patched TOML (simulating the daemon
    // re-reading config.toml) and confirm the tiers stuck across every key.
    let reloaded: Config = toml::from_str(&patched).expect("patched pi profile re-parses");
    let (strong, weak) = reloaded.pi_tiers();
    assert_eq!(strong.as_deref(), Some("openrouter:qwen/qwen3-max"));
    assert_eq!(weak.as_deref(), Some("openrouter:deepseek/deepseek-v3.1"));

    // Every strong key followed the strong tier...
    assert_eq!(reloaded.agent.model, "openrouter:qwen/qwen3-max");
    assert_eq!(
        reloaded.coordinator.model.as_deref(),
        Some("openrouter:qwen/qwen3-max")
    );
    assert_eq!(
        reloaded.tiers.standard.as_deref(),
        Some("openrouter:qwen/qwen3-max")
    );
    assert_eq!(
        reloaded.tiers.premium.as_deref(),
        Some("openrouter:qwen/qwen3-max")
    );
    assert_eq!(
        reloaded
            .models
            .task_agent
            .as_ref()
            .and_then(|m| m.model.as_deref()),
        Some("openrouter:qwen/qwen3-max")
    );

    // ...and every weak agency one-shot followed the weak tier.
    assert_eq!(
        reloaded.tiers.fast.as_deref(),
        Some("openrouter:deepseek/deepseek-v3.1")
    );
    for role in [
        reloaded.models.evaluator.as_ref(),
        reloaded.models.assigner.as_ref(),
        reloaded.models.flip_inference.as_ref(),
        reloaded.models.flip_comparison.as_ref(),
    ] {
        assert_eq!(
            role.and_then(|m| m.model.as_deref()),
            Some("openrouter:deepseek/deepseek-v3.1")
        );
    }
}

#[test]
fn test_pi_partial_apply_leaves_other_tier_unchanged() {
    // Apply only weak; strong must remain the starter value.
    let patched = patch_in_memory(STARTER_PI, None, Some("openrouter:deepseek/deepseek-v3.1"));
    let reloaded: Config = toml::from_str(&patched).unwrap();
    let (strong, weak) = reloaded.pi_tiers();
    assert_eq!(
        strong.as_deref(),
        Some("pi:openrouter/z-ai/glm-5.2"),
        "strong tier must be untouched by a weak-only update"
    );
    assert_eq!(weak.as_deref(), Some("openrouter:deepseek/deepseek-v3.1"));
}

#[test]
fn test_pi_apply_preserves_plugin_comment_block() {
    // The hand-written plugin-placement documentation must survive a write
    // (this is why the patcher is line-based, not a toml round-trip).
    let patched = patch_in_memory(STARTER_PI, Some("openrouter:z-ai/glm-5.2"), None);
    assert!(patched.contains("PLUGIN INSTALL"));
    assert!(patched.contains("wg-pi-host.mjs"));
    assert!(patched.contains("~/.pi/agent/extensions/"));
}

#[test]
fn test_pi_starter_premium_roles_ride_strong_tier() {
    // Migration (design §4/§8): the starter no longer pins evolver/creator/
    // verification to the cheap model — they ride tiers.premium = strong.
    let cfg: Config = toml::from_str(STARTER_PI).unwrap();
    assert!(
        cfg.models.evolver.is_none(),
        "evolver must not carry an explicit pin (rides tiers.premium)"
    );
    assert!(cfg.models.creator.is_none(), "creator rides tiers.premium");
    assert!(
        cfg.models.verification.is_none(),
        "verification rides tiers.premium"
    );
    // The four agency one-shots remain explicit (they ignore the tier cascade).
    assert!(cfg.models.evaluator.is_some());
    assert!(cfg.models.assigner.is_some());
    assert!(cfg.models.flip_inference.is_some());
    assert!(cfg.models.flip_comparison.is_some());
}
