use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serial_test::serial;
use tempfile::TempDir;

// This target is the task's immutable validation entry point. Re-run the
// dedicated Eval/FLIP suites here so route-authority changes cannot pass while
// their real fake-handler execution coverage is omitted by the configured gate.
#[path = "integration_dedicated_pi_evaluation.rs"]
mod dedicated_evaluation_coverage;
#[path = "integration_deep_readonly_flip.rs"]
mod deep_flip_coverage;

fn project(tmp: &TempDir) -> (PathBuf, PathBuf) {
    let root = tmp.path().join("project");
    let graph = root.join(".wg");
    fs::create_dir_all(&graph).unwrap();
    worksgood::parser::save_graph(
        &worksgood::graph::WorkGraph::new(),
        &graph.join("graph.jsonl"),
    )
    .unwrap();
    (root, graph)
}

fn wg(home: &Path, graph: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wg"));
    command
        .current_dir(graph.parent().unwrap())
        .arg("--dir")
        .arg(graph)
        .args(args)
        .env("HOME", home);
    for key in [
        "WG_DIR",
        "WG_PROJECT_ROOT",
        "WG_TASK_ID",
        "WG_AGENT_ID",
        "WG_WORKER_CONTROL_MODE",
        "OPENROUTER_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
    ] {
        command.env_remove(key);
    }
    command.output().unwrap()
}

#[test]
fn setup_defaults_to_authoritative_project_file_without_machine_state() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (root, graph) = project(&tmp);
    let output = wg(
        &home,
        &graph,
        &[
            "setup",
            "--route",
            "pi",
            "--model",
            "pi:openai-codex:gpt-5.6-sol",
            "--yes",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let project_config = fs::read_to_string(root.join("worksgood.toml")).unwrap();
    assert!(project_config.contains("schema_version = 1"));
    assert!(project_config.contains("pi:openai-codex:gpt-5.6-sol"));
    assert!(!home.join(".wg/config.toml").exists());
    assert!(!home.join(".wg/active-profile").exists());
    assert!(!home.join(".pi").exists());
}

#[cfg(unix)]
#[test]
fn interactive_setup_announces_project_default_before_first_prompt() {
    if Command::new("script").arg("--version").output().is_err() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (root, graph) = project(&tmp);
    let command_line = format!(
        "{} --dir {} setup",
        env!("CARGO_BIN_EXE_wg"),
        graph.display()
    );
    let mut child = Command::new("script")
        .current_dir(&root)
        .args(["-qec", &command_line, "/dev/null"])
        .env("HOME", &home)
        .env_remove("WG_DIR")
        .env_remove("WG_PROJECT_ROOT")
        .env_remove("WG_TASK_ID")
        .env_remove("WG_AGENT_ID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(2));
    child.stdin.take().unwrap().write_all(b"\x03").unwrap();
    let output = child.wait_with_output().unwrap();
    let terminal = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(terminal.contains("Scope: project only"), "{terminal}");
    assert!(!home.join(".wg/config.toml").exists());
    assert!(!home.join(".wg/active-profile").exists());
}

#[test]
fn profile_select_and_deprecated_use_preserve_project_guardrails() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (root, graph) = project(&tmp);
    fs::write(
        root.join("worksgood.toml"),
        "schema_version = 1\n[dispatcher]\nmax_agents = 7\n[dispatcher.resource_management]\ndisk_sentinel_enabled = false\n",
    )
    .unwrap();
    let selected = wg(&home, &graph, &["profile", "select", "pi", "--no-reload"]);
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let after_select = fs::read_to_string(root.join("worksgood.toml")).unwrap();
    assert!(after_select.contains("max_agents = 7"));
    assert!(after_select.contains("disk_sentinel_enabled = false"));
    assert!(after_select.contains("[profile_origin]"));
    assert!(!home.join(".wg/config.toml").exists());
    assert!(!home.join(".wg/active-profile").exists());

    let shown = wg(&home, &graph, &["profile", "show"]);
    assert!(shown.status.success());
    let shown = String::from_utf8_lossy(&shown.stdout);
    assert!(shown.contains("Project selected profile: pi"), "{shown}");
    assert!(shown.contains("Source: project-profile-import"), "{shown}");
    assert!(
        shown.contains("project worksgood.toml is authoritative"),
        "{shown}"
    );
    assert!(!shown.contains("global config is authoritative"), "{shown}");

    let listed = wg(&home, &graph, &["profile", "list"]);
    assert!(listed.status.success());
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listed.contains("Project selection: pi (materialized)"),
        "{listed}"
    );

    let used = wg(&home, &graph, &["profile", "use", "pi", "--no-reload"]);
    assert!(used.status.success());
    assert!(String::from_utf8_lossy(&used.stderr).contains("deprecated"));
    assert_eq!(
        after_select,
        fs::read_to_string(root.join("worksgood.toml")).unwrap()
    );
    assert!(!home.join(".wg/config.toml").exists());
    assert!(!home.join(".wg/active-profile").exists());
}

