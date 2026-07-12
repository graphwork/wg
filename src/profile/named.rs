//! Named runtime profiles: complete config snapshots a user can swap in.
//!
//! Storage: `~/.wg/profiles/<name>.toml` (one file per profile).
//! Active pointer: `~/.wg/active-profile` (one-line, absent = no profile).
//!
//! Design pivot (2026-07): profiles are **overlays**, not byte-for-byte
//! swaps. Each profile file is a `Config` TOML snapshot whose *routing* keys
//! (`description`, `[agent].model`, `[dispatcher].model`/`max_agents`,
//! `[tiers]`, `[models]`, and `[llm_endpoints]` when declared) are overlaid
//! onto the existing `~/.wg/config.toml`. Unrelated global state — notably a
//! configured OpenRouter endpoint / credential — survives a `wg profile use
//! <name>`, and the merged file is canonicalized (the same predicates as `wg
//! migrate config`) so no deprecated/removed keys are (re)introduced.
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
pub const STARTER_OPENCODE: &str = include_str!("templates/opencode.toml");
pub const STARTER_PI: &str = include_str!("templates/pi.toml");

/// The built-in starter profile names.
pub const STARTER_NAMES: &[&str] = &["claude", "codex", "nex", "opencode", "pi"];

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
        "opencode" => Some(STARTER_OPENCODE),
        "pi" => Some(STARTER_PI),
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
        // Built-in starter fallback: a starter profile (`claude`, `codex`,
        // `nex`, `opencode`, `pi`) is usable via `wg profile use <name>` even when it
        // has not been materialized to `~/.wg/profiles/<name>.toml` yet — the
        // canonical snapshot ships in the binary. This is strictly additive:
        // it only fires where `load()` previously errored (missing file), and
        // an on-disk file (user customization) always wins via the path above.
        if let Some(template) = starter_template(name) {
            return parse_profile(template, &path, name);
        }
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

// ── Profile-overlay (the new core operation) ────────────────────────────────────

