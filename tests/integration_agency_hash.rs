use workgraph::agency::{
    ComponentCategory, ContentRef, content_hash_agent, content_hash_component,
    content_hash_outcome, content_hash_role, content_hash_tradeoff, description_hash,
};

fn wg_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("could not get current exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("wg");
    assert!(path.exists(), "wg binary not found at {path:?}");
    path
}

#[test]
fn test_agency_hash_equals_agentbureau_agency_v1_2_4() {
    let fixture = include_str!("fixtures/agency-hash-equality.txt");

    for line in fixture.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (description, expected) = line
            .split_once('\t')
            .unwrap_or_else(|| panic!("fixture line must be tab-separated: {line:?}"));

        assert_eq!(description_hash(description), expected);
        assert_eq!(
            content_hash_component(
                description,
                &ComponentCategory::Novel,
                &ContentRef::Inline("local extension not hashed".into()),
            ),
            expected
        );
        assert_eq!(
            content_hash_outcome(description, &["local extension not hashed".into()]),
            expected
        );
        assert_eq!(
            content_hash_tradeoff(
                &["local extension not hashed".into()],
                &["another local extension not hashed".into()],
                description,
            ),
            expected
        );
    }
}

#[test]
fn test_agency_primitive_hash_ignores_wg_extension_fields() {
    let description = "Same federation primitive";

    assert_eq!(
        content_hash_component(
            description,
            &ComponentCategory::Novel,
            &ContentRef::Inline("inline content".into()),
        ),
        content_hash_component(
            description,
            &ComponentCategory::Translated,
            &ContentRef::File("skill.md".into()),
        )
    );
    assert_eq!(
        content_hash_outcome(description, &["unit tests pass".into()]),
        content_hash_outcome(description, &["integration tests pass".into()])
    );
    assert_eq!(
        content_hash_tradeoff(&["slow".into()], &[], description),
        content_hash_tradeoff(&[], &["incomplete".into()], description)
    );
}

#[test]
fn test_agency_hash_migration_rewrites_existing_yaml_and_is_idempotent() {
    let tmp = tempfile::TempDir::new().unwrap();
    let wg_dir = tmp.path().join(".wg");
    let agency_dir = wg_dir.join("agency");
    let components_dir = agency_dir.join("primitives/components");
    let outcomes_dir = agency_dir.join("primitives/outcomes");
    let tradeoffs_dir = agency_dir.join("primitives/tradeoffs");
    let roles_dir = agency_dir.join("cache/roles");
    let agents_dir = agency_dir.join("cache/agents");

    std::fs::create_dir_all(&components_dir).unwrap();
    std::fs::create_dir_all(&outcomes_dir).unwrap();
    std::fs::create_dir_all(&tradeoffs_dir).unwrap();
    std::fs::create_dir_all(&roles_dir).unwrap();
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(
        agency_dir.join("import-manifest.yaml"),
        r#"
source: test.csv
version: v0.1.0
imported_at: 2026-05-04T00:00:00Z
counts:
  role_components: 1
  desired_outcomes: 1
  trade_off_configs: 1
content_hash: old
"#,
    )
    .unwrap();

    std::fs::write(
        components_dir.join("old-component.yaml"),
        r#"
id: old-component
name: Component
description: Component federation description
category: novel
content: !inline local content
performance:
  task_count: 0
  avg_score: null
"#,
    )
    .unwrap();
    std::fs::write(
        outcomes_dir.join("old-outcome.yaml"),
        r#"
id: old-outcome
name: Outcome
description: Outcome federation description
success_criteria:
- Local criterion
performance:
  task_count: 0
  avg_score: null
"#,
    )
    .unwrap();
    std::fs::write(
        tradeoffs_dir.join("old-tradeoff.yaml"),
        r#"
id: old-tradeoff
name: Tradeoff
description: Tradeoff federation description
acceptable_tradeoffs:
- Slow
unacceptable_tradeoffs:
- Incomplete
performance:
  task_count: 0
  avg_score: null
"#,
    )
    .unwrap();
    std::fs::write(
        roles_dir.join("old-role.yaml"),
        r#"
id: old-role
name: Role
description: Role composition
component_ids:
- old-component
outcome_id: old-outcome
performance:
  task_count: 0
  avg_score: null
"#,
    )
    .unwrap();
    std::fs::write(
        agents_dir.join("old-agent.yaml"),
        r#"
id: old-agent
role_id: old-role
tradeoff_id: old-tradeoff
name: Agent
performance:
  task_count: 0
  avg_score: null
"#,
    )
    .unwrap();

    let output = std::process::Command::new(wg_binary())
        .arg("--dir")
        .arg(&wg_dir)
        .args(["agency", "migrate"])
        .output()
        .expect("run wg agency migrate");
    assert!(
        output.status.success(),
        "migration failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let component_id = description_hash("Component federation description");
    let outcome_id = description_hash("Outcome federation description");
    let tradeoff_id = description_hash("Tradeoff federation description");
    let role_id = content_hash_role(std::slice::from_ref(&component_id), &outcome_id);
    let agent_id = content_hash_agent(&role_id, &tradeoff_id);

    assert!(components_dir.join(format!("{component_id}.yaml")).exists());
    assert!(outcomes_dir.join(format!("{outcome_id}.yaml")).exists());
    assert!(tradeoffs_dir.join(format!("{tradeoff_id}.yaml")).exists());
    assert!(roles_dir.join(format!("{role_id}.yaml")).exists());
    assert!(agents_dir.join(format!("{agent_id}.yaml")).exists());
    assert!(!components_dir.join("old-component.yaml").exists());
    assert!(!roles_dir.join("old-role.yaml").exists());
    assert!(!agents_dir.join("old-agent.yaml").exists());

    let role_yaml = std::fs::read_to_string(roles_dir.join(format!("{role_id}.yaml"))).unwrap();
    assert!(role_yaml.contains(&component_id));
    assert!(role_yaml.contains(&outcome_id));
    let agent_yaml = std::fs::read_to_string(agents_dir.join(format!("{agent_id}.yaml"))).unwrap();
    assert!(agent_yaml.contains(&role_id));
    assert!(agent_yaml.contains(&tradeoff_id));
    let manifest_yaml = std::fs::read_to_string(agency_dir.join("import-manifest.yaml")).unwrap();
    assert!(manifest_yaml.contains("schema_version: agency-hash-v1.2.4-description-only"));

    let output = std::process::Command::new(wg_binary())
        .arg("--dir")
        .arg(&wg_dir)
        .args(["agency", "migrate"])
        .output()
        .expect("rerun wg agency migrate");
    assert!(
        output.status.success(),
        "second migration failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Nothing to migrate"),
        "second migration should be a no-op, got stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}
