//! Config canonicalization predicates shared by `wg migrate config` (the
//! wg-binary `commands::migrate` module) and profile activation
//! (`profile::named::apply_profile_as_global_config`).
//!
//! These transforms operate on a parsed `toml::Value` in place — they do not
//! touch the filesystem. `wg migrate config` wraps them with file I/O + a
//! backup; profile activation runs them on the merged profile + global-config
//! tree before writing `~/.wg/config.toml`, so a `wg profile use <name>`
//! produces a lint-clean config without round-tripping through
//! `Config::save_global` (which re-emits deprecated field names like
//! `dispatcher.poll_interval` and removed compaction/verify keys with their
//! serde defaults — the root cause of the profile-clobbering regression).
//!
//! Single source of truth: the wg-binary `commands::migrate` module re-exports
//! these so its `migrate_one` / lint helpers and the lib-side profile applier
//! can never drift.

/// Aggregate report from [`canonicalize_in_place`] — the keys dropped, the
/// legacy keys renamed, and the stale model strings rewritten. Mirrors the
/// per-predicate `Vec`s `migrate_one` collects, so callers can surface the
/// same "what would change?" summary without re-running the predicates.
pub struct CanonicalizeReport {
    /// Deprecated/no-op keys removed (e.g. `dispatcher.verify_mode`).
    pub removed: Vec<String>,
    /// Legacy key → canonical key renames (e.g. `dispatcher.poll_interval`
    /// → `dispatcher.safety_interval`).
    pub renamed: Vec<(String, String)>,
    /// Stale model-string rewrites (path, old, new).
    pub rewritten: Vec<(String, String, String)>,
}

/// Run the canonicalization pipeline in place on a parsed TOML document:
/// drop deprecated/no-op keys, rename legacy field names, fix stale model
/// strings, and strip the orphaned `[openrouter]` section when nothing uses
/// OpenRouter. Does NOT read or write any file — this is the pure transform
/// shared by `wg migrate config` (file-backed) and profile activation
/// (in-memory before writing `~/.wg/config.toml`).
pub fn canonicalize_in_place(doc: &mut toml::Value) -> CanonicalizeReport {
    let mut removed = Vec::new();
    let mut renamed = Vec::new();
    let mut rewritten = Vec::new();
    drop_deprecated(doc, &mut removed);
    rename_legacy_fields(doc, &mut renamed);
    fix_stale_model_strings(doc, &mut rewritten);
    drop_orphaned_openrouter(doc, &mut removed);
    CanonicalizeReport {
        removed,
        renamed,
        rewritten,
    }
}

/// Top-level `[section].key` pairs that the migration removes outright.
/// These are deprecated/no-op as of the audit and are never written by
/// the canonical defaults or by `wg config init`.
const DEPRECATED_KEYS: &[(&str, &str)] = &[
    // Handler is now derived from model spec's provider prefix.
    ("agent", "executor"),
    ("dispatcher", "executor"),
    ("coordinator", "executor"),
    // Compactor (.compact-N) cycle was retired.
    ("dispatcher", "compactor_interval"),
    ("dispatcher", "compactor_ops_threshold"),
    ("dispatcher", "compaction_token_threshold"),
    ("dispatcher", "compaction_threshold_ratio"),
    ("coordinator", "compactor_interval"),
    ("coordinator", "compactor_ops_threshold"),
    ("coordinator", "compaction_token_threshold"),
    ("coordinator", "compaction_threshold_ratio"),
    // Verify-shadow-task auto-spawn was replaced by .evaluate-* + wg rescue.
    ("dispatcher", "verify_autospawn_enabled"),
    ("coordinator", "verify_autospawn_enabled"),
    // Legacy verify_mode predates the ## Validation pattern.
    ("dispatcher", "verify_mode"),
    ("coordinator", "verify_mode"),
    // Old FLIP threshold knob — replaced by per-agent eval thresholds.
    ("agency", "flip_verification_threshold"),
];

