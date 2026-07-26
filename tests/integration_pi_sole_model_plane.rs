use tempfile::TempDir;
use worksgood::config::{Config, DispatchRole, ModelRegistryEntry, ReasoningLevel, Tier};
use worksgood::config_defaults::{RouteParams, SetupRoute, config_for_route};
use worksgood::execution_selection::{SelectionState, resolve};
use worksgood::service::executor::ExecutorRegistry;

#[test]
fn explicit_codex_route_selects_existing_native_adapter() {
    let dir = TempDir::new().unwrap();
    let adapter = ExecutorRegistry::new(dir.path())
        .load_config("codex")
        .expect("native Codex adapter is built in");
    assert_eq!(adapter.executor.executor_type, "codex");
    assert_eq!(adapter.executor.command, "codex");

    let selection = resolve(dir.path(), Some(("codex:gpt-native-opaque", true)))
        .expect("an explicit handler-first Codex route must be selectable");
    assert_eq!(selection.state, SelectionState::Selected);
    assert_eq!(selection.route.as_deref(), Some("codex:gpt-native-opaque"));
    let system = selection
        .system
        .expect("selected route has execution identity");
    assert_eq!(system.handler, "codex");
    assert_eq!(system.wire, "openai-codex-cli");
}

#[test]
fn direct_codex_config_keeps_opaque_identity_distinct_from_pi_codex() {
    let mut codex: Config = toml::from_str(worksgood::profile::named::STARTER_CODEX).unwrap();
    codex.models.task_agent.as_mut().unwrap().model = Some("codex:future/opaque:model-v9".into());
    codex.validate_execution_model_plane().unwrap();
    let worker = codex
        .resolve_execution_route_for_role(DispatchRole::TaskAgent)
        .unwrap();
    assert_eq!(worker.handler, "codex");
    assert_eq!(worker.route, "codex:future/opaque:model-v9");
    assert_eq!(worker.model, "future/opaque:model-v9");

    let pi = config_for_route(
        SetupRoute::Pi,
        RouteParams {
            model: Some("pi:openai-codex:future/opaque:model-v9".into()),
            ..Default::default()
        },
    );
    let pi_worker = pi
        .resolve_execution_route_for_role(DispatchRole::TaskAgent)
        .unwrap();
    assert_eq!(pi_worker.handler, "pi");
    assert_eq!(pi_worker.route, "pi:openai-codex:future/opaque:model-v9");
    assert_ne!(worker.route, pi_worker.route);
}

#[test]
fn explicit_claude_route_selects_existing_native_adapter() {
    let dir = TempDir::new().unwrap();
    let adapter = ExecutorRegistry::new(dir.path())
        .load_config("claude")
        .expect("native Claude adapter is built in");
    assert_eq!(adapter.executor.executor_type, "claude");
    assert_eq!(adapter.executor.command, "claude");

    let selection = resolve(dir.path(), Some(("claude:future/opaque:model-v9", true)))
        .expect("an explicit handler-first Claude route must be selectable");
    assert_eq!(selection.state, SelectionState::Selected);
    assert_eq!(
        selection.route.as_deref(),
        Some("claude:future/opaque:model-v9")
    );
    let system = selection
        .system
        .expect("selected route has execution identity");
    assert_eq!(system.handler, "claude");
    assert_eq!(system.wire, "anthropic-cli");
}

#[test]
fn direct_claude_config_keeps_exact_identity_distinct_from_pi_anthropic() {
    let mut claude: Config = toml::from_str(worksgood::profile::named::STARTER_CLAUDE).unwrap();
    claude.models.task_agent.as_mut().unwrap().model = Some("claude:future/opaque:model-v9".into());
    claude.validate_execution_model_plane().unwrap();
    let worker = claude
        .resolve_execution_route_for_role(DispatchRole::TaskAgent)
        .unwrap();
    assert_eq!(worker.handler, "claude");
    assert_eq!(worker.provider.as_deref(), Some("anthropic"));
    assert_eq!(worker.route, "claude:future/opaque:model-v9");
    assert_eq!(worker.model, "future/opaque:model-v9");

    let pi = config_for_route(
        SetupRoute::Pi,
        RouteParams {
            model: Some("pi:anthropic:future/opaque:model-v9".into()),
            ..Default::default()
        },
    );
    let pi_worker = pi
        .resolve_execution_route_for_role(DispatchRole::TaskAgent)
        .unwrap();
    assert_eq!(pi_worker.handler, "pi");
    assert_eq!(pi_worker.route, "pi:anthropic:future/opaque:model-v9");
    assert_ne!(worker.route, pi_worker.route);
}

