//! Integration tests for special agent composition at bootstrap.
//!
//! Verifies that `wg agency init` correctly composes Agent entities
//! for the four special agent types (assigner, evaluator, evolver, creator),
//! stores their hashes in config, and that the operation is idempotent.

use tempfile::TempDir;
use workgraph::agency;
use workgraph::config::Config;

/// Helper: run agency init on a fresh temp directory and return the WG dir path.
fn init_fresh() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    std::fs::create_dir_all(&wg_dir).unwrap();

    // Use the same init pathway as `wg agency init`
    let agency_dir = wg_dir.join("agency");
    agency::seed_starters(&agency_dir).unwrap();

    // Now run the full agency_init command (which also composes special agents + config)
    // We replicate the command logic since agency_init::run is in the binary crate.
    // Instead, we call the library functions directly.
    compose_special_agents_and_config(&wg_dir, &agency_dir);

    (tmp, wg_dir)
}

/// Replicate the agency_init logic for composing special agents and setting config.
/// This mirrors src/commands/agency_init.rs lines 23-192.
fn compose_special_agents_and_config(wg_dir: &std::path::Path, agency_dir: &std::path::Path) {
    let agents_dir = agency_dir.join("cache/agents");
    std::fs::create_dir_all(&agents_dir).unwrap();

    // Create default agent (Programmer + Careful)
    let roles = agency::starter_roles();
    let tradeoffs = agency::starter_tradeoffs();
    let programmer = roles.iter().find(|r| r.name == "Programmer").unwrap();
    let careful = tradeoffs.iter().find(|t| t.name == "Careful").unwrap();
    let default_id = agency::content_hash_agent(&programmer.id, &careful.id);
    let default_path = agents_dir.join(format!("{}.yaml", default_id));
    if !default_path.exists() {
        let agent = agency::Agent {
            id: default_id,
            role_id: programmer.id.clone(),
            tradeoff_id: careful.id.clone(),
            name: "Careful Programmer".to_string(),
            performance: agency::PerformanceRecord::default(),
            lineage: agency::Lineage::default(),
            capabilities: vec![],
            rate: None,
            capacity: None,
            trust_level: workgraph::graph::TrustLevel::default(),
            contact: None,
            executor: "claude".to_string(),
            preferred_model: None,
            preferred_provider: None,
            deployment_history: vec![],
            attractor_weight: 0.5,
            staleness_flags: vec![],
        };
        agency::save_agent(&agent, &agents_dir).unwrap();
    }

    // Compose special agents
    let special_roles = agency::special_agent_roles();
    let special_tradeoffs = agency::special_agent_tradeoffs();

    let special_agents: Vec<(&str, &str, &str)> = vec![
        ("Assigner", "Assigner Balanced", "Default Assigner"),
        ("Evaluator", "Evaluator Balanced", "Default Evaluator"),
        ("Evolver", "Evolver Balanced", "Default Evolver"),
        ("Agent Creator", "Creator Unconstrained", "Default Creator"),
    ];

    let mut special_agent_ids: Vec<(&str, String)> = Vec::new();

    for (role_name, tradeoff_name, agent_name) in &special_agents {
        let role = special_roles.iter().find(|r| r.name == *role_name).unwrap();
        let tradeoff = special_tradeoffs
            .iter()
            .find(|t| t.name == *tradeoff_name)
            .unwrap();

        let sa_id = agency::content_hash_agent(&role.id, &tradeoff.id);
        let sa_path = agents_dir.join(format!("{}.yaml", sa_id));

        if !sa_path.exists() {
            let agent = agency::Agent {
                id: sa_id.clone(),
                role_id: role.id.clone(),
                tradeoff_id: tradeoff.id.clone(),
                name: agent_name.to_string(),
                performance: agency::PerformanceRecord::default(),
                lineage: agency::Lineage::default(),
                capabilities: vec![],
                rate: None,
                capacity: None,
                trust_level: workgraph::graph::TrustLevel::default(),
                contact: None,
                executor: "claude".to_string(),
                preferred_model: None,
                preferred_provider: None,
                deployment_history: vec![],
                attractor_weight: 0.5,
                staleness_flags: vec![],
            };
            agency::save_agent(&agent, &agents_dir).unwrap();
        }

        special_agent_ids.push((role_name, sa_id));
    }

    // Set config
    let mut config = Config::load(wg_dir).unwrap_or_default();
    config.agency.auto_assign = true;
    config.agency.auto_evaluate = true;

    for (role_name, sa_id) in &special_agent_ids {
        match *role_name {
            "Assigner" => config.agency.assigner_agent = Some(sa_id.clone()),
            "Evaluator" => config.agency.evaluator_agent = Some(sa_id.clone()),
            "Evolver" => config.agency.evolver_agent = Some(sa_id.clone()),
            "Agent Creator" => config.agency.creator_agent = Some(sa_id.clone()),
            _ => {}
        }
    }
    config.save(wg_dir).unwrap();
}