#[test]
fn profile_pi_reset_weak_updates_only_the_selected_project_projection() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (root, graph) = project(&tmp);
    let selected = wg(&home, &graph, &["profile", "select", "pi", "--no-reload"]);
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let definition_path = home.join(".wg/profiles/pi.toml");
    let definition_before = fs::read(&definition_path).ok();

    let split = wg(
        &home,
        &graph,
        &[
            "profile",
            "pi",
            "--weak",
            "pi:openrouter:test/weak",
            "--no-reload",
        ],
    );
    assert!(
        split.status.success(),
        "{}",
        String::from_utf8_lossy(&split.stderr)
    );
    let split_config = worksgood::config::Config::load_merged(&graph).unwrap();
    let strong = split_config.strong_tier_spec().unwrap();
    assert_eq!(
        split_config.weak_tier_spec().as_deref(),
        Some("pi:openrouter:test/weak")
    );
    assert_ne!(
        split_config.weak_tier_spec().as_deref(),
        Some(strong.as_str())
    );
    let split_bytes = fs::read(root.join("worksgood.toml")).unwrap();

    let reset = wg(
        &home,
        &graph,
        &["profile", "pi", "--reset-weak", "--no-reload"],
    );
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    assert!(String::from_utf8_lossy(&reset.stdout).contains("reset → inherit strong"));
    let inherited = worksgood::config::Config::load_merged(&graph).unwrap();
    assert!(inherited.tiers.fast.is_none());
    assert_eq!(inherited.weak_tier_spec(), inherited.strong_tier_spec());
    assert_ne!(
        fs::read(root.join("worksgood.toml")).unwrap(),
        split_bytes,
        "the selected project's canonical revision must update immediately"
    );
    assert_eq!(
        fs::read(&definition_path).ok(),
        definition_before,
        "project-local tier edits must not mutate the reusable profile definition"
    );
    assert!(!home.join(".wg/config.toml").exists());
    assert!(!home.join(".wg/active-profile").exists());
}

