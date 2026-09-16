//! Read-only Pi catalog reader — WG's single source of truth for model
//! catalog data (design: `docs/design-retire-model-registry.md`).
//!
//! Pi maintains `models-store.json` inside its agent dir
//! (`PI_CODING_AGENT_DIR`, default `$HOME/.pi/agent`) — per-provider model
//! lists with pricing, context windows, and capability metadata, kept fresh
//! by Pi's own update machinery. WG performs **no network I/O** and requests
//! **no API keys** for catalog data, ever; it only reads the file Pi already
//! maintains.
//!
//! Contract (design §3 D2):
//! - `find` returns `None` for unknown provider/model — callers decide; the
//!   reader never errors, never guesses, never substitutes.
//! - Unknown vs free: a **negative or missing** `cost` field ⇒ `None`
//!   (unknown); an explicit `0` ⇒ `Some(0.0)` (genuinely free — `:free`
//!   variants are real).
//! - File missing / JSON unparseable / schema drift ⇒ empty catalog. The
//!   accounting path must never break or distort accounting; a missing
//!   catalog only removes one input. Interactive surfaces surface a single
//!   one-line warning themselves (see [`PiCatalog::is_empty`] callers).
//! - Parsing is defensive: unknown fields are ignored (Pi may add fields any
//!   release), and a per-entry failure skips the entry, not the catalog.
//! - Results are cached per process keyed on `(path, mtime, len)` — the same
//!   pattern as `executor_discovery::pi_registration_revision` — so hot loops
//!   (daemon token accounting) parse the file once per file version.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One model entry from Pi's catalog.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PiModel {
    /// Store section key (e.g. `"lunaroute"`, `"openrouter"`, `"openai-codex"`).
    pub provider: String,
    /// Model id inside the provider section (e.g. `"glm-5.3-flash"` or the
    /// OpenRouter-style `"z-ai/glm-5.2"`).
    pub id: String,
    /// USD per 1M input tokens. `None` = unknown (missing or negative
    /// sentinel); `Some(0.0)` = genuinely free.
    pub cost_input_per_mtok: Option<f64>,
    /// USD per 1M output tokens. Same unknown-vs-free contract.
    pub cost_output_per_mtok: Option<f64>,
    /// USD per 1M cache-read tokens. Same unknown-vs-free contract.
    pub cost_cache_read_per_mtok: Option<f64>,
    /// Context window in tokens, when the store carries one.
    pub context_window: Option<u64>,
}

impl PiModel {
    /// Cost fields are only trustworthy when they are present and
    /// non-negative; Pi uses negative sentinels (e.g. `-1000000`) for
    /// "unknown / router-decided" entries.
    fn cost_from_json(value: Option<&serde_json::Value>) -> Option<f64> {
        value
            .and_then(|v| v.as_f64())
            .filter(|cost| cost.is_finite() && *cost >= 0.0)
    }
}

/// The parsed Pi catalog: provider section key → that provider's models.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PiCatalog {
    providers: BTreeMap<String, Vec<PiModel>>,
}

impl PiCatalog {
    /// True when the catalog carries no entries (missing file, unparseable
    /// JSON, or schema drift). Interactive surfaces warn once on this.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Look up a model by provider section key and model id. Returns `None`
    /// for unknown provider/model — the reader never guesses.
    pub fn find(&self, provider: &str, model: &str) -> Option<&PiModel> {
        self.providers
            .get(provider)?
            .iter()
            .find(|entry| entry.id == model)
    }

    /// All provider section keys present in the catalog (sorted).
    pub fn provider_names(&self) -> impl Iterator<Item = &str> {
        self.providers.keys().map(String::as_str)
    }

    /// Every model entry across all providers (provider-grouped order).
    pub fn models(&self) -> impl Iterator<Item = &PiModel> {
        self.providers.values().flat_map(Vec::as_slice)
    }
}

/// Resolve the Pi agent dir: `PI_CODING_AGENT_DIR` → `$HOME/.pi/agent`
/// (same resolution as `executor_discovery::pi_registration_revision`).
/// `None` when neither is available.
pub fn agent_dir() -> Option<PathBuf> {
    std::env::var_os("PI_CODING_AGENT_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".pi/agent")))
}

/// Load the catalog from the resolved Pi agent dir. Missing dir ⇒ empty
/// catalog, never an error.
pub fn load() -> PiCatalog {
    agent_dir()
        .map(|dir| load_from_dir(&dir))
        .unwrap_or_default()
}

/// Parse `models-store.json` from `dir`. Missing file / unparseable JSON /
/// schema drift ⇒ empty catalog (treated as "no entries"), never an error —
/// a missing catalog must never break or distort accounting.
pub fn load_from_dir(dir: &Path) -> PiCatalog {
    let path = dir.join("models-store.json");
    cached_parse(&path)
}

/// Per-process cache keyed on `(path, mtime, len)` so hot loops parse the
/// file once per file version, not once per task.
fn cached_parse(path: &Path) -> PiCatalog {
    type CacheKey = (PathBuf, Option<std::time::SystemTime>, u64);
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<CacheKey, PiCatalog>>,
    > = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));

    let metadata = std::fs::metadata(path).ok();
    let key: CacheKey = (
        path.to_path_buf(),
        metadata.as_ref().and_then(|m| m.modified().ok()),
        metadata.as_ref().map(|m| m.len()).unwrap_or(0),
    );
    if let Ok(cache) = cache.lock()
        && let Some(hit) = cache.get(&key)
    {
        return hit.clone();
    }

    let catalog = std::fs::read(path)
        .map(|bytes| parse(&bytes))
        .unwrap_or_default();
    if let Ok(mut cache) = cache.lock() {
        cache.insert(key, catalog.clone());
    }
    catalog
}

