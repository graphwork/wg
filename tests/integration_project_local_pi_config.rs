use serial_test::serial;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use worksgood::config::{Config, ConfigSource, DispatchRole};
use worksgood::execution_selection::{self, SelectionState};
use worksgood::project_config::projection_fingerprint;

struct EnvRestore {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvRestore {
    fn set_path(key: &'static str, value: &Path) -> Self {
        let previous = std::env::var_os(key);
        // SAFETY: every test in this file that changes process environment is
        // serialized with `serial_test`.
        unsafe { std::env::set_var(key, value) };
        Self { key, previous }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        // SAFETY: see `set_path`; the serial guard remains held through drop.
        unsafe {
            if let Some(previous) = self.previous.take() {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }
}

fn graph_fixture() -> (TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let graph = root.path().join(".wg");
    fs::create_dir(&graph).unwrap();
    (root, graph)
}

fn stale_global_fixture(global: &Path) {
    fs::create_dir_all(global).unwrap();
    fs::write(
        global.join("config.toml"),
        r#"
[agent]
model = "pi:global-provider:stale-model"

[dispatcher]
model = "pi:global-provider:stale-model"
max_agents = 99

[models.task_agent]
model = "pi:global-provider:stale-model"
reasoning = "high"

[secrets]
allow_plaintext = true

[auth]
claude_code_oauth_token = "must-not-enter-project-config"
"#,
    )
    .unwrap();
    fs::write(global.join("active-profile"), "stale-pi\n").unwrap();
}

#[test]
#[serial]
fn stale_global_route_is_inactive_but_graph_config_loads() {
    let (_root, graph) = graph_fixture();
    let global = tempfile::tempdir().unwrap();
    stale_global_fixture(global.path());
    let _global = EnvRestore::set_path("WG_GLOBAL_DIR", global.path());

    let (config, sources) = Config::load_with_sources(&graph).unwrap();
    assert!(config.agent.model.is_empty());
    assert_eq!(
        sources.get("dispatcher.max_agents"),
        Some(&ConfigSource::Default),
        "global non-routing values must not inherit"
    );
    assert!(
        sources
            .values()
            .all(|source| *source != ConfigSource::Global)
    );

    assert_eq!(
        execution_selection::resolve(&graph, None).unwrap().state,
        SelectionState::Unselected
    );
    let refusal = execution_selection::require(&graph, None, "wg service start")
        .unwrap_err()
        .to_string();
    assert!(refusal.contains("WG-EXEC-UNSELECTED"), "{refusal}");
    assert!(refusal.contains("Ignored legacy routing"), "{refusal}");
    assert!(refusal.contains("active-profile"), "{refusal}");
    assert!(!refusal.contains("must-not-enter-project-config"));
}

#[test]
#[serial]
fn project_file_route_wins_and_wg_auth_secret_sections_are_not_inherited() {
    let (root, graph) = graph_fixture();
    let global = tempfile::tempdir().unwrap();
    stale_global_fixture(global.path());
    let _global = EnvRestore::set_path("WG_GLOBAL_DIR", global.path());

    fs::write(
        root.path().join("worksgood.toml"),
        r#"
schema_version = 1

[agent]
model = "pi:project-provider:exact-model"

[dispatcher]
model = "pi:project-provider:exact-model"
max_agents = 4

[models.default]
model = "pi:project-provider:exact-model"
reasoning = "high"

[models.task_agent]
model = "pi:project-provider:exact-model"
reasoning = "high"
"#,
    )
    .unwrap();

    let (config, sources) = Config::load_with_sources(&graph).unwrap();
    assert_eq!(config.agent.model, "pi:project-provider:exact-model");
    assert_eq!(config.coordinator.max_agents, 4);
    assert_eq!(config.auth.claude_code_oauth_token, None);
    assert!(!config.secrets.allow_plaintext);
    assert!(config.llm_endpoints.endpoints.is_empty());
    assert_eq!(sources.get("agent.model"), Some(&ConfigSource::ProjectFile));
    assert_eq!(
        sources.get("dispatcher.max_agents"),
        Some(&ConfigSource::ProjectFile)
    );
    assert!(
        sources
            .values()
            .all(|source| *source != ConfigSource::Global)
    );

    let selected = execution_selection::resolve(&graph, None).unwrap();
    assert_eq!(selected.state, SelectionState::Selected);
    assert_eq!(
        selected.route.as_deref(),
        Some("pi:project-provider:exact-model")
    );
}

#[test]
#[serial]
fn materialized_profile_origin_is_copy_by_value_with_precise_sources() {
    let (root, graph) = graph_fixture();
    let global = tempfile::tempdir().unwrap();
    stale_global_fixture(global.path());
    let _global = EnvRestore::set_path("WG_GLOBAL_DIR", global.path());

    let mut payload = String::from(
        r#"
[agent]
model = "pi:project-provider:profile-model"

[dispatcher]
model = "pi:project-provider:profile-model"
max_agents = 7
"#,
    );
    for role in std::iter::once(DispatchRole::Default).chain(DispatchRole::ALL.iter().copied()) {
        payload.push_str(&format!(
            "\n[models.{role}]\nmodel = \"pi:project-provider:profile-model\"\nreasoning = \"high\"\n"
        ));
    }
    let payload_value: toml::Value = payload.parse().unwrap();
    let projection = projection_fingerprint(&payload_value);
    fs::write(
        root.path().join("worksgood.toml"),
        format!(
            "schema_version = 1\n\n[profile_origin]\nname = \"pi-team\"\ndefinition_fingerprint = \"b3:{}\"\nprojection_fingerprint = \"{}\"\n{}",
            "1".repeat(64),
            projection,
            payload
        ),
    )
    .unwrap();

    // No reusable profile definition exists, and the stale active pointer names
    // an unrelated definition. Runtime resolution must not reopen either.
    let (config, sources) = Config::load_with_sources(&graph).unwrap();
    assert_eq!(config.coordinator.max_agents, 7);
    assert_eq!(
        sources.get("agent.model"),
        Some(&ConfigSource::ProjectProfileImport)
    );
    assert_eq!(
        sources.get("models.task_agent.reasoning"),
        Some(&ConfigSource::ProjectProfileImport)
    );
    assert_eq!(
        sources.get("dispatcher.max_agents"),
        Some(&ConfigSource::ProjectFile),
        "profile origin covers only the closed route/reasoning projection"
    );

    let selected = execution_selection::resolve(&graph, None).unwrap();
    assert_eq!(
        selected.route.as_deref(),
        Some("pi:project-provider:profile-model")
    );
    let rendered = serde_json::to_value(selected.source).unwrap();
    assert_eq!(rendered["kind"], "profile");
    assert_eq!(rendered["name"], "pi-team");
    assert!(
        rendered["path"]
            .as_str()
            .unwrap()
            .ends_with("worksgood.toml")
    );
    assert_eq!(
        worksgood::service_identity::selected_profile_identity(&graph).unwrap(),
        (Some("pi-team".to_string()), Some(projection)),
        "service identity must use materialized origin rather than a global profile"
    );
}

#[test]
#[serial]
fn stale_materialized_profile_origin_fails_closed_without_definition_or_global_fallback() {
    let (root, graph) = graph_fixture();
    let global = tempfile::tempdir().unwrap();
    stale_global_fixture(global.path());
    let _global = EnvRestore::set_path("WG_GLOBAL_DIR", global.path());
    let original: toml::Value = "[agent]\nmodel='pi:project:before'\n".parse().unwrap();
    let stale_fingerprint = projection_fingerprint(&original);
    fs::write(
        root.path().join("worksgood.toml"),
        format!(
            "schema_version=1\n[profile_origin]\nname='pi-team'\ndefinition_fingerprint='b3:{}'\nprojection_fingerprint='{stale_fingerprint}'\n[agent]\nmodel='pi:project:after'\n",
            "3".repeat(64)
        ),
    )
    .unwrap();

    let error = execution_selection::resolve(&graph, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("WG-PROFILE-ORIGIN-DRIFT"), "{error}");
    assert!(!error.contains("global-provider"), "{error}");
    let blocked = Config::load_or_default(&graph);
    assert_ne!(blocked.agent.model, "pi:global-provider:stale-model");
    assert!(
        blocked
            .load_diagnostics
            .items
            .iter()
            .any(|item| item.severity == worksgood::config::ConfigLoadDiagnosticSeverity::Error)
    );
}

#[test]
#[serial]
fn legacy_project_local_pi_route_is_exclusive_and_labeled() {
    let (_root, graph) = graph_fixture();
    let global = tempfile::tempdir().unwrap();
    stale_global_fixture(global.path());
    let _global = EnvRestore::set_path("WG_GLOBAL_DIR", global.path());
    fs::write(
        graph.join("config.toml"),
        "[agent]\nmodel='pi:legacy-project:exact-model'\n",
    )
    .unwrap();

    let (config, sources) = Config::load_with_sources(&graph).unwrap();
    assert_eq!(config.agent.model, "pi:legacy-project:exact-model");
    assert_eq!(sources.get("agent.model"), Some(&ConfigSource::Local));
    assert!(
        config
            .load_diagnostics
            .items
            .iter()
            .any(|item| item.code == "legacy-project-source")
    );
    let selected = execution_selection::resolve(&graph, None).unwrap();
    assert_eq!(selected.state, SelectionState::Selected);
    assert_eq!(
        selected.route.as_deref(),
        Some("pi:legacy-project:exact-model")
    );
}

#[test]
#[serial]
fn invalid_legacy_profile_association_blocks_global_fallback() {
    let (_root, graph) = graph_fixture();
    let global = tempfile::tempdir().unwrap();
    stale_global_fixture(global.path());
    let _global = EnvRestore::set_path("WG_GLOBAL_DIR", global.path());
    let project_digest = worksgood::profile::project::project_digest(&graph).unwrap();
    fs::write(
        graph.join("profile-selection.json"),
        serde_json::json!({
            "version": 1,
            "profile": "definition-does-not-exist",
            "profile_fingerprint": format!("b3:{}", "2".repeat(64)),
            "selected_at": "2026-01-01T00:00:00Z",
            "project_digest": project_digest,
        })
        .to_string(),
    )
    .unwrap();

    let error = Config::load_merged(&graph).unwrap_err().to_string();
    assert!(error.contains("missing"), "{error}");
    assert!(!error.contains("global-provider"), "{error}");
    let blocked = Config::load_or_default(&graph);
    assert!(
        blocked
            .load_diagnostics
            .items
            .iter()
            .any(|item| item.severity == worksgood::config::ConfigLoadDiagnosticSeverity::Error)
    );
    assert_ne!(blocked.agent.model, "pi:global-provider:stale-model");
}

#[test]
#[serial]
fn worksgood_document_disables_legacy_project_route_instead_of_merging() {
    let (root, graph) = graph_fixture();
    fs::write(
        graph.join("config.toml"),
        "[agent]\nmodel='pi:legacy:must-not-win'\n[dispatcher]\nmax_agents=91\n",
    )
    .unwrap();
    fs::write(
        root.path().join("worksgood.toml"),
        "schema_version=1\n[dispatcher]\nmax_agents=3\n",
    )
    .unwrap();
    fs::write(graph.join("profile-selection.json"), "not valid json").unwrap();

    let (config, sources) = Config::load_with_sources(&graph).unwrap();
    assert!(config.agent.model.is_empty());
    assert_eq!(config.coordinator.max_agents, 3);
    assert_eq!(
        sources.get("dispatcher.max_agents"),
        Some(&ConfigSource::ProjectFile)
    );
    assert_eq!(
        execution_selection::resolve(&graph, None).unwrap().state,
        SelectionState::Unselected
    );
    assert_eq!(
        worksgood::service_identity::selected_profile_identity(&graph).unwrap(),
        (None, None),
        "inactive malformed legacy association must not affect service identity"
    );
}
