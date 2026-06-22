use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NexRuntimeMode {
    Standalone,
    WgIntegrated,
    WgAutonomous,
    Eval,
    LegacyWgCompat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NexSessionLayout {
    StandaloneSessions,
    WgChat,
}

#[derive(Debug, Clone)]
pub struct NexRuntime {
    pub mode: NexRuntimeMode,
    pub state_root: PathBuf,
    pub session_root: PathBuf,
    pub cache_root: PathBuf,
    pub session_layout: NexSessionLayout,
    pub config_paths: Vec<PathBuf>,
    pub model_registry_paths: Vec<PathBuf>,
    pub legacy_session_roots: Vec<PathBuf>,
    pub wg_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct NexRuntimeResolveInput {
    pub cwd: Option<PathBuf>,
    pub home_dir: Option<PathBuf>,
    pub cli_nex_dir: Option<PathBuf>,
    pub env_nex_dir: Option<PathBuf>,
    pub env_nex_home: Option<PathBuf>,
    pub explicit_config: Option<PathBuf>,
}

pub fn resolve_standalone(input: &NexRuntimeResolveInput) -> NexRuntime {
    let cwd = input.cwd.clone().unwrap_or_else(|| PathBuf::from("."));
    let home = input.home_dir.clone();
    let user_nex_home = input
        .env_nex_home
        .clone()
        .or_else(|| home.as_ref().map(|h| h.join(".nex")))
        .unwrap_or_else(|| PathBuf::from(".nex"));
    let project_nex = input
        .cli_nex_dir
        .clone()
        .or_else(|| input.env_nex_dir.clone())
        .or_else(|| nearest_dir_named(&cwd, ".nex"));
    let state_root = project_nex.clone().unwrap_or_else(|| user_nex_home.clone());

    let project_wg = nearest_wg_dir(&cwd);
    let mut config_paths = Vec::new();
    if let Some(home) = &home {
        config_paths.push(home.join(".wg").join("config.toml"));
    }
    if let Some(wg) = &project_wg {
        config_paths.push(wg.join("config.toml"));
    }
    config_paths.push(user_nex_home.join("config.toml"));
    if let Some(project_nex) = &project_nex {
        config_paths.push(project_nex.join("config.toml"));
    }
    if let Some(explicit) = &input.explicit_config {
        config_paths.push(explicit.clone());
    }

    let mut model_registry_paths = Vec::new();
    if let Some(home) = &home {
        model_registry_paths.push(home.join(".wg").join("models.yaml"));
    }
    if let Some(wg) = &project_wg {
        model_registry_paths.push(wg.join("models.yaml"));
    }
    model_registry_paths.push(user_nex_home.join("models.yaml"));
    if let Some(project_nex) = &project_nex {
        model_registry_paths.push(project_nex.join("models.yaml"));
    }

    let mut legacy_session_roots = Vec::new();
    if let Some(wg) = &project_wg {
        legacy_session_roots.push(wg.join("chat"));
    }
    if let Some(home) = &home {
        legacy_session_roots.push(home.join(".wg").join("chat"));
    }

    NexRuntime {
        mode: NexRuntimeMode::Standalone,
        session_root: state_root.join("sessions"),
        cache_root: state_root.join("cache"),
        state_root,
        session_layout: NexSessionLayout::StandaloneSessions,
        config_paths,
        model_registry_paths,
        legacy_session_roots,
        wg_dir: None,
    }
}

pub fn resolve_wg_integrated(workgraph_dir: impl Into<PathBuf>) -> NexRuntime {
    resolve_wg_runtime(
        workgraph_dir.into(),
        dirs::home_dir(),
        NexRuntimeMode::WgIntegrated,
    )
}

pub fn resolve_wg_integrated_with_home(
    workgraph_dir: impl Into<PathBuf>,
    home_dir: Option<PathBuf>,
) -> NexRuntime {
    resolve_wg_runtime(workgraph_dir.into(), home_dir, NexRuntimeMode::WgIntegrated)
}

pub fn resolve_legacy_wg_compat(
    workgraph_dir: impl Into<PathBuf>,
    home_dir: Option<PathBuf>,
) -> NexRuntime {
    resolve_wg_runtime(
        workgraph_dir.into(),
        home_dir,
        NexRuntimeMode::LegacyWgCompat,
    )
}

pub fn resolve_eval(input: &NexRuntimeResolveInput) -> NexRuntime {
    let cwd = input.cwd.clone().unwrap_or_else(|| PathBuf::from("."));
    let state_root = input
        .cli_nex_dir
        .clone()
        .or_else(|| input.env_nex_dir.clone())
        .unwrap_or_else(|| cwd.join(".nex-eval"));
    let mut config_paths = Vec::new();
    if let Some(explicit) = &input.explicit_config {
        config_paths.push(explicit.clone());
    } else {
        config_paths.push(state_root.join("config.toml"));
    }
    NexRuntime {
        mode: NexRuntimeMode::Eval,
        session_root: state_root.join("sessions"),
        cache_root: state_root.join("cache"),
        state_root,
        session_layout: NexSessionLayout::StandaloneSessions,
        config_paths,
        model_registry_paths: Vec::new(),
        legacy_session_roots: Vec::new(),
        wg_dir: None,
    }
}

fn resolve_wg_runtime(
    state_root: PathBuf,
    home_dir: Option<PathBuf>,
    mode: NexRuntimeMode,
) -> NexRuntime {
    let nex_overlay = state_root.join("nex").join("config.toml");
    let include_overlay = mode != NexRuntimeMode::WgAutonomous
        || nex_overlay_apply_to_autonomous(&nex_overlay).unwrap_or(false);
    let include_standalone = mode == NexRuntimeMode::WgIntegrated
        && nex_overlay_inherits_standalone(&nex_overlay).unwrap_or(false);

    let mut config_paths = Vec::new();
    let mut model_registry_paths = Vec::new();
    if include_standalone && let Some(home) = &home_dir {
        config_paths.push(home.join(".nex").join("config.toml"));
        model_registry_paths.push(home.join(".nex").join("models.yaml"));
    }
    if let Some(home) = &home_dir {
        config_paths.push(home.join(".wg").join("config.toml"));
        model_registry_paths.push(home.join(".wg").join("models.yaml"));
    }
    config_paths.push(state_root.join("config.toml"));
    model_registry_paths.push(state_root.join("models.yaml"));
    if include_overlay {
        config_paths.push(nex_overlay);
        model_registry_paths.push(state_root.join("nex").join("models.yaml"));
    }

    NexRuntime {
        mode,
        session_root: state_root.join("chat"),
        cache_root: state_root.join("nex").join("cache"),
        state_root: state_root.clone(),
        session_layout: NexSessionLayout::WgChat,
        config_paths,
        model_registry_paths,
        legacy_session_roots: Vec::new(),
        wg_dir: Some(state_root),
    }
}

pub fn resolve_wg_autonomous(
    workgraph_dir: impl Into<PathBuf>,
    home_dir: Option<PathBuf>,
) -> NexRuntime {
    resolve_wg_runtime(workgraph_dir.into(), home_dir, NexRuntimeMode::WgAutonomous)
}

pub fn load_config(runtime: &NexRuntime) -> Result<crate::config::Config> {
    let merged = load_toml_value(runtime)?;
    let mut config: crate::config::Config = merged
        .try_into()
        .context("Failed to deserialize nex runtime config")?;
    config.agent_model_is_local = runtime
        .config_paths
        .iter()
        .rev()
        .filter_map(|path| crate::config::Config::load_toml_value(path).ok())
        .any(|v| {
            v.get("agent")
                .and_then(|a| a.get("model"))
                .and_then(|m| m.as_str())
                .is_some()
        });
    config.validate_model_format()?;
    Ok(config)
}

pub fn load_toml_value(runtime: &NexRuntime) -> Result<toml::Value> {
    let mut values: Vec<(PathBuf, toml::Value)> = Vec::new();
    for path in &runtime.config_paths {
        let mut value = crate::config::Config::load_toml_value(path)?;
        let mut warnings = Vec::new();
        crate::config::normalize_legacy_tables(
            &mut value,
            &path.display().to_string(),
            &mut warnings,
        );
        values.push((path.clone(), value));
    }

    if matches!(
        runtime.mode,
        NexRuntimeMode::WgIntegrated
            | NexRuntimeMode::WgAutonomous
            | NexRuntimeMode::LegacyWgCompat
    ) {
        apply_wg_endpoint_inheritance_policy(runtime, &mut values);
    }

    let mut merged = toml::Value::Table(toml::map::Map::new());
    for (_path, value) in values {
        merged = merge_toml_for_nex(merged, value);
    }
    Ok(merged)
}

pub fn load_model_registry(runtime: &NexRuntime) -> crate::models::ModelRegistry {
    let mut merged = crate::models::ModelRegistry::with_defaults();
    for path in &runtime.model_registry_paths {
        if !path.exists() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(registry) = serde_yaml::from_str::<crate::models::ModelRegistry>(&content) else {
            continue;
        };
        if registry.default_model.is_some() {
            merged.default_model = registry.default_model;
        }
        for (id, entry) in registry.models {
            merged.models.insert(id, entry);
        }
    }
    merged
}

pub fn chat_id_from_session_ref(session_ref: &str) -> Option<u32> {
    session_ref
        .strip_prefix("chat-")
        .or_else(|| session_ref.strip_prefix("coordinator-"))
        .unwrap_or(session_ref)
        .parse()
        .ok()
}

#[derive(Debug, Deserialize)]
struct ChatCoordinatorState {
    #[serde(default)]
    model_override: Option<String>,
}

pub fn chat_model_override_for_session(state_root: &Path, session_ref: &str) -> Option<String> {
    let chat_id = chat_id_from_session_ref(session_ref)?;
    let per_id_path = state_root
        .join("service")
        .join(format!("coordinator-state-{}.json", chat_id));
    let legacy_path = state_root.join("service").join("coordinator-state.json");
    let path = if per_id_path.exists() {
        per_id_path
    } else if chat_id == 0 && legacy_path.exists() {
        legacy_path
    } else {
        return None;
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<ChatCoordinatorState>(&raw).ok())
        .and_then(|state| state.model_override)
        .filter(|model| !model.trim().is_empty())
}

pub fn session_dir_for_ref(runtime: &NexRuntime, session_ref: &str) -> PathBuf {
    match runtime.session_layout {
        NexSessionLayout::WgChat => crate::chat::chat_dir_for_ref(&runtime.state_root, session_ref),
        NexSessionLayout::StandaloneSessions => runtime.session_root.join(session_ref),
    }
}

pub fn create_fresh_session(runtime: &NexRuntime) -> Result<String> {
    match runtime.session_layout {
        NexSessionLayout::WgChat => {
            let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
            let alias = default_interactive_alias(&stamp);
            crate::chat_sessions::ensure_session(
                &runtime.state_root,
                &alias,
                crate::chat_sessions::SessionKind::Interactive,
                Some(format!("interactive {}", alias)),
            )
            .map_err(|e| anyhow::anyhow!("failed to register fresh session: {}", e))?;
            Ok(alias)
        }
        NexSessionLayout::StandaloneSessions => {
            let id = uuid::Uuid::now_v7().to_string();
            let dir = runtime.session_root.join(&id);
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("create standalone session dir {}", dir.display()))?;
            Ok(id)
        }
    }
}

pub fn pick_resume_session(runtime: &NexRuntime, pattern: &str) -> Result<String> {
    match runtime.session_layout {
        NexSessionLayout::WgChat => pick_wg_resume_session(&runtime.state_root, pattern),
        NexSessionLayout::StandaloneSessions => pick_standalone_resume_session(runtime, pattern),
    }
}

#[derive(Debug, Clone, Default)]
pub struct NexConfigMigrationOptions {
    pub copy_inline_secrets: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NexConfigMigrationReport {
    pub copied_inline_secrets: usize,
    pub refused_inline_secrets: usize,
}

pub fn migrate_wg_config_to_nex(
    source_wg_config: &Path,
    dest_nex_config: &Path,
    options: &NexConfigMigrationOptions,
) -> anyhow::Result<NexConfigMigrationReport> {
    let mut source = crate::config::Config::load_toml_value(source_wg_config)
        .with_context(|| format!("load WG config {}", source_wg_config.display()))?;
    retain_standalone_relevant_sections(&mut source);

    let mut report = NexConfigMigrationReport::default();
    scrub_or_count_inline_api_keys(&mut source, options.copy_inline_secrets, &mut report)?;

    if !options.copy_inline_secrets && report.refused_inline_secrets > 0 {
        bail!(
            "refusing to copy inline plaintext api_key from {}; pass copy_inline_secrets to opt in explicitly",
            source_wg_config.display()
        );
    }

    if let Some(parent) = dest_nex_config.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if dest_nex_config.exists() {
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
        let backup = dest_nex_config.with_extension(format!("toml.bak-{}", stamp));
        std::fs::copy(dest_nex_config, backup)?;
    }
    std::fs::write(dest_nex_config, toml::to_string_pretty(&source)?)?;
    Ok(report)
}

fn nearest_dir_named(start: &Path, name: &str) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        let candidate = cur.join(name);
        if candidate.is_dir() {
            return Some(candidate);
        }
        cur = cur.parent()?;
    }
}