/// Parse the raw `models-store.json` bytes. Defensive by construction:
/// unknown fields ignored, per-entry failure skips the entry, whole-file
/// failure ⇒ empty catalog.
fn parse(bytes: &[u8]) -> PiCatalog {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return PiCatalog::default();
    };
    let Some(sections) = value.as_object() else {
        return PiCatalog::default();
    };

    let mut providers = BTreeMap::new();
    for (provider, section) in sections {
        let Some(models) = section.get("models").and_then(|m| m.as_array()) else {
            continue;
        };
        let mut entries = Vec::new();
        for model in models {
            // Skip non-object entries rather than failing the whole catalog.
            let Some(obj) = model.as_object() else {
                continue;
            };
            let Some(id) = obj.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let cost = obj.get("cost");
            entries.push(PiModel {
                provider: provider.clone(),
                id: id.to_string(),
                cost_input_per_mtok: PiModel::cost_from_json(cost.and_then(|c| c.get("input"))),
                cost_output_per_mtok: PiModel::cost_from_json(cost.and_then(|c| c.get("output"))),
                cost_cache_read_per_mtok: PiModel::cost_from_json(
                    cost.and_then(|c| c.get("cacheRead").or_else(|| c.get("cache_read"))),
                ),
                context_window: obj
                    .get("contextWindow")
                    .and_then(|v| v.as_u64())
                    .or_else(|| obj.get("context_window").and_then(|v| v.as_u64())),
            });
        }
        providers.insert(provider.clone(), entries);
    }

    PiCatalog { providers }
}