#[test]
fn profile_pi_edits_never_collapse_historical_looking_explicit_routes() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    let profiles = home.join(".wg/profiles");
    fs::create_dir_all(&profiles).unwrap();
    let (_root, graph) = project(&tmp);
    let definition_path = profiles.join("pi.toml");
    fs::write(
        &definition_path,
        r#"description = "historical-looking but operator-owned"

[agent]
model = "pi:openrouter:test/strong-old"

[dispatcher]
model = "pi:openrouter:test/strong-old"

[tiers]
standard = "pi:openrouter:test/strong-old"
standard_reasoning = "high"
fast = "pi:openrouter:test/weak-old"
fast_reasoning = "low"
premium = "pi:openrouter:test/strong-old"
premium_reasoning = "xhigh"

[models.default]
model = "pi:openrouter:test/strong-old"
reasoning = "high"

[models.task_agent]
model = "pi:openrouter:test/strong-old"
reasoning = "high"

[models.evaluator]
model = "pi:openrouter:test/weak-old"
reasoning = "low"

[models.assigner]
model = "pi:openrouter:test/weak-old"
reasoning = "low"

[models.flip_inference]
model = "pi:openrouter:test/weak-old"
reasoning = "low"

[models.flip_comparison]
model = "pi:openrouter:test/weak-old"
reasoning = "low"

[models.reviewer]
model = "pi:openrouter:test/weak-old"
reasoning = "low"
"#,
    )
    .unwrap();

    let updated = wg(
        &home,
        &graph,
        &[
            "profile",
            "pi",
            "--strong",
            "pi:openrouter:test/strong-new",
            "--no-reload",
        ],
    );
    assert!(
        updated.status.success(),
        "{}",
        String::from_utf8_lossy(&updated.stderr)
    );
    let after_update = fs::read_to_string(&definition_path).unwrap();
    let parsed: worksgood::config::Config = toml::from_str(&after_update).unwrap();
    assert_eq!(
        parsed.tiers.standard.as_deref(),
        Some("pi:openrouter:test/strong-new")
    );
    assert_eq!(parsed.agent.model, "pi:openrouter:test/strong-old");
    assert_eq!(
        parsed.coordinator.model.as_deref(),
        Some("pi:openrouter:test/strong-old")
    );
    assert_eq!(
        parsed.tiers.premium.as_deref(),
        Some("pi:openrouter:test/strong-old")
    );
    assert_eq!(
        parsed
            .models
            .task_agent
            .as_ref()
            .and_then(|entry| entry.model.as_deref()),
        Some("pi:openrouter:test/strong-old")
    );
    for role in [
        worksgood::config::DispatchRole::Evaluator,
        worksgood::config::DispatchRole::Assigner,
        worksgood::config::DispatchRole::FlipInference,
        worksgood::config::DispatchRole::FlipComparison,
        worksgood::config::DispatchRole::Reviewer,
    ] {
        assert_eq!(
            parsed
                .models
                .get_role(role)
                .and_then(|entry| entry.model.as_deref()),
            Some("pi:openrouter:test/weak-old"),
            "explicit {role} route must survive a strong-tier edit"
        );
    }

    let reset = wg(
        &home,
        &graph,
        &["profile", "pi", "--reset-weak", "--no-reload"],
    );
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let after_reset: worksgood::config::Config =
        toml::from_str(&fs::read_to_string(&definition_path).unwrap()).unwrap();
    assert!(after_reset.tiers.fast.is_none());
    for role in [
        worksgood::config::DispatchRole::Evaluator,
        worksgood::config::DispatchRole::Assigner,
        worksgood::config::DispatchRole::FlipInference,
        worksgood::config::DispatchRole::FlipComparison,
        worksgood::config::DispatchRole::Reviewer,
    ] {
        assert_eq!(
            after_reset
                .models
                .get_role(role)
                .and_then(|entry| entry.model.as_deref()),
            Some("pi:openrouter:test/weak-old"),
            "reset-weak must not erase explicit {role} routing authority"
        );
    }
}

#[test]
fn profile_select_rejects_non_pi_qualified_and_invalid_inputs_without_writing() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (root, graph) = project(&tmp);

    for (args, expected) in [
        (
            vec!["profile", "select"],
            "Choose a Pi profile or pass --clear",
        ),
        (
            vec!["profile", "select", "codex", "--no-reload"],
            "not Pi-only",
        ),
        (
            vec![
                "profile",
                "select",
                "pi:openai-codex:gpt-5.6-sol",
                "--no-reload",
            ],
            "Profile 'pi:openai-codex:gpt-5.6-sol' not found",
        ),
        (
            vec!["profile", "select", "opencode:openrouter/x", "--no-reload"],
            "Invalid profile name",
        ),
        (
            vec!["profile", "select", "../pi", "--no-reload"],
            "Invalid profile name",
        ),
    ] {
        let output = wg(&home, &graph, &args);
        assert!(!output.status.success(), "unexpected success for {args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(expected), "{args:?}: {stderr}");
        assert!(!root.join("worksgood.toml").exists());
        assert!(!home.join(".wg/config.toml").exists());
        assert!(!home.join(".wg/active-profile").exists());
        assert!(!home.join(".pi").exists());
    }
}

