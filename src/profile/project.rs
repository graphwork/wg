//! Project-scoped named-profile selection and local profile-usage history.
//!
//! Named profile definitions stay reusable machine-global files under
//! `~/.wg/profiles/`. A project selection is a small, explicit association at
//! `<graph>/profile-selection.json`; it never rewrites the global config or the
//! legacy `~/.wg/active-profile` pointer. The association pins the selected
//! definition's content fingerprint. If that definition is edited, renamed, or
//! deleted, config resolution fails closed until the project explicitly
//! re-selects a profile.
//!
//! Usage history is local-only JSONL under `~/.wg/profile-usage.jsonl`. Records
//! contain only a profile name, profile fingerprint, timestamp, canonical-path
//! digest, and coarse successful-event category. They contain no path, prompt,
//! command line, endpoint URL, credential reference, or telemetry identifier.

use crate::atomic_file::write_atomic;
use crate::config::{Config, DispatchRole, ReasoningLevel, handler_first_rewrite};
use crate::dispatch::handler_for_model;
use crate::profile::named;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub const PROJECT_SELECTION_FILE: &str = "profile-selection.json";
pub const PROFILE_USAGE_FILE: &str = "profile-usage.jsonl";
pub const PROJECT_SELECTION_VERSION: u32 = 1;
pub const DEFAULT_MAX_USAGE_RECORDS: usize = 2048;
const USAGE_HALF_LIFE_DAYS: f64 = 30.0;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectProfileAssociation {
    pub version: u32,
    pub profile: String,
    pub profile_fingerprint: String,
    pub selected_at: String,
    pub project_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileSource {
    Installed,
    BuiltinTemplate,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AssociationState {
    None,
    Ready,
    ContentDrift,
    Unavailable,
    ProjectMoved,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssociationInspection {
    pub state: AssociationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub association: Option<ProjectProfileAssociation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_fingerprint: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum UsageEventCategory {
    ProfileSelected,
    TaskCreated,
    ServiceStarted,
    ConfigApplied,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileUsageRecord {
    pub profile: String,
    pub profile_fingerprint: String,
    pub timestamp: String,
    pub project_digest: String,
    pub category: UsageEventCategory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExactRoute {
    pub role: String,
    pub route: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<ReasoningLevel>,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandlerReadiness {
    pub handler: String,
    pub installed: bool,
    pub auth_owner: String,
    pub auth_status: String,
    pub endpoint_status: String,
    pub plugin_status: String,
    pub annotation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileReadiness {
    pub annotation: String,
    pub strong_route: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strong_reasoning: Option<ReasoningLevel>,
    pub weak_route: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weak_reasoning: Option<ReasoningLevel>,
    pub routes: Vec<ExactRoute>,
    pub handlers: Vec<HandlerReadiness>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileCatalogEntry {
    pub name: String,
    pub source: ProfileSource,
    pub selected_for_project: bool,
    pub global_active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    pub association_state: AssociationState,
    pub readiness: ProfileReadiness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_label: Option<String>,
}

/// Immutable, redacted selection plan. It contains digests and exact routes,
/// never canonical paths, endpoint URLs, secret references, or raw profile
/// bytes. Apply rechecks both the project identity and all preimages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectSelectionPlan {
    pub version: u32,
    pub scope: String,
    pub writes_global_config: bool,
    pub writes_global_active_profile: bool,
    pub materializes_global_profile_definition: bool,
    pub project_digest: String,
    pub profile: String,
    pub profile_fingerprint: String,
    pub profile_source: ProfileSource,
    pub selected_at: String,
    pub expected_association_preimage: String,
    pub expected_profile_preimage: String,
    pub readiness: ProfileReadiness,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectClearPlan {
    pub version: u32,
    pub project_digest: String,
    pub expected_association_preimage: String,
    pub had_selection: bool,
}

pub fn association_path(workgraph_dir: &Path) -> PathBuf {
    workgraph_dir.join(PROJECT_SELECTION_FILE)
}

fn usage_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("WG_PROFILE_USAGE_PATH") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return Ok(path);
        }
    }
    Ok(Config::global_dir()?.join(PROFILE_USAGE_FILE))
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("b3:{}", blake3::hash(bytes).to_hex())
}

fn file_preimage(path: &Path) -> Result<String> {
    match fs::read(path) {
        Ok(bytes) => Ok(digest_bytes(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("absent".to_string()),
        Err(e) => Err(e).with_context(|| format!("Failed to read {}", path.display())),
    }
}

/// A path-safe project identity. Canonical aliases resolve to one digest, but
/// the raw path is never persisted in global usage history or selection plans.
pub fn project_digest(workgraph_dir: &Path) -> Result<String> {
    let canonical = workgraph_dir.canonicalize().with_context(|| {
        format!(
            "Project profile selection requires an existing canonical WG directory; could not resolve {}",
            workgraph_dir.display()
        )
    })?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"worksgood-project-profile-v1\0");
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        hasher.update(canonical.as_os_str().as_bytes());
    }
    #[cfg(not(unix))]
    hasher.update(canonical.to_string_lossy().as_bytes());
    Ok(format!("b3:{}", hasher.finalize().to_hex()))
}

fn canonical_json(value: &serde_json::Value, out: &mut Vec<u8>) {
    match value {
        serde_json::Value::Object(map) => {
            out.push(b'{');
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend(serde_json::to_vec(key).unwrap_or_default());
                out.push(b':');
                canonical_json(&map[key], out);
            }
            out.push(b'}');
        }
        serde_json::Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                canonical_json(item, out);
            }
            out.push(b']');
        }
        _ => out.extend(serde_json::to_vec(value).unwrap_or_default()),
    }
}

/// Fingerprint semantic profile content. TOML comments and formatting do not
/// affect the result; any parsed content change does. Only the digest leaves
/// this function, so embedded secret values/paths cannot leak into history.
pub fn profile_content_fingerprint(content: &str) -> Result<String> {
    let value: toml::Value = content.parse().context("Profile TOML is invalid")?;
    let json = serde_json::to_value(value).context("Failed to canonicalize profile TOML")?;
    let mut canonical = Vec::new();
    canonical_json(&json, &mut canonical);
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"worksgood-profile-content-v1\0");
    hasher.update(&canonical);
    Ok(format!("b3:{}", hasher.finalize().to_hex()))
}

fn profile_content(name: &str, allow_builtin: bool) -> Result<(String, ProfileSource)> {
    let path = named::profile_path(name)?;
    if path.is_file() {
        return Ok((
            fs::read_to_string(&path)
                .with_context(|| format!("Failed to read profile '{}'", name))?,
            ProfileSource::Installed,
        ));
    }
    if allow_builtin && let Some(template) = named::starter_template(name) {
        return Ok((template.to_string(), ProfileSource::BuiltinTemplate));
    }
    anyhow::bail!(
        "Selected project profile '{}' is unavailable. Its reusable definition is missing at {}. Recover with `wg profile create {} ...` or select another profile with `wg profile select <name>`.",
        name,
        path.display(),
        name
    )
}

pub fn read_association(workgraph_dir: &Path) -> Result<Option<ProjectProfileAssociation>> {
    let path = association_path(workgraph_dir);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "Failed to read project profile association {}",
            path.display()
        )
    })?;
    let association: ProjectProfileAssociation = serde_json::from_slice(&bytes).with_context(|| {
        format!(
            "Project profile association {} is malformed. It was not used; recover with `wg profile select --clear` or select a profile again.",
            path.display()
        )
    })?;
    if association.version != PROJECT_SELECTION_VERSION {
        anyhow::bail!(
            "Project profile association {} has unsupported version {} (expected {}). No route was selected.",
            path.display(),
            association.version,
            PROJECT_SELECTION_VERSION
        );
    }
    named::validate_profile_name(&association.profile)?;
    Ok(Some(association))
}

