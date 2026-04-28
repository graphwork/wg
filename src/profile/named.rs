//! Named runtime profiles: user-created (model, endpoint, per-role) bundles.
//!
//! Storage: `~/.wg/profiles/<name>.toml` (one file per profile).
//! Active pointer: `~/.wg/active-profile` (one-line, absent = no profile).
//!
//! A profile is a strict-allowlist overlay on top of the base config.
//! Only the fields listed in `NamedProfile` may appear in a profile file;
//! unknown keys are rejected at parse time via `deny_unknown_fields`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config::{Config, EndpointConfig, RoleModelConfig, TierConfig};

// ── Starter templates (baked into binary) ────────────────────────────────────

pub const STARTER_CLAUDE: &str = include_str!("templates/claude.toml");
pub const STARTER_CODEX: &str = include_str!("templates/codex.toml");
pub const STARTER_WGNEXT: &str = include_str!("templates/wgnext.toml");

/// The three built-in starter profile names.
pub const STARTER_NAMES: &[&str] = &["claude", "codex", "wgnext"];

/// Return the baked-in template content for a starter name, or None.
pub fn starter_template(name: &str) -> Option<&'static str> {
    match name {
        "claude" => Some(STARTER_CLAUDE),
        "codex" => Some(STARTER_CODEX),
        "wgnext" => Some(STARTER_WGNEXT),
        _ => None,
    }
}

// ── Allowlisted sub-structs ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProfileAgentSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProfileDispatcherSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Per-role models allowed in a profile (strict subset of ModelRoutingConfig).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProfileModelsSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<RoleModelConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluator: Option<RoleModelConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigner: Option<RoleModelConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flip: Option<RoleModelConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<RoleModelConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evolver: Option<RoleModelConfig>,
}

/// LLM endpoints section in a profile (replaces, not merges, the base array).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProfileEndpointsSection {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<EndpointConfig>,
}

// ── NamedProfile (the allowlist struct) ──────────────────────────────────────

/// A named runtime profile: a strict-allowlist overlay on `~/.wg/config.toml`.
///
/// Only these top-level keys may appear in a profile file.
/// Anything else causes a parse error: "unknown profile field: [tui]; profiles
/// control models and endpoints only".
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct NamedProfile {
    /// Human-readable description shown by `wg profile list`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// `[agent]` section — only `model` is profile-able.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<ProfileAgentSection>,

    /// `[dispatcher]` section — only `model` is profile-able.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatcher: Option<ProfileDispatcherSection>,

    /// `[tiers]` section — fast/standard/premium tier→model mappings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tiers: Option<TierConfig>,

    /// `[models.*]` per-role overrides (evaluator, assigner, flip, creator, evolver).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<ProfileModelsSection>,

    /// `[[llm_endpoints.endpoints]]` — replaces (not merges) the base endpoint array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_endpoints: Option<ProfileEndpointsSection>,
}

// ── File I/O ──────────────────────────────────────────────────────────────────

/// Return the profiles directory: `<global_dir>/profiles/`.
pub fn profiles_dir() -> Result<PathBuf> {
    Ok(Config::global_dir()?.join("profiles"))
}

/// Return the active-pointer file path: `<global_dir>/active-profile`.
pub fn active_pointer_path() -> Result<PathBuf> {
    Ok(Config::global_dir()?.join("active-profile"))
}

/// Read the active profile name from `~/.wg/active-profile`.
/// Returns `None` when the file is absent (no profile active).
pub fn active() -> Result<Option<String>> {
    let path = active_pointer_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let name = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read active-profile from {}", path.display()))?;
    let name = name.trim().to_string();
    if name.is_empty() {
        Ok(None)
    } else {
        Ok(Some(name))
    }
}

/// Write (or remove) the active-pointer file.
/// `None` removes the file (reverts to base config).
pub fn set_active(name: Option<&str>) -> Result<()> {
    let path = active_pointer_path()?;
    match name {
        Some(n) => {
            // Ensure global dir exists
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory {}", parent.display()))?;
            }
            std::fs::write(&path, n)
                .with_context(|| format!("Failed to write active-profile to {}", path.display()))?;
        }
        None => {
            if path.exists() {
                std::fs::remove_file(&path)
                    .with_context(|| format!("Failed to remove {}", path.display()))?;
            }
        }
    }
    Ok(())
}