/// Route → `(provider, model)` for handler-first `pi:` specs:
///
/// - `pi:lunaroute/glm-5.3-flash` → `("lunaroute", "glm-5.3-flash")`
/// - `pi:openrouter:z-ai/glm-5.2` → `("openrouter", "z-ai/glm-5.2")`
/// - `pi:zai/glm-5.2`             → `("zai", "glm-5.2")`
///
/// Non-`pi:` specs (`claude:` / `codex:` / `nex:` / bare ids) ⇒ `None`
/// (callers keep their own path).
pub fn split_pi_spec(route: &str) -> Option<(String, String)> {
    let rest = route.strip_prefix("pi:")?;
    if rest.is_empty() {
        return None;
    }
    // A wire-provider prefix inside the pi spec (`pi:openrouter:z-ai/glm-5.2`)
    // names the store section directly; a bare `provider/model` path splits on
    // the slash. Anything else has no determinable provider section.
    if let Some((provider, model)) = rest.split_once(':') {
        if provider.is_empty() || model.is_empty() {
            return None;
        }
        return Some((provider.to_string(), model.to_string()));
    }
    let (provider, model) = rest.split_once('/')?;
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    Some((provider.to_string(), model.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
  "lunaroute": {
    "checkedAt": 1789590888281,
    "models": [
      {
        "id": "glm-x",
        "name": "GLM X",
        "api": "openai-completions",
        "provider": "lunaroute",
        "baseUrl": "https://gw.example/v1",
        "reasoning": true,
        "input": ["text"],
        "cost": { "input": 0.2, "output": 1.1, "cacheRead": 0.02, "cacheWrite": 0 },
        "contextWindow": 200000,
        "maxTokens": 32768,
        "futureFieldPiMayAdd": { "anything": true }
      },
      {
        "id": "free-tier",
        "cost": { "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0 },
        "contextWindow": 8192
      },
      {
        "id": "router-auto",
        "cost": { "input": -1000000, "output": -1000000, "cacheRead": -1000000 }
      },
      {
        "id": "no-cost-field"
      }
    ]
  },
  "openrouter": {
    "models": [
      { "id": "z-ai/glm-5.2", "cost": { "input": 0.09, "output": 0.3, "cacheRead": 0.015 } }
    ]
  },
  "broken-section": { "nope": true }
}"#;

    fn fixture_dir() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        std::fs::write(dir.join("models-store.json"), FIXTURE).unwrap();
        (tmp, dir)
    }

    #[test]
    fn find_returns_catalog_entries_with_rates() {
        let (_tmp, dir) = fixture_dir();
        let catalog = load_from_dir(&dir);
        let entry = catalog.find("lunaroute", "glm-x").unwrap();
        assert_eq!(entry.provider, "lunaroute");
        assert_eq!(entry.id, "glm-x");
        assert_eq!(entry.cost_input_per_mtok, Some(0.2));
        assert_eq!(entry.cost_output_per_mtok, Some(1.1));
        assert_eq!(entry.cost_cache_read_per_mtok, Some(0.02));
        assert_eq!(entry.context_window, Some(200000));
    }

    #[test]
    fn find_returns_none_for_unknown_provider_or_model() {
        let (_tmp, dir) = fixture_dir();
        let catalog = load_from_dir(&dir);
        assert!(catalog.find("no-such-provider", "glm-x").is_none());
        assert!(catalog.find("lunaroute", "no-such-model").is_none());
    }

    #[test]
    fn explicit_zero_is_free_but_negative_sentinel_is_unknown() {
        let (_tmp, dir) = fixture_dir();
        let catalog = load_from_dir(&dir);

        let free = catalog.find("lunaroute", "free-tier").unwrap();
        assert_eq!(free.cost_input_per_mtok, Some(0.0));
        assert_eq!(free.cost_output_per_mtok, Some(0.0));

        let sentinel = catalog.find("lunaroute", "router-auto").unwrap();
        assert_eq!(sentinel.cost_input_per_mtok, None);
        assert_eq!(sentinel.cost_output_per_mtok, None);
        assert_eq!(sentinel.cost_cache_read_per_mtok, None);

        let absent = catalog.find("lunaroute", "no-cost-field").unwrap();
        assert_eq!(absent.cost_input_per_mtok, None);
        assert_eq!(absent.context_window, None);
    }

    #[test]
    fn per_entry_failure_skips_entry_not_catalog() {
        let (_tmp, dir) = fixture_dir();
        let catalog = load_from_dir(&dir);
        // The malformed "broken-section" section and any non-object entries
        // are skipped; every other section still parses.
        assert!(catalog.find("broken-section", "x").is_none());
        assert!(catalog.find("lunaroute", "glm-x").is_some());
        assert!(catalog.find("openrouter", "z-ai/glm-5.2").is_some());
    }

    #[test]
    fn missing_file_is_an_empty_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let catalog = load_from_dir(tmp.path());
        assert!(catalog.is_empty());
    }

    #[test]
    fn unparseable_json_is_an_empty_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("models-store.json"), "{not json").unwrap();
        assert!(load_from_dir(tmp.path()).is_empty());
    }

    #[test]
    fn schema_drift_top_level_array_is_an_empty_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("models-store.json"), "[1,2,3]").unwrap();
        assert!(load_from_dir(tmp.path()).is_empty());
    }

    #[test]
    fn split_pi_spec_handler_first_forms() {
        assert_eq!(
            split_pi_spec("pi:lunaroute/glm-5.3-flash"),
            Some(("lunaroute".to_string(), "glm-5.3-flash".to_string()))
        );
        assert_eq!(
            split_pi_spec("pi:openrouter:z-ai/glm-5.2"),
            Some(("openrouter".to_string(), "z-ai/glm-5.2".to_string()))
        );
        assert_eq!(
            split_pi_spec("pi:zai/glm-5.2"),
            Some(("zai".to_string(), "glm-5.2".to_string()))
        );
    }

    #[test]
    fn split_pi_spec_rejects_non_pi_and_ambiguous_specs() {
        assert_eq!(split_pi_spec("claude:opus"), None);
        assert_eq!(split_pi_spec("codex:gpt-5.5"), None);
        assert_eq!(split_pi_spec("nex:openrouter:z-ai/glm-5.2"), None);
        assert_eq!(split_pi_spec("pi:"), None);
        // No provider section determinable.
        assert_eq!(split_pi_spec("pi:bare-alias"), None);
        assert_eq!(split_pi_spec("not-pi:lunaroute/x"), None);
    }

    #[test]
    fn agent_dir_prefers_env_over_home() {
        // The resolution mirrors executor_discovery: PI_CODING_AGENT_DIR wins,
        // HOME/.pi/agent is the fallback, neither ⇒ None.
        let _env_guard = crate::test_helpers::env_lock();
        let saved = std::env::var_os("PI_CODING_AGENT_DIR");
        unsafe {
            std::env::set_var("PI_CODING_AGENT_DIR", "/tmp/pi-catalog-test-agent");
            assert_eq!(
                agent_dir(),
                Some(PathBuf::from("/tmp/pi-catalog-test-agent"))
            );
            std::env::remove_var("PI_CODING_AGENT_DIR");
            // HOME should exist in any test environment.
            if std::env::var_os("HOME").is_some() {
                assert!(agent_dir().is_some());
            }
        }
        match saved {
            Some(v) => unsafe { std::env::set_var("PI_CODING_AGENT_DIR", v) },
            None => unsafe { std::env::remove_var("PI_CODING_AGENT_DIR") },
        }
    }
}