fn nearest_wg_dir(start: &Path) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        for name in crate::workgraph_dir::WORKGRAPH_DIR_NAMES {
            let candidate = cur.join(name);
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
        cur = cur.parent()?;
    }
}

fn nex_overlay_inherits_standalone(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let value = crate::config::Config::load_toml_value(path)?;
    Ok(value
        .get("nex")
        .and_then(|v| v.get("inherit_standalone_config"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

fn nex_overlay_apply_to_autonomous(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let value = crate::config::Config::load_toml_value(path)?;
    Ok(value
        .get("nex")
        .and_then(|v| v.get("apply_to_autonomous"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

fn apply_wg_endpoint_inheritance_policy(
    runtime: &NexRuntime,
    values: &mut [(PathBuf, toml::Value)],
) {
    let local_path = runtime.state_root.join("config.toml");
    let Some(local_idx) = values.iter().position(|(path, _)| path == &local_path) else {
        return;
    };
    let local = &values[local_idx].1;
    let explicit_inherit = local
        .get("llm_endpoints")
        .and_then(|t| t.get("inherit_global"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let local_declares_endpoints = local
        .get("llm_endpoints")
        .and_then(|t| t.get("endpoints"))
        .and_then(|v| v.as_array())
        .is_some();
    if explicit_inherit || local_declares_endpoints {
        return;
    }
    for (path, value) in values.iter_mut().take(local_idx) {
        let path_str = path.to_string_lossy();
        if !path_str.contains(".wg/config.toml") && !path_str.contains(".workgraph/config.toml") {
            continue;
        }
        if let Some(table) = value
            .get_mut("llm_endpoints")
            .and_then(|v| v.as_table_mut())
        {
            table.remove("endpoints");
        }
    }
}

fn merge_toml_for_nex(low: toml::Value, high: toml::Value) -> toml::Value {
    merge_toml_at_path(low, high, &[])
}

fn merge_toml_at_path(low: toml::Value, high: toml::Value, path: &[&str]) -> toml::Value {
    match (low, high) {
        (toml::Value::Table(mut low), toml::Value::Table(high)) => {
            for (key, high_val) in high {
                let merged = if let Some(low_val) = low.remove(&key) {
                    let mut child = Vec::with_capacity(path.len() + 1);
                    child.extend_from_slice(path);
                    child.push(key.as_str());
                    merge_toml_at_path(low_val, high_val, &child)
                } else {
                    high_val
                };
                low.insert(key, merged);
            }
            toml::Value::Table(low)
        }
        (toml::Value::Array(low), toml::Value::Array(high))
            if path == ["llm_endpoints", "endpoints"] =>
        {
            toml::Value::Array(merge_table_array_by_key(low, high, "name"))
        }
        (toml::Value::Array(low), toml::Value::Array(high)) if path == ["model_registry"] => {
            toml::Value::Array(merge_table_array_by_key(low, high, "id"))
        }
        (_low, high) => high,
    }
}

fn merge_table_array_by_key(
    low: Vec<toml::Value>,
    high: Vec<toml::Value>,
    key: &str,
) -> Vec<toml::Value> {
    let mut keyed_high: BTreeMap<String, toml::Value> = high
        .iter()
        .filter_map(|value| {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(|id| (id.to_string(), value.clone()))
        })
        .collect();
    let mut result = Vec::new();
    for value in low {
        if let Some(id) = value.get(key).and_then(|v| v.as_str())
            && let Some(replacement) = keyed_high.remove(id)
        {
            result.push(replacement);
            continue;
        }
        result.push(value);
    }
    for value in high {
        let Some(id) = value.get(key).and_then(|v| v.as_str()) else {
            result.push(value);
            continue;
        };
        if keyed_high.remove(id).is_some() {
            result.push(value);
        }
    }
    result
}

fn retain_standalone_relevant_sections(value: &mut toml::Value) {
    let Some(table) = value.as_table_mut() else {
        return;
    };
    table.retain(|key, _| {
        let key = key.to_string();
        matches!(
            key.as_str(),
            "nex"
                | "llm_endpoints"
                | "model_registry"
                | "models"
                | "tiers"
                | "native_executor"
                | "mcp"
                | "secrets"
        )
    });
}

fn scrub_or_count_inline_api_keys(
    value: &mut toml::Value,
    copy_inline_secrets: bool,
    report: &mut NexConfigMigrationReport,
) -> Result<()> {
    if let Some(endpoints) = value
        .get_mut("llm_endpoints")
        .and_then(|v| v.get_mut("endpoints"))
        .and_then(|v| v.as_array_mut())
    {
        for endpoint in endpoints {
            let Some(table) = endpoint.as_table_mut() else {
                continue;
            };
            if table.contains_key("api_key") {
                if copy_inline_secrets {
                    report.copied_inline_secrets += 1;
                } else {
                    table.remove("api_key");
                    report.refused_inline_secrets += 1;
                }
            }
        }
    }
    Ok(())
}

fn pick_wg_resume_session(workgraph_dir: &Path, pattern: &str) -> Result<String> {
    let sessions = crate::chat_sessions::list(workgraph_dir).context("failed to list sessions")?;
    if sessions.is_empty() {
        bail!("no sessions to resume");
    }
    let mut ranked: Vec<_> = sessions
        .into_iter()
        .map(|(uuid, meta)| {
            let journal = workgraph_dir
                .join("chat")
                .join(&uuid)
                .join("conversation.jsonl");
            let mtime = std::fs::metadata(&journal).and_then(|m| m.modified()).ok();
            (uuid, meta, mtime)
        })
        .collect();
    ranked.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| b.1.created.cmp(&a.1.created)));

    let pat = pattern.trim();
    if !pat.is_empty() {
        let needle = pat.to_lowercase();
        for (uuid, meta, _) in &ranked {
            let kind = format!("{:?}", meta.kind).to_lowercase();
            if uuid.to_lowercase().starts_with(&needle)
                || meta
                    .aliases
                    .iter()
                    .any(|a| a.to_lowercase().contains(&needle))
                || kind.contains(&needle)
            {
                return Ok(pick_best_wg_ref(uuid, meta));
            }
        }
        bail!("no session matches pattern {:?}", pattern);
    }

    let (uuid, meta, _) = ranked
        .first()
        .ok_or_else(|| anyhow::anyhow!("no sessions to resume"))?;
    Ok(pick_best_wg_ref(uuid, meta))
}

fn pick_best_wg_ref(uuid: &str, meta: &crate::chat_sessions::SessionMeta) -> String {
    meta.aliases
        .first()
        .cloned()
        .unwrap_or_else(|| uuid.to_string())
}

fn pick_standalone_resume_session(runtime: &NexRuntime, pattern: &str) -> Result<String> {
    let mut candidates = Vec::new();
    collect_session_dirs(&runtime.session_root, &mut candidates);
    for root in &runtime.legacy_session_roots {
        collect_session_dirs(root, &mut candidates);
    }
    if candidates.is_empty() {
        bail!("no sessions to resume");
    }
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    let needle = pattern.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(candidates[0].0.clone());
    }
    candidates
        .into_iter()
        .find(|(name, _)| name.to_lowercase().contains(&needle))
        .map(|(name, _)| name)
        .ok_or_else(|| anyhow::anyhow!("no session matches pattern {:?}", pattern))
}

fn collect_session_dirs(root: &Path, out: &mut Vec<(String, Option<std::time::SystemTime>)>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let mtime = std::fs::metadata(path.join("conversation.jsonl"))
            .and_then(|m| m.modified())
            .ok();
        out.push((name.to_string(), mtime));
    }
}

fn default_interactive_alias(stamp: &str) -> String {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        unsafe {
            let name = libc::ttyname(0);
            if !name.is_null() {
                let s = CStr::from_ptr(name).to_string_lossy();
                let slug = s
                    .trim_start_matches("/dev/")
                    .replace('/', "-")
                    .replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "-");
                if !slug.is_empty() {
                    return format!("tty-{}-{}", slug, stamp);
                }
            }
        }
    }
    format!("session-{}", stamp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn standalone_prefers_project_nex_when_initialized_else_user_home() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let nested = project.join("src").join("deep");
        std::fs::create_dir_all(project.join(".nex")).unwrap();
        std::fs::create_dir_all(project.join(".wg")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();

        let runtime = resolve_standalone(&NexRuntimeResolveInput {
            cwd: Some(nested.clone()),
            home_dir: Some(home.clone()),
            ..Default::default()
        });

        assert_eq!(runtime.mode, NexRuntimeMode::Standalone);
        assert_eq!(runtime.state_root, project.join(".nex"));
        assert_eq!(runtime.session_root, project.join(".nex").join("sessions"));
        assert_eq!(runtime.session_layout, NexSessionLayout::StandaloneSessions);

        std::fs::remove_dir_all(project.join(".nex")).unwrap();
        let runtime = resolve_standalone(&NexRuntimeResolveInput {
            cwd: Some(nested),
            home_dir: Some(home.clone()),
            ..Default::default()
        });

        assert_eq!(runtime.state_root, home.join(".nex"));
        assert_eq!(runtime.session_root, home.join(".nex").join("sessions"));
    }

    #[test]
    fn autonomous_ignores_user_nex_model_and_endpoint_config() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let wg = tmp.path().join("project").join(".wg");

        write(
            &home.join(".nex").join("config.toml"),
            r#"
[models.task_agent]
model = "nex:standalone-model"

[[llm_endpoints.endpoints]]
name = "standalone"
provider = "openai"
url = "https://standalone.invalid/v1"
is_default = true
"#,
        );
        write(
            &wg.join("config.toml"),
            r#"
[models.task_agent]
model = "nex:wg-model"

[[llm_endpoints.endpoints]]
name = "wg"
provider = "openai"
url = "https://wg.invalid/v1"
is_default = true
"#,
        );

        let runtime = resolve_wg_autonomous(&wg, Some(home));
        let config = load_config(&runtime).unwrap();

        let resolved = config.resolve_model_for_role(crate::config::DispatchRole::TaskAgent);
        assert_eq!(resolved.model, "wg-model");
        assert!(config.llm_endpoints.find_by_name("wg").is_some());
        assert!(config.llm_endpoints.find_by_name("standalone").is_none());
    }

    #[test]
    fn standalone_config_merges_endpoints_and_models_by_identity() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let project = tmp.path().join("project");
        let nested = project.join("nested");
        std::fs::create_dir_all(project.join(".nex")).unwrap();
        std::fs::create_dir_all(project.join(".wg")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();

        write(
            &home.join(".wg").join("config.toml"),
            r#"
[[llm_endpoints.endpoints]]
name = "shared"
provider = "openai"
url = "https://global.invalid/v1"

[[model_registry]]
id = "shared-model"
provider = "openai"
model = "global-wire"
tier = "standard"
"#,
        );
        write(
            &home.join(".nex").join("config.toml"),
            r#"
[[llm_endpoints.endpoints]]
name = "shared"
provider = "openai"
url = "https://user.invalid/v1"

[[llm_endpoints.endpoints]]
name = "user-only"
provider = "openai"
url = "https://user-only.invalid/v1"

[[model_registry]]
id = "shared-model"
provider = "openai"
model = "user-wire"
tier = "standard"
"#,
        );
        write(
            &project.join(".nex").join("config.toml"),
            r#"
[[llm_endpoints.endpoints]]
name = "project-only"
provider = "openai"
url = "https://project-only.invalid/v1"

[[model_registry]]
id = "project-model"
provider = "openai"
model = "project-wire"
tier = "standard"
"#,
        );

        let runtime = resolve_standalone(&NexRuntimeResolveInput {
            cwd: Some(nested),
            home_dir: Some(home),
            ..Default::default()
        });
        let config = load_config(&runtime).unwrap();

        assert_eq!(
            config
                .llm_endpoints
                .find_by_name("shared")
                .and_then(|ep| ep.url.as_deref()),
            Some("https://user.invalid/v1")
        );
        assert!(config.llm_endpoints.find_by_name("user-only").is_some());
        assert!(config.llm_endpoints.find_by_name("project-only").is_some());
        let shared_count = config
            .llm_endpoints
            .endpoints
            .iter()
            .filter(|ep| ep.name == "shared")
            .count();
        assert_eq!(shared_count, 1);
        assert_eq!(
            config
                .registry_lookup("shared-model")
                .map(|entry| entry.model),
            Some("user-wire".to_string())
        );
        assert!(config.registry_lookup("project-model").is_some());
    }

    #[test]
    fn wg_integrated_reads_wg_nex_config_overlay() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        let wg = tmp.path().join("project").join(".wg");
        write(
            &wg.join("config.toml"),
            r#"
[models.task_agent]
model = "nex:wg-model"
"#,
        );
        write(
            &wg.join("nex").join("config.toml"),
            r#"
[models.task_agent]
model = "nex:overlay-model"
"#,
        );

        let runtime = resolve_wg_integrated_with_home(&wg, Some(home));
        let config = load_config(&runtime).unwrap();
        let resolved = config.resolve_model_for_role(crate::config::DispatchRole::TaskAgent);
        assert_eq!(resolved.model, "overlay-model");
    }

    #[test]
    fn migration_refuses_inline_plaintext_api_key_without_opt_in() {
        let tmp = TempDir::new().unwrap();
        let source = tmp.path().join(".wg").join("config.toml");
        let dest = tmp.path().join(".nex").join("config.toml");
        write(
            &source,
            r#"
[[llm_endpoints.endpoints]]
name = "openrouter"
provider = "openrouter"
api_key = "sk-plaintext"
"#,
        );

        let err = migrate_wg_config_to_nex(
            &source,
            &dest,
            &NexConfigMigrationOptions {
                copy_inline_secrets: false,
            },
        )
        .unwrap_err();

        assert!(err.to_string().contains("inline plaintext api_key"));
        assert!(!dest.exists());
    }
}