pub fn inspect_association(workgraph_dir: &Path) -> AssociationInspection {
    let association = match read_association(workgraph_dir) {
        Ok(Some(a)) => a,
        Ok(None) => {
            return AssociationInspection {
                state: AssociationState::None,
                association: None,
                current_fingerprint: None,
                message:
                    "No project profile selected; global `wg profile use` state remains separate."
                        .to_string(),
            };
        }
        Err(e) => {
            return AssociationInspection {
                state: AssociationState::Invalid,
                association: None,
                current_fingerprint: None,
                message: e.to_string(),
            };
        }
    };

    match project_digest(workgraph_dir) {
        Ok(digest) if digest != association.project_digest => {
            return AssociationInspection {
                state: AssociationState::ProjectMoved,
                association: Some(association),
                current_fingerprint: None,
                message: "The canonical project identity changed. Re-select the profile in this location; no route was inferred from the old path.".to_string(),
            };
        }
        Err(e) => {
            return AssociationInspection {
                state: AssociationState::Invalid,
                association: Some(association),
                current_fingerprint: None,
                message: e.to_string(),
            };
        }
        _ => {}
    }

    let content = match profile_content(&association.profile, false) {
        Ok((content, _)) => content,
        Err(e) => {
            return AssociationInspection {
                state: AssociationState::Unavailable,
                association: Some(association),
                current_fingerprint: None,
                message: e.to_string(),
            };
        }
    };
    let current = match profile_content_fingerprint(&content) {
        Ok(fp) => fp,
        Err(e) => {
            return AssociationInspection {
                state: AssociationState::Invalid,
                association: Some(association),
                current_fingerprint: None,
                message: format!("Selected profile definition is invalid: {e}"),
            };
        }
    };
    if current != association.profile_fingerprint {
        return AssociationInspection {
            state: AssociationState::ContentDrift,
            association: Some(association),
            current_fingerprint: Some(current),
            message: "The reusable profile definition changed after this project selected it. Re-run `wg profile select <name>` to inspect and acknowledge the new routes; the changed definition is not used automatically.".to_string(),
        };
    }
    AssociationInspection {
        state: AssociationState::Ready,
        association: Some(association),
        current_fingerprint: Some(current),
        message: "Project selection matches the reusable profile definition.".to_string(),
    }
}

/// Return a verified selected-profile TOML overlay for Config resolution.
/// Missing, renamed, moved, malformed, or content-drifted selections fail
/// closed rather than falling back to a global profile/provider.
pub fn selected_profile_toml(workgraph_dir: &Path) -> Result<Option<toml::Value>> {
    let Some(association) = read_association(workgraph_dir)? else {
        return Ok(None);
    };
    let current_project = project_digest(workgraph_dir)?;
    if current_project != association.project_digest {
        anyhow::bail!(
            "Project profile selection belongs to a different canonical project identity. Run `wg profile select {}` here to acknowledge the move; no global route fallback was used.",
            association.profile
        );
    }
    let (content, _) = profile_content(&association.profile, false)?;
    let fingerprint = profile_content_fingerprint(&content)?;
    if fingerprint != association.profile_fingerprint {
        anyhow::bail!(
            "Project profile '{}' changed after selection (selected {}, current {}). Run `wg profile show {}` and then `wg profile select {}` to acknowledge it. The changed route was not activated and no global fallback was used.",
            association.profile,
            association.profile_fingerprint,
            fingerprint,
            association.profile,
            association.profile
        );
    }
    content
        .parse::<toml::Value>()
        .with_context(|| format!("Selected profile '{}' is invalid", association.profile))
        .map(Some)
}

fn canonical_route(route: &str) -> String {
    handler_first_rewrite(route).unwrap_or_else(|| route.to_string())
}

fn raw_route_for_role(config: &Config, role: DispatchRole) -> (String, String) {
    if let Some(role_cfg) = config.models.get_role(role) {
        if let Some(model) = role_cfg.model.as_deref() {
            return (canonical_route(model), "per-role".to_string());
        }
        if let Some(tier) = role_cfg.tier
            && let Some(model) = config.configured_tier_spec(tier)
        {
            return (canonical_route(&model), "role-tier".to_string());
        }
    }
    if let Some(model) = config.configured_tier_spec(role.default_tier()) {
        return (canonical_route(&model), "profile-tier".to_string());
    }
    if let Some(model) = config
        .models
        .default
        .as_ref()
        .and_then(|m| m.model.as_deref())
    {
        return (canonical_route(model), "profile-default".to_string());
    }
    (
        canonical_route(&config.agent.model),
        "profile-agent".to_string(),
    )
}

fn profile_routes(config: &Config) -> Vec<ExactRoute> {
    let mut routes = Vec::with_capacity(DispatchRole::ALL.len() + 3);
    routes.push(ExactRoute {
        role: "agent".to_string(),
        route: canonical_route(&config.agent.model),
        reasoning: config.resolve_reasoning_for_role(DispatchRole::TaskAgent),
        source: "profile-agent".to_string(),
    });
    routes.push(ExactRoute {
        role: "dispatcher".to_string(),
        route: canonical_route(
            config
                .coordinator
                .model
                .as_deref()
                .unwrap_or(&config.agent.model),
        ),
        reasoning: config.resolve_reasoning_for_role(DispatchRole::Default),
        source: "profile-dispatcher".to_string(),
    });
    let (default_route, default_source) = raw_route_for_role(config, DispatchRole::Default);
    routes.push(ExactRoute {
        role: "default".to_string(),
        route: default_route,
        reasoning: config.resolve_reasoning_for_role(DispatchRole::Default),
        source: default_source,
    });
    for role in DispatchRole::ALL {
        if *role == DispatchRole::Default {
            continue;
        }
        let (route, source) = raw_route_for_role(config, *role);
        routes.push(ExactRoute {
            role: role.to_string(),
            route,
            reasoning: config.resolve_reasoning_for_role(*role),
            source,
        });
    }
    routes
}