/// Apply a profile to the global config by **overlaying** the profile's
/// routing keys onto the existing `~/.wg/config.toml`, preserving unrelated
/// global state (notably `[[llm_endpoints]]` credentials/endpoints such as
/// an OpenRouter login), then writing the merged, canonicalized result.
///
/// This is an **overlay, not a byte-for-byte swap**. The profile owns a
/// well-defined set of routing keys — `description`, `[agent].model`,
/// `[dispatcher].model` / `max_agents`, `[tiers]`, `[models]`, and (when the
/// profile declares it) `[llm_endpoints]`. Everything else in the existing
/// global config (credentials/endpoints, agency flags, TUI settings, …)
/// survives a `wg profile use <name>`. A profile with no `[llm_endpoints]`
/// (the `pi` / `claude` / `codex` starters) therefore leaves a configured
/// OpenRouter endpoint untouched; a profile that ships its own endpoint
/// (the `nex` starter's localhost) installs it.
///
/// The merged document is canonicalized via the same predicates as
/// `wg migrate config` (drop deprecated keys, rename `poll_interval` →
/// `safety_interval`, fix stale model strings, drop orphaned `[openrouter]`)
/// before writing, so the result is lint-clean and never *reintroduces*
/// deprecated/removed keys the way a `Config::save_global` round-trip would.
///
/// The pre-existing global config is backed up once per swap so a typo'd
/// `wg profile use` doesn't silently lose hand-tuned keys.
pub fn apply_profile_as_global_config(name: &str) -> Result<PathBuf> {
    let src = profile_path(name)?;
    if !src.exists() {
        // Built-in starter self-bootstrap: `wg profile use pi` (and the
        // other starters) should work out of the box without a prior
        // `wg profile init-starters`. Materialize the in-binary snapshot to
        // `~/.wg/profiles/<name>.toml` so the overlay below has a source. An
        // on-disk file (user customization) is never overwritten — we only
        // reach here when the file is absent.
        if let Some(template) = starter_template(name) {
            save_raw(name, template).with_context(|| {
                format!("Failed to materialize built-in starter profile '{}'", name)
            })?;
        } else {
            // Use load() to get a consistent error/suggestion (covers wgnext
            // legacy fall-through and closest-match suggestions).
            load(name)?;
            // If load() somehow succeeded but the file path doesn't exist,
            // that's a bug — bail with a clear message.
            anyhow::bail!(
                "Profile '{}' source file not found at {}",
                name,
                src.display()
            );
        }
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

    // Load the existing global config as a TOML tree (or an empty table when
    // there is no prior global config). Parse failures fall back to an empty
    // table rather than aborting: a corrupt global config should not block a
    // profile swap (the backup above preserves the corrupt file for repair).
    let existing: toml::Value = if dst.exists() {
        std::fs::read_to_string(&dst)
            .with_context(|| format!("Failed to read existing global config {}", dst.display()))?
            .parse::<toml::Value>()
            .unwrap_or_else(|_| toml::Value::Table(toml::map::Map::new()))
    } else {
        toml::Value::Table(toml::map::Map::new())
    };

    let profile_content = std::fs::read_to_string(&src)
        .with_context(|| format!("Failed to read profile {}", src.display()))?;
    let profile: toml::Value = profile_content
        .parse()
        .with_context(|| format!("Failed to parse profile {} as TOML", src.display()))?;

    // Overlay the profile's routing keys onto the existing global config.
    let mut merged = overlay_profile_onto_global(existing, &profile);

    // Canonicalize the merged document so the written config is lint-clean and
    // never reintroduces deprecated/removed keys (the bug: a `Config::save_global`
    // round-trip re-emits `dispatcher.poll_interval` and removed compaction/verify
    // keys with their serde defaults).
    crate::config_migrate::canonicalize_in_place(&mut merged);

    let content = toml::to_string_pretty(&merged)
        .context("Failed to serialize merged profile + global config")?;
    std::fs::write(&dst, content)
        .with_context(|| format!("Failed to write merged global config to {}", dst.display()))?;
    Ok(dst)
}

/// The set of top-level TOML keys a profile is authoritative for. Everything
/// outside this set in the existing global config is preserved verbatim by
/// [`overlay_profile_onto_global`].
const PROFILE_ROUTING_TOP_KEYS: &[&str] = &[
    "description",
    "profile",
    "agent",
    "dispatcher",
    "tiers",
    "models",
];

/// Overlay a profile's routing keys onto an existing global config TOML tree,
/// preserving unrelated global state (endpoints/credentials, agency flags, …).
///
/// Merge rules:
/// - Scalar top-level routing keys (`description`, `profile`): profile wins,
///   replace.
/// - Subtable routing keys (`agent`, `dispatcher`, `tiers`, `models`): merge
///   field-by-field with the profile winning on conflict, so an existing
///   `dispatcher.safety_interval` or `agent.interval` survives a swap while
///   the profile's `model` / `max_agents` / tier / role pins take effect.
/// - `llm_endpoints`: when the profile declares it, the profile owns the
///   endpoint set (replace) — this lets the `nex` starter install its
///   localhost endpoint. When the profile omits it (the `pi` / `claude` /
///   `codex` starters), the existing global endpoints are preserved — this
///   is the fix for the OpenRouter-login-clobbering bug.
/// - Any other top-level key in the existing config (agency, tui, checkpoint,
///   log, …) is left untouched.
fn overlay_profile_onto_global(mut existing: toml::Value, profile: &toml::Value) -> toml::Value {
    let prof_table = match profile.as_table() {
        Some(t) => t,
        None => return existing,
    };
    let dst = match existing.as_table_mut() {
        Some(t) => t,
        None => return existing,
    };

    // Scalar top-level routing keys: profile wins (replace).
    for key in ["description", "profile"] {
        if let Some(v) = prof_table.get(key) {
            dst.insert(key.to_string(), v.clone());
        }
    }

    // Subtable routing keys: merge field-by-field (profile wins on conflict).
    for key in ["agent", "dispatcher", "tiers", "models"] {
        if let Some(prof_sub) = prof_table.get(key) {
            let entry = dst
                .entry(key.to_string())
                .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
            merge_tables_field_by_field(entry, prof_sub);
        }
    }

    // `llm_endpoints`: profile-declared → replace; profile-omitted → preserve
    // existing (the OpenRouter-login-clobbering fix).
    if let Some(prof_ep) = prof_table.get("llm_endpoints") {
        dst.insert("llm_endpoints".to_string(), prof_ep.clone());
    }

    let _ = PROFILE_ROUTING_TOP_KEYS; // single source of truth for the key set
    existing
}

/// Recursively merge `profile` into `existing` field-by-field, with `profile`
/// winning on conflict. For (Table, Table) pairs both sides' keys survive; for
/// any other shape (or a non-table `existing`) `profile` replaces it.
fn merge_tables_field_by_field(existing: &mut toml::Value, profile: &toml::Value) {
    match (existing.as_table_mut(), profile.as_table()) {
        (Some(dst), Some(src)) => {
            for (k, v) in src {
                dst.insert(k.clone(), v.clone());
            }
        }
        _ => {
            *existing = profile.clone();
        }
    }
}

/// Patch a pinned default-route model into the freshly-written global config
/// (`~/.wg/config.toml`) in place, then re-canonicalize. Used by model-qualified
/// profile activation (`wg profile use claude:opus` / `codex:gpt-5.5` /
/// `nex:<model>`) to pin the strong-tier route without round-tripping through
/// `Config::save_global` (which re-emits deprecated field names).
///
/// Pins exactly the [`Config::PI_STRONG_TOML_KEYS`] set. The companion default
/// (fast tier + agency roles) is left alone — after `apply_profile_as_global_config`
/// merged the starter template, those already hold the matching provider's
/// starter values, so [`Config::pin_provider_companion_defaults`] would be a
/// no-op; pinning only the strong keys reproduces `pin_default_route_model`
/// for the same-provider activations `parse_profile_use_target` emits.
pub fn patch_global_pinned_model(model: &str) -> Result<()> {
    let dst = Config::global_config_path()?;
    let content = std::fs::read_to_string(&dst)
        .with_context(|| format!("Failed to read global config {} for pin", dst.display()))?;
    let mut doc: toml::Value = content.parse().with_context(|| {
        format!(
            "Failed to parse global config {} after profile apply",
            dst.display()
        )
    })?;
    for dotted in Config::PI_STRONG_TOML_KEYS {
        set_dotted_string(&mut doc, dotted, model);
    }
    crate::config_migrate::canonicalize_in_place(&mut doc);
    let body = toml::to_string_pretty(&doc).context("Failed to serialize pinned global config")?;
    std::fs::write(&dst, body)
        .with_context(|| format!("Failed to write pinned global config {}", dst.display()))?;
    Ok(())
}

/// Set a dotted TOML key (`table.key` / `table.sub.key`, or a bare top-level
/// `key`) to a string value inside `doc`, creating intermediate tables as
/// needed. Used by [`patch_global_pinned_model`] to pin the strong-tier route
/// on the parsed TOML tree (the file is `toml::to_string_pretty` output, so
/// this lossless tree edit is preferred over the line patcher).
fn set_dotted_string(doc: &mut toml::Value, dotted: &str, value: &str) {
    let segments: Vec<&str> = dotted.split('.').collect();
    let (path_segs, leaf) = segments.split_at(segments.len() - 1);
    let leaf = leaf[0];
    let mut cursor = doc;
    for seg in path_segs {
        let entry = cursor
            .as_table_mut()
            .expect("config tree must be table-valued at every routing prefix")
            .entry(seg.to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        cursor = entry;
    }
    if let Some(table) = cursor.as_table_mut() {
        table.insert(leaf.to_string(), toml::Value::String(value.to_string()));
    }
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

    // Canonicalize the local config so it cannot conflict with the canonical
    // GLOBAL config the profile overlay just wrote. Without this, a local
    // `[dispatcher] poll_interval` (written by `wg init`'s `Config::save`,
    // which serializes the deprecated field name) survives alongside a GLOBAL
    // `[dispatcher] safety_interval` (canonicalized by `apply_profile_as_global_config`),
    // and the merged-config deserialize fails with "duplicate field `poll_interval`"
    // — exactly the regression the bug report calls out.
    let canon = crate::config_migrate::canonicalize_in_place(&mut value);
    let canon_changed =
        !canon.removed.is_empty() || !canon.renamed.is_empty() || !canon.rewritten.is_empty();

    if removed_keys.is_empty() && !canon_changed {
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

    // Fold the canonicalization renames into the reported removed-keys list so
    // the user sees what changed (e.g. `dispatcher.poll_interval` removed as
    // part of canonicalization).
    removed_keys.extend(canon.removed);

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

// ── Two-tier Pi profile patch (`wg profile pi`) ──────────────────────────────

/// Does a trimmed TOML line define the bare key `key` (i.e. `key = ...`)?
///
/// Matches `key = x`, `key=x`, and `key   = x`, but not `keyfoo = x` or
/// `foo_key = x`. Quoted keys are not handled — the Pi starter uses bare keys.
fn line_defines_key(trimmed: &str, key: &str) -> bool {
    match trimmed.strip_prefix(key) {
        Some(rest) => rest.trim_start().starts_with('='),
        None => false,
    }
}

/// Set a dotted TOML key (`table.key` / `table.sub.key`, or a bare top-level
/// `key`) to a string value in `content`, preserving comments, ordering, blank
/// lines, and every unrelated key.
///
/// The last dotted segment is the key; everything before it is the table header
/// (`[table]` / `[table.sub]`). Behaviour:
/// - key present under its table  → replace the value in place (keep indent);
/// - table present, key absent    → insert `key = "value"` right after the
///   header line;
/// - table absent                 → append `[table]` + `key = "value"` at EOF.
///
/// Array-of-tables headers (`[[x]]`) reset the active-table tracking so a key in
/// `[tiers]` is never confused with one inside an array element. This is a
/// deliberately small line patcher (not a full TOML round-trip) precisely so the
/// hand-written `pi.toml` comment blocks survive a write — `toml::to_string`
/// would discard them.
pub fn set_toml_string_value(content: &str, dotted: &str, value: &str) -> String {
    let (table, key) = match dotted.rsplit_once('.') {
        Some((t, k)) => (t, k),
        None => ("", dotted),
    };
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    let assignment = format!("{} = \"{}\"", key, escaped);

    let mut out: Vec<String> = Vec::with_capacity(content.lines().count() + 2);
    let mut in_target = table.is_empty();
    let mut replaced = false;
    // Top-level keys live before the first header; treat the file start as the
    // "header" insertion anchor when targeting a top-level key.
    let mut header_idx: Option<usize> = if in_target { Some(0) } else { None };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[[") {
            in_target = false;
            out.push(line.to_string());
            continue;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_target = trimmed[1..trimmed.len() - 1].trim() == table;
            out.push(line.to_string());
            if in_target && !replaced {
                header_idx = Some(out.len());
            }
            continue;
        }
        if !replaced && in_target && line_defines_key(trimmed, key) {
            let indent_len = line.len() - line.trim_start().len();
            out.push(format!("{}{}", &line[..indent_len], assignment));
            replaced = true;
            continue;
        }
        out.push(line.to_string());
    }

    if !replaced {
        match header_idx {
            // Table (or top-level) present but key missing: insert after header.
            Some(idx) if table.is_empty() || idx > 0 => out.insert(idx, assignment),
            _ => {
                if out.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
                    out.push(String::new());
                }
                out.push(format!("[{}]", table));
                out.push(assignment);
            }
        }
    }

    let mut result = out.join("\n");
    if content.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Apply a two-tier (`strong`/`weak`) update to a named profile's TOML file,
/// preserving comments and unrelated keys.
///
/// Writes [`Config::PI_STRONG_TOML_KEYS`] for `strong` and
/// [`Config::PI_WEAK_TOML_KEYS`] for `weak` (each only when `Some`). When the
/// profile file does not yet exist it is seeded from the baked-in starter
/// template first (so a first `wg profile pi` never fails on a missing file —
/// design §6.1). Returns the path written.
pub fn patch_pi_tiers(name: &str, strong: Option<&str>, weak: Option<&str>) -> Result<PathBuf> {
    let path = profile_path(name)?;
    let mut content = if path.exists() {
        std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read profile file {}", path.display()))?
    } else if let Some(template) = starter_template(name) {
        template.to_string()
    } else {
        // Reuse load()'s closest-match / suggestion error.
        load(name)?;
        anyhow::bail!(
            "Profile '{}' source file not found at {}",
            name,
            path.display()
        );
    };

    if let Some(s) = strong {
        // Strong tier must route through the self-authenticating pi handler, not
        // the in-process nex OpenRouter client (which requires a wg-side key).
        // Normalize an `openrouter:`/bare route to a `pi:` route before writing;
        // CLI / pi / nex-local specs pass through unchanged. Mirrors
        // `Config::set_pi_tiers` so the file patcher and in-memory writer agree.
        let s = crate::config::pi_strong_route(s);
        for dotted in Config::PI_STRONG_TOML_KEYS {
            content = set_toml_string_value(&content, dotted, &s);
        }
    }
    if let Some(w) = weak {
        for dotted in Config::PI_WEAK_TOML_KEYS {
            content = set_toml_string_value(&content, dotted, w);
        }
    }

    save_raw(name, &content)?;
    Ok(path)
}

/// Apply a per-role model override (`models.<role>.model`) to a named profile's
/// TOML file, preserving comments, ordering, and every unrelated key.
///
/// The `dotted` key is the full dotted TOML path (e.g. `models.task_agent.model`).
/// The model spec is written **verbatim** — no strong-tier `pi:` normalization,
/// because a per-role override may legitimately be a native `openrouter:` route
/// (the weak agency tier) or a `pi:` route; the caller (`wg profile set-model`)
/// validates handler-first form before calling. When the profile file does not
/// yet exist it is seeded from the baked-in starter template first, mirroring
/// [`patch_pi_tiers`]. Returns the path written.
///
/// Used by `wg profile set-model <profile> <role> <model>` (the per-role escape
/// hatch that wins over the two-tier strong/weak key-set).
pub fn patch_role_model(name: &str, dotted: &str, model: &str) -> Result<PathBuf> {
    let path = profile_path(name)?;
    let mut content = if path.exists() {
        std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read profile file {}", path.display()))?
    } else if let Some(template) = starter_template(name) {
        template.to_string()
    } else {
        // Reuse load()'s closest-match / suggestion error.
        load(name)?;
        anyhow::bail!(
            "Profile '{}' source file not found at {}",
            name,
            path.display()
        );
    };

    content = set_toml_string_value(&content, dotted, model);

    // Validate the patched content still parses as a Config so a malformed
    // edit never leaves a broken profile on disk.
    let _check: Config = toml::from_str(&content).map_err(|e| {
        anyhow::anyhow!(
            "Patched profile '{}' failed to parse as Config after setting {} = \"{}\": {}",
            name,
            dotted,
            model,
            e,
        )
    })?;

    save_raw(name, &content)?;
    Ok(path)
}

// ── Per-role model override (`wg profile set-model`) ──────────────────────────

/// The durable outcome of a per-role model override write, surfaced to the
/// caller (the `wg profile set-model` CLI) so it can print a consistent echo
/// and decide on daemon reload. Built by [`set_role_model_override`].
#[derive(Debug, Clone)]
pub struct RoleModelOverrideOutcome {
    /// Profile name the override was applied to.
    pub profile: String,
    /// Dispatch role display name (e.g. `task_agent`, `default`).
    pub role: String,
    /// Dotted TOML key that was written (e.g. `models.task_agent.model`).
    pub dotted: String,
    /// Model spec written verbatim (handler-first form preserved).
    pub model: String,
    /// Previous value at this dotted key, if any (`None` = newly created).
    pub previous: Option<String>,
    /// Whether the edited profile is the active one (so a re-apply is warranted).
    pub is_active: bool,
    /// Whether this was a dry-run (no files written).
    pub dry_run: bool,
    /// Path written, when not a dry-run.
    pub wrote_path: Option<PathBuf>,
    /// Whether the global config was re-applied (true only when active + not dry-run).
    pub reapplied_global: bool,
}

/// Apply a per-role model override (`models.<role>.model`) to a named profile.
///
/// This is the durable, testable core of `wg profile set-model <profile> <role>
/// <model>`. It lives in the lib (not the bin) so its HOME-mutating tests run
/// in the lib's single test process alongside the other `with_home` tests,
/// avoiding the cross-binary `HOME` race that two parallel test binaries
/// (lib unit tests + bin unit tests) would otherwise hit.
///
/// Validates the role parses as a [`crate::config::DispatchRole`] and the model
/// spec is handler-first ([`crate::config::parse_model_spec_strict`]), then —
/// unless `dry_run` — patches `~/.wg/profiles/<profile>.toml` via the
/// comment-preserving line patcher ([`patch_role_model`]) and, when the edited
/// profile is the active one, re-applies it as the global config so the next
/// spawned worker picks up the change.
///
/// Handler-first model specs are preserved **verbatim** — a `pi:openrouter/...`
/// route stays a `pi:` route. We do NOT run the strong-tier `pi_strong_route`
/// normalization, because a per-role override may legitimately be a native
/// `openrouter:` route (the weak agency tier) or a `pi:` route; the user
/// explicitly picks the route for this one role. Per-role overrides always win
/// over the two-tier (`wg profile pi`) strong/weak key-set, so this is the
/// escape hatch when a single role needs to diverge from its tier.
///
/// The caller (the CLI) is responsible for the daemon hot-reload IPC and the
/// human-facing echo; this function returns the structured outcome.
pub fn set_role_model_override(
    profile: &str,
    role: &str,
    model: &str,
    dry_run: bool,
) -> Result<RoleModelOverrideOutcome> {
    use crate::config::{DispatchRole, parse_model_spec_strict};

    let dispatch_role: DispatchRole = role.parse().with_context(|| {
        format!(
            "Unknown role '{}'. Valid roles: default, task_agent, evaluator, \
             flip_inference, flip_comparison, assigner, evolver, verification, \
             triage, creator, compactor, placer, chat_compactor, reviewer.",
            role,
        )
    })?;

    parse_model_spec_strict(model).with_context(|| {
        format!(
            "Invalid model spec '{}'. Use handler-first provider:model format \
             (e.g., 'claude:opus', 'pi:openrouter/z-ai/glm-5.2', \
             'openrouter:deepseek/deepseek-chat').",
            model,
        )
    })?;

    let prof = load(profile)?;
    let dotted = format!("models.{}.model", dispatch_role);
    let previous = prof
        .config
        .models
        .get_role(dispatch_role)
        .and_then(|r| r.model.clone());
    let is_active = active().unwrap_or(None).as_deref() == Some(profile);

    if dry_run {
        return Ok(RoleModelOverrideOutcome {
            profile: profile.to_string(),
            role: dispatch_role.to_string(),
            dotted,
            model: model.to_string(),
            previous,
            is_active,
            dry_run: true,
            wrote_path: None,
            reapplied_global: false,
        });
    }

    let wrote_path = patch_role_model(profile, &dotted, model)?;
    let mut reapplied_global = false;
    if is_active {
        apply_profile_as_global_config(profile)?;
        reapplied_global = true;
    }

    Ok(RoleModelOverrideOutcome {
        profile: profile.to_string(),
        role: dispatch_role.to_string(),
        dotted,
        model: model.to_string(),
        previous,
        is_active,
        dry_run: false,
        wrote_path: Some(wrote_path),
        reapplied_global,
    })
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
        assert_eq!(prof.config.agent.model, "codex:gpt-5.6-sol");
        assert_eq!(
            prof.config.coordinator.model.as_deref(),
            Some("codex:gpt-5.6-sol")
        );
        assert_eq!(
            prof.config.tiers.fast.as_deref(),
            Some("codex:gpt-5.6-luna")
        );
        assert_eq!(
            prof.config.tiers.fast_reasoning,
            Some(crate::config::ReasoningLevel::Low)
        );
        assert_eq!(
            prof.config.tiers.standard.as_deref(),
            Some("codex:gpt-5.6-sol")
        );
        assert_eq!(
            prof.config.tiers.standard_reasoning,
            Some(crate::config::ReasoningLevel::High)
        );
        assert_eq!(
            prof.config.tiers.premium.as_deref(),
            Some("codex:gpt-5.6-sol")
        );
        assert_eq!(
            prof.config.tiers.premium_reasoning,
            Some(crate::config::ReasoningLevel::Xhigh)
        );
        assert_eq!(
            prof.config
                .models
                .default
                .as_ref()
                .and_then(|m| m.model.as_deref()),
            Some("codex:gpt-5.6-sol")
        );
        assert_eq!(
            prof.config
                .models
                .task_agent
                .as_ref()
                .and_then(|m| m.model.as_deref()),
            Some("codex:gpt-5.6-sol")
        );
        // Per-role overrides for agency meta-tasks should also be codex models.
        let eval = prof
            .config
            .models
            .evaluator
            .as_ref()
            .expect("evaluator set");
        assert_eq!(eval.model.as_deref(), Some("codex:gpt-5.6-luna"));
        assert_eq!(eval.reasoning, Some(crate::config::ReasoningLevel::Low));
        let assigner = prof.config.models.assigner.as_ref().expect("assigner set");
        assert_eq!(assigner.model.as_deref(), Some("codex:gpt-5.6-luna"));
        assert_eq!(assigner.reasoning, Some(crate::config::ReasoningLevel::Low));
    }

    #[test]
    fn test_opencode_starter_has_opencode_worker_and_claude_agency_models() {
        // The opencode starter pins worker roles to opencode routes while
        // keeping the agency one-shot roles on claude:haiku, per CLAUDE.md
        // "Agency tasks run on claude CLI". It ships BOTH research-picked
        // tiers: lightweight (stepfun, the proven live route) as the default
        // worker/coordinator model, and premium (minimax-m2.7) on the premium
        // tier (see research-opencode-default-models).
        let prof = parse_profile(STARTER_OPENCODE, Path::new("opencode.toml"), "opencode").unwrap();
        let lightweight = "opencode:openrouter/stepfun/step-3.7-flash";
        let premium = "opencode:openrouter/minimax/minimax-m2.7";
        assert_eq!(prof.config.agent.model, lightweight);
        assert_eq!(prof.config.coordinator.model.as_deref(), Some(lightweight));
        assert_eq!(prof.config.tiers.fast.as_deref(), Some(lightweight));
        assert_eq!(prof.config.tiers.standard.as_deref(), Some(lightweight));
        // Premium tier escalates to the research premium pick.
        assert_eq!(prof.config.tiers.premium.as_deref(), Some(premium));
        assert_eq!(
            prof.config
                .models
                .default
                .as_ref()
                .and_then(|m| m.model.as_deref()),
            Some(lightweight)
        );
        assert_eq!(
            prof.config
                .models
                .task_agent
                .as_ref()
                .and_then(|m| m.model.as_deref()),
            Some(lightweight)
        );
        // Both research-picked routes must be reachable from the profile.
        assert!(
            prof.config.tiers.premium.as_deref() == Some(premium),
            "premium tier must carry the research premium pick (minimax-m2.7)"
        );
        // Agency meta-roles stay on claude:haiku — opencode is worker-only and
        // does not serve the agency one-shot LLM path.
        assert_eq!(
            prof.config
                .models
                .evaluator
                .as_ref()
                .and_then(|m| m.model.as_deref()),
            Some("claude:haiku")
        );
        assert_eq!(
            prof.config
                .models
                .assigner
                .as_ref()
                .and_then(|m| m.model.as_deref()),
            Some("claude:haiku")
        );
    }

    #[test]
    fn test_pi_starter_has_glm_workers_and_deepseek_agency_models() {
        // The pi starter pins worker/chat roles to Pi + OpenRouter GLM 5.2,
        // while the four agency one-shot roles (the "weak" tier) use a cheaper
        // native OpenRouter DeepSeek route. Per docs/design-two-tier-pi-profile.md
        // §4/§8, the premium roles (evolver/creator/verification) and the other
        // fast-tier roles (triage/placer/compactor/chat_compactor) no longer
        // carry an explicit pin — they ride their tier (premium=strong /
        // fast=weak) so `wg profile pi` can move them as a unit.
        let prof = parse_profile(STARTER_PI, Path::new("pi.toml"), "pi").unwrap();
        let worker = "pi:openrouter/z-ai/glm-5.2";
        let agency = "openrouter:deepseek/deepseek-chat";
        assert_eq!(prof.config.agent.model, worker);
        assert_eq!(prof.config.coordinator.model.as_deref(), Some(worker));
        assert_eq!(prof.config.tiers.fast.as_deref(), Some(agency));
        assert_eq!(prof.config.tiers.standard.as_deref(), Some(worker));
        assert_eq!(prof.config.tiers.premium.as_deref(), Some(worker));
        assert_eq!(
            prof.config
                .models
                .default
                .as_ref()
                .and_then(|m| m.model.as_deref()),
            Some(worker)
        );
        assert_eq!(
            prof.config
                .models
                .task_agent
                .as_ref()
                .and_then(|m| m.model.as_deref()),
            Some(worker)
        );

        // Only the four agency one-shots are explicitly pinned to DeepSeek
        // (they ignore the tier cascade today, so they must be explicit).
        let agency_oneshots = [
            prof.config.models.evaluator.as_ref(),
            prof.config.models.assigner.as_ref(),
            prof.config.models.flip_inference.as_ref(),
            prof.config.models.flip_comparison.as_ref(),
        ];
        for role in agency_oneshots {
            assert_eq!(
                role.and_then(|m| m.model.as_deref()),
                Some(agency),
                "the four pi agency one-shot roles must be pinned to DeepSeek (weak tier)"
            );
        }

        // The premium + remaining fast roles must NOT carry an explicit pin —
        // they ride their tier so the two-tier setter can move them as a unit.
        let tier_riding_roles = [
            prof.config.models.verification.as_ref(),
            prof.config.models.triage.as_ref(),
            prof.config.models.placer.as_ref(),
            prof.config.models.creator.as_ref(),
            prof.config.models.evolver.as_ref(),
            prof.config.models.compactor.as_ref(),
            prof.config.models.chat_compactor.as_ref(),
        ];
        for role in tier_riding_roles {
            assert!(
                role.is_none(),
                "premium/fast roles must ride their tier, not carry an explicit pin"
            );
        }
    }

    #[test]
    fn test_pi_starter_documents_plugin_placement() {
        // The pi.toml comment block must document plugin install/placement so a
        // user activating the profile knows the plugin must be present in the pi
        // process (task validation: "pi.toml documents the plugin
        // install/placement in a comment block").
        let tmpl = starter_template("pi").unwrap();
        assert!(
            tmpl.contains("~/.pi/agent/extensions/"),
            "pi.toml must document the global extensions dir (sidesteps the project trust gate)"
        );
        assert!(
            tmpl.contains(".pi/extensions") && tmpl.contains("project_trust"),
            "pi.toml must document the project extensions dir + its project_trust gate"
        );
        assert!(
            tmpl.contains("wg-pi-host.mjs"),
            "pi.toml must document the Topology-B Node-host bundle path"
        );
    }

    #[test]
    fn test_pi_in_starter_names() {
        assert!(
            STARTER_NAMES.contains(&"pi"),
            "STARTER_NAMES must include 'pi'; got {:?}",
            STARTER_NAMES
        );
        assert!(
            starter_template("pi").is_some(),
            "starter_template(\"pi\") must return the template"
        );
    }

    #[test]
    fn test_claude_starter_has_opus_default_worker_models() {
        let prof = parse_profile(STARTER_CLAUDE, Path::new("claude.toml"), "claude").unwrap();
        assert_eq!(prof.config.agent.model, "claude:opus");
        assert_eq!(
            prof.config.coordinator.model.as_deref(),
            Some("claude:opus")
        );
        assert_eq!(prof.config.tiers.fast.as_deref(), Some("claude:haiku"));
        assert_eq!(prof.config.tiers.standard.as_deref(), Some("claude:opus"));
        assert_eq!(prof.config.tiers.premium.as_deref(), Some("claude:opus"));
        assert_eq!(
            prof.config
                .models
                .default
                .as_ref()
                .and_then(|m| m.model.as_deref()),
            Some("claude:opus")
        );
        assert_eq!(
            prof.config
                .models
                .task_agent
                .as_ref()
                .and_then(|m| m.model.as_deref()),
            Some("claude:opus")
        );
        assert_eq!(
            prof.config
                .models
                .evaluator
                .as_ref()
                .and_then(|m| m.model.as_deref()),
            Some("claude:haiku")
        );
    }

    #[test]
    fn test_apply_profile_overlays_routing_keys_canonically() {
        let _tmp = with_home(|| {
            // Install the codex starter into the temp HOME's profiles dir.
            save_raw("codex", STARTER_CODEX).unwrap();
            let dst = apply_profile_as_global_config("codex").unwrap();
            assert!(dst.exists(), "global config must exist after apply");
            let written = std::fs::read_to_string(&dst).unwrap();
            // Sanity: ensure the actual [agent].model line in the written file
            // names codex, not claude. This is the verbatim check the task
            // validation calls for (`grep 'model = ' ~/.wg/config.toml`).
            assert!(
                written.contains("model = \"codex:gpt-5.6-sol\""),
                "global config must contain codex models, not claude. Got:\n{}",
                written,
            );
            assert!(
                !written.contains("claude:opus"),
                "global config must not retain any claude models after codex swap. Got:\n{}",
                written,
            );
            // Canonicalization: profile activation must NOT reintroduce
            // deprecated `dispatcher.poll_interval` or removed compaction/verify
            // keys (the regression: a `Config::save_global` round-trip re-emits
            // them with serde defaults).
            assert!(
                !written.contains("poll_interval"),
                "canonical global config must not carry deprecated poll_interval. Got:\n{}",
                written,
            );
            assert!(
                !written.contains("compactor_interval")
                    && !written.contains("compaction_token_threshold")
                    && !written.contains("verify_autospawn_enabled")
                    && !written.contains("verify_mode"),
                "canonical global config must not carry removed compaction/verify keys. Got:\n{}",
                written,
            );
            // Re-loading the written config must reflect codex everywhere.
            let cfg = Config::load_global()
                .unwrap()
                .expect("global must be present");
            assert_eq!(cfg.agent.model, "codex:gpt-5.6-sol");
            assert_eq!(cfg.coordinator.model.as_deref(), Some("codex:gpt-5.6-sol"));
            assert_eq!(cfg.tiers.fast.as_deref(), Some("codex:gpt-5.6-luna"));
        });
    }

    #[test]
    fn test_apply_profile_preserves_openrouter_endpoint() {
        // The bug: `wg profile use pi` overwrote `~/.wg/config.toml` byte-for-byte,
        // dropping a configured OpenRouter endpoint. With profile-as-overlay the
        // endpoint must survive a `pi` swap (the pi starter has no `llm_endpoints`).
        let _tmp = with_home(|| {
            save_raw("pi", STARTER_PI).unwrap();
            // Pre-seed the global config with an OpenRouter endpoint + the
            // deprecated keys `wg login openrouter --global`'s `save_global`
            // round-trip would write, so we also exercise canonicalization.
            let dst = Config::global_config_path().unwrap();
            std::fs::write(
                &dst,
                r#"
[agent]
model = "claude:opus"

[dispatcher]
model = "claude:opus"
poll_interval = 5
compaction_token_threshold = 50000
verify_autospawn_enabled = true

[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_ref = "keyring:openrouter"
is_default = true
"#,
            )
            .unwrap();
            apply_profile_as_global_config("pi").unwrap();

            let written = std::fs::read_to_string(&dst).unwrap();
            // The OpenRouter endpoint must survive the pi swap.
            assert!(
                written.contains("name = \"openrouter\""),
                "OpenRouter endpoint name must survive profile swap. Got:\n{}",
                written,
            );
            assert!(
                written.contains("api_key_ref = \"keyring:openrouter\""),
                "OpenRouter api_key_ref must survive profile swap. Got:\n{}",
                written,
            );
            assert!(
                written.contains("https://openrouter.ai/api/v1"),
                "OpenRouter endpoint URL must survive profile swap. Got:\n{}",
                written,
            );
            // The pi profile's routing keys must take effect.
            assert!(
                written.contains("pi:openrouter/z-ai/glm-5.2"),
                "pi strong-tier route must be present. Got:\n{}",
                written,
            );
            // Deprecated/removed keys must be canonicalized away.
            assert!(
                !written.contains("poll_interval"),
                "deprecated poll_interval must be renamed away. Got:\n{}",
                written,
            );
            assert!(
                !written.contains("verify_autospawn_enabled")
                    && !written.contains("compaction_token_threshold"),
                "removed compaction/verify keys must be dropped. Got:\n{}",
                written,
            );
            // The merged config must load and report the OpenRouter endpoint.
            let cfg = Config::load_global()
                .unwrap()
                .expect("global must be present");
            assert_eq!(cfg.agent.model, "pi:openrouter/z-ai/glm-5.2");
            let ep = cfg
                .llm_endpoints
                .find_by_name("openrouter")
                .expect("OpenRouter endpoint must survive profile swap");
            assert_eq!(ep.provider, "openrouter");
            assert_eq!(
                ep.api_key_ref.as_deref(),
                Some("keyring:openrouter"),
                "api_key_ref must survive profile swap",
            );
            assert!(ep.is_default, "endpoint must remain default");
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
            assert_eq!(cfg.agent.model, "codex:gpt-5.6-sol");
            assert_eq!(cfg.coordinator.model.as_deref(), Some("codex:gpt-5.6-sol"));
            assert_eq!(cfg.tiers.fast.as_deref(), Some("codex:gpt-5.6-luna"));
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
            assert_eq!(merged.agent.model, "codex:gpt-5.6-sol");
            assert_eq!(
                merged.coordinator.model.as_deref(),
                Some("codex:gpt-5.6-sol")
            );
            assert_eq!(merged.coordinator.effective_executor(), "codex");
            assert_eq!(merged.tiers.fast.as_deref(), Some("codex:gpt-5.6-luna"));
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

    // ── set_toml_string_value / patch_pi_tiers ───────────────────────────────

    #[test]
    fn test_set_toml_string_value_replaces_in_place() {
        let src = "[tiers]\nfast = \"old\"\nstandard = \"keep\"\n";
        let out = set_toml_string_value(src, "tiers.fast", "new:model");
        assert!(out.contains("fast = \"new:model\""));
        assert!(out.contains("standard = \"keep\""), "siblings preserved");
        assert!(!out.contains("\"old\""));
    }

    #[test]
    fn test_set_toml_string_value_preserves_comments_and_unrelated_keys() {
        let src = "# leading comment\ndescription = \"x\"\n\n[agent]\n# pick the worker model\nmodel = \"a\"\n";
        let out = set_toml_string_value(src, "agent.model", "b");
        assert!(out.contains("# leading comment"));
        assert!(out.contains("# pick the worker model"));
        assert!(out.contains("description = \"x\""));
        assert!(out.contains("model = \"b\""));
    }

    #[test]
    fn test_set_toml_string_value_inserts_missing_key_under_existing_table() {
        let src = "[tiers]\nstandard = \"s\"\n";
        let out = set_toml_string_value(src, "tiers.fast", "f");
        // Parses and both keys present.
        let val: toml::Value = out.parse().unwrap();
        let tiers = val.get("tiers").unwrap();
        assert_eq!(tiers.get("fast").unwrap().as_str(), Some("f"));
        assert_eq!(tiers.get("standard").unwrap().as_str(), Some("s"));
    }

    #[test]
    fn test_set_toml_string_value_appends_missing_table() {
        let src = "[agent]\nmodel = \"a\"\n";
        let out = set_toml_string_value(src, "models.evaluator.model", "weak:m");
        let val: toml::Value = out.parse().unwrap();
        assert_eq!(
            val.get("models")
                .and_then(|m| m.get("evaluator"))
                .and_then(|e| e.get("model"))
                .and_then(|m| m.as_str()),
            Some("weak:m")
        );
    }

    #[test]
    fn test_set_toml_string_value_ignores_array_of_tables_keys() {
        // A `model` key inside an [[llm_endpoints.endpoints]] element must not be
        // mistaken for the [agent].model target.
        let src =
            "[agent]\nmodel = \"a\"\n\n[[llm_endpoints.endpoints]]\nmodel = \"do-not-touch\"\n";
        let out = set_toml_string_value(src, "agent.model", "b");
        assert!(out.contains("model = \"do-not-touch\""));
        assert!(out.contains("model = \"b\""));
    }

    #[test]
    fn test_patch_pi_tiers_seeds_from_template_and_writes_both_tiers() {
        let _tmp = with_home(|| {
            // pi.toml absent → seeds from the baked-in starter, then patches.
            let path = patch_pi_tiers(
                "pi",
                Some("openrouter:z-ai/glm-5.2"),
                Some("openrouter:deepseek/deepseek-v3.1"),
            )
            .unwrap();
            let content = std::fs::read_to_string(&path).unwrap();
            // Comment block survives the write.
            assert!(content.contains("PLUGIN INSTALL"));
            // Parse and verify the full key-set via the reader. The strong tier
            // is normalized to a pi: route on write (so it runs through the
            // self-authenticating pi handler, not the in-process nex OpenRouter
            // client); the weak/agency tier keeps its native openrouter: route.
            let cfg: Config = toml::from_str(&content).unwrap();
            let (strong, weak) = cfg.pi_tiers();
            assert_eq!(strong.as_deref(), Some("pi:openrouter/z-ai/glm-5.2"));
            assert_eq!(weak.as_deref(), Some("openrouter:deepseek/deepseek-v3.1"));
            assert_eq!(
                cfg.tiers.premium.as_deref(),
                Some("pi:openrouter/z-ai/glm-5.2")
            );
            assert_eq!(
                cfg.models
                    .assigner
                    .as_ref()
                    .and_then(|m| m.model.as_deref()),
                Some("openrouter:deepseek/deepseek-v3.1")
            );
        });
    }

    #[test]
    fn test_patch_pi_tiers_partial_leaves_other_tier_intact() {
        let _tmp = with_home(|| {
            // Seed by setting both, then patch only weak; strong must persist.
            patch_pi_tiers("pi", Some("strong:v1"), Some("weak:v1")).unwrap();
            let path = patch_pi_tiers("pi", None, Some("weak:v2")).unwrap();
            let cfg: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let (strong, weak) = cfg.pi_tiers();
            assert_eq!(strong.as_deref(), Some("strong:v1"), "strong untouched");
            assert_eq!(weak.as_deref(), Some("weak:v2"));
        });
    }

    #[test]
    fn test_patch_role_model_overrides_task_agent_preserving_handler_first_pi_route() {
        // The motivating case for `wg profile set-model`: keep default on GLM
        // while routing task_agent through a different pi: model. The pi:
        // handler-first spec is written verbatim (no strong-tier normalization).
        let _tmp = with_home(|| {
            // pi.toml absent → seeds from the baked-in starter.
            let path = patch_role_model(
                "pi",
                "models.task_agent.model",
                "pi:openrouter/deepseek/deepseek-v4-flash",
            )
            .unwrap();
            let content = std::fs::read_to_string(&path).unwrap();
            // Comments survive.
            assert!(content.contains("PLUGIN INSTALL"));
            let cfg: Config = toml::from_str(&content).unwrap();
            // default stays on the starter GLM route (untouched).
            assert_eq!(
                cfg.models.default.as_ref().and_then(|m| m.model.as_deref()),
                Some("pi:openrouter/z-ai/glm-5.2")
            );
            // task_agent now carries the override verbatim, pi: route preserved.
            assert_eq!(
                cfg.models
                    .task_agent
                    .as_ref()
                    .and_then(|m| m.model.as_deref()),
                Some("pi:openrouter/deepseek/deepseek-v4-flash")
            );
            // The two-tier reader still reports the starter strong (agent.model
            // is untouched by a per-role override) — per-role wins at dispatch.
            let (strong, _weak) = cfg.pi_tiers();
            assert_eq!(strong.as_deref(), Some("pi:openrouter/z-ai/glm-5.2"));
        });
    }

    #[test]
    fn test_patch_role_model_writes_native_openrouter_route_verbatim() {
        // A weak-tier agency role override keeps its native openrouter: route
        // (no pi: normalization) — the loud keyless-native fallback stays armed.
        let _tmp = with_home(|| {
            let path = patch_role_model(
                "pi",
                "models.evaluator.model",
                "openrouter:deepseek/deepseek-chat",
            )
            .unwrap();
            let cfg: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(
                cfg.models
                    .evaluator
                    .as_ref()
                    .and_then(|m| m.model.as_deref()),
                Some("openrouter:deepseek/deepseek-chat")
            );
        });
    }

    #[test]
    fn test_patch_role_model_creates_missing_role_table() {
        // Setting a role whose [models.<role>] section is absent appends a new
        // table at EOF rather than corrupting an existing one.
        let _tmp = with_home(|| {
            // claude starter has no [models.triage]; patching it must add one.
            let path = patch_role_model("claude", "models.triage.model", "claude:haiku").unwrap();
            let cfg: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(
                cfg.models.triage.as_ref().and_then(|m| m.model.as_deref()),
                Some("claude:haiku")
            );
            // And the existing default is untouched.
            assert_eq!(
                cfg.models.default.as_ref().and_then(|m| m.model.as_deref()),
                Some("claude:opus")
            );
        });
    }

    // ── set_role_model_override (the `wg profile set-model` core) ──────────────

    #[test]
    fn test_set_role_model_override_writes_pi_task_agent_verbatim() {
        // The motivating case: default stays on GLM, task_agent moves to a
        // different pi: model — written to ~/.wg/profiles/pi.toml verbatim.
        let _tmp = with_home(|| {
            let out = set_role_model_override(
                "pi",
                "task_agent",
                "pi:openrouter/deepseek/deepseek-v4-flash",
                false,
            )
            .unwrap();
            assert_eq!(out.role, "task_agent");
            assert_eq!(out.dotted, "models.task_agent.model");
            assert_eq!(out.model, "pi:openrouter/deepseek/deepseek-v4-flash");
            assert!(!out.dry_run);
            assert!(out.wrote_path.is_some());

            let cfg: Config =
                toml::from_str(&std::fs::read_to_string(&out.wrote_path.unwrap()).unwrap())
                    .unwrap();
            assert_eq!(
                cfg.models.default.as_ref().and_then(|m| m.model.as_deref()),
                Some("pi:openrouter/z-ai/glm-5.2"),
                "default must stay on the starter GLM route"
            );
            assert_eq!(
                cfg.models
                    .task_agent
                    .as_ref()
                    .and_then(|m| m.model.as_deref()),
                Some("pi:openrouter/deepseek/deepseek-v4-flash"),
                "task_agent override written verbatim (pi: route preserved)"
            );
        });
    }

    #[test]
    fn test_set_role_model_override_dry_run_writes_nothing() {
        let _tmp = with_home(|| {
            let out = set_role_model_override(
                "pi",
                "task_agent",
                "pi:openrouter/deepseek/deepseek-v4-flash",
                true,
            )
            .unwrap();
            assert!(out.dry_run);
            assert!(out.wrote_path.is_none());
            let path = profile_path("pi").unwrap();
            assert!(
                !path.exists(),
                "dry run must not materialize the profile file"
            );
        });
    }

    #[test]
    fn test_set_role_model_override_rejects_invalid_role() {
        let _tmp = with_home(|| {
            let err =
                set_role_model_override("pi", "not_a_role", "claude:opus", false).unwrap_err();
            assert!(err.to_string().contains("Unknown role"));
        });
    }

    #[test]
    fn test_set_role_model_override_rejects_bare_model_name() {
        // A bare model name (no handler prefix) is rejected by the strict
        // parser — handler-first form is required.
        let _tmp = with_home(|| {
            let err = set_role_model_override("pi", "task_agent", "opus", false).unwrap_err();
            assert!(err.to_string().contains("Invalid model spec"));
        });
    }

    #[test]
    fn test_set_role_model_override_reapplies_when_active() {
        // When the edited profile is active, the override must land in the
        // materialized ~/.wg/config.toml too (so `wg config --models` shows it).
        let _tmp = with_home(|| {
            apply_profile_as_global_config("pi").unwrap();
            set_active(Some("pi")).unwrap();

            let out = set_role_model_override(
                "pi",
                "task_agent",
                "pi:openrouter/deepseek/deepseek-v4-flash",
                false,
            )
            .unwrap();
            assert!(out.is_active);
            assert!(
                out.reapplied_global,
                "active profile must re-apply global config"
            );

            let global = Config::global_config_path().unwrap();
            let cfg: Config = toml::from_str(&std::fs::read_to_string(&global).unwrap()).unwrap();
            assert_eq!(
                cfg.models
                    .task_agent
                    .as_ref()
                    .and_then(|m| m.model.as_deref()),
                Some("pi:openrouter/deepseek/deepseek-v4-flash"),
                "active profile override must be re-applied to the global config"
            );
            assert_eq!(
                cfg.models.default.as_ref().and_then(|m| m.model.as_deref()),
                Some("pi:openrouter/z-ai/glm-5.2"),
                "default stays on GLM in the global config"
            );
        });
    }

    #[test]
    fn test_set_role_model_override_does_not_reapply_when_inactive() {
        let _tmp = with_home(|| {
            // pi not active (no active-pointer file).
            let out = set_role_model_override(
                "pi",
                "task_agent",
                "pi:openrouter/deepseek/deepseek-v4-flash",
                false,
            )
            .unwrap();
            assert!(!out.is_active);
            assert!(!out.reapplied_global);
            // Global config must NOT exist (nothing re-applied).
            assert!(!Config::global_config_path().unwrap().exists());
        });
    }

    #[test]
    fn test_set_role_model_override_survives_profile_use_round_trip() {
        // Validation criterion: switching away (codex) and back (pi) preserves
        // the Pi profile override. The override lives in the profile FILE, so a
        // round-trip through `apply_profile_as_global_config` must restore it.
        let _tmp = with_home(|| {
            set_role_model_override(
                "pi",
                "task_agent",
                "pi:openrouter/deepseek/deepseek-v4-flash",
                false,
            )
            .unwrap();

            // Switch away to codex (materializes + applies codex starter).
            apply_profile_as_global_config("codex").unwrap();
            set_active(Some("codex")).unwrap();
            let codex_cfg: Config = toml::from_str(
                &std::fs::read_to_string(&Config::global_config_path().unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(
                codex_cfg
                    .models
                    .task_agent
                    .as_ref()
                    .and_then(|m| m.model.as_deref()),
                Some("codex:gpt-5.6-sol"),
                "codex is active — task_agent must be codex's"
            );

            // Switch back to pi — the override must survive in the file.
            apply_profile_as_global_config("pi").unwrap();
            set_active(Some("pi")).unwrap();
            let pi_cfg: Config = toml::from_str(
                &std::fs::read_to_string(&Config::global_config_path().unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(
                pi_cfg
                    .models
                    .task_agent
                    .as_ref()
                    .and_then(|m| m.model.as_deref()),
                Some("pi:openrouter/deepseek/deepseek-v4-flash"),
                "pi override must survive the codex round-trip"
            );
            assert_eq!(
                pi_cfg
                    .models
                    .default
                    .as_ref()
                    .and_then(|m| m.model.as_deref()),
                Some("pi:openrouter/z-ai/glm-5.2"),
                "default still on GLM after round-trip"
            );
        });
    }

    #[test]
    fn test_set_role_model_override_preserves_native_openrouter_route() {
        // A weak-tier agency role override keeps its native openrouter: route
        // (no pi: normalization) — the loud keyless-native fallback stays armed.
        let _tmp = with_home(|| {
            set_role_model_override(
                "pi",
                "evaluator",
                "openrouter:deepseek/deepseek-chat",
                false,
            )
            .unwrap();
            let cfg: Config =
                toml::from_str(&std::fs::read_to_string(&profile_path("pi").unwrap()).unwrap())
                    .unwrap();
            assert_eq!(
                cfg.models
                    .evaluator
                    .as_ref()
                    .and_then(|m| m.model.as_deref()),
                Some("openrouter:deepseek/deepseek-chat")
            );
        });
    }
}