#[test]
fn explicit_global_non_routing_write_warns_and_routing_rewrites_are_refused() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (root, graph) = project(&tmp);

    let global = wg(
        &home,
        &graph,
        &[
            "config",
            "set",
            "dispatcher.max_agents",
            "23",
            "--global",
            "--no-reload",
        ],
    );
    assert!(
        global.status.success(),
        "{}",
        String::from_utf8_lossy(&global.stderr)
    );
    let warning = String::from_utf8_lossy(&global.stderr);
    assert!(warning.contains("explicit --global"), "{warning}");
    assert!(warning.contains(".wg/config.toml"), "{warning}");
    assert!(
        warning.contains("inactive for project behavior"),
        "{warning}"
    );
    let report = String::from_utf8_lossy(&global.stdout);
    assert!(report.contains("legacy-global-inactive"), "{report}");
    assert!(
        fs::read_to_string(home.join(".wg/config.toml"))
            .unwrap()
            .contains("max_agents = 23")
    );
    assert!(!root.join("worksgood.toml").exists());

    let global_before = fs::read(home.join(".wg/config.toml")).unwrap();
    for args in [
        vec!["config", "set", "agent.model", "pi:test:model", "--global"],
        vec!["config", "set", "profile", "pi", "--global"],
        vec![
            "config",
            "set",
            "coordinator.model",
            "pi:test:model",
            "--global",
        ],
        vec![
            "config",
            "set",
            "execution.fallbacks",
            "pi:test:model",
            "--global",
        ],
        vec![
            "config",
            "set",
            "openrouter.default_model",
            "pi:test:model",
            "--global",
        ],
        vec!["model", "set-default", "anything", "--global"],
        vec!["model", "set", "task_agent", "pi:test:model", "--global"],
        vec![
            "setup",
            "--route",
            "pi",
            "--model",
            "pi:test:model",
            "--scope",
            "global",
            "--yes",
        ],
        vec![
            "setup",
            "--route",
            "pi",
            "--model",
            "pi:test:model",
            "--scope",
            "both",
            "--yes",
        ],
    ] {
        let output = wg(&home, &graph, &args);
        assert!(!output.status.success(), "unexpected success for {args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("WG-GLOBAL-CONFIG-WRITE-REFUSED"));
        assert_eq!(
            fs::read(home.join(".wg/config.toml")).unwrap(),
            global_before
        );
        assert!(!home.join(".wg/active-profile").exists());
    }

    // The legacy whole-file shortcut is another global routing write. It must
    // refuse even with --force, rather than copying an old graph-local route
    // into the inactive machine layer under a misleading "default" label.
    fs::write(
        graph.join("config.toml"),
        "[agent]\nmodel = 'pi:test:must-not-copy'\n",
    )
    .unwrap();
    let install = wg(&home, &graph, &["config", "--install-global", "--force"]);
    assert!(!install.status.success());
    assert!(String::from_utf8_lossy(&install.stderr).contains("WG-GLOBAL-CONFIG-WRITE-REFUSED"));
    assert_eq!(
        fs::read(home.join(".wg/config.toml")).unwrap(),
        global_before
    );
}

#[test]
fn service_model_compatibility_updates_project_authority_and_preserves_explicit_roles() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (root, graph) = project(&tmp);
    fs::write(
        root.join("worksgood.toml"),
        r#"schema_version = 1

[models.default]
model = "pi:test:route-a"
reasoning = "high"

[models.reviewer]
model = "pi:test:route-a"
reasoning = "low"
"#,
    )
    .unwrap();

    let started = wg(
        &home,
        &graph,
        &[
            "service",
            "start",
            "--model",
            "pi:test:route-b",
            "--max-agents",
            "0",
            "--no-coordinator-agent",
            "--no-supervise",
        ],
    );
    assert!(
        started.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&started.stdout),
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(String::from_utf8_lossy(&started.stderr).contains("selected the project default"));

    let reloaded = wg(
        &home,
        &graph,
        &[
            "service",
            "reload",
            "--model",
            "pi:test:route-c",
            "--max-agents",
            "0",
        ],
    );
    assert!(
        reloaded.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&reloaded.stdout),
        String::from_utf8_lossy(&reloaded.stderr)
    );
    let status = wg(&home, &graph, &["service", "status", "--json"]);
    let overall = wg(&home, &graph, &["status", "--json"]);
    let routes = wg(&home, &graph, &["config", "--models"]);
    let config = worksgood::config::Config::load_merged(&graph).unwrap();
    let service_state: serde_json::Value =
        serde_json::from_slice(&fs::read(graph.join("service/state.json")).unwrap()).unwrap();
    let effective_fingerprint = worksgood::service_identity::config_fingerprint(&config).unwrap();
    let bytes = fs::read_to_string(root.join("worksgood.toml")).unwrap();
    let stopped = wg(&home, &graph, &["service", "stop", "--force"]);

    assert!(status.status.success());
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["coordinator"]["model"], "pi:test:route-c");
    assert!(overall.status.success());
    let overall: serde_json::Value = serde_json::from_slice(&overall.stdout).unwrap();
    assert_eq!(
        overall["coordinator"]["configured_model"],
        "pi:test:route-c"
    );
    assert_eq!(
        overall["coordinator"]["effective_strong"]["route"],
        "pi:test:route-c"
    );
    assert_eq!(
        overall["coordinator"]["effective_weak"]["route"],
        "pi:test:route-c"
    );
    assert!(
        overall["coordinator"]["config_revision"]
            .as_str()
            .is_some_and(|revision| revision.starts_with("b3:"))
    );
    let routes = String::from_utf8_lossy(&routes.stdout);
    assert!(
        routes.contains("effective strong = pi:test:route-c"),
        "{routes}"
    );
    assert!(
        routes.contains("effective weak   = pi:test:route-c"),
        "{routes}"
    );
    assert!(routes.contains("active revision  = b3:"), "{routes}");
    assert_eq!(
        service_state["identity"]["config_fingerprint"], effective_fingerprint,
        "a combined route+capacity reload must refresh service identity to the loaded project generation"
    );
    assert_eq!(
        config
            .resolve_execution_route_for_role(worksgood::config::DispatchRole::TaskAgent)
            .unwrap()
            .route,
        "pi:test:route-c"
    );
    assert_eq!(
        config
            .resolve_execution_route_for_role(worksgood::config::DispatchRole::Reviewer)
            .unwrap()
            .route,
        "pi:test:route-a",
        "an explicit role selector equal to the old default must remain pinned"
    );
    assert!(bytes.contains("model = \"pi:test:route-c\""));
    assert!(bytes.contains("model = \"pi:test:route-a\""));
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
}