// -----------------------------------------------------------------------
// Test 1: Four special agent entities exist in cache/agents/
// -----------------------------------------------------------------------
#[test]
fn test_special_agents_exist_in_cache() {
    let (_tmp, wg_dir) = init_fresh();
    let agents_dir = wg_dir.join("agency/cache/agents");

    // Should have exactly 5 agents: 1 default + 4 special
    let agent_files: Vec<_> = std::fs::read_dir(&agents_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "yaml"))
        .collect();
    assert_eq!(
        agent_files.len(),
        5,
        "Expected 5 agents (1 default + 4 special), got {}",
        agent_files.len()
    );

    // Load config and verify the 4 special agent hashes point to real files
    let config = Config::load(&wg_dir).unwrap();
    let special_hashes = vec![
        ("assigner_agent", config.agency.assigner_agent.as_ref()),
        ("evaluator_agent", config.agency.evaluator_agent.as_ref()),
        ("evolver_agent", config.agency.evolver_agent.as_ref()),
        ("creator_agent", config.agency.creator_agent.as_ref()),
    ];

    for (name, hash) in &special_hashes {
        let hash = hash.unwrap_or_else(|| panic!("{} should be set in config", name));
        let agent_path = agents_dir.join(format!("{}.yaml", hash));
        assert!(
            agent_path.exists(),
            "Agent file for {} ({}) should exist at {:?}",
            name,
            hash,
            agent_path
        );
    }

    // Verify all 4 special hashes are distinct
    let hash_set: std::collections::HashSet<&str> = special_hashes
        .iter()
        .filter_map(|(_, h)| h.map(|s| s.as_str()))
        .collect();
    assert_eq!(
        hash_set.len(),
        4,
        "All 4 special agent hashes should be distinct"
    );
}