fn safe_status_identifier(value: &str) -> String {
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        value.to_string()
    } else {
        "custom".to_string()
    }
}

fn handler_auth_owner(handler: &str) -> &'static str {
    match handler {
        "pi" => "Pi owns authentication",
        "claude" => "Claude CLI owns authentication",
        "codex" => "Codex CLI owns authentication",
        "opencode" => "OpenCode owns authentication",
        "native" => "WorksGood endpoint configuration owns authentication",
        _ => "Selected handler owns authentication",
    }
}

fn readiness_for_config(config: &Config) -> ProfileReadiness {
    let routes = profile_routes(config);
    let strong = routes
        .iter()
        .find(|r| r.role == "task_agent")
        .or_else(|| routes.first())
        .cloned()
        .unwrap_or(ExactRoute {
            role: "task_agent".to_string(),
            route: canonical_route(&config.agent.model),
            reasoning: None,
            source: "profile-agent".to_string(),
        });
    let weak = routes
        .iter()
        .find(|r| r.role == "evaluator")
        .or_else(|| routes.first())
        .cloned()
        .unwrap_or_else(|| strong.clone());

    let discovered: HashMap<String, bool> = crate::executor_discovery::discover()
        .into_iter()
        .map(|e| (e.name.to_string(), e.available))
        .collect();
    let mut handler_names: Vec<String> = routes
        .iter()
        .map(|r| handler_for_model(&r.route).as_str().to_string())
        .collect();
    handler_names.sort();
    handler_names.dedup();

    let pi_plugin = crate::pi_plugin::status();
    let endpoint = config.llm_endpoints.find_default();
    let mut handlers = Vec::new();
    for handler in handler_names {
        let discovery_name = if handler == "native" {
            "native"
        } else {
            &handler
        };
        let installed = discovered.get(discovery_name).copied().unwrap_or(false);
        let auth_status = if handler == "native" {
            "credential status not inspected".to_string()
        } else {
            "auth status unknown — attended check required".to_string()
        };
        let endpoint_status = if handler == "native" {
            match endpoint {
                Some(ep) => format!("configured ({})", safe_status_identifier(&ep.provider)),
                None => "not configured".to_string(),
            }
        } else {
            "owned by handler".to_string()
        };
        let plugin_status = if handler == "pi" {
            format!(
                "pi-worksgood compat {}; build {}; console {}",
                pi_plugin.compat,
                if pi_plugin.ready { "ready" } else { "missing" },
                if pi_plugin.console_wired {
                    "wired"
                } else {
                    "not wired"
                }
            )
        } else {
            "not required".to_string()
        };
        let annotation = if !installed {
            format!("{} handler unavailable", handler)
        } else if handler == "native" && endpoint.is_none() {
            "built-in handler installed; endpoint not configured".to_string()
        } else if handler == "native" {
            "built-in handler installed; endpoint configured; credential status not inspected"
                .to_string()
        } else if handler == "pi" && !pi_plugin.ready {
            "Pi installed; auth unknown; compatible plugin build missing".to_string()
        } else {
            format!("{} installed; {}", handler, auth_status)
        };
        handlers.push(HandlerReadiness {
            handler: handler.clone(),
            installed,
            auth_owner: handler_auth_owner(&handler).to_string(),
            auth_status,
            endpoint_status,
            plugin_status,
            annotation,
        });
    }
    let annotation = handlers
        .iter()
        .map(|h| h.annotation.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    ProfileReadiness {
        annotation,
        strong_route: strong.route,
        strong_reasoning: strong.reasoning,
        weak_route: weak.route,
        weak_reasoning: weak.reasoning,
        routes,
        handlers,
    }
}

pub fn plan_project_selection_at(
    workgraph_dir: &Path,
    name: &str,
    now: DateTime<Utc>,
) -> Result<ProjectSelectionPlan> {
    named::validate_profile_name(name)?;
    let project_digest = project_digest(workgraph_dir)?;
    let association = association_path(workgraph_dir);
    let expected_association_preimage = file_preimage(&association)?;
    let path = named::profile_path(name)?;
    let expected_profile_preimage = file_preimage(&path)?;
    let (content, source) = profile_content(name, true)?;
    let fingerprint = profile_content_fingerprint(&content)?;
    let profile: Config = toml::from_str(&content)
        .with_context(|| format!("Profile '{}' is not a valid WorksGood config", name))?;
    profile.validate_model_format()?;
    Ok(ProjectSelectionPlan {
        version: PROJECT_SELECTION_VERSION,
        scope: "project".to_string(),
        writes_global_config: false,
        writes_global_active_profile: false,
        materializes_global_profile_definition: source == ProfileSource::BuiltinTemplate,
        project_digest,
        profile: name.to_string(),
        profile_fingerprint: fingerprint,
        profile_source: source,
        selected_at: now.to_rfc3339(),
        expected_association_preimage,
        expected_profile_preimage,
        readiness: readiness_for_config(&profile),
    })
}

pub fn plan_project_selection(workgraph_dir: &Path, name: &str) -> Result<ProjectSelectionPlan> {
    plan_project_selection_at(workgraph_dir, name, Utc::now())
}

pub fn apply_project_selection(
    workgraph_dir: &Path,
    plan: &ProjectSelectionPlan,
) -> Result<ProjectProfileAssociation> {
    if plan.version != PROJECT_SELECTION_VERSION
        || plan.scope != "project"
        || plan.writes_global_config
        || plan.writes_global_active_profile
        || plan.materializes_global_profile_definition
            != (plan.profile_source == ProfileSource::BuiltinTemplate)
    {
        anyhow::bail!("Invalid or unsupported project profile selection plan");
    }
    let current_project = project_digest(workgraph_dir)?;
    if current_project != plan.project_digest {
        anyhow::bail!("Project identity changed after planning; build a new selection plan.");
    }
    let association_path = association_path(workgraph_dir);
    let _lock = ExclusiveFileLock::acquire(&association_path.with_extension("lock"))?;
    let current_preimage = file_preimage(&association_path)?;
    if current_preimage != plan.expected_association_preimage {
        anyhow::bail!(
            "Project profile association changed after planning (expected {}, found {}). Nothing was written; inspect and retry.",
            plan.expected_association_preimage,
            current_preimage
        );
    }

    let profile_path = named::profile_path(&plan.profile)?;
    let mut materialized_preimage = None;
    match plan.profile_source {
        ProfileSource::BuiltinTemplate => {
            let current = file_preimage(&profile_path)?;
            if current != plan.expected_profile_preimage {
                anyhow::bail!(
                    "Profile '{}' was created or changed after planning. Nothing was selected; inspect it and retry.",
                    plan.profile
                );
            }
            let template = named::starter_template(&plan.profile)
                .ok_or_else(|| anyhow::anyhow!("Built-in profile template disappeared"))?;
            if profile_content_fingerprint(template)? != plan.profile_fingerprint {
                anyhow::bail!("Built-in profile content changed after planning; retry.");
            }
            if let Err(e) =
                crate::atomic_file::write_atomic_create_new(&profile_path, template.as_bytes())
            {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    anyhow::bail!(
                        "Profile '{}' was created concurrently after planning. Nothing was overwritten or selected; inspect it and retry.",
                        plan.profile
                    );
                }
                return Err(e).with_context(|| {
                    format!(
                        "Failed to atomically materialize built-in profile '{}'",
                        plan.profile
                    )
                });
            }
            materialized_preimage = Some(digest_bytes(template.as_bytes()));
        }
        ProfileSource::Installed => {
            let current_preimage = file_preimage(&profile_path)?;
            if current_preimage != plan.expected_profile_preimage {
                anyhow::bail!(
                    "Profile '{}' definition changed after planning (expected preimage {}, found {}). Nothing was selected; inspect it and retry.",
                    plan.profile,
                    plan.expected_profile_preimage,
                    current_preimage
                );
            }
            let (content, _) = profile_content(&plan.profile, false)?;
            let current = profile_content_fingerprint(&content)?;
            if current != plan.profile_fingerprint {
                anyhow::bail!(
                    "Profile '{}' changed after planning (expected {}, found {}). Nothing was selected; inspect and retry.",
                    plan.profile,
                    plan.profile_fingerprint,
                    current
                );
            }
        }
        ProfileSource::Unavailable => anyhow::bail!("Cannot select an unavailable profile"),
    }

    let association = ProjectProfileAssociation {
        version: PROJECT_SELECTION_VERSION,
        profile: plan.profile.clone(),
        profile_fingerprint: plan.profile_fingerprint.clone(),
        selected_at: plan.selected_at.clone(),
        project_digest: plan.project_digest.clone(),
    };
    let mut body = serde_json::to_vec_pretty(&association)?;
    body.push(b'\n');
    if let Err(e) = write_atomic(&association_path, &body) {
        // Roll back only the exact bytes this apply materialized. Never delete
        // a concurrently replaced or even formatting-edited definition.
        if materialized_preimage.as_deref() == file_preimage(&profile_path).ok().as_deref() {
            let _ = fs::remove_file(profile_path);
        }
        return Err(e).with_context(|| {
            format!(
                "Failed to atomically write project profile association {}",
                association_path.display()
            )
        });
    }
    Ok(association)
}