#[test]
fn project_default_route_setter_updates_the_effective_closed_projection() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let (root, graph) = project(&tmp);
    let setup = wg(
        &home,
        &graph,
        &[
            "setup",
            "--route",
            "pi",
            "--model",
            "pi:test:before",
            "--yes",
        ],
    );
    assert!(setup.status.success());
    let mut parsed: toml::Value = fs::read_to_string(root.join("worksgood.toml"))
        .unwrap()
        .parse()
        .unwrap();
    parsed
        .get_mut("models")
        .and_then(toml::Value::as_table_mut)
        .unwrap()
        .remove("default");
    let mut document = toml::to_string_pretty(&parsed).unwrap();
    document.push_str(
        r#"
[agent]
model = "claude:sonnet"

[dispatcher]
model = "codex:gpt-5.5"
"#,
    );
    fs::write(root.join("worksgood.toml"), document).unwrap();

    let changed = wg(
        &home,
        &graph,
        &[
            "config",
            "set",
            "agent.model",
            "pi:test:after",
            "--no-reload",
        ],
    );
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    let selected = worksgood::execution_selection::resolve(&graph, None).unwrap();
    assert_eq!(selected.route.as_deref(), Some("pi:test:after"));
    let config = worksgood::config::Config::load_merged(&graph).unwrap();
    assert_eq!(config.agent.model, "claude:sonnet");
    assert_eq!(config.coordinator.model.as_deref(), Some("codex:gpt-5.5"));
    assert_eq!(
        config
            .models
            .default
            .as_ref()
            .and_then(|role| role.model.as_deref()),
        Some("pi:test:after")
    );
    assert!(
        config
            .models
            .task_agent
            .as_ref()
            .and_then(|role| role.model.as_deref())
            .is_none()
    );
    assert_eq!(
        config
            .resolve_execution_route_for_role(worksgood::config::DispatchRole::TaskAgent)
            .unwrap()
            .route,
        "pi:test:after"
    );
}

#[test]
#[serial]
fn unified_project_route_authority_public_cli_smoke() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest_path = root.join("tests/smoke/manifest.toml");
    let manifest = worksgood::smoke::Manifest::load_from(&manifest_path).unwrap();
    let scenarios = manifest.scenarios_for_task("unify-project-route-authority");
    assert_eq!(
        scenarios.len(),
        1,
        "owned smoke must remain uniquely registered"
    );

    let prior = std::env::var_os("WG_UNIFIED_ROUTE_TEST_BIN");
    unsafe {
        std::env::set_var("WG_UNIFIED_ROUTE_TEST_BIN", env!("CARGO_BIN_EXE_wg"));
    }
    let report = worksgood::smoke::run_scenarios(&scenarios, manifest_path.parent().unwrap());
    unsafe {
        if let Some(value) = prior {
            std::env::set_var("WG_UNIFIED_ROUTE_TEST_BIN", value);
        } else {
            std::env::remove_var("WG_UNIFIED_ROUTE_TEST_BIN");
        }
    }

    assert!(!report.blocks_done(), "{}", report.render());
}