/// Load a named profile by name from `~/.wg/profiles/<name>.toml`.
pub fn load(name: &str) -> Result<NamedProfile> {
    let path = profile_path(name)?;
    if !path.exists() {
        // Suggest closest match
        let installed = list_installed().unwrap_or_default();
        let suggestion = find_closest(name, &installed);
        if let Some(s) = suggestion {
            anyhow::bail!(
                "Profile '{}' not found at {}.\nDid you mean: {}?\nRun `wg profile list` to see available profiles.",
                name,
                path.display(),
                s,
            );
        } else {
            anyhow::bail!(
                "Profile '{}' not found at {}.\nRun `wg profile init-starters` to create the starter profiles,\nor `wg profile create {}` to create a new one.",
                name,
                path.display(),
                name,
            );
        }
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read profile file {}", path.display()))?;
    let profile: NamedProfile = toml::from_str(&content).map_err(|e| {
        anyhow::anyhow!(
            "Failed to parse profile '{}' ({}): {}\n\
             Profiles may only contain: description, [agent], [dispatcher], [tiers], [models.*], [[llm_endpoints.endpoints]]",
            name,
            path.display(),
            e,
        )
    })?;
    Ok(profile)
}

/// Save a named profile to `~/.wg/profiles/<name>.toml`.
pub fn save(name: &str, profile: &NamedProfile) -> Result<()> {
    let dir = profiles_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create profiles directory {}", dir.display()))?;
    let path = dir.join(format!("{}.toml", name));
    let content = toml::to_string_pretty(profile)
        .with_context(|| format!("Failed to serialize profile '{}'", name))?;
    std::fs::write(&path, content)
        .with_context(|| format!("Failed to write profile file {}", path.display()))?;
    Ok(())
}

/// List installed profile names (files in `~/.wg/profiles/`).
pub fn list_installed() -> Result<Vec<String>> {
    let dir = profiles_dir()?;
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut names = vec![];
    for entry in std::fs::read_dir(&dir)
        .with_context(|| format!("Failed to read profiles directory {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Return the path for a named profile file.
pub fn profile_path(name: &str) -> Result<PathBuf> {
    Ok(profiles_dir()?.join(format!("{}.toml", name)))
}

// ── Overlay semantics ─────────────────────────────────────────────────────────

/// Apply a named profile as an overlay onto a base Config.
///
/// Semantics (per design §2.4):
/// - Scalar keys: profile wins over base.
/// - `[[llm_endpoints.endpoints]]` array: profile REPLACES the base array entirely.
pub fn overlay_onto(base: &mut Config, prof: &NamedProfile) {
    if let Some(ref agent) = prof.agent {
        if let Some(ref m) = agent.model {
            base.agent.model = m.clone();
        }
    }
    if let Some(ref dispatcher) = prof.dispatcher {
        if let Some(ref m) = dispatcher.model {
            base.coordinator.model = Some(m.clone());
        }
    }
    if let Some(ref tiers) = prof.tiers {
        if let Some(ref f) = tiers.fast {
            base.tiers.fast = Some(f.clone());
        }
        if let Some(ref s) = tiers.standard {
            base.tiers.standard = Some(s.clone());
        }
        if let Some(ref p) = tiers.premium {
            base.tiers.premium = Some(p.clone());
        }
    }
    if let Some(ref models) = prof.models {
        if let Some(ref m) = models.default {
            base.models.default = Some(m.clone());
        }
        if let Some(ref m) = models.evaluator {
            base.models.evaluator = Some(m.clone());
        }
        if let Some(ref m) = models.assigner {
            base.models.assigner = Some(m.clone());
        }
        if let Some(ref m) = models.flip {
            base.models.flip_inference = Some(m.clone());
            base.models.flip_comparison = Some(m.clone());
        }
        if let Some(ref m) = models.creator {
            base.models.creator = Some(m.clone());
        }
        if let Some(ref m) = models.evolver {
            base.models.evolver = Some(m.clone());
        }
    }
    if let Some(ref ep) = prof.llm_endpoints {
        base.llm_endpoints.endpoints = ep.endpoints.clone();
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Find the closest matching profile name (simple edit-distance heuristic).
fn find_closest<'a>(target: &str, candidates: &'a [String]) -> Option<&'a str> {
    candidates
        .iter()
        .filter(|c| {
            let c = c.to_lowercase();
            let t = target.to_lowercase();
            c.contains(&t) || t.contains(&c) || edit_distance(&c, &t) <= 2
        })
        .map(|s| s.as_str())
        .next()
}

/// Trivial edit distance (Levenshtein) for closest-match suggestions.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 0..=m {
        dp[i][0] = i;
    }
    for j in 0..=n {
        dp[0][j] = j;
    }
    for i in 1..=m {
        for j in 1..=n {
            dp[i][j] = if a[i - 1] == b[j - 1] {
                dp[i - 1][j - 1]
            } else {
                1 + dp[i - 1][j].min(dp[i][j - 1]).min(dp[i - 1][j - 1])
            };
        }
    }
    dp[m][n]
}

// ── Helper for IPC: build model string from active profile ────────────────────

/// Return the primary agent model from the active profile, if one is set.
/// Used by `wg profile use` to include `model` in the Reconfigure IPC
/// so the daemon's in-memory `daemon_cfg.model` is updated immediately
/// (rather than waiting for the next config.toml re-read).
pub fn active_profile_model() -> Option<String> {
    let name = active().ok()??;
    let prof = load(&name).ok()?;
    prof.agent.and_then(|a| a.model)
}

// ── Validation helper ─────────────────────────────────────────────────────────

/// Validate a profile file path (parse it and return the profile).
/// Used by `wg profile edit` to validate after the user saves.
pub fn validate_file(path: &Path) -> Result<NamedProfile> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let profile: NamedProfile = toml::from_str(&content).map_err(|e| {
        anyhow::anyhow!(
            "Invalid profile file ({}): {}\n\
             Profiles may only contain: description, [agent], [dispatcher], [tiers], [models.*], [[llm_endpoints.endpoints]]",
            path.display(),
            e,
        )
    })?;
    Ok(profile)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Serialize HOME-mutating tests to avoid cross-test interference.
    static HOME_MUTEX: Mutex<()> = Mutex::new(());

    fn with_home<F: FnOnce()>(f: F) -> TempDir {
        let _guard = HOME_MUTEX.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        // Ensure the .wg dir exists so Config::global_dir() is stable.
        let wg_dir = tmp.path().join(".wg");
        std::fs::create_dir_all(&wg_dir).unwrap();
        // SAFETY: HOME_MUTEX serializes all callers; single-threaded at this point.
        unsafe { std::env::set_var("HOME", tmp.path()) };
        f();
        tmp
    }

    #[test]
    fn test_named_profile_rejects_unknown_field() {
        let toml = r#"
description = "test"
[tui]
theme = "dark"
"#;
        let result: Result<NamedProfile, _> = toml::from_str(toml);
        assert!(result.is_err(), "Should reject unknown field [tui]");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("tui") || err.contains("unknown"),
            "Error should mention tui or unknown: {}",
            err
        );
    }

    #[test]
    fn test_named_profile_accepts_all_allowed_fields() {
        let toml = r#"
description = "Full profile"

[agent]
model = "claude:opus"

[dispatcher]
model = "claude:opus"

[tiers]
fast = "claude:haiku"
standard = "claude:sonnet"
premium = "claude:opus"

[models.evaluator]
model = "claude:haiku"

[models.assigner]
model = "claude:haiku"

[[llm_endpoints.endpoints]]
name = "default"
provider = "oai-compat"
url = "http://127.0.0.1:8088"
is_default = true
"#;
        let result: Result<NamedProfile, _> = toml::from_str(toml);
        assert!(result.is_ok(), "Should accept all allowed fields: {:?}", result);
    }

    #[test]
    fn test_overlay_agent_model() {
        let mut base = Config::default();
        base.agent.model = "claude:sonnet".to_string();

        let prof = NamedProfile {
            agent: Some(ProfileAgentSection {
                model: Some("codex:gpt-5.5".to_string()),
            }),
            ..Default::default()
        };

        overlay_onto(&mut base, &prof);
        assert_eq!(base.agent.model, "codex:gpt-5.5");
    }

    #[test]
    fn test_overlay_tiers() {
        let mut base = Config::default();
        base.tiers.fast = Some("claude:haiku".to_string());
        base.tiers.standard = Some("claude:sonnet".to_string());

        let prof = NamedProfile {
            tiers: Some(TierConfig {
                fast: Some("codex:gpt-5.4-mini".to_string()),
                standard: Some("codex:gpt-5.4".to_string()),
                premium: Some("codex:gpt-5.5".to_string()),
            }),
            ..Default::default()
        };

        overlay_onto(&mut base, &prof);
        assert_eq!(base.tiers.fast.as_deref(), Some("codex:gpt-5.4-mini"));
        assert_eq!(base.tiers.standard.as_deref(), Some("codex:gpt-5.4"));
        assert_eq!(base.tiers.premium.as_deref(), Some("codex:gpt-5.5"));
    }

    fn make_endpoint(name: &str, provider: &str, url: &str) -> EndpointConfig {
        EndpointConfig {
            name: name.to_string(),
            provider: provider.to_string(),
            url: Some(url.to_string()),
            model: None,
            api_key: None,
            api_key_file: None,
            api_key_env: None,
            is_default: true,
            context_window: None,
        }
    }

    #[test]
    fn test_overlay_endpoints_replace() {
        use crate::config::EndpointsConfig;

        let mut base = Config::default();
        base.llm_endpoints = EndpointsConfig {
            inherit_global: false,
            endpoints: vec![make_endpoint("old", "openai", "http://old.example.com")],
        };

        let prof = NamedProfile {
            llm_endpoints: Some(ProfileEndpointsSection {
                endpoints: vec![make_endpoint("default", "oai-compat", "http://127.0.0.1:8088")],
            }),
            ..Default::default()
        };

        overlay_onto(&mut base, &prof);
        assert_eq!(base.llm_endpoints.endpoints.len(), 1);
        assert_eq!(base.llm_endpoints.endpoints[0].name, "default");
        assert_eq!(
            base.llm_endpoints.endpoints[0].url.as_deref(),
            Some("http://127.0.0.1:8088")
        );
    }

    #[test]
    fn test_starter_templates_parse() {
        for name in STARTER_NAMES {
            let tmpl = starter_template(name).unwrap();
            let result: Result<NamedProfile, _> = toml::from_str(tmpl);
            assert!(
                result.is_ok(),
                "Starter template '{}' should parse: {:?}",
                name,
                result
            );
        }
    }

    #[test]
    fn test_set_and_read_active() {
        let _tmp = with_home(|| {
            set_active(Some("codex")).unwrap();
            let name = active().unwrap();
            assert_eq!(name.as_deref(), Some("codex"));

            set_active(None).unwrap();
            let name = active().unwrap();
            assert!(name.is_none());
        });
    }

    #[test]
    fn test_save_and_load_profile() {
        let _tmp = with_home(|| {
            let prof = NamedProfile {
                description: Some("test profile".to_string()),
                agent: Some(ProfileAgentSection {
                    model: Some("claude:opus".to_string()),
                }),
                ..Default::default()
            };
            save("testprof", &prof).unwrap();
            let loaded = load("testprof").unwrap();
            assert_eq!(loaded.description.as_deref(), Some("test profile"));
            assert_eq!(
                loaded.agent.unwrap().model.as_deref(),
                Some("claude:opus")
            );
        });
    }

    #[test]
    fn test_list_installed() {
        let _tmp = with_home(|| {
            let prof = NamedProfile::default();
            save("alpha", &prof).unwrap();
            save("beta", &prof).unwrap();
            let names = list_installed().unwrap();
            assert!(names.contains(&"alpha".to_string()));
            assert!(names.contains(&"beta".to_string()));
        });
    }
}
