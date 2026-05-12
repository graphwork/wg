//! Named runtime profiles: complete config snapshots a user can swap in.
//!
//! Storage: `~/.wg/profiles/<name>.toml` (one file per profile).
//! Active pointer: `~/.wg/active-profile` (one-line, absent = no profile).
//!
//! Design pivot (2026-05): profiles are no longer overlays. Each profile file
//! is a *complete* `Config` snapshot. `wg profile use <name>` writes the
//! profile file as `~/.wg/config.toml` (the global config), full stop. No
//! merge logic, no resolution chain — what's in the profile file is exactly
//! what runs.
//!
//! `wg profile use <name>` also removes project-local model-routing keys that
//! would shadow the selected profile. Unrelated local settings remain local.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::config::Config;

// ── Starter templates (baked into binary) ────────────────────────────────────

pub const STARTER_CLAUDE: &str = include_str!("templates/claude.toml");
pub const STARTER_CODEX: &str = include_str!("templates/codex.toml");
pub const STARTER_NEX: &str = include_str!("templates/nex.toml");

/// The three built-in starter profile names.
pub const STARTER_NAMES: &[&str] = &["claude", "codex", "nex"];

/// Legacy starter name retired in favour of the canonical `nex` name (matching
/// the `wg nex` subcommand). Recognised by `load()` and `init_starters()` so
/// existing `~/.wg/profiles/wgnext.toml` files keep working with a deprecation
/// hint until the user migrates.
pub const LEGACY_NEX_NAME: &str = "wgnext";

/// Return the baked-in template content for a starter name, or None.
pub fn starter_template(name: &str) -> Option<&'static str> {
    match name {
        "claude" => Some(STARTER_CLAUDE),
        "codex" => Some(STARTER_CODEX),
        "nex" => Some(STARTER_NEX),
        _ => None,
    }
}

// ── NamedProfile (a complete config snapshot, with optional description) ─────

/// A named runtime profile: a complete `Config` snapshot, optionally tagged
/// with a one-line human-readable description.
///
/// The profile file format is just a regular `Config` TOML file with an
/// optional top-level `description` key. Any unknown top-level keys are
/// silently ignored (same as `Config` itself), which is what lets a profile
/// file double as a full `~/.wg/config.toml`.
#[derive(Debug, Clone, Default)]
pub struct NamedProfile {
    /// Human-readable description shown by `wg profile list` / `show`.
    pub description: Option<String>,
    /// The complete config snapshot.
    pub config: Config,
}

/// Summary of project-local routing keys removed by profile activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalRoutingCleanup {
    /// The local `.wg/config.toml` path that was rewritten.
    pub path: PathBuf,
    /// Backup written before changing the local config.
    pub backup_path: PathBuf,
    /// Dotted/table keys removed from the local config.
    pub removed_keys: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct DescriptionOnly {
    #[serde(default)]
    description: Option<String>,
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
///
/// Legacy alias: when called with `name == "wgnext"` and `wgnext.toml` is
/// absent but `nex.toml` exists, transparently load `nex.toml` and emit a
/// one-line note. This keeps existing `wg profile use wgnext` invocations and
/// scripts working after the rename.
pub fn load(name: &str) -> Result<NamedProfile> {
    let path = profile_path(name)?;
    if name == LEGACY_NEX_NAME {
        if path.exists() {
            eprintln!(
                "warning: profile '{}' is deprecated — the canonical name is 'nex' (matches `wg nex`).\n\
                 Rename your profile file: mv {} {}",
                LEGACY_NEX_NAME,
                path.display(),
                path.with_file_name("nex.toml").display(),
            );
        } else {
            // wgnext.toml is absent — fall back to the canonical nex.toml so
            // legacy `wg profile use wgnext` still works after the rename.
            let canonical = path.with_file_name("nex.toml");
            if canonical.exists() {
                eprintln!(
                    "note: 'wgnext' is the legacy name; loading {} (use 'nex' going forward).",
                    canonical.display(),
                );
                return load_from_path(&canonical, "nex");
            }
        }
    }
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
    load_from_path(&path, name)
}

/// Read and parse a profile file at a given path, attributing errors to `name`.
fn load_from_path(path: &Path, name: &str) -> Result<NamedProfile> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read profile file {}", path.display()))?;
    parse_profile(&content, path, name)
}

