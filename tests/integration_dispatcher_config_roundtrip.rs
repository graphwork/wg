#[test]
fn test_dispatcher_config_roundtrip() {
    let mut cfg = worksgood::config::Config::default();
    cfg.coordinator.max_agents = 42;
    let toml_str = toml::to_string_pretty(&cfg).unwrap();
    eprintln!("=== TOML ===");
    eprintln!("{}", toml_str);

    let reload: worksgood::config::Config = toml::from_str(&toml_str).unwrap();
    eprintln!(
        "=== RELOADED max_agents = {} ===",
        reload.coordinator.max_agents
    );
    assert_eq!(reload.coordinator.max_agents, 42);
}