pub fn plan_clear_project_selection(workgraph_dir: &Path) -> Result<ProjectClearPlan> {
    let path = association_path(workgraph_dir);
    Ok(ProjectClearPlan {
        version: PROJECT_SELECTION_VERSION,
        project_digest: project_digest(workgraph_dir)?,
        expected_association_preimage: file_preimage(&path)?,
        had_selection: path.exists(),
    })
}

pub fn apply_clear_project_selection(workgraph_dir: &Path, plan: &ProjectClearPlan) -> Result<()> {
    if plan.version != PROJECT_SELECTION_VERSION {
        anyhow::bail!(
            "Unsupported project profile clear plan version {} (expected {}); nothing was cleared.",
            plan.version,
            PROJECT_SELECTION_VERSION
        );
    }
    if project_digest(workgraph_dir)? != plan.project_digest {
        anyhow::bail!("Project identity changed after planning; nothing was cleared.");
    }
    let path = association_path(workgraph_dir);
    let _lock = ExclusiveFileLock::acquire(&path.with_extension("lock"))?;
    let current = file_preimage(&path)?;
    if current != plan.expected_association_preimage {
        anyhow::bail!("Project profile association changed after planning; nothing was cleared.");
    }
    if path.exists() {
        fs::remove_file(&path)
            .with_context(|| format!("Failed to remove project selection {}", path.display()))?;
    }
    Ok(())
}

fn load_usage_from(path: &Path) -> Vec<ProfileUsageRecord> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(|line| line.ok())
        .filter_map(|line| serde_json::from_str::<ProfileUsageRecord>(&line).ok())
        .filter(|record| named::validate_profile_name(&record.profile).is_ok())
        .filter(|record| record.profile_fingerprint.starts_with("b3:"))
        .filter(|record| record.project_digest.starts_with("b3:"))
        .collect()
}

pub fn usage_records() -> Result<Vec<ProfileUsageRecord>> {
    Ok(load_usage_from(&usage_path()?))
}

