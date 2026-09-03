//! Authoritative, source-owned project configuration.
//!
//! A WG graph may read project behavior from exactly one checked-in document:
//! `worksgood.toml`, next to the ordinary `.wg`/`.workgraph` control directory.
//! Machine-global WG configuration is deliberately outside this loader.  The
//! legacy graph-local files are handled by `Config` only when this document is
//! absent.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const PROJECT_CONFIG_FILE: &str = "worksgood.toml";
pub const PROJECT_CONFIG_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileOrigin {
    pub name: String,
    pub definition_fingerprint: String,
    pub projection_fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct LoadedProjectConfig {
    /// Configuration payload with document metadata removed, ready for
    /// deserialization as `Config`.
    pub value: toml::Value,
    pub path: PathBuf,
    pub fingerprint: String,
    pub profile_origin: Option<ProfileOrigin>,
}

/// Resolve the source-owned config path for an ordinary WG graph layout.
///
/// Non-standard graph directories intentionally have no guessed source root.
/// They continue through the legacy project-only compatibility reader until a
/// later project-root binding migration supplies an explicit root.
pub fn path_for_graph(workgraph_dir: &Path) -> Option<PathBuf> {
    let basename = workgraph_dir.file_name().and_then(|name| name.to_str());
    if !matches!(basename, Some(".wg") | Some(".workgraph")) {
        return None;
    }
    // Resolve symlinks before choosing the sibling. A logical `.wg` symlink
    // must never pair one graph with the checkout containing the link while
    // the graph itself lives under a different project root.
    let bound_graph = workgraph_dir
        .canonicalize()
        .unwrap_or_else(|_| workgraph_dir.to_path_buf());
    bound_graph
        .parent()
        .map(|root| root.join(PROJECT_CONFIG_FILE))
}

pub fn exists_for_graph(workgraph_dir: &Path) -> bool {
    path_for_graph(workgraph_dir).is_some_and(|path| path.is_file())
}

pub fn load_for_graph(workgraph_dir: &Path) -> Result<Option<LoadedProjectConfig>> {
    let Some(path) = path_for_graph(workgraph_dir) else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)
        .with_context(|| format!("Failed to read project config at {}", path.display()))?;
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("Project config at {} is not UTF-8", path.display()))?;
    let mut value: toml::Value = text
        .parse()
        .with_context(|| format!("Failed to parse project config at {}", path.display()))?;
    let table = value.as_table_mut().ok_or_else(|| {
        anyhow::anyhow!(
            "error[WG-CONFIG-UPGRADE-REQUIRED]: project config {} must be a TOML table",
            path.display()
        )
    })?;

    let schema_version = table
        .remove("schema_version")
        .and_then(|value| value.as_integer())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "error[WG-CONFIG-UPGRADE-REQUIRED]: project config {} must declare `schema_version = {PROJECT_CONFIG_SCHEMA_VERSION}`",
                path.display()
            )
        })?;
    if schema_version != i64::from(PROJECT_CONFIG_SCHEMA_VERSION) {
        bail!(
            "error[WG-CONFIG-UPGRADE-REQUIRED]: project config {} has schema_version={schema_version}; this WG supports schema_version={PROJECT_CONFIG_SCHEMA_VERSION}",
            path.display()
        );
    }

    let profile_origin = table
        .remove("profile_origin")
        .map(|origin| {
            origin.try_into().with_context(|| {
                format!(
                    "error[WG-CONFIG-UPGRADE-REQUIRED]: invalid [profile_origin] in {}",
                    path.display()
                )
            })
        })
        .transpose()?;

    validate_project_payload(&value, &path)?;
    if let Some(origin) = &profile_origin {
        validate_origin(origin, &value, &path)?;
    }

    Ok(Some(LoadedProjectConfig {
        value,
        path,
        fingerprint: digest_bytes(&bytes),
        profile_origin,
    }))
}