#[test]
fn route_authority_capability_subprocess_helper() {
    if std::env::var_os("WG_ROUTE_CAPABILITY_HELPER").is_none() {
        return;
    }

    let agent_dir = PathBuf::from(std::env::var_os("PI_CODING_AGENT_DIR").unwrap());
    let invoked = PathBuf::from(std::env::var_os("WG_MODEL_INVOKED").unwrap());
    use worksgood::executor_discovery::{PiCapabilityLane, pi_route_supported};

    assert!(!pi_route_supported("registered", "fixture", PiCapabilityLane::Worker).unwrap());
    assert!(
        !pi_route_supported("registered", "fixture", PiCapabilityLane::HermeticReview).unwrap()
    );
    fs::write(agent_dir.join("models.json"), b"registered now").unwrap();
    assert!(
        pi_route_supported("registered", "fixture", PiCapabilityLane::Worker).unwrap(),
        "registration revision must invalidate the cached worker rejection"
    );
    assert!(
        pi_route_supported("registered", "fixture", PiCapabilityLane::HermeticReview).unwrap(),
        "registration revision must invalidate the cached review rejection"
    );

    let mut claude = worksgood::config::Config::default();
    claude.authority_revision = Some("b3:claude-agency-revision".into());
    claude.models.default = Some(worksgood::config::RoleModelConfig {
        provider: None,
        model: Some("claude:sonnet".into()),
        tier: None,
        endpoint: None,
        reasoning: None,
    });
    for role in [
        worksgood::config::DispatchRole::Evaluator,
        worksgood::config::DispatchRole::FlipInference,
        worksgood::config::DispatchRole::FlipComparison,
    ] {
        let dispatch = worksgood::service::llm::resolve_agency_dispatch(&claude, role).unwrap();
        assert_eq!(dispatch.handler, worksgood::dispatch::ExecutorKind::Claude);
        assert_eq!(dispatch.raw_spec, "claude:sonnet");
        assert_eq!(dispatch.reasoning, None);
        assert_eq!(dispatch.role, Some(role));
        assert_eq!(dispatch.config_revision, "b3:claude-agency-revision");
        assert_eq!(dispatch.capability_lane, PiCapabilityLane::HermeticReview);
    }
    let claude_source = worksgood::graph::Task {
        id: "claude-agency-source".into(),
        title: "claude-agency-source".into(),
        ..Default::default()
    };
    let claude_flip = worksgood::eval_lifecycle::build_plan(
        &claude,
        &claude_source,
        ".flip-claude-agency-source",
        worksgood::eval_lifecycle::DispatchSelectionSource::ScaffoldConfig,
    )
    .unwrap();
    assert!(claude_flip.calls.iter().all(|call| {
        call.route == "claude:sonnet"
            && call.system.handler == "claude"
            && call.config_revision.as_deref() == Some("b3:claude-agency-revision")
    }));
    let codex_comparison = worksgood::service::llm::AgencyDispatch::from_pinned_route(
        "codex:gpt-5.5",
        None,
        worksgood::config::DispatchRole::FlipComparison,
        Some("b3:codex-agency-revision"),
    );
    assert_eq!(
        codex_comparison.handler,
        worksgood::dispatch::ExecutorKind::Codex
    );
    assert_eq!(codex_comparison.raw_spec, "codex:gpt-5.5");
    assert_eq!(codex_comparison.config_revision, "b3:codex-agency-revision");
    let claude_call = worksgood::service::llm::run_model_oneshot(
        &claude,
        "claude:sonnet",
        "MODEL ONESHOT ROUTE TEST",
        10,
    )
    .unwrap();
    assert_eq!(claude_call.text, "claude-handler-ok");
    let codex_call = worksgood::service::llm::run_model_oneshot(
        &claude,
        "codex:gpt-5.5",
        "MODEL ONESHOT ROUTE TEST",
        10,
    )
    .unwrap();
    assert_eq!(codex_call.text, "codex-handler-ok");

    let mut canonical_pi = worksgood::config::Config::default();
    canonical_pi.authority_revision = Some("b3:agency-revision-a".into());
    canonical_pi.models.default = Some(worksgood::config::RoleModelConfig {
        provider: None,
        model: Some("pi:registered:fixture".into()),
        tier: None,
        endpoint: None,
        reasoning: Some(worksgood::config::ReasoningLevel::Low),
    });
    for role in [
        worksgood::config::DispatchRole::Evaluator,
        worksgood::config::DispatchRole::FlipInference,
        worksgood::config::DispatchRole::FlipComparison,
    ] {
        let dispatch =
            worksgood::service::llm::resolve_agency_dispatch(&canonical_pi, role).unwrap();
        assert_eq!(dispatch.raw_spec, "pi:registered:fixture");
        assert_eq!(dispatch.role, Some(role));
        assert_eq!(dispatch.config_revision, "b3:agency-revision-a");
        assert_eq!(dispatch.capability_lane, PiCapabilityLane::HermeticReview);
    }
    let source = worksgood::graph::Task {
        id: "agency-source".into(),
        title: "agency-source".into(),
        ..Default::default()
    };
    let eval_plan = worksgood::eval_lifecycle::build_plan(
        &canonical_pi,
        &source,
        ".evaluate-agency-source",
        worksgood::eval_lifecycle::DispatchSelectionSource::ScaffoldConfig,
    )
    .unwrap();
    let flip_plan = worksgood::eval_lifecycle::build_plan(
        &canonical_pi,
        &source,
        ".flip-agency-source",
        worksgood::eval_lifecycle::DispatchSelectionSource::ScaffoldConfig,
    )
    .unwrap();
    assert_eq!(eval_plan.calls.len(), 1);
    assert_eq!(flip_plan.calls.len(), 2);
    for call in eval_plan.calls.iter().chain(&flip_plan.calls) {
        assert_eq!(
            call.config_revision.as_deref(),
            Some("b3:agency-revision-a")
        );
        assert_eq!(call.route, "pi:registered:fixture");
    }
    let mut later_pi = canonical_pi.clone();
    later_pi.authority_revision = Some("b3:agency-revision-b".into());
    let later_eval_plan = worksgood::eval_lifecycle::build_plan(
        &later_pi,
        &source,
        ".evaluate-agency-source",
        worksgood::eval_lifecycle::DispatchSelectionSource::ScaffoldConfig,
    )
    .unwrap();
    let later_flip_plan = worksgood::eval_lifecycle::build_plan(
        &later_pi,
        &source,
        ".flip-agency-source",
        worksgood::eval_lifecycle::DispatchSelectionSource::ScaffoldConfig,
    )
    .unwrap();
    assert_ne!(
        eval_plan.plan_hash, later_eval_plan.plan_hash,
        "a revision-only config change must invalidate the immutable Eval plan"
    );
    assert_ne!(
        flip_plan.plan_hash, later_flip_plan.plan_hash,
        "a revision-only config change must invalidate the immutable two-phase FLIP plan"
    );
    let first = worksgood::service::llm::resolve_agency_dispatch(
        &canonical_pi,
        worksgood::config::DispatchRole::FlipInference,
    )
    .unwrap();
    let second = worksgood::service::llm::resolve_agency_dispatch(
        &later_pi,
        worksgood::config::DispatchRole::FlipInference,
    )
    .unwrap();
    assert_ne!(first.binding_id(), second.binding_id());

    let mut pi_without_reasoning = worksgood::config::Config::default();
    pi_without_reasoning.models.default = Some(worksgood::config::RoleModelConfig {
        provider: None,
        model: Some("pi:registered:fixture".into()),
        tier: None,
        endpoint: None,
        reasoning: None,
    });
    let error = worksgood::service::llm::resolve_agency_dispatch(
        &pi_without_reasoning,
        worksgood::config::DispatchRole::Evaluator,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("WG-EXEC-REASONING-MISSING"));

    let mut unsupported = worksgood::config::Config::default();
    unsupported.models.default = Some(worksgood::config::RoleModelConfig {
        provider: None,
        model: Some("pi:unregistered:fixture".into()),
        tier: None,
        endpoint: None,
        reasoning: Some(worksgood::config::ReasoningLevel::Low),
    });
    for role in [
        worksgood::config::DispatchRole::Evaluator,
        worksgood::config::DispatchRole::FlipInference,
        worksgood::config::DispatchRole::FlipComparison,
    ] {
        let error =
            worksgood::service::llm::run_lightweight_llm_call(&unsupported, role, "prompt", 10)
                .unwrap_err();
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("WG-PI-PROVIDER-UNSUPPORTED"));
        assert!(diagnostic.contains("lane=hermetic-review"));
        assert!(
            !invoked.exists(),
            "capability rejection for {role} invoked the model"
        );
    }
}