// -----------------------------------------------------------------------
// Test 2: Each agent's role_id resolves to a role with correct component_ids
// -----------------------------------------------------------------------
#[test]
fn test_special_agent_roles_have_correct_components() {
    let (_tmp, wg_dir) = init_fresh();
    let agency_dir = wg_dir.join("agency");
    let agents_dir = agency_dir.join("cache/agents");
    let roles_dir = agency_dir.join("cache/roles");

    let config = Config::load(&wg_dir).unwrap();

    // Expected component counts from starters.rs definitions
    let expected_roles: Vec<(&str, &str, usize)> = vec![
        (
            "assigner_agent",
            "Assigner",
            agency::assigner_components().len(),
        ),
        (
            "evaluator_agent",
            "Evaluator",
            agency::evaluator_components().len(),
        ),
        (
            "evolver_agent",
            "Evolver",
            agency::evolver_components().len(),
        ),
        (
            "creator_agent",
            "Agent Creator",
            agency::creator_components().len(),
        ),
    ];

    // Get expected component IDs from starters
    let expected_assigner_ids: Vec<String> = agency::assigner_components()
        .iter()
        .map(|c| c.id.clone())
        .collect();
    let expected_evaluator_ids: Vec<String> = agency::evaluator_components()
        .iter()
        .map(|c| c.id.clone())
        .collect();
    let expected_evolver_ids: Vec<String> = agency::evolver_components()
        .iter()
        .map(|c| c.id.clone())
        .collect();
    let expected_creator_ids: Vec<String> = agency::creator_components()
        .iter()
        .map(|c| c.id.clone())
        .collect();

    for (config_key, expected_role_name, expected_comp_count) in &expected_roles {
        let agent_hash = match *config_key {
            "assigner_agent" => config.agency.assigner_agent.as_ref().unwrap(),
            "evaluator_agent" => config.agency.evaluator_agent.as_ref().unwrap(),
            "evolver_agent" => config.agency.evolver_agent.as_ref().unwrap(),
            "creator_agent" => config.agency.creator_agent.as_ref().unwrap(),
            _ => unreachable!(),
        };

        // Load the agent
        let agent_path = agents_dir.join(format!("{}.yaml", agent_hash));
        let agent = agency::load_agent(&agent_path).unwrap();

        // Load the role via the agent's role_id
        let role_path = roles_dir.join(format!("{}.yaml", agent.role_id));
        let role = agency::load_role(&role_path).unwrap();

        assert_eq!(
            role.name, *expected_role_name,
            "Agent {} should have role {}, got {}",
            config_key, expected_role_name, role.name
        );

        assert_eq!(
            role.component_ids.len(),
            *expected_comp_count,
            "Role {} should have {} components, got {}",
            role.name,
            expected_comp_count,
            role.component_ids.len()
        );

        // Verify component_ids match the starters definitions exactly
        let expected_ids = match *config_key {
            "assigner_agent" => &expected_assigner_ids,
            "evaluator_agent" => &expected_evaluator_ids,
            "evolver_agent" => &expected_evolver_ids,
            "creator_agent" => &expected_creator_ids,
            _ => unreachable!(),
        };

        let mut actual_sorted = role.component_ids.clone();
        actual_sorted.sort();
        let mut expected_sorted = expected_ids.clone();
        expected_sorted.sort();
        assert_eq!(
            actual_sorted, expected_sorted,
            "Role {} component_ids don't match starters definition",
            role.name
        );
    }
}

// -----------------------------------------------------------------------
// Test 3: Config keys are set to valid hashes
// -----------------------------------------------------------------------
#[test]
fn test_config_keys_set_to_valid_hashes() {
    let (_tmp, wg_dir) = init_fresh();
    let config = Config::load(&wg_dir).unwrap();

    let keys = [
        ("assigner_agent", &config.agency.assigner_agent),
        ("evaluator_agent", &config.agency.evaluator_agent),
        ("evolver_agent", &config.agency.evolver_agent),
        ("creator_agent", &config.agency.creator_agent),
    ];

    for (name, value) in &keys {
        let hash = value
            .as_ref()
            .unwrap_or_else(|| panic!("{} config key should be Some", name));

        // Valid content-hash: 64 hex chars (SHA-256)
        assert_eq!(
            hash.len(),
            64,
            "{} hash should be 64 chars (SHA-256), got {} chars: {}",
            name,
            hash.len(),
            hash
        );
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "{} hash should be all hex digits: {}",
            name,
            hash
        );

        // Verify the hash can be used to load an actual agent
        let agent_path = wg_dir.join(format!("agency/cache/agents/{}.yaml", hash));
        assert!(
            agent_path.exists(),
            "{} hash {} should point to existing agent file",
            name,
            hash
        );
        let agent = agency::load_agent(&agent_path).unwrap();
        assert_eq!(
            agent.id, *hash,
            "Loaded agent ID should match config hash for {}",
            name
        );
    }
}