#[test]
fn unregistered_exact_pi_route_is_dispatch_authority() {
    let dir = TempDir::new().unwrap();
    let route = "pi:future-provider:vendor/model-not-in-wg";
    let config = config_for_route(
        SetupRoute::Pi,
        RouteParams {
            model: Some(route.to_string()),
            ..Default::default()
        },
    );
    assert!(config.model_registry.is_empty());
    config.save(dir.path()).unwrap();

    let selection = resolve(dir.path(), None).unwrap();
    assert_eq!(selection.state, SelectionState::Selected);
    assert_eq!(selection.route.as_deref(), Some(route));
    let worker = config
        .resolve_pi_route_for_role(DispatchRole::TaskAgent)
        .unwrap();
    assert_eq!(worker.route, route);
    assert_eq!(worker.provider, "future-provider");
    assert_eq!(worker.model, "vendor/model-not-in-wg");
    assert_eq!(worker.reasoning, ReasoningLevel::High);
}

#[test]
fn legacy_registry_cannot_rewrite_or_authorize_pi_dispatch() {
    let route = "pi:future-provider:exact/model";
    let mut config = config_for_route(
        SetupRoute::Pi,
        RouteParams {
            model: Some(route.to_string()),
            ..Default::default()
        },
    );
    config.model_registry.push(ModelRegistryEntry {
        id: route.to_string(),
        provider: "legacy".into(),
        model: "silently-substituted".into(),
        tier: Tier::Standard,
        ..Default::default()
    });
    let resolved = config
        .resolve_pi_route_for_role(DispatchRole::TaskAgent)
        .unwrap();
    assert_eq!(resolved.route, route);
    assert_eq!(resolved.model, "exact/model");
}

#[test]
fn every_effective_role_is_exact_pi_with_visible_reasoning() {
    let config = config_for_route(SetupRoute::Pi, RouteParams::default());
    for role in std::iter::once(DispatchRole::Default).chain(DispatchRole::ALL.iter().copied()) {
        let route = config.resolve_pi_route_for_role(role).unwrap();
        assert!(route.route.starts_with("pi:"), "{role}: {}", route.route);
        assert!(!route.provider.is_empty());
        assert!(!route.model.is_empty());
        assert!(!route.reasoning.as_str().is_empty());
    }
    config.validate_pi_model_plane().unwrap();
}

#[test]
fn missing_non_pi_and_missing_reasoning_fail_closed() {
    let empty = Config::default();
    assert!(empty.validate_pi_model_plane().is_err());

    let mut non_pi = config_for_route(SetupRoute::Pi, RouteParams::default());
    non_pi.models.task_agent.as_mut().unwrap().model = Some("codex:gpt-x".into());
    let error = non_pi
        .resolve_pi_route_for_role(DispatchRole::TaskAgent)
        .unwrap_err()
        .to_string();
    assert!(error.contains("WG-PI-ROUTE-REQUIRED"), "{error}");

    let mut no_reasoning = config_for_route(SetupRoute::Pi, RouteParams::default());
    no_reasoning.models.task_agent.as_mut().unwrap().reasoning = None;
    no_reasoning.models.default.as_mut().unwrap().reasoning = None;
    no_reasoning.tiers.standard_reasoning = None;
    let error = no_reasoning
        .resolve_pi_route_for_role(DispatchRole::TaskAgent)
        .unwrap_err()
        .to_string();
    assert!(error.contains("WG-EXEC-REASONING-MISSING"), "{error}");
}