pub(crate) fn drop_deprecated(doc: &mut toml::Value, removed: &mut Vec<String>) {
    let table = match doc.as_table_mut() {
        Some(t) => t,
        None => return,
    };
    for (section, key) in DEPRECATED_KEYS {
        if let Some(toml::Value::Table(sec)) = table.get_mut(*section)
            && sec.remove(*key).is_some()
        {
            removed.push(format!("{}.{}", section, key));
        }
        // Also drop empty sections we just emptied.
        if let Some(toml::Value::Table(sec)) = table.get(*section)
            && sec.is_empty()
        {
            table.remove(*section);
            removed.push(format!("{} (empty section)", section));
        }
    }
}

/// Remove the `[openrouter]` section when nothing in the config actually
/// uses OpenRouter. "Uses" means: a top-level model spec / tier / endpoint
/// references the `openrouter:` provider prefix. If anything points at
/// openrouter we leave the section alone — the cost-cap settings inside
/// might be intentional.
///
/// This catches the common case where an old `wg init` (before the
/// fix-remove-openrouter change) wrote a default `[openrouter]` block
/// into a claude-cli or codex-cli project. The default block has no
/// API key, so the daemon's registry-refresh job spins on auth errors.
pub(crate) fn drop_orphaned_openrouter(doc: &mut toml::Value, removed: &mut Vec<String>) {
    let table = match doc.as_table_mut() {
        Some(t) => t,
        None => return,
    };
    if !table.contains_key("openrouter") {
        return;
    }

    // Scan for any string in the doc that mentions "openrouter" — model
    // specs, tier values, endpoint provider/url, etc. The check has to
    // skip the [openrouter] section itself, otherwise a default section
    // would always look "in use".
    let mut uses_openrouter = false;
    for (k, v) in table.iter() {
        if k == "openrouter" {
            continue;
        }
        if value_mentions_openrouter(v) {
            uses_openrouter = true;
            break;
        }
    }
    if uses_openrouter {
        return;
    }

    table.remove("openrouter");
    removed.push("openrouter (orphaned section — no openrouter usage in config)".to_string());
}

/// Recursive predicate: returns true if any string-leaf inside `v`
/// mentions "openrouter" — model specs, endpoint provider names, URLs,
/// etc. Used by [`drop_orphaned_openrouter`] to decide whether the
/// section is still load-bearing.
fn value_mentions_openrouter(v: &toml::Value) -> bool {
    match v {
        toml::Value::String(s) => s.contains("openrouter"),
        toml::Value::Array(arr) => arr.iter().any(value_mentions_openrouter),
        toml::Value::Table(t) => t.values().any(value_mentions_openrouter),
        _ => false,
    }
}

pub(crate) fn rename_legacy_fields(doc: &mut toml::Value, renamed: &mut Vec<(String, String)>) {
    let table = match doc.as_table_mut() {
        Some(t) => t,
        None => return,
    };
    // Rename top-level [coordinator] section to [dispatcher] when no
    // [dispatcher] section already exists. If both exist, leave them
    // alone — the user has manually split them and we don't want to
    // silently merge.
    if table.contains_key("coordinator") && !table.contains_key("dispatcher") {
        if let Some(v) = table.remove("coordinator") {
            table.insert("dispatcher".to_string(), v);
            renamed.push(("[coordinator]".to_string(), "[dispatcher]".to_string()));
        }
    }

    // Within [dispatcher], rename chat_agent → coordinator_agent + max_chats → max_coordinators.
    if let Some(toml::Value::Table(disp)) = table.get_mut("dispatcher") {
        for (old, new) in &[
            ("chat_agent", "coordinator_agent"),
            ("max_chats", "max_coordinators"),
        ] {
            if disp.contains_key(*old) && !disp.contains_key(*new) {
                if let Some(v) = disp.remove(*old) {
                    disp.insert(new.to_string(), v);
                    renamed.push((format!("dispatcher.{}", old), format!("dispatcher.{}", new)));
                }
            }
        }
    }

    // `poll_interval` remains accepted by config deserialization for one
    // release, but `safety_interval` is the canonical key. Keep migration and
    // lint in sync with the daemon's startup deprecation scan.
    for section in ["dispatcher", "coordinator"] {
        if let Some(toml::Value::Table(sec)) = table.get_mut(section)
            && let Some(v) = sec.remove("poll_interval")
        {
            if !sec.contains_key("safety_interval") {
                sec.insert("safety_interval".to_string(), v);
            }
            renamed.push((
                format!("{}.poll_interval", section),
                format!("{}.safety_interval", section),
            ));
        }
    }
}