// -----------------------------------------------------------------------
// Test 4: Re-running init is idempotent (no duplicates)
// -----------------------------------------------------------------------
#[test]
fn test_init_idempotent_no_duplicates() {
    let tmp = TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    std::fs::create_dir_all(&wg_dir).unwrap();
    let agency_dir = wg_dir.join("agency");

    // First run
    agency::seed_starters(&agency_dir).unwrap();
    compose_special_agents_and_config(&wg_dir, &agency_dir);

    let agents_dir = agency_dir.join("cache/agents");
    let count_after_first = std::fs::read_dir(&agents_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .count();
    let config_first = Config::load(&wg_dir).unwrap();

    // Second run
    agency::seed_starters(&agency_dir).unwrap();
    compose_special_agents_and_config(&wg_dir, &agency_dir);

    let count_after_second = std::fs::read_dir(&agents_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .count();
    let config_second = Config::load(&wg_dir).unwrap();

    // Same number of agents
    assert_eq!(
        count_after_first, count_after_second,
        "Agent count should not change on re-run: {} vs {}",
        count_after_first, count_after_second
    );

    // Same config hashes
    assert_eq!(
        config_first.agency.assigner_agent, config_second.agency.assigner_agent,
        "assigner_agent hash should be stable across runs"
    );
    assert_eq!(
        config_first.agency.evaluator_agent, config_second.agency.evaluator_agent,
        "evaluator_agent hash should be stable across runs"
    );
    assert_eq!(
        config_first.agency.evolver_agent, config_second.agency.evolver_agent,
        "evolver_agent hash should be stable across runs"
    );
    assert_eq!(
        config_first.agency.creator_agent, config_second.agency.creator_agent,
        "creator_agent hash should be stable across runs"
    );

    // Third run for good measure
    agency::seed_starters(&agency_dir).unwrap();
    compose_special_agents_and_config(&wg_dir, &agency_dir);

    let count_after_third = std::fs::read_dir(&agents_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .count();
    assert_eq!(
        count_after_first, count_after_third,
        "Agent count should remain stable after 3 runs"
    );
}

// -----------------------------------------------------------------------
// Test 5: All component_ids and outcome_ids resolve to existing primitives
// -----------------------------------------------------------------------
#[test]
fn test_all_component_and_outcome_ids_resolve() {
    let (_tmp, wg_dir) = init_fresh();
    let agency_dir = wg_dir.join("agency");
    let agents_dir = agency_dir.join("cache/agents");
    let roles_dir = agency_dir.join("cache/roles");
    let components_dir = agency_dir.join("primitives/components");
    let outcomes_dir = agency_dir.join("primitives/outcomes");

    let config = Config::load(&wg_dir).unwrap();

    let agent_hashes = [
        config.agency.assigner_agent.as_ref().unwrap(),
        config.agency.evaluator_agent.as_ref().unwrap(),
        config.agency.evolver_agent.as_ref().unwrap(),
        config.agency.creator_agent.as_ref().unwrap(),
    ];

    for agent_hash in &agent_hashes {
        // Load agent
        let agent_path = agents_dir.join(format!("{}.yaml", agent_hash));
        let agent = agency::load_agent(&agent_path).unwrap();

        // Load role
        let role_path = roles_dir.join(format!("{}.yaml", agent.role_id));
        let role = agency::load_role(&role_path).unwrap_or_else(|e| {
            panic!(
                "Role {} for agent {} should load: {}",
                agent.role_id, agent_hash, e
            )
        });

        // Verify each component_id resolves
        for comp_id in &role.component_ids {
            let comp_path = components_dir.join(format!("{}.yaml", comp_id));
            assert!(
                comp_path.exists(),
                "Component {} referenced by role {} should exist at {:?}",
                comp_id,
                role.name,
                comp_path
            );
            let comp = agency::load_component(&comp_path)
                .unwrap_or_else(|e| panic!("Component {} should be loadable: {}", comp_id, e));
            assert_eq!(comp.id, *comp_id, "Component ID should match filename");
        }

        // Verify outcome_id resolves
        assert!(
            !role.outcome_id.is_empty(),
            "Role {} should have a non-empty outcome_id",
            role.name
        );
        let outcome_path = outcomes_dir.join(format!("{}.yaml", role.outcome_id));
        assert!(
            outcome_path.exists(),
            "Outcome {} referenced by role {} should exist at {:?}",
            role.outcome_id,
            role.name,
            outcome_path
        );
        let outcome = agency::load_outcome(&outcome_path)
            .unwrap_or_else(|e| panic!("Outcome {} should be loadable: {}", role.outcome_id, e));
        assert_eq!(
            outcome.id, role.outcome_id,
            "Outcome ID should match filename"
        );
    }
}

// -----------------------------------------------------------------------
// Test: Agent tradeoff_ids also resolve to existing primitives
// -----------------------------------------------------------------------
#[test]
fn test_special_agent_tradeoff_ids_resolve() {
    let (_tmp, wg_dir) = init_fresh();
    let agency_dir = wg_dir.join("agency");
    let agents_dir = agency_dir.join("cache/agents");
    let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");

    let config = Config::load(&wg_dir).unwrap();

    let agent_hashes = [
        ("assigner", config.agency.assigner_agent.as_ref().unwrap()),
        ("evaluator", config.agency.evaluator_agent.as_ref().unwrap()),
        ("evolver", config.agency.evolver_agent.as_ref().unwrap()),
        ("creator", config.agency.creator_agent.as_ref().unwrap()),
    ];

    for (label, agent_hash) in &agent_hashes {
        let agent_path = agents_dir.join(format!("{}.yaml", agent_hash));
        let agent = agency::load_agent(&agent_path).unwrap();

        let tradeoff_path = tradeoffs_dir.join(format!("{}.yaml", agent.tradeoff_id));
        assert!(
            tradeoff_path.exists(),
            "Tradeoff {} for {} agent should exist",
            agent.tradeoff_id,
            label
        );
        let tradeoff = agency::load_tradeoff(&tradeoff_path).unwrap();
        assert_eq!(tradeoff.id, agent.tradeoff_id);
    }
}

// -----------------------------------------------------------------------
// Test: Agent IDs are deterministic content hashes of (role_id, tradeoff_id)
// -----------------------------------------------------------------------
#[test]
fn test_agent_ids_are_content_hashes() {
    let (_tmp, wg_dir) = init_fresh();
    let agency_dir = wg_dir.join("agency");
    let agents_dir = agency_dir.join("cache/agents");

    let config = Config::load(&wg_dir).unwrap();
    let special_roles = agency::special_agent_roles();
    let special_tradeoffs = agency::special_agent_tradeoffs();

    let expected: Vec<(&str, &str, &str)> = vec![
        ("assigner_agent", "Assigner", "Assigner Balanced"),
        ("evaluator_agent", "Evaluator", "Evaluator Balanced"),
        ("evolver_agent", "Evolver", "Evolver Balanced"),
        ("creator_agent", "Agent Creator", "Creator Unconstrained"),
    ];

    for (config_key, role_name, tradeoff_name) in &expected {
        let role = special_roles.iter().find(|r| r.name == *role_name).unwrap();
        let tradeoff = special_tradeoffs
            .iter()
            .find(|t| t.name == *tradeoff_name)
            .unwrap();

        let expected_hash = agency::content_hash_agent(&role.id, &tradeoff.id);
        let actual_hash = match *config_key {
            "assigner_agent" => config.agency.assigner_agent.as_ref().unwrap(),
            "evaluator_agent" => config.agency.evaluator_agent.as_ref().unwrap(),
            "evolver_agent" => config.agency.evolver_agent.as_ref().unwrap(),
            "creator_agent" => config.agency.creator_agent.as_ref().unwrap(),
            _ => unreachable!(),
        };

        assert_eq!(
            *actual_hash, expected_hash,
            "{} config hash should match content_hash_agent(role={}, tradeoff={})",
            config_key, role_name, tradeoff_name
        );

        // Also verify the agent file matches
        let agent_path = agents_dir.join(format!("{}.yaml", actual_hash));
        let agent = agency::load_agent(&agent_path).unwrap();
        assert_eq!(agent.id, expected_hash);
        assert_eq!(agent.role_id, role.id);
        assert_eq!(agent.tradeoff_id, tradeoff.id);
    }
}