fn validate_origin(origin: &ProfileOrigin, value: &toml::Value, path: &Path) -> Result<()> {
    if origin.name.trim().is_empty()
        || !is_digest(&origin.definition_fingerprint)
        || !is_digest(&origin.projection_fingerprint)
    {
        bail!(
            "error[WG-CONFIG-UPGRADE-REQUIRED]: project config {} has malformed profile_origin metadata",
            path.display()
        );
    }
    let actual = projection_fingerprint(value);
    if actual != origin.projection_fingerprint {
        bail!(
            "error[WG-PROFILE-ORIGIN-DRIFT]: project config {} no longer matches its materialized profile projection (expected {}, found {}). Run `wg profile select {}` again or `wg profile select --clear`; no machine-global fallback was used.",
            path.display(),
            origin.projection_fingerprint,
            actual,
            origin.name
        );
    }

    let models = value
        .get("models")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "error[WG-PROFILE-PROJECTION-INCOMPLETE]: materialized profile {} in {} must contain an exact entry for every dispatch role",
                origin.name,
                path.display()
            )
        })?;
    for role in std::iter::once(crate::config::DispatchRole::Default)
        .chain(crate::config::DispatchRole::ALL.iter().copied())
    {
        let role_name = role.to_string();
        let entry = models
            .get(&role_name)
            .and_then(toml::Value::as_table)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "error[WG-PROFILE-PROJECTION-INCOMPLETE]: materialized profile {} in {} is missing models.{role_name}",
                    origin.name,
                    path.display()
                )
            })?;
        if !entry.contains_key("model")
            || (entry.contains_key("reasoning") == entry.contains_key("reasoning_mode"))
        {
            bail!(
                "error[WG-PROFILE-PROJECTION-INCOMPLETE]: materialized profile {} in {} requires models.{role_name}.model plus exactly one of reasoning or reasoning_mode",
                origin.name,
                path.display()
            );
        }
    }
    Ok(())
}