/// Stale model string rewrites: maps `<old>` → `<new>` substrings inside
/// any string field anywhere in the config. Conservative — only matches
/// exact full strings, not arbitrary substrings, to avoid surprising
/// rewrites of unrelated values.
const STALE_MODEL_REWRITES: &[(&str, &str)] = &[
    (
        "openrouter:anthropic/claude-sonnet-4",
        "openrouter:anthropic/claude-sonnet-4-6",
    ),
    (
        "openrouter:anthropic/claude-haiku-4",
        "openrouter:anthropic/claude-haiku-4-5",
    ),
    (
        "openrouter:anthropic/claude-opus-4",
        "openrouter:anthropic/claude-opus-4-7",
    ),
    ("anthropic/claude-sonnet-4", "anthropic/claude-sonnet-4-6"),
    ("anthropic/claude-haiku-4", "anthropic/claude-haiku-4-5"),
    ("anthropic/claude-opus-4", "anthropic/claude-opus-4-7"),
    // Codex / OpenAI model rewrites (2026-04-28):
    // o1-pro deprecated 2026-10-23; gpt-5.4 remains the balanced catalog entry.
    ("codex:o1-pro", "codex:gpt-5.4"),
    // Old tier names predating the gpt-5.4 generation.
    ("codex:gpt-5-mini", "codex:gpt-5.4-mini"),
    ("codex:gpt-5", "codex:gpt-5.4"),
    // gpt-5-codex sunsets 2026-07-23; gpt-5.4 is the direct replacement.
    ("codex:gpt-5-codex", "codex:gpt-5.4"),
    // gpt-5.4-pro superseded by gpt-5.5 (newer, cheaper at $5/$30 vs $30/$180).
    ("codex:gpt-5.4-pro", "codex:gpt-5.5"),
];

pub(crate) fn fix_stale_model_strings(
    doc: &mut toml::Value,
    rewritten: &mut Vec<(String, String, String)>,
) {
    walk_strings(doc, "", &mut |path, s| {
        if let Some(new_str) = rewrite_stale_default_route_pin(path, s) {
            rewritten.push((path.to_string(), s.clone(), new_str.clone()));
            return Some(new_str);
        }
        for (old, new) in STALE_MODEL_REWRITES {
            // Match exact full string only (not substring) so e.g.
            // `claude-sonnet-4` doesn't fire when the value is already
            // `claude-sonnet-4-6`. The `-4` suffix is a prefix of `-4-6`,
            // so a naive substring match would loop.
            if s == *old {
                let new_str = (*new).to_string();
                rewritten.push((path.to_string(), s.clone(), new_str.clone()));
                return Some(new_str);
            }
        }
        // Handler-first rewrite for deprecated **leading** provider prefixes
        // (docs/design-handler-first-model-spec.md §5.2): the leading token
        // must name a handler, never a bare provider. Two rewrite shapes:
        //   - swap (pure aliases of the nex wire): `oai-compat:X`/`openai:X`/
        //     `local:X`/`native:X` → `nex:X` (drop the prefix);
        //   - prepend (wire-distinct providers): `openrouter:X`/`ollama:X`/
        //     `vllm:X`/`llamacpp:X`/`gemini:X` → `nex:<prefix>:X` (keep it as
        //     the inner dialect).
        // The `local:`/`oai-compat:` swap behavior is unchanged (it is now a
        // subset of `handler_first_rewrite`). Already handler-first specs
        // (`claude:`, `codex:`, `nex:`, `pi:…`) and bare aliases are no-ops.
        if let Some(new_str) = crate::config::handler_first_rewrite(s) {
            rewritten.push((path.to_string(), s.clone(), new_str.clone()));
            return Some(new_str);
        }
        None
    });
}