#[test]
fn project_revision_is_pinned_into_the_exact_attempt_binding() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    let graph_dir = project.join(".wg");
    fs::create_dir_all(&graph_dir).unwrap();
    let mut task = worksgood::graph::Task {
        id: "revision-bound-attempt".into(),
        title: "revision-bound-attempt".into(),
        ..Default::default()
    };
    task.lifecycle.generation = 7;
    task.lifecycle.current_attempt = Some(worksgood::lifecycle::AttemptRef {
        id: "attempt-revision-a".into(),
        generation: 7,
        fence: 3,
        actor_id: "agent-1".into(),
        disposition: None,
    });

    let mut config = worksgood::config::Config::default();
    config.authority_revision = Some("b3:revision-a".into());
    let plan = worksgood::dispatch::plan_spawn(&task, &config, None, Some("claude:revision-model"))
        .unwrap();
    assert_eq!(plan.config_revision.as_deref(), Some("b3:revision-a"));
    let runtime_key = worksgood::attempt_runtime::AttemptRuntimeKey::current(&task).unwrap();
    worksgood::attempt_runtime::ensure_namespace(&graph_dir, &runtime_key).unwrap();
    let binding =
        worksgood::source_provider_recovery::persist_launch_binding(&graph_dir, &task, &plan)
            .unwrap();
    assert_eq!(binding.attempt_id, "attempt-revision-a");
    assert_eq!(binding.config_revision.as_deref(), Some("b3:revision-a"));

    let route_id = worksgood::service::HealthRouteKey::from_spawn_plan(&plan).id();
    let pinned_plan_id = worksgood::dispatch::spawn_plan_binding_id(&plan, &route_id);
    let mut later_revision = plan.clone();
    later_revision.config_revision = Some("b3:revision-b".into());
    assert_ne!(
        pinned_plan_id,
        worksgood::dispatch::spawn_plan_binding_id(&later_revision, &route_id),
        "changing only the project revision must invalidate the immutable attempt plan"
    );

    let mut graph = worksgood::graph::WorkGraph::new();
    graph.add_node(worksgood::graph::Node::Task(task));
    worksgood::parser::save_graph(&graph, graph_dir.join("graph.jsonl")).unwrap();
    let mut registry = worksgood::service::registry::AgentRegistry::new();
    let agent_id = registry.register_agent_with_model(
        std::process::id(),
        "revision-bound-attempt",
        "claude",
        "/tmp/revision-bound-output",
        Some("claude:revision-model"),
    );
    assert_eq!(agent_id, "agent-1");
    registry.save(&graph_dir).unwrap();

    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let output = wg(&home, &graph_dir, &["status", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        status["agents"]["active"][0]["config_revision"],
        "b3:revision-a"
    );
}