fn is_digest(value: &str) -> bool {
    value.len() == 67
        && value.starts_with("b3:")
        && value[3..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Validate the security-relevant boundary of a source-owned project file.
/// Pi provider login, endpoints, and secret backend policy remain owned by
/// their machine subsystems and can never enter the effective `Config` through
/// this document.
pub fn validate_project_payload(value: &toml::Value, path: &Path) -> Result<()> {
    let Some(root) = value.as_table() else {
        bail!("project config {} must be a TOML table", path.display());
    };

    for forbidden in [
        "auth",
        "secrets",
        "llm_endpoints",
        "native_executor",
        "openrouter",
        "model_registry",
        "tag_routing",
    ] {
        if root.contains_key(forbidden) {
            bail!(
                "error[WG-PROJECT-CONFIG-MACHINE-SETTING]: `{forbidden}` is machine-owned and is not allowed in {}; Pi owns provider authentication/endpoints and WG secret policy remains purpose-scoped",
                path.display()
            );
        }
    }
    for key in root.keys() {
        if !matches!(
            key.as_str(),
            "agent"
                | "dispatcher"
                | "project"
                | "help"
                | "agency"
                | "evaluation"
                | "log"
                | "replay"
                | "guardrails"
                | "viz"
                | "tui"
                | "checkpoint"
                | "worker_control"
                | "worktree_observer"
                | "pi_watchdog"
                | "models"
                | "execution"
                | "tiers"
                | "chat"
                | "bash"
                | "mcp"
                | "profile"
        ) {
            bail!(
                "error[WG-CONFIG-UPGRADE-REQUIRED]: unknown top-level project setting `{key}` in {}; upgrade WG before this setting can affect execution",
                path.display()
            );
        }
    }
    if root.contains_key("profile") {
        bail!(
            "error[WG-CONFIG-UPGRADE-REQUIRED]: legacy top-level `profile` is not runtime authority in {}; use [profile_origin] plus materialized exact Pi routes",
            path.display()
        );
    }
    if root
        .get("bash")
        .and_then(|value| value.get("path"))
        .is_some()
    {
        bail!(
            "error[WG-PROJECT-AUTHORIZATION-REQUIRED]: `bash.path` in {} requests a host executable, but no digest-bound operator authorization is active; refusing before spawn",
            path.display()
        );
    }

    reject_legacy_route_key(root, &["agent", "executor"], path)?;
    reject_legacy_route_key(root, &["dispatcher", "executor"], path)?;
    reject_legacy_route_key(root, &["dispatcher", "provider"], path)?;
    if root.contains_key("coordinator") {
        bail!(
            "error[WG-CONFIG-UPGRADE-REQUIRED]: use canonical [dispatcher] in {}, not legacy [coordinator]",
            path.display()
        );
    }

    check_route_at(root, &["agent", "model"], path)?;
    check_route_at(root, &["dispatcher", "model"], path)?;
    if root.contains_key("tiers") {
        bail!(
            "error[WG-CONFIG-UPGRADE-REQUIRED]: [tiers] in {} is an unresolved routing alias; flatten every dispatch role to an exact [models.<role>] Pi route",
            path.display()
        );
    }
    if let Some(models) = root.get("models").and_then(toml::Value::as_table) {
        for (role, entry) in models {
            role.parse::<crate::config::DispatchRole>().map_err(|_| {
                anyhow::anyhow!(
                    "error[WG-CONFIG-UPGRADE-REQUIRED]: unknown dispatch role models.{role} in {}; upgrade WG before this role can affect execution",
                    path.display()
                )
            })?;
            let entry = entry.as_table().ok_or_else(|| {
                anyhow::anyhow!(
                    "error[WG-CONFIG-UPGRADE-REQUIRED]: models.{role} in {} must be a table",
                    path.display()
                )
            })?;
            for field in entry.keys() {
                if !matches!(field.as_str(), "model" | "reasoning" | "reasoning_mode") {
                    bail!(
                        "error[WG-CONFIG-UPGRADE-REQUIRED]: unsupported project field models.{role}.{field} in {}; exact model plus one reasoning instruction are the closed profile projection",
                        path.display()
                    );
                }
            }
            if let Some(route) = entry.get("model") {
                let route = route.as_str().ok_or_else(|| {
                    anyhow::anyhow!(
                        "error[WG-PI-ROUTE-REQUIRED]: models.{role}.model in {} must be a string",
                        path.display()
                    )
                })?;
                require_exact_pi(&format!("models.{role}.model"), route, path)?;
            }
            let reasoning_mode = entry.get("reasoning_mode");
            if reasoning_mode.is_some() && entry.contains_key("reasoning") {
                bail!(
                    "error[WG-CONFIG-UPGRADE-REQUIRED]: models.{role} in {} cannot set both reasoning and reasoning_mode",
                    path.display()
                );
            }
            if let Some(mode) = reasoning_mode {
                if mode.as_str() != Some("provider-default") {
                    bail!(
                        "error[WG-CONFIG-UPGRADE-REQUIRED]: models.{role}.reasoning_mode in {} must be \"provider-default\"",
                        path.display()
                    );
                }
            }
        }
    }
    if let Some(servers) = root
        .get("mcp")
        .and_then(|value| value.get("servers"))
        .and_then(toml::Value::as_array)
    {
        for (index, server) in servers.iter().enumerate() {
            if let Some(env) = server
                .get("env")
                .and_then(toml::Value::as_table)
                .filter(|env| !env.is_empty())
            {
                let mut keys: Vec<_> = env.keys().cloned().collect();
                keys.sort();
                bail!(
                    "error[WG-PROJECT-CONFIG-INLINE-SECRET]: mcp.servers[{index}].env in {} contains inline environment keys [{}]; checked-in project config may request typed secret references but cannot contain environment values",
                    path.display(),
                    keys.join(", ")
                );
            }
        }
        if !servers.is_empty() {
            bail!(
                "error[WG-PROJECT-AUTHORIZATION-REQUIRED]: mcp.servers in {} request host processes, but no digest-bound operator authorization is active; refusing before spawn",
                path.display()
            );
        }
    }
    if let Some(fallbacks) = root
        .get("execution")
        .and_then(|value| value.get("fallbacks"))
        .and_then(toml::Value::as_array)
    {
        for (index, fallback) in fallbacks.iter().enumerate() {
            let Some(fallback) = fallback.as_table() else {
                continue;
            };
            if let Some(primary) = fallback.get("primary").and_then(toml::Value::as_str) {
                require_exact_pi(
                    &format!("execution.fallbacks[{index}].primary"),
                    primary,
                    path,
                )?;
            }
            if let Some(models) = fallback.get("models").and_then(toml::Value::as_array) {
                for (model_index, route) in
                    models.iter().filter_map(toml::Value::as_str).enumerate()
                {
                    require_exact_pi(
                        &format!("execution.fallbacks[{index}].models[{model_index}]"),
                        route,
                        path,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn reject_legacy_route_key(
    root: &toml::map::Map<String, toml::Value>,
    path: &[&str],
    document: &Path,
) -> Result<()> {
    if lookup(root, path).is_some() {
        bail!(
            "error[WG-PI-ROUTE-REQUIRED]: legacy selector `{}` is not allowed in {}; exact Pi model routes are the only project routing authority",
            path.join("."),
            document.display()
        );
    }
    Ok(())
}

fn check_route_at(
    root: &toml::map::Map<String, toml::Value>,
    path: &[&str],
    document: &Path,
) -> Result<()> {
    if let Some(value) = lookup(root, path) {
        let route = value.as_str().ok_or_else(|| {
            anyhow::anyhow!(
                "error[WG-PI-ROUTE-REQUIRED]: `{}` in {} must be a string",
                path.join("."),
                document.display()
            )
        })?;
        require_exact_pi(&path.join("."), route, document)?;
    }
    Ok(())
}

fn lookup<'a>(
    root: &'a toml::map::Map<String, toml::Value>,
    path: &[&str],
) -> Option<&'a toml::Value> {
    let mut value = root.get(*path.first()?)?;
    for segment in &path[1..] {
        value = value.as_table()?.get(*segment)?;
    }
    Some(value)
}

fn require_exact_pi(key: &str, route: &str, path: &Path) -> Result<()> {
    crate::config::parse_exact_pi_route(route).map(|_| ()).map_err(|error| {
        anyhow::anyhow!(
            "error[WG-PI-ROUTE-REQUIRED]: {key} in {} must be an exact `pi:<provider>:<model>` route: {error}",
            path.display()
        )
    })
}

/// Return the stable digest used to bind `[profile_origin]` to the narrow
/// materialized routing/reasoning projection. Profile-definition bytes are not
/// consulted at runtime.
pub fn projection_fingerprint(value: &toml::Value) -> String {
    let mut projection = serde_json::Map::new();
    let root = value.as_table();
    if let Some(root) = root {
        copy_projection_leaf(root, &["agent", "model"], &mut projection);
        copy_projection_leaf(root, &["dispatcher", "model"], &mut projection);
        if let Some(models) = root.get("models").and_then(toml::Value::as_table) {
            for (role, entry) in models {
                let Some(entry) = entry.as_table() else {
                    continue;
                };
                for field in ["model", "reasoning", "reasoning_mode"] {
                    if let Some(value) = entry.get(field) {
                        projection.insert(format!("models.{role}.{field}"), toml_to_json(value));
                    }
                }
            }
        }
    }
    digest_bytes(&canonical_json_bytes(&serde_json::Value::Object(
        projection,
    )))
}

fn copy_projection_leaf(
    root: &toml::map::Map<String, toml::Value>,
    path: &[&str],
    out: &mut serde_json::Map<String, serde_json::Value>,
) {
    if let Some(value) = lookup(root, path) {
        out.insert(path.join("."), toml_to_json(value));
    }
}

fn toml_to_json(value: &toml::Value) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

fn canonical_json_bytes(value: &serde_json::Value) -> Vec<u8> {
    fn write(value: &serde_json::Value, out: &mut Vec<u8>) {
        match value {
            serde_json::Value::Object(map) => {
                out.push(b'{');
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();
                for (index, key) in keys.into_iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    out.extend(serde_json::to_vec(key).unwrap_or_default());
                    out.push(b':');
                    write(&map[key], out);
                }
                out.push(b'}');
            }
            serde_json::Value::Array(values) => {
                out.push(b'[');
                for (index, item) in values.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    write(item, out);
                }
                out.push(b']');
            }
            _ => out.extend(serde_json::to_vec(value).unwrap_or_default()),
        }
    }
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("b3:{}", blake3::hash(bytes).to_hex())
}

/// Routing/reasoning leaves covered by a materialized profile origin.
pub fn is_profile_projection_key(key: &str) -> bool {
    matches!(key, "agent.model" | "dispatcher.model")
        || (key.starts_with("models.")
            && matches!(
                key.rsplit('.').next(),
                Some("model" | "reasoning" | "reasoning_mode")
            ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_path_is_sibling_of_control_dir_only() {
        assert_eq!(
            path_for_graph(Path::new("/tmp/demo/.wg")),
            Some(PathBuf::from("/tmp/demo/worksgood.toml"))
        );
        assert_eq!(path_for_graph(Path::new("/tmp/external-state")), None);
    }

    #[test]
    fn project_payload_rejects_machine_auth_and_native_routes() {
        let auth: toml::Value = toml::from_str("[auth]\nclaude_oauth_token = 'secret'").unwrap();
        assert!(validate_project_payload(&auth, Path::new("worksgood.toml")).is_err());
        let native: toml::Value = toml::from_str("[agent]\nmodel = 'claude:opus'").unwrap();
        assert!(validate_project_payload(&native, Path::new("worksgood.toml")).is_err());
    }

    #[test]
    fn project_payload_rejects_unknown_sections_and_inline_mcp_env() {
        let unknown: toml::Value = toml::from_str("[future_authority]\nenabled=true").unwrap();
        let error = validate_project_payload(&unknown, Path::new("worksgood.toml"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("WG-CONFIG-UPGRADE-REQUIRED"));

        let mcp: toml::Value =
            toml::from_str("[[mcp.servers]]\nname='x'\ncommand='x'\nenv={TOKEN='secret'}\n")
                .unwrap();
        let error = validate_project_payload(&mcp, Path::new("worksgood.toml"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("WG-PROJECT-CONFIG-INLINE-SECRET"));
        assert!(!error.contains("TOKEN='secret'"));
    }

    #[test]
    fn project_models_reject_unknown_roles_and_ambiguous_reasoning() {
        let unknown: toml::Value =
            toml::from_str("[models.future_role]\nmodel='pi:test:worker'").unwrap();
        let error = validate_project_payload(&unknown, Path::new("worksgood.toml"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("WG-CONFIG-UPGRADE-REQUIRED"));

        let ambiguous: toml::Value = toml::from_str(
            "[models.task_agent]\nmodel='pi:test:worker'\nreasoning='high'\nreasoning_mode='provider-default'",
        )
        .unwrap();
        let error = validate_project_payload(&ambiguous, Path::new("worksgood.toml"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot set both"));
    }

    #[test]
    fn project_host_process_requests_fail_closed_without_operator_ceiling() {
        let bash: toml::Value = toml::from_str("[bash]\npath='/tmp/hostile'").unwrap();
        let error = validate_project_payload(&bash, Path::new("worksgood.toml"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("WG-PROJECT-AUTHORIZATION-REQUIRED"));

        let mcp: toml::Value =
            toml::from_str("[[mcp.servers]]\nname='x'\ncommand='/tmp/hostile'\n").unwrap();
        let error = validate_project_payload(&mcp, Path::new("worksgood.toml"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("WG-PROJECT-AUTHORIZATION-REQUIRED"));
    }

    #[test]
    fn profile_origin_requires_a_closed_entry_for_every_role() {
        let value: toml::Value =
            toml::from_str("[models.default]\nmodel='pi:test:worker'\nreasoning='high'").unwrap();
        let origin = ProfileOrigin {
            name: "pi-team".to_string(),
            definition_fingerprint: format!("b3:{}", "1".repeat(64)),
            projection_fingerprint: projection_fingerprint(&value),
        };
        let error = validate_origin(&origin, &value, Path::new("worksgood.toml"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("WG-PROFILE-PROJECTION-INCOMPLETE"));
        assert!(error.contains("models.task_agent"));
    }

    #[test]
    fn projection_fingerprint_ignores_project_guardrails() {
        let a: toml::Value =
            toml::from_str("[agent]\nmodel='pi:test:one'\n[dispatcher]\nmax_agents=2\n").unwrap();
        let b: toml::Value =
            toml::from_str("[agent]\nmodel='pi:test:one'\n[dispatcher]\nmax_agents=99\n").unwrap();
        assert_eq!(projection_fingerprint(&a), projection_fingerprint(&b));
    }
}