/// Upgrade stale default/task-agent pins to the current top worker defaults.
///
/// This is intentionally path-scoped: lower-cost models can still appear in
/// fast tiers, registries, or explicit role overrides. The regression was stale
/// default worker routing, not the existence of cheaper catalog entries.
fn rewrite_stale_default_route_pin(path: &str, value: &str) -> Option<String> {
    const DEFAULT_ROUTE_PATHS: &[&str] = &[
        "agent.model",
        "dispatcher.model",
        "coordinator.model",
        "models.default.model",
        "models.task_agent.model",
        "tiers.standard",
        "tiers.premium",
    ];
    if !DEFAULT_ROUTE_PATHS.contains(&path) {
        return None;
    }

    match value {
        "codex:gpt-5.4" | "gpt-5.4" | "codex:gpt-5" | "gpt-5" | "codex:o1-pro" | "o1-pro"
        | "codex:gpt-5-codex" | "gpt-5-codex" => Some("codex:gpt-5.5".to_string()),
        "claude:sonnet" | "sonnet" => Some("claude:opus".to_string()),
        _ => None,
    }
}

/// Walk every string value in a TOML doc, calling `f(path, &value)`.
/// If `f` returns `Some(new)`, replace the value with `new`. The path
/// uses dotted notation: `"agent.model"`, `"tiers.standard"`, etc.
fn walk_strings(
    val: &mut toml::Value,
    path: &str,
    f: &mut dyn FnMut(&str, &String) -> Option<String>,
) {
    match val {
        toml::Value::String(s) => {
            if let Some(new) = f(path, s) {
                *s = new;
            }
        }
        toml::Value::Array(arr) => {
            for (i, child) in arr.iter_mut().enumerate() {
                let child_path = format!("{}[{}]", path, i);
                walk_strings(child, &child_path, f);
            }
        }
        toml::Value::Table(tbl) => {
            for (k, child) in tbl.iter_mut() {
                let child_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", path, k)
                };
                walk_strings(child, &child_path, f);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> toml::Value {
        s.parse().expect("valid TOML")
    }

    #[test]
    fn canonicalize_drops_deprecated_dispatcher_keys() {
        let mut doc = parse(
            r#"
[dispatcher]
poll_interval = 5
compactor_interval = 30
verify_autospawn_enabled = true
verify_mode = "separate"
compaction_token_threshold = 50000

[agent]
executor = "claude"
model = "claude:opus"
"#,
        );
        let report = canonicalize_in_place(&mut doc);
        let body = toml::to_string_pretty(&doc).unwrap();
        assert!(
            !body.contains("poll_interval"),
            "poll_interval must be renamed: {body}"
        );
        assert!(
            body.contains("safety_interval"),
            "safety_interval must be present: {body}"
        );
        assert!(
            !body.contains("compactor_interval"),
            "compactor_interval must be dropped: {body}"
        );
        assert!(
            !body.contains("verify_autospawn_enabled"),
            "verify_autospawn_enabled must be dropped: {body}",
        );
        assert!(
            !body.contains("verify_mode"),
            "verify_mode must be dropped: {body}"
        );
        assert!(
            !body.contains("compaction_token_threshold"),
            "compaction_token_threshold must be dropped: {body}",
        );
        assert!(
            !body.contains("executor"),
            "agent.executor must be dropped: {body}"
        );
        assert!(
            body.contains("claude:opus"),
            "agent.model must be preserved: {body}"
        );
        assert!(!report.removed.is_empty());
        assert!(!report.renamed.is_empty());
    }

    #[test]
    fn canonicalize_preserves_openrouter_endpoint_section() {
        let mut doc = parse(
            r#"
[agent]
model = "pi:openrouter/z-ai/glm-5.2"

[dispatcher]
model = "pi:openrouter/z-ai/glm-5.2"

[llm_endpoints]
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
url = "https://openrouter.ai/api/v1"
api_key_ref = "keyring:openrouter"
is_default = true
"#,
        );
        canonicalize_in_place(&mut doc);
        let body = toml::to_string_pretty(&doc).unwrap();
        assert!(
            body.contains("api_key_ref = \"keyring:openrouter\""),
            "OpenRouter endpoint credential must survive canonicalization: {body}",
        );
        assert!(
            body.contains("https://openrouter.ai/api/v1"),
            "OpenRouter endpoint URL must survive canonicalization: {body}",
        );
    }
}