#[test]
fn assigned_target_covers_capability_cache_and_shared_dispatch() {
    let temp = TempDir::new().unwrap();
    let bin = temp.path().join("bin");
    let agent_dir = temp.path().join("agent");
    let invoked = temp.path().join("model-invoked");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&agent_dir).unwrap();
    let pi = bin.join("pi");
    fs::write(
        &pi,
        r#"#!/bin/sh
case "$*" in
  *--list-models*)
    printf 'provider model context max-out thinking images\n'
    if [ -f "$PI_CODING_AGENT_DIR/models.json" ]; then
      printf 'registered fixture 1K 1K no no\n'
    fi
    exit 0
    ;;
esac
: >"$WG_MODEL_INVOKED"
exit 19
"#,
    )
    .unwrap();
    let claude = bin.join("claude");
    fs::write(
        &claude,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"result\":\"claude-handler-ok\",\"usage\":{}}'\n",
    )
    .unwrap();
    let codex = bin.join("codex");
    fs::write(
        &codex,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"codex-handler-ok\"}}' '{\"type\":\"turn.completed\",\"usage\":{}}'\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for executable in [&pi, &claude, &codex] {
            fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "route_authority_capability_subprocess_helper",
            "--nocapture",
        ])
        .env("WG_ROUTE_CAPABILITY_HELPER", "1")
        .env("PI_CODING_AGENT_DIR", &agent_dir)
        .env("WG_MODEL_INVOKED", &invoked)
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "helper failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