/// Parse profile content (TOML) into a `NamedProfile`.
fn parse_profile(content: &str, path: &Path, name: &str) -> Result<NamedProfile> {
    let meta: DescriptionOnly = toml::from_str(content).unwrap_or_default();
    let config: Config = toml::from_str(content).map_err(|e| {
        anyhow::anyhow!(
            "Failed to parse profile '{}' ({}): {}\n\
             A profile is a complete config snapshot — same shape as `~/.wg/config.toml`.",
            name,
            path.display(),
            e,
        )
    })?;
    Ok(NamedProfile {
        description: meta.description,
        config,
    })
}

/// Migrate a stale legacy `wg-next:` description prefix in an existing
/// `nex.toml` to the canonical `wg nex:`. Returns true when the file was
/// rewritten. Conservative: only touches the description line, preserves all
/// other fields, comments, and formatting verbatim.
///
/// This catches users whose `nex.toml` was created (or renamed from
/// `wgnext.toml`) with the old template content, before the description was
/// updated. The previous rename only updated the in-binary template, so users
/// with on-disk files saw stale `wg-next:` text in `wg profile list`.
pub fn migrate_stale_description(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let mut changed = false;
    let mut out: Vec<String> = Vec::with_capacity(content.lines().count());
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("description") && line.contains("wg-next") {
            out.push(line.replace("wg-next", "wg nex"));
            changed = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !changed {
        return Ok(false);
    }
    let mut new_content = out.join("\n");
    if content.ends_with('\n') && !new_content.ends_with('\n') {
        new_content.push('\n');
    }
    std::fs::write(path, new_content)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(true)
}

/// Save raw TOML content as a named profile in `~/.wg/profiles/<name>.toml`.
pub fn save_raw(name: &str, content: &str) -> Result<()> {
    let dir = profiles_dir()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create profiles directory {}", dir.display()))?;
    let path = dir.join(format!("{}.toml", name));
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

// ── Profile-swap (the new core operation) ────────────────────────────────────

/// Apply a profile to the global config: copy `~/.wg/profiles/<name>.toml`
/// to `~/.wg/config.toml`, byte-for-byte, after backing up any pre-existing
/// global config.
///
/// This is the single source of truth for what `wg profile use` does. The
/// profile file IS the global config. No merge, no overlay, no resolution
/// chain. Returns the destination path written.
pub fn apply_profile_as_global_config(name: &str) -> Result<PathBuf> {
    let src = profile_path(name)?;
    if !src.exists() {
        // Use load() to get a consistent error/suggestion (covers wgnext
        // legacy fall-through and closest-match suggestions).
        load(name)?;
        // If load() somehow succeeded but the file path doesn't exist, that's
        // a bug — bail with a clear message.
        anyhow::bail!(
            "Profile '{}' source file not found at {}",
            name,
            src.display()
        );
    }
    let dst = Config::global_config_path()?;
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    if dst.exists() {
        // Skip-back-compat-ceremony stance (per repo guidance) is fine for in-binary
        // logic, but the *global config* is user-edited state — back it up once per
        // swap so a typo'd `wg profile use` doesn't silently lose hand-tuned keys.
        let ts = chrono::Local::now()
            .format("%Y-%m-%dT%H-%M-%SZ")
            .to_string();
        let bak = dst.with_file_name(format!("config.toml.bak-{}", ts));
        std::fs::copy(&dst, &bak)
            .with_context(|| format!("Failed to back up {} to {}", dst.display(), bak.display()))?;
    }
    std::fs::copy(&src, &dst).with_context(|| {
        format!(
            "Failed to copy profile {} to {}",
            src.display(),
            dst.display()
        )
    })?;
    Ok(dst)
}

/// Remove project-local model/provider routing keys that would shadow an
/// active named profile.
///
/// Named profiles are complete global config snapshots, but the regular
/// global+local merge means a repo-local `.wg/config.toml` can still pin
/// stale model routes. Profile activation is authoritative for routing, so
/// `wg profile use <name>` calls this helper for the current repository.
///
/// This intentionally removes only routing keys:
/// - top-level legacy `profile`
/// - `[agent].model` / `[agent].executor`
/// - `[dispatcher]` or legacy `[coordinator]` model/executor/provider
/// - entire `[tiers]`
/// - entire `[models]`
/// - local `llm_endpoints` endpoint routing
///
/// Other local settings, including agency flags, TUI preferences,
/// `dispatcher.max_agents`, worktree settings, MCP config, etc. are preserved.
pub fn clear_local_profile_routing_overrides(
    workgraph_dir: &Path,
) -> Result<Option<LocalRoutingCleanup>> {
    let path = workgraph_dir.join("config.toml");
    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read local config {}", path.display()))?;
    let mut value: toml::Value = content.parse().with_context(|| {
        format!(
            "Failed to parse local config {} before profile switch",
            path.display()
        )
    })?;

    let mut removed_keys = Vec::new();
    let Some(root) = value.as_table_mut() else {
        return Ok(None);
    };

    remove_top_level(root, "profile", &mut removed_keys);
    remove_section_keys(root, "agent", &["model", "executor"], &mut removed_keys);
    remove_section_keys(
        root,
        "dispatcher",
        &["model", "executor", "provider"],
        &mut removed_keys,
    );
    remove_section_keys(
        root,
        "coordinator",
        &["model", "executor", "provider"],
        &mut removed_keys,
    );
    remove_top_level(root, "tiers", &mut removed_keys);
    remove_top_level(root, "models", &mut removed_keys);
    remove_section_keys(
        root,
        "llm_endpoints",
        &["endpoints", "inherit_global"],
        &mut removed_keys,
    );

    if removed_keys.is_empty() {
        return Ok(None);
    }

    let backup_path = backup_local_config(&path)?;
    let cleaned = toml::to_string_pretty(&value).with_context(|| {
        format!(
            "Failed to serialize cleaned local config {} after removing {:?}",
            path.display(),
            removed_keys
        )
    })?;
    std::fs::write(&path, cleaned)
        .with_context(|| format!("Failed to write cleaned local config {}", path.display()))?;

    Ok(Some(LocalRoutingCleanup {
        path,
        backup_path,
        removed_keys,
    }))
}

fn remove_top_level(
    root: &mut toml::map::Map<String, toml::Value>,
    key: &str,
    removed_keys: &mut Vec<String>,
) {
    if root.remove(key).is_some() {
        removed_keys.push(key.to_string());
    }
}

fn remove_section_keys(
    root: &mut toml::map::Map<String, toml::Value>,
    section: &str,
    keys: &[&str],
    removed_keys: &mut Vec<String>,
) {
    let Some(table) = root.get_mut(section).and_then(|v| v.as_table_mut()) else {
        return;
    };

    for key in keys {
        if table.remove(*key).is_some() {
            removed_keys.push(format!("{}.{}", section, key));
        }
    }

    if table.is_empty() {
        root.remove(section);
    }
}

fn backup_local_config(path: &Path) -> Result<PathBuf> {
    let ts = chrono::Local::now()
        .format("%Y-%m-%dT%H-%M-%SZ")
        .to_string();
    let backup_path = path.with_file_name(format!("config.toml.bak-profile-{}", ts));
    std::fs::copy(path, &backup_path).with_context(|| {
        format!(
            "Failed to back up local config {} to {}",
            path.display(),
            backup_path.display()
        )
    })?;
    Ok(backup_path)
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
    Some(prof.config.agent.model)
}

// ── Validation helper ─────────────────────────────────────────────────────────

/// Validate a profile file path (parse it and return the profile).
/// Used by `wg profile edit` to validate after the user saves.
pub fn validate_file(path: &Path) -> Result<NamedProfile> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    parse_profile(
        &content,
        path,
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("(profile)"),
    )
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
    fn test_named_profile_parses_full_config_shape() {
        // A profile is a complete config snapshot — accept all the same keys
        // a `~/.wg/config.toml` would accept.
        let toml = r#"
description = "Full profile"

[agent]
model = "claude:opus"

[dispatcher]
model = "claude:opus"
max_agents = 8

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
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("p.toml");
        std::fs::write(&path, toml).unwrap();
        let prof = parse_profile(toml, &path, "test").unwrap();
        assert_eq!(prof.description.as_deref(), Some("Full profile"));
        assert_eq!(prof.config.agent.model, "claude:opus");
        assert_eq!(
            prof.config.coordinator.model.as_deref(),
            Some("claude:opus")
        );
        assert_eq!(prof.config.tiers.fast.as_deref(), Some("claude:haiku"));
        assert_eq!(prof.config.llm_endpoints.endpoints.len(), 1);
    }

    #[test]
    fn test_starter_templates_parse_as_full_config() {
        // Each starter template must parse as a complete Config — that's
        // the whole point of the snapshot model: profile file = config file.
        for name in STARTER_NAMES {
            let tmpl = starter_template(name).unwrap();
            let result = toml::from_str::<Config>(tmpl);
            assert!(
                result.is_ok(),
                "Starter template '{}' must parse as Config: {:?}",
                name,
                result
            );
        }
    }

    #[test]
    fn test_codex_starter_has_codex_models_everywhere() {
        // Regression: codex profile must NOT leave any role pinned to a claude
        // model. The original bug was that activating codex still ran claude
        // because [agent].model wasn't propagating; the snapshot model fixes
        // this by making the file itself the authoritative source.
        let prof = parse_profile(STARTER_CODEX, Path::new("codex.toml"), "codex").unwrap();
        assert_eq!(prof.config.agent.model, "codex:gpt-5.5");
        assert_eq!(
            prof.config.coordinator.model.as_deref(),
            Some("codex:gpt-5.5")
        );
        assert_eq!(
            prof.config.tiers.fast.as_deref(),
            Some("codex:gpt-5.4-mini")
        );
        assert_eq!(prof.config.tiers.standard.as_deref(), Some("codex:gpt-5.4"));
        assert_eq!(prof.config.tiers.premium.as_deref(), Some("codex:gpt-5.5"));
        // Per-role overrides for agency meta-tasks should also be codex models.
        let eval = prof
            .config
            .models
            .evaluator
            .as_ref()
            .expect("evaluator set");
        assert_eq!(eval.model.as_deref(), Some("codex:gpt-5.4-mini"));
        let assigner = prof.config.models.assigner.as_ref().expect("assigner set");
        assert_eq!(assigner.model.as_deref(), Some("codex:gpt-5.4-mini"));
    }

    #[test]
    fn test_apply_profile_as_global_config_writes_file_byte_for_byte() {
        let _tmp = with_home(|| {
            // Install the codex starter into the temp HOME's profiles dir.
            save_raw("codex", STARTER_CODEX).unwrap();
            let dst = apply_profile_as_global_config("codex").unwrap();
            assert!(dst.exists(), "global config must exist after apply");
            let written = std::fs::read_to_string(&dst).unwrap();
            assert_eq!(
                written, STARTER_CODEX,
                "global config must be byte-identical to the profile snapshot"
            );
            // Sanity: ensure the actual [agent].model line in the written file
            // names codex, not claude. This is the verbatim check the task
            // validation calls for (`grep 'model = ' ~/.wg/config.toml`).
            assert!(
                written.contains("model = \"codex:gpt-5.5\""),
                "global config must contain codex models, not claude. Got:\n{}",
                written,
            );
            assert!(
                !written.contains("claude:opus"),
                "global config must not retain any claude models after codex swap. Got:\n{}",
                written,
            );
        });
    }

    #[test]
    fn test_apply_profile_backs_up_existing_global_config() {
        let _tmp = with_home(|| {
            save_raw("codex", STARTER_CODEX).unwrap();
            let dst = Config::global_config_path().unwrap();
            std::fs::write(
                &dst,
                "# pre-existing user content\n[agent]\nmodel = \"claude:opus\"\n",
            )
            .unwrap();
            apply_profile_as_global_config("codex").unwrap();
            // At least one backup file must exist alongside config.toml.
            let dir = dst.parent().unwrap();
            let bak_count = std::fs::read_dir(dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("config.toml.bak-")
                })
                .count();
            assert!(bak_count >= 1, "expected a config.toml.bak-* backup file");
        });
    }

    #[test]
    fn test_apply_profile_overwrites_claude_with_codex() {
        // The original bug: activating codex profile left agent.model at
        // claude:opus. With profile-as-swap, the new config.toml must reflect
        // codex everywhere — verified by re-loading Config from disk.
        let _tmp = with_home(|| {
            save_raw("codex", STARTER_CODEX).unwrap();
            // Pre-seed global config with claude (simulates a user previously
            // on the claude profile).
            let dst = Config::global_config_path().unwrap();
            std::fs::write(&dst, STARTER_CLAUDE).unwrap();
            // Swap.
            apply_profile_as_global_config("codex").unwrap();
            // Re-load the global config from disk and verify codex models.
            let cfg = Config::load_global()
                .unwrap()
                .expect("global must be present");
            assert_eq!(cfg.agent.model, "codex:gpt-5.5");
            assert_eq!(cfg.coordinator.model.as_deref(), Some("codex:gpt-5.5"));
            assert_eq!(cfg.tiers.fast.as_deref(), Some("codex:gpt-5.4-mini"));
        });
    }

    #[test]
    fn test_clear_local_profile_routing_overrides_preserves_unrelated_settings() {
        let _tmp = with_home(|| {
            save_raw("codex", STARTER_CODEX).unwrap();
            apply_profile_as_global_config("codex").unwrap();

            let project = tempfile::tempdir().unwrap();
            let wg_dir = project.path().join(".wg");
            std::fs::create_dir_all(&wg_dir).unwrap();
            std::fs::write(
                wg_dir.join("config.toml"),
                r#"
profile = "openai"

[agent]
model = "claude:opus"
executor = "claude"
interval = 13

[dispatcher]
model = "claude:opus"
executor = "claude"
provider = "claude"
max_agents = 3

[tiers]
fast = "claude:haiku"
standard = "claude:sonnet"
premium = "claude:opus"

[models.default]
model = "claude:opus"

[models.evaluator]
model = "claude:haiku"

[llm_endpoints]
inherit_global = false

[[llm_endpoints.endpoints]]
name = "stale"
provider = "openrouter"
url = "https://stale.invalid/v1"
is_default = true

[agency]
auto_assign = false
auto_evaluate = false
assigner_agent = "local-agent"
"#,
            )
            .unwrap();

            let cleanup = clear_local_profile_routing_overrides(&wg_dir)
                .unwrap()
                .expect("local routing keys should be removed");

            for key in [
                "profile",
                "agent.model",
                "agent.executor",
                "dispatcher.model",
                "dispatcher.executor",
                "dispatcher.provider",
                "tiers",
                "models",
                "llm_endpoints.endpoints",
                "llm_endpoints.inherit_global",
            ] {
                assert!(
                    cleanup.removed_keys.contains(&key.to_string()),
                    "cleanup should report removed key {key}; got {:?}",
                    cleanup.removed_keys
                );
            }
            assert!(cleanup.backup_path.exists(), "backup file must exist");

            let cleaned = std::fs::read_to_string(wg_dir.join("config.toml")).unwrap();
            assert!(
                cleaned.contains("interval = 13"),
                "agent.interval must be preserved:\n{}",
                cleaned
            );
            assert!(
                cleaned.contains("max_agents = 3"),
                "dispatcher.max_agents must be preserved:\n{}",
                cleaned
            );
            assert!(
                cleaned.contains("assigner_agent = \"local-agent\""),
                "agency settings must be preserved:\n{}",
                cleaned
            );
            assert!(
                !cleaned.contains("claude:opus") && !cleaned.contains("claude:haiku"),
                "stale claude model pins must be removed:\n{}",
                cleaned
            );
            assert!(
                !cleaned.contains("[tiers]") && !cleaned.contains("[models"),
                "local tiers/models routing tables must be removed:\n{}",
                cleaned
            );

            let merged = Config::load_merged(&wg_dir).unwrap();
            assert_eq!(merged.agent.model, "codex:gpt-5.5");
            assert_eq!(merged.coordinator.model.as_deref(), Some("codex:gpt-5.5"));
            assert_eq!(merged.coordinator.effective_executor(), "codex");
            assert_eq!(merged.tiers.fast.as_deref(), Some("codex:gpt-5.4-mini"));
            assert_eq!(merged.coordinator.max_agents, 3);
            assert_eq!(merged.agent.interval, 13);
            assert!(!merged.agency.auto_assign);
            assert_eq!(merged.agency.assigner_agent.as_deref(), Some("local-agent"));
        });
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
            let content = "description = \"test profile\"\n\n[agent]\nmodel = \"claude:opus\"\n";
            save_raw("testprof", content).unwrap();
            let loaded = load("testprof").unwrap();
            assert_eq!(loaded.description.as_deref(), Some("test profile"));
            assert_eq!(loaded.config.agent.model, "claude:opus");
        });
    }

    #[test]
    fn test_list_installed() {
        let _tmp = with_home(|| {
            save_raw("alpha", "[agent]\nmodel = \"claude:opus\"\n").unwrap();
            save_raw("beta", "[agent]\nmodel = \"claude:sonnet\"\n").unwrap();
            let names = list_installed().unwrap();
            assert!(names.contains(&"alpha".to_string()));
            assert!(names.contains(&"beta".to_string()));
        });
    }

    #[test]
    fn test_starter_names_uses_canonical_nex_name() {
        assert!(
            STARTER_NAMES.contains(&"nex"),
            "STARTER_NAMES must include the canonical 'nex' name (matches `wg nex`); got {:?}",
            STARTER_NAMES
        );
        assert!(
            !STARTER_NAMES.contains(&"wgnext"),
            "STARTER_NAMES must NOT include the legacy 'wgnext' name; got {:?}",
            STARTER_NAMES
        );
        assert!(
            starter_template("nex").is_some(),
            "starter_template(\"nex\") must return the template"
        );
        assert!(
            starter_template("wgnext").is_none(),
            "starter_template(\"wgnext\") must NOT return a template (legacy name retired)"
        );
    }

    #[test]
    fn test_nex_starter_template_uses_wg_nex_phrasing() {
        let tmpl = starter_template("nex").unwrap();
        assert!(
            tmpl.contains("wg nex"),
            "nex starter template must describe itself as `wg nex` (matches the subcommand); got: {}",
            tmpl
        );
        assert!(
            !tmpl.contains("wg-next"),
            "nex starter template must not contain the legacy 'wg-next' hyphenation; got: {}",
            tmpl
        );
        assert!(
            !tmpl.contains("wgnext"),
            "nex starter template must not contain the legacy 'wgnext' spelling; got: {}",
            tmpl
        );
    }

    #[test]
    fn test_load_legacy_wgnext_profile_still_works() {
        let _tmp = with_home(|| {
            // Simulate a user who ran `wg profile init-starters` on an older
            // build that wrote `wgnext.toml` and never re-ran init-starters.
            save_raw(
                LEGACY_NEX_NAME,
                "description = \"legacy\"\n[agent]\nmodel = \"nex:qwen3-coder-30b\"\n",
            )
            .unwrap();
            // Loading by the legacy name must still succeed (backward compat).
            let loaded = load(LEGACY_NEX_NAME).unwrap();
            assert_eq!(loaded.description.as_deref(), Some("legacy"));
        });
    }

    #[test]
    fn test_load_wgnext_falls_back_to_nex_when_legacy_file_absent() {
        let _tmp = with_home(|| {
            save_raw(
                "nex",
                "description = \"canonical-nex\"\n[agent]\nmodel = \"nex:qwen3-coder-30b\"\n",
            )
            .unwrap();
            assert!(!profile_path(LEGACY_NEX_NAME).unwrap().exists());

            let loaded = load(LEGACY_NEX_NAME).unwrap();
            assert_eq!(
                loaded.description.as_deref(),
                Some("canonical-nex"),
                "load(\"wgnext\") must fall back to nex.toml when wgnext.toml is absent"
            );
        });
    }

    #[test]
    fn test_migrate_stale_description_rewrites_wg_next_to_wg_nex() {
        let _tmp = with_home(|| {
            let stale = "description = \"wg-next: in-process nex handler at a localhost endpoint (edit URL per machine)\"\n\n[agent]\nmodel = \"nex:qwen3-coder-30b\"\n\n# user comment that must be preserved\n";
            let path = profile_path("nex").unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, stale).unwrap();

            let changed = migrate_stale_description(&path).unwrap();
            assert!(changed, "expected migration to rewrite stale description");

            let after = std::fs::read_to_string(&path).unwrap();
            assert!(
                after.contains("description = \"wg nex:"),
                "description must be rewritten to start with `wg nex:`; got: {}",
                after
            );
            assert!(
                !after.contains("wg-next"),
                "no 'wg-next' substring may remain; got: {}",
                after
            );
            assert!(
                after.contains("# user comment that must be preserved"),
                "comments must be preserved verbatim; got: {}",
                after
            );

            // Idempotent: a second migrate call returns false, file unchanged.
            let changed_again = migrate_stale_description(&path).unwrap();
            assert!(!changed_again, "second migration must be a no-op");
        });
    }

    #[test]
    fn test_migrate_stale_description_leaves_clean_files_alone() {
        let _tmp = with_home(|| {
            let clean = STARTER_NEX;
            let path = profile_path("nex").unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, clean).unwrap();

            let changed = migrate_stale_description(&path).unwrap();
            assert!(
                !changed,
                "fresh nex.toml from the current template must not be rewritten"
            );
            let after = std::fs::read_to_string(&path).unwrap();
            assert_eq!(
                after, clean,
                "file must be byte-identical when no migration needed"
            );
        });
    }

    #[test]
    fn test_migrate_stale_description_only_touches_description_line() {
        let _tmp = with_home(|| {
            let mixed = "description = \"my custom\"\n\n[agent]\nmodel = \"nex:custom-wg-next-model\"\n# wg-next: legacy reference in a comment\n";
            let path = profile_path("nex").unwrap();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, mixed).unwrap();

            let changed = migrate_stale_description(&path).unwrap();
            assert!(!changed, "must not migrate when description is clean");
            let after = std::fs::read_to_string(&path).unwrap();
            assert_eq!(after, mixed);
        });
    }
}