fn record_usage_at_path(path: &Path, record: ProfileUsageRecord, max_records: usize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _lock = ExclusiveFileLock::acquire(&path.with_extension("lock"))?;
    let mut records = load_usage_from(path);
    records.push(record);
    if records.len() > max_records {
        records.drain(..records.len() - max_records);
    }
    let mut body = Vec::new();
    for record in records {
        serde_json::to_writer(&mut body, &record)?;
        body.push(b'\n');
    }
    write_atomic(path, body).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// Record a coarse successful event for the current project's explicit
/// selection. Drifted/unavailable associations are ignored, so changed profile
/// content cannot inherit old usage or acquire new usage before acknowledgement.
pub fn record_successful_event_at(
    workgraph_dir: &Path,
    category: UsageEventCategory,
    now: DateTime<Utc>,
) -> Result<bool> {
    let inspection = inspect_association(workgraph_dir);
    if inspection.state != AssociationState::Ready {
        return Ok(false);
    }
    let association = inspection.association.expect("ready has association");
    let record = ProfileUsageRecord {
        profile: association.profile,
        profile_fingerprint: association.profile_fingerprint,
        timestamp: now.to_rfc3339(),
        project_digest: association.project_digest,
        category,
    };
    record_usage_at_path(&usage_path()?, record, DEFAULT_MAX_USAGE_RECORDS)?;
    Ok(true)
}

pub fn record_successful_event(workgraph_dir: &Path, category: UsageEventCategory) -> Result<bool> {
    record_successful_event_at(workgraph_dir, category, Utc::now())
}

pub fn clear_usage_history() -> Result<()> {
    let path = usage_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let _lock = ExclusiveFileLock::acquire(&path.with_extension("lock"))?;
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct UsageScore {
    score: f64,
    latest: DateTime<Utc>,
    label: String,
}

fn score_usage(
    records: &[ProfileUsageRecord],
    fingerprints: &HashMap<String, String>,
    now: DateTime<Utc>,
) -> HashMap<String, UsageScore> {
    let mut grouped: HashMap<String, Vec<DateTime<Utc>>> = HashMap::new();
    for record in records {
        if fingerprints.get(&record.profile) != Some(&record.profile_fingerprint) {
            continue;
        }
        let Ok(timestamp) = DateTime::parse_from_rfc3339(&record.timestamp) else {
            continue;
        };
        grouped
            .entry(record.profile.clone())
            .or_default()
            .push(timestamp.with_timezone(&Utc));
    }
    grouped
        .into_iter()
        .filter_map(|(name, timestamps)| {
            let latest = timestamps.iter().max().copied()?;
            let score: f64 = timestamps
                .iter()
                .map(|timestamp| {
                    let age_secs =
                        now.signed_duration_since(*timestamp).num_seconds().max(0) as f64;
                    let age_days = age_secs / 86_400.0;
                    0.5_f64.powf(age_days / USAGE_HALF_LIFE_DAYS)
                })
                .sum();
            let label = if latest.date_naive() == now.date_naive() {
                "used today"
            } else if score >= 2.5 {
                "frequent"
            } else {
                "recent"
            };
            Some((
                name,
                UsageScore {
                    score,
                    latest,
                    label: label.to_string(),
                },
            ))
        })
        .collect()
}

fn launcher_recent_route_names(
    profiles: &HashMap<String, ProfileReadiness>,
    now: DateTime<Utc>,
) -> HashSet<String> {
    let Ok(entries) = crate::launcher_history::recent_combos(50) else {
        return HashSet::new();
    };
    let recent_routes: HashSet<String> = entries
        .into_iter()
        .filter(|entry| {
            DateTime::parse_from_rfc3339(&entry.timestamp)
                .map(|ts| now.signed_duration_since(ts.with_timezone(&Utc)).num_days() <= 30)
                .unwrap_or(false)
        })
        .filter_map(|entry| entry.model.map(|model| canonical_route(&model)))
        .collect();
    profiles
        .iter()
        .filter(|(_, readiness)| {
            readiness
                .routes
                .iter()
                .any(|route| recent_routes.contains(&route.route))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

fn unavailable_readiness() -> ProfileReadiness {
    ProfileReadiness {
        annotation:
            "profile definition unavailable — select another profile or restore the definition"
                .to_string(),
        strong_route: "unavailable".to_string(),
        strong_reasoning: None,
        weak_route: "unavailable".to_string(),
        weak_reasoning: None,
        routes: Vec::new(),
        handlers: Vec::new(),
    }
}

/// Read-only catalog for concierge/profile pickers. No directory, cache,
/// history, profile, plugin, or config file is created or changed.
pub fn catalog_at(workgraph_dir: &Path, now: DateTime<Utc>) -> Result<Vec<ProfileCatalogEntry>> {
    let inspection = inspect_association(workgraph_dir);
    let selected_name = inspection
        .association
        .as_ref()
        .map(|association| association.profile.clone());
    let global_active = named::active().unwrap_or(None);
    let installed = named::list_installed().unwrap_or_default();
    let installed_set: HashSet<String> = installed.iter().cloned().collect();

    let mut names = installed.clone();
    for starter in named::STARTER_NAMES {
        if !installed_set.contains(*starter) {
            names.push((*starter).to_string());
        }
    }
    if let Some(selected) = selected_name.as_ref()
        && !names.contains(selected)
    {
        names.insert(0, selected.clone());
    }

    let mut content_by_name = HashMap::new();
    let mut config_by_name = HashMap::new();
    let mut fingerprint_by_name = HashMap::new();
    let mut readiness_by_name = HashMap::new();
    let mut description_by_name = HashMap::new();
    let mut source_by_name = HashMap::new();
    for name in &names {
        if let Ok((content, source)) = profile_content(name, true) {
            if let Ok(config) = toml::from_str::<Config>(&content) {
                let description = content
                    .parse::<toml::Value>()
                    .ok()
                    .and_then(|v| v.get("description")?.as_str().map(str::to_string));
                if let Ok(fingerprint) = profile_content_fingerprint(&content) {
                    fingerprint_by_name.insert(name.clone(), fingerprint);
                }
                description_by_name.insert(name.clone(), description);
                readiness_by_name.insert(name.clone(), readiness_for_config(&config));
                config_by_name.insert(name.clone(), config);
                content_by_name.insert(name.clone(), content);
                source_by_name.insert(name.clone(), source);
            }
        }
    }
    let _ = (&content_by_name, &config_by_name);

    let records = usage_records().unwrap_or_default();
    let usage_scores = score_usage(&records, &fingerprint_by_name, now);
    let route_evidence = launcher_recent_route_names(&readiness_by_name, now);

    let mut installed_names: Vec<String> = names
        .iter()
        .filter(|name| installed_set.contains(*name))
        .cloned()
        .collect();
    let selected_for_sort = selected_name.clone();
    installed_names.sort_by(|a, b| {
        let a_selected = selected_for_sort.as_deref() == Some(a.as_str());
        let b_selected = selected_for_sort.as_deref() == Some(b.as_str());
        if a_selected != b_selected {
            return b_selected.cmp(&a_selected);
        }
        match (usage_scores.get(a), usage_scores.get(b)) {
            (Some(sa), Some(sb)) => sb
                .score
                .total_cmp(&sa.score)
                .then_with(|| sb.latest.cmp(&sa.latest))
                .then_with(|| a.cmp(b)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.cmp(b),
        }
    });

    let mut remaining: Vec<String> = names
        .into_iter()
        .filter(|name| !installed_set.contains(name))
        .collect();
    remaining.sort_by(|a, b| {
        let a_selected = selected_for_sort.as_deref() == Some(a.as_str());
        let b_selected = selected_for_sort.as_deref() == Some(b.as_str());
        if a_selected != b_selected {
            return b_selected.cmp(&a_selected);
        }
        match (usage_scores.get(a), usage_scores.get(b)) {
            (Some(sa), Some(sb)) => sb
                .score
                .total_cmp(&sa.score)
                .then_with(|| sb.latest.cmp(&sa.latest))
                .then_with(|| a.cmp(b)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.cmp(b),
        }
    });

    // A selected-but-missing definition is project state, not an installed
    // profile. Pin it before every available choice so recovery is obvious.
    if let Some(selected) = selected_name.as_ref()
        && !installed_names.contains(selected)
        && !remaining.contains(selected)
    {
        remaining.insert(0, selected.clone());
    }
    if let Some(selected) = selected_name.as_ref()
        && remaining.contains(selected)
        && !installed_names.contains(selected)
        && inspection.state == AssociationState::Unavailable
    {
        remaining.retain(|name| name != selected);
        installed_names.insert(0, selected.clone());
    }

    let mut ordered = installed_names;
    ordered.extend(remaining);
    let mut result = Vec::new();
    for name in ordered {
        let selected = selected_name.as_deref() == Some(name.as_str());
        let unavailable_selection = selected && inspection.state == AssociationState::Unavailable;
        let unready_selection = selected && inspection.state != AssociationState::Ready;
        let source = if unavailable_selection {
            ProfileSource::Unavailable
        } else {
            source_by_name
                .get(&name)
                .cloned()
                .unwrap_or(ProfileSource::Unavailable)
        };
        let readiness = if unavailable_selection {
            unavailable_readiness()
        } else {
            readiness_by_name
                .get(&name)
                .cloned()
                .unwrap_or_else(unavailable_readiness)
        };
        let usage_label = if unready_selection {
            None
        } else {
            usage_scores
                .get(&name)
                .map(|score| score.label.clone())
                .or_else(|| {
                    route_evidence
                        .contains(&name)
                        .then(|| "recent route".to_string())
                })
        };
        result.push(ProfileCatalogEntry {
            name: name.clone(),
            source,
            selected_for_project: selected,
            global_active: global_active.as_deref() == Some(name.as_str()),
            description: if unavailable_selection {
                None
            } else {
                description_by_name.get(&name).cloned().flatten()
            },
            fingerprint: if unavailable_selection {
                None
            } else {
                fingerprint_by_name.get(&name).cloned()
            },
            association_state: if selected {
                inspection.state.clone()
            } else {
                AssociationState::None
            },
            readiness,
            usage_label,
        });
    }
    Ok(result)
}

pub fn catalog(workgraph_dir: &Path) -> Result<Vec<ProfileCatalogEntry>> {
    catalog_at(workgraph_dir, Utc::now())
}

/// Advisory exclusive lock used only for mutating selection/history paths.
/// Read-only catalog/plan APIs never create a lock file.
struct ExclusiveFileLock {
    file: File,
}

impl ExclusiveFileLock {
    fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if rc != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("Failed to lock profile state");
            }
        }
        Ok(Self { file })
    }
}

impl Drop for ExclusiveFileLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::Write;
    use std::sync::{Arc, Barrier, Mutex};
    use tempfile::TempDir;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    struct TestEnv {
        _guard: std::sync::MutexGuard<'static, ()>,
        root: TempDir,
        global: PathBuf,
        history: PathBuf,
    }

    impl TestEnv {
        fn new() -> Self {
            let guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
            let root = tempfile::tempdir().unwrap();
            let global = root.path().join("global");
            let history = root.path().join("usage.jsonl");
            fs::create_dir_all(global.join("profiles")).unwrap();
            unsafe {
                std::env::set_var("WG_GLOBAL_DIR", &global);
                std::env::set_var("WG_PROFILE_USAGE_PATH", &history);
                std::env::set_var(
                    "WG_LAUNCHER_HISTORY_PATH",
                    root.path().join("launcher.jsonl"),
                );
            }
            Self {
                _guard: guard,
                root,
                global,
                history,
            }
        }

        fn project(&self, name: &str) -> PathBuf {
            let dir = self.root.path().join(name).join(".wg");
            fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn profile(&self, name: &str, route: &str) {
            named::save_raw(
                name,
                &format!(
                    "description = \"{}\"\n[agent]\nmodel = \"{}\"\n[dispatcher]\nmodel = \"{}\"\n[tiers]\nfast = \"{}\"\nstandard = \"{}\"\npremium = \"{}\"\n[models.default]\nmodel = \"{}\"\n[models.task_agent]\nmodel = \"{}\"\n[models.evaluator]\nmodel = \"{}\"\n",
                    name, route, route, route, route, route, route, route, route
                ),
            )
            .unwrap();
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("WG_GLOBAL_DIR");
                std::env::remove_var("WG_PROFILE_USAGE_PATH");
                std::env::remove_var("WG_LAUNCHER_HISTORY_PATH");
            }
        }
    }

    #[test]
    fn two_projects_select_different_reusable_profiles_without_global_mutation() {
        let env = TestEnv::new();
        env.profile("alpha", "claude:opus");
        env.profile("beta", "codex:gpt-5.6-sol");
        fs::write(
            env.global.join("config.toml"),
            "[agent]\nmodel = \"nex:base\"\n",
        )
        .unwrap();
        let global_before = fs::read(env.global.join("config.toml")).unwrap();
        let a = env.project("a");
        let b = env.project("b");

        apply_project_selection(&a, &plan_project_selection(&a, "alpha").unwrap()).unwrap();
        apply_project_selection(&b, &plan_project_selection(&b, "beta").unwrap()).unwrap();

        assert_eq!(Config::load_merged(&a).unwrap().agent.model, "claude:opus");
        assert_eq!(
            Config::load_merged(&b).unwrap().agent.model,
            "codex:gpt-5.6-sol"
        );
        let execution_a = crate::execution_selection::resolve(&a, None).unwrap();
        let execution_b = crate::execution_selection::resolve(&b, None).unwrap();
        assert_eq!(execution_a.route.as_deref(), Some("claude:opus"));
        assert_eq!(execution_b.route.as_deref(), Some("codex:gpt-5.6-sol"));
        assert!(matches!(
            execution_a.source,
            Some(crate::execution_selection::ExecutionSelectionSource::Profile { ref name, .. })
                if name == "alpha"
        ));
        assert!(matches!(
            execution_b.source,
            Some(crate::execution_selection::ExecutionSelectionSource::Profile { ref name, .. })
                if name == "beta"
        ));
        assert_eq!(
            fs::read(env.global.join("config.toml")).unwrap(),
            global_before
        );
        assert!(!env.global.join("active-profile").exists());
    }

    #[test]
    fn content_edit_fails_closed_until_explicit_reselection() {
        let env = TestEnv::new();
        env.profile("alpha", "claude:opus");
        let project = env.project("p");
        apply_project_selection(
            &project,
            &plan_project_selection(&project, "alpha").unwrap(),
        )
        .unwrap();
        env.profile("alpha", "codex:gpt-5.6-sol");
        let inspection = inspect_association(&project);
        assert_eq!(inspection.state, AssociationState::ContentDrift);
        let err = Config::load_merged(&project).unwrap_err().to_string();
        assert!(err.contains("changed after selection"), "{err}");
        assert!(err.contains("no global fallback"), "{err}");
        assert!(
            crate::execution_selection::resolve(&project, None)
                .unwrap_err()
                .to_string()
                .contains("changed after selection")
        );
        let blocked = Config::load_or_default(&project);
        assert_eq!(
            blocked.agent.model,
            "nex:__project-profile-selection-invalid__"
        );
        assert_eq!(blocked.effective_dispatcher_executor(), "native");
        assert!(blocked.llm_endpoints.endpoints.is_empty());

        apply_project_selection(
            &project,
            &plan_project_selection(&project, "alpha").unwrap(),
        )
        .unwrap();
        assert_eq!(
            Config::load_merged(&project).unwrap().agent.model,
            "codex:gpt-5.6-sol"
        );
    }

    #[test]
    fn deleted_or_renamed_definition_is_unavailable_not_reconstructed() {
        let env = TestEnv::new();
        env.profile("custom", "claude:opus");
        let project = env.project("p");
        apply_project_selection(
            &project,
            &plan_project_selection(&project, "custom").unwrap(),
        )
        .unwrap();
        fs::rename(
            env.global.join("profiles/custom.toml"),
            env.global.join("profiles/renamed.toml"),
        )
        .unwrap();
        assert_eq!(
            inspect_association(&project).state,
            AssociationState::Unavailable
        );
        assert!(
            Config::load_merged(&project)
                .unwrap_err()
                .to_string()
                .contains("unavailable")
        );
    }

    #[test]
    fn deleted_selected_starter_is_unavailable_not_silently_rebuilt_from_template() {
        let env = TestEnv::new();
        let project = env.project("p");
        let plan = plan_project_selection(&project, "claude").unwrap();
        apply_project_selection(&project, &plan).unwrap();
        fs::remove_file(env.global.join("profiles/claude.toml")).unwrap();
        let catalog = catalog_at(&project, Utc::now()).unwrap();
        assert_eq!(catalog[0].name, "claude");
        assert_eq!(catalog[0].source, ProfileSource::Unavailable);
        assert_eq!(catalog[0].association_state, AssociationState::Unavailable);
        assert!(catalog[0].fingerprint.is_none());
        assert!(!env.global.join("profiles/claude.toml").exists());
    }

    #[test]
    fn profile_names_cannot_escape_global_definition_directory() {
        let _env = TestEnv::new();
        for invalid in [
            "",
            "../secret",
            "/tmp/secret",
            "a/b",
            "a\\b",
            "line\nbreak",
            "..",
            ".",
        ] {
            assert!(named::profile_path(invalid).is_err(), "accepted {invalid}");
            assert!(named::save_raw(invalid, "[agent]\nmodel='claude:opus'\n").is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn canonical_symlink_alias_has_same_project_digest() {
        let env = TestEnv::new();
        let project = env.project("real");
        let alias = env.root.path().join("alias-wg");
        std::os::unix::fs::symlink(&project, &alias).unwrap();
        assert_eq!(
            project_digest(&project).unwrap(),
            project_digest(&alias).unwrap()
        );
    }

    #[test]
    fn dry_run_plan_is_redacted_and_writes_nothing() {
        let env = TestEnv::new();
        let project = env.project("p");
        let before: Vec<PathBuf> = fs::read_dir(&project)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        let plan = plan_project_selection_at(
            &project,
            "pi",
            DateTime::parse_from_rfc3339("2026-07-22T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap();
        assert!(plan.materializes_global_profile_definition);
        assert!(!plan.writes_global_config);
        assert!(!plan.writes_global_active_profile);
        let json = serde_json::to_string(&plan).unwrap();
        assert!(!json.contains(env.root.path().to_string_lossy().as_ref()));
        assert!(!json.contains("api_key"));
        assert!(!json.contains("endpoint URL"));
        assert!(json.contains("pi:openrouter/z-ai/glm-5.2"));
        let after: Vec<PathBuf> = fs::read_dir(&project)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(before, after);
        assert!(!env.global.join("profiles/pi.toml").exists());
        assert!(!env.history.exists());
    }

    #[test]
    fn selection_preimage_guard_rejects_concurrent_writer() {
        let env = TestEnv::new();
        env.profile("alpha", "claude:opus");
        env.profile("beta", "codex:gpt-5.6-sol");
        let project = env.project("p");
        let stale = plan_project_selection(&project, "alpha").unwrap();
        let current = plan_project_selection(&project, "beta").unwrap();
        apply_project_selection(&project, &current).unwrap();
        assert!(
            apply_project_selection(&project, &stale)
                .unwrap_err()
                .to_string()
                .contains("changed after planning")
        );
        assert_eq!(read_association(&project).unwrap().unwrap().profile, "beta");
    }

    #[test]
    fn selection_plan_rechecks_exact_profile_preimage_not_only_semantics() {
        let env = TestEnv::new();
        env.profile("alpha", "claude:opus");
        let project = env.project("p");
        let plan = plan_project_selection(&project, "alpha").unwrap();
        let path = env.global.join("profiles/alpha.toml");
        let original = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("# concurrent formatting edit\n{original}")).unwrap();
        assert_eq!(
            profile_content_fingerprint(&original).unwrap(),
            profile_content_fingerprint(&fs::read_to_string(&path).unwrap()).unwrap()
        );
        let err = apply_project_selection(&project, &plan)
            .unwrap_err()
            .to_string();
        assert!(err.contains("definition changed after planning"), "{err}");
        assert!(!association_path(&project).exists());
    }

    #[test]
    fn clear_plan_rejects_unsupported_version_without_writing() {
        let env = TestEnv::new();
        env.profile("alpha", "claude:opus");
        let project = env.project("p");
        apply_project_selection(
            &project,
            &plan_project_selection(&project, "alpha").unwrap(),
        )
        .unwrap();
        let before = fs::read(association_path(&project)).unwrap();
        let mut clear = plan_clear_project_selection(&project).unwrap();
        clear.version += 1;
        assert!(
            apply_clear_project_selection(&project, &clear)
                .unwrap_err()
                .to_string()
                .contains("Unsupported")
        );
        assert_eq!(fs::read(association_path(&project)).unwrap(), before);
    }

    fn record(name: &str, fingerprint: &str, timestamp: &str) -> ProfileUsageRecord {
        ProfileUsageRecord {
            profile: name.to_string(),
            profile_fingerprint: fingerprint.to_string(),
            timestamp: timestamp.to_string(),
            project_digest: format!("b3:{}", "1".repeat(64)),
            category: UsageEventCategory::TaskCreated,
        }
    }

    #[test]
    fn usage_score_decay_ties_and_content_changes_are_deterministic() {
        let now = DateTime::parse_from_rfc3339("2026-07-22T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let fingerprints = HashMap::from([
            ("alpha".to_string(), "b3:a".to_string()),
            ("beta".to_string(), "b3:b".to_string()),
            ("changed".to_string(), "b3:new".to_string()),
        ]);
        let records = vec![
            record("alpha", "b3:a", "2026-07-22T11:00:00Z"),
            record("beta", "b3:b", "2026-07-22T11:00:00Z"),
            record("changed", "b3:old", "2026-07-22T11:59:00Z"),
            record("alpha", "b3:a", "2026-06-22T12:00:00Z"),
        ];
        let scores = score_usage(&records, &fingerprints, now);
        assert!(scores["alpha"].score > scores["beta"].score);
        assert!(!scores.contains_key("changed"));
        assert_eq!(scores["alpha"].label, "used today");
        assert_eq!(scores["alpha"].latest, scores["beta"].latest);
    }

    #[test]
    fn malformed_and_truncated_history_is_ignored_and_retention_is_bounded() {
        let env = TestEnv::new();
        let fp = format!("b3:{}", "a".repeat(64));
        let mut file = File::create(&env.history).unwrap();
        writeln!(file, "not json").unwrap();
        writeln!(file, "{{\"profile\":\"truncated\"").unwrap();
        writeln!(
            file,
            "{}",
            serde_json::to_string(&record("ok", &fp, "2026-07-22T00:00:00Z")).unwrap()
        )
        .unwrap();
        assert_eq!(load_usage_from(&env.history).len(), 1);
        for i in 0..5 {
            record_usage_at_path(
                &env.history,
                record("ok", &fp, &format!("2026-07-22T00:00:0{}Z", i)),
                3,
            )
            .unwrap();
        }
        let loaded = load_usage_from(&env.history);
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].timestamp, "2026-07-22T00:00:02Z");
    }

    #[test]
    fn concurrent_usage_writers_lose_no_records() {
        let env = TestEnv::new();
        let path = env.history.clone();
        let barrier = Arc::new(Barrier::new(9));
        let mut threads = Vec::new();
        for i in 0..8 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                barrier.wait();
                record_usage_at_path(
                    &path,
                    record(
                        &format!("p{}", i),
                        &format!("b3:{:064x}", i + 1),
                        "2026-07-22T00:00:00Z",
                    ),
                    100,
                )
                .unwrap();
            }));
        }
        barrier.wait();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(load_usage_from(&path).len(), 8);
    }

    #[test]
    fn successful_events_only_record_ready_selected_fingerprint_and_clear() {
        let env = TestEnv::new();
        env.profile("alpha", "claude:opus");
        let project = env.project("p");
        assert!(
            !record_successful_event_at(&project, UsageEventCategory::TaskCreated, Utc::now())
                .unwrap()
        );
        let plan = plan_project_selection(&project, "alpha").unwrap();
        apply_project_selection(&project, &plan).unwrap();
        assert!(
            record_successful_event_at(&project, UsageEventCategory::TaskCreated, Utc::now())
                .unwrap()
        );
        let records = usage_records().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].profile_fingerprint, plan.profile_fingerprint);
        let json = serde_json::to_string(&records).unwrap();
        assert!(!json.contains(env.root.path().to_string_lossy().as_ref()));
        assert!(!json.contains("prompt"));
        assert!(!json.contains("command"));
        clear_usage_history().unwrap();
        assert!(usage_records().unwrap().is_empty());
    }

    #[test]
    fn catalog_pins_project_selection_then_ranks_frequency_with_quiet_labels() {
        let env = TestEnv::new();
        env.profile("alpha", "claude:opus");
        env.profile("beta", "codex:gpt-5.6-sol");
        env.profile("current", "nex:qwen3-coder");
        let project = env.project("p");
        let now = DateTime::parse_from_rfc3339("2026-07-22T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let plan = plan_project_selection_at(&project, "current", now).unwrap();
        apply_project_selection(&project, &plan).unwrap();
        let alpha_fp = profile_content_fingerprint(
            &fs::read_to_string(env.global.join("profiles/alpha.toml")).unwrap(),
        )
        .unwrap();
        let beta_fp = profile_content_fingerprint(
            &fs::read_to_string(env.global.join("profiles/beta.toml")).unwrap(),
        )
        .unwrap();
        for ts in [
            "2026-07-20T00:00:00Z",
            "2026-07-19T00:00:00Z",
            "2026-07-18T00:00:00Z",
        ] {
            record_usage_at_path(&env.history, record("beta", &beta_fp, ts), 100).unwrap();
        }
        record_usage_at_path(
            &env.history,
            record("alpha", &alpha_fp, "2026-07-01T00:00:00Z"),
            100,
        )
        .unwrap();
        let catalog = catalog_at(&project, now).unwrap();
        assert_eq!(catalog[0].name, "current");
        assert!(catalog[0].selected_for_project);
        assert_eq!(catalog[1].name, "beta");
        assert_eq!(catalog[1].usage_label.as_deref(), Some("frequent"));
        assert_eq!(catalog[2].name, "alpha");
        assert_eq!(catalog[2].usage_label.as_deref(), Some("recent"));
        assert!(catalog.iter().all(|entry| {
            entry
                .usage_label
                .as_deref()
                .map(|label| ["frequent", "recent", "used today", "recent route"].contains(&label))
                .unwrap_or(true)
        }));
    }

    #[test]
    fn legacy_active_pointer_and_launcher_route_never_invent_project_attribution() {
        let env = TestEnv::new();
        env.profile("alpha", "claude:opus");
        env.profile("beta", "codex:gpt-5.6-sol");
        named::set_active(Some("alpha")).unwrap();
        let project = env.project("p");
        let launcher_path = env.root.path().join("launcher.jsonl");
        let entry = crate::launcher_history::HistoryEntry {
            timestamp: Utc::now().to_rfc3339(),
            executor: "codex".to_string(),
            model: Some("codex:gpt-5.6-sol".to_string()),
            endpoint: None,
            source: "legacy".to_string(),
            project: Some("/private/path/must-not-migrate".to_string()),
        };
        fs::write(
            &launcher_path,
            format!("{}\n", serde_json::to_string(&entry).unwrap()),
        )
        .unwrap();

        let catalog = catalog_at(&project, Utc::now()).unwrap();
        assert!(catalog.iter().all(|item| !item.selected_for_project));
        assert!(
            catalog
                .iter()
                .find(|item| item.name == "alpha")
                .unwrap()
                .global_active
        );
        assert_eq!(
            catalog
                .iter()
                .find(|item| item.name == "beta")
                .unwrap()
                .usage_label
                .as_deref(),
            Some("recent route")
        );
        assert!(read_association(&project).unwrap().is_none());
        assert!(usage_records().unwrap().is_empty());
    }

    #[test]
    fn exact_routes_and_reasoning_cover_core_starters_without_secret_paths() {
        let env = TestEnv::new();
        let project = env.project("p");
        let catalog = catalog_at(&project, Utc::now()).unwrap();
        let by_name: BTreeMap<_, _> = catalog.iter().map(|entry| (&entry.name, entry)).collect();
        for name in ["pi", "codex", "claude", "nex", "opencode"] {
            assert!(by_name.contains_key(&name.to_string()), "missing {name}");
        }
        let pi = by_name[&"pi".to_string()];
        assert!(pi.readiness.strong_route.starts_with("pi:"));
        assert!(pi.readiness.weak_route.starts_with("nex:openrouter:"));
        let codex = by_name[&"codex".to_string()];
        assert_eq!(codex.readiness.strong_reasoning, Some(ReasoningLevel::High));
        assert_eq!(codex.readiness.weak_reasoning, Some(ReasoningLevel::Low));
        assert!(
            by_name[&"opencode".to_string()]
                .readiness
                .strong_route
                .starts_with("opencode:")
        );
        let json = serde_json::to_string(&catalog).unwrap();
        assert!(!json.contains("api_key_file"));
        assert!(!json.contains(env.root.path().to_string_lossy().as_ref()));
    }
}
