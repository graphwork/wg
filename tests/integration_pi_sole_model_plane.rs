use tempfile::TempDir;
use worksgood::config::{Config, DispatchRole, ModelRegistryEntry, ReasoningLevel, Tier};
use worksgood::config_defaults::{RouteParams, SetupRoute, config_for_route};
use worksgood::execution_selection::{SelectionState, resolve};

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
    assert!(error.contains("WG-PI-REASONING-MISSING"), "{error}");
}
