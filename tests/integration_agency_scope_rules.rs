//! Integration tests for the agency scope field + composition-rules.csv overlay.
//!
//! These tests cover:
//! - Composer biases primitive selection toward `scope=meta:evaluator` for `.evaluate-*` tasks.
//! - composition-rules.csv parser caps `max_role_components` etc. at assignment time.
//! - File-watch semantics: edits to composition-rules.csv are picked up without restart.
//! - Backwards-compat: primitives without typed `scope` still work (metadata fallback).

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tempfile::tempdir;
use workgraph::agency::{
    AccessControl, ComponentCategory, ContentRef, Lineage, PerformanceRecord, RoleComponent,
    composition_rules::{CompositionRule, CompositionRulesOverlay, load_composition_rules},
    filter_components_by_required_scope, required_scope_for_task,
};

/// Build a minimal RoleComponent for tests.
fn make_component(id: &str, scope: Option<&str>, metadata_scope: Option<&str>) -> RoleComponent {
    let mut metadata = HashMap::new();
    if let Some(s) = metadata_scope {
        metadata.insert("scope".to_string(), s.to_string());
    }
    RoleComponent {
        id: id.to_string(),
        name: id.to_string(),
        description: format!("Component {}", id),
        quality: 100,
        domain_specificity: 0,
        domain: vec![],
        scope: scope.map(str::to_string),
        origin_instance_id: None,
        parent_content_hash: None,
        category: ComponentCategory::Translated,
        content: ContentRef::Inline("test content".to_string()),
        performance: PerformanceRecord::default(),
        lineage: Lineage::default(),
        access_control: AccessControl::default(),
        domain_tags: vec![],
        metadata,
        former_agents: vec![],
        former_deployments: vec![],
    }
}

/// Core test from the validation criteria: composer biases evaluator-scoped
/// composition toward primitives with `scope=meta:evaluator`.
///
/// The composer reads `required_scope_for_task(".evaluate-foo") == "meta:evaluator"`,
/// then `filter_components_by_required_scope` returns only components matching
/// that scope. Components without scope still match (backward-compat fallback),
/// but components with a different explicit scope are filtered out.
#[test]
fn test_evaluator_composition_prefers_meta_evaluator_scope() {
    let components = vec![
        make_component("eval-comp-1", Some("meta:evaluator"), None),
        make_component("eval-comp-2", Some("meta:evaluator"), None),
        make_component("assign-comp", Some("meta:assigner"), None),
        make_component("evolve-comp", Some("meta:evolver"), None),
        make_component("task-comp", Some("task"), None),
    ];

    let required = required_scope_for_task(".evaluate-impl-something");
    assert_eq!(required, "meta:evaluator");

    let filtered = filter_components_by_required_scope(&components, required);

    let ids: Vec<&str> = filtered.iter().map(|c| c.id.as_str()).collect();
    assert!(
        ids.contains(&"eval-comp-1") && ids.contains(&"eval-comp-2"),
        "evaluator-scoped components must be selected; got {:?}",
        ids
    );
    assert!(
        !ids.contains(&"assign-comp"),
        "assigner-scoped component must be filtered out for .evaluate-* task; got {:?}",
        ids
    );
    assert!(
        !ids.contains(&"evolve-comp"),
        "evolver-scoped component must be filtered out for .evaluate-* task; got {:?}",
        ids
    );
    assert!(
        !ids.contains(&"task-comp"),
        "task-scoped component must be filtered out for .evaluate-* task; got {:?}",
        ids
    );
}

/// Backward-compat: primitives without the typed `scope` field but with
/// `metadata.scope` set must still be filtered correctly.
#[test]
fn test_metadata_scope_fallback_when_typed_field_absent() {
    let components = vec![
        // typed field absent, metadata says meta:evaluator
        make_component("legacy-eval", None, Some("meta:evaluator")),
        // typed field absent, metadata says meta:assigner
        make_component("legacy-assign", None, Some("meta:assigner")),
        // both absent — match-everything legacy behaviour
        make_component("legacy-untagged", None, None),
    ];

    let filtered = filter_components_by_required_scope(&components, "meta:evaluator");
    let ids: Vec<&str> = filtered.iter().map(|c| c.id.as_str()).collect();

    assert!(
        ids.contains(&"legacy-eval"),
        "metadata.scope=meta:evaluator must match required meta:evaluator; got {:?}",
        ids
    );
    assert!(
        ids.contains(&"legacy-untagged"),
        "untagged primitives must match everything (backward-compat); got {:?}",
        ids
    );
    assert!(
        !ids.contains(&"legacy-assign"),
        "metadata.scope=meta:assigner must NOT match required meta:evaluator; got {:?}",
        ids
    );
}

/// Backward-compat: primitives with NO scope at all (typed or metadata)
/// continue to match every required scope, so legacy stores keep working.
#[test]
fn test_legacy_primitives_default_to_match_all() {
    let components = vec![
        make_component("legacy-1", None, None),
        make_component("legacy-2", None, None),
    ];

    // Even for a meta scope, untagged components fall through (match-all).
    let filtered = filter_components_by_required_scope(&components, "meta:assigner");
    assert_eq!(filtered.len(), 2);
}

/// composition-rules.csv parser: parses a row with all 7 columns.
///
/// Schema (from agency v1.2.4):
///   agent_type,rule,max_role_components,max_desired_outcomes,max_trade_off_configs,all_projects,project_ids
#[test]
fn test_composition_rules_csv_parser_parses_full_row() {
    let dir = tempdir().unwrap();
    let csv_path = dir.path().join("composition-rules.csv");
    fs::write(
        &csv_path,
        "agent_type,rule,max_role_components,max_desired_outcomes,max_trade_off_configs,all_projects,project_ids\n\
         assigner,balanced,2,1,1,true,\n\
         evaluator,strict,3,1,1,true,\n\
         evolver,exploratory,5,2,1,false,proj-a;proj-b\n",
    )
    .unwrap();

    let overlay = load_composition_rules(&csv_path).expect("parser should succeed");
    assert_eq!(overlay.rules.len(), 3);

    let assigner_rule = overlay.rule_for("assigner").expect("assigner rule");
    assert_eq!(assigner_rule.rule, "balanced");
    assert_eq!(assigner_rule.max_role_components, Some(2));
    assert_eq!(assigner_rule.max_desired_outcomes, Some(1));
    assert_eq!(assigner_rule.max_trade_off_configs, Some(1));
    assert!(assigner_rule.all_projects);
    assert!(assigner_rule.project_ids.is_empty());

    let evolver_rule = overlay.rule_for("evolver").expect("evolver rule");
    assert!(!evolver_rule.all_projects);
    assert_eq!(evolver_rule.project_ids, vec!["proj-a", "proj-b"]);
}

/// composition-rules.csv parser: returns an empty overlay when file does not exist.
#[test]
fn test_composition_rules_missing_file_returns_empty_overlay() {
    let overlay = load_composition_rules(Path::new("/nonexistent/composition-rules.csv")).unwrap();
    assert!(overlay.rules.is_empty());
}

/// composition-rules cap: when a rule sets `max_role_components=2`, role with
/// more than 2 components is rejected by `rule.role_components_within_cap`.
#[test]
fn test_composition_rules_cap_enforces_max_role_components() {
    let rule = CompositionRule {
        agent_type: "assigner".to_string(),
        rule: "balanced".to_string(),
        max_role_components: Some(2),
        max_desired_outcomes: Some(1),
        max_trade_off_configs: Some(1),
        all_projects: true,
        project_ids: vec![],
    };

    assert!(rule.role_components_within_cap(0));
    assert!(rule.role_components_within_cap(1));
    assert!(rule.role_components_within_cap(2));
    assert!(!rule.role_components_within_cap(3));
    assert!(!rule.role_components_within_cap(7));
}

/// composition-rules cap: missing cap means no constraint (None = unlimited).
#[test]
fn test_composition_rules_cap_unlimited_when_none() {
    let rule = CompositionRule {
        agent_type: "evaluator".to_string(),
        rule: "open".to_string(),
        max_role_components: None,
        max_desired_outcomes: None,
        max_trade_off_configs: None,
        all_projects: true,
        project_ids: vec![],
    };
    assert!(rule.role_components_within_cap(99));
}

/// File-watch semantics: editing the file invalidates the mtime cache and
/// the next load returns the updated overlay.
#[test]
fn test_composition_rules_reload_after_edit() {
    let dir = tempdir().unwrap();
    let csv_path = dir.path().join("composition-rules.csv");
    fs::write(
        &csv_path,
        "agent_type,rule,max_role_components,max_desired_outcomes,max_trade_off_configs,all_projects,project_ids\n\
         assigner,balanced,5,1,1,true,\n",
    )
    .unwrap();

    let mut watcher = workgraph::agency::composition_rules::CompositionRulesWatcher::new(&csv_path);

    let v1 = watcher.current().clone();
    assert_eq!(
        v1.rule_for("assigner").unwrap().max_role_components,
        Some(5)
    );

    // Sleep briefly so mtime advances reliably across filesystems
    std::thread::sleep(std::time::Duration::from_millis(10));

    fs::write(
        &csv_path,
        "agent_type,rule,max_role_components,max_desired_outcomes,max_trade_off_configs,all_projects,project_ids\n\
         assigner,balanced,2,1,1,true,\n",
    )
    .unwrap();

    let v2 = watcher.current().clone();
    assert_eq!(
        v2.rule_for("assigner").unwrap().max_role_components,
        Some(2),
        "watcher must pick up edits without restart"
    );
}

/// File-watch semantics: when the file is created where there was none,
/// the watcher picks it up on the next read.
#[test]
fn test_composition_rules_watcher_picks_up_new_file() {
    let dir = tempdir().unwrap();
    let csv_path = dir.path().join("composition-rules.csv");

    let mut watcher = workgraph::agency::composition_rules::CompositionRulesWatcher::new(&csv_path);
    assert!(watcher.current().rules.is_empty());

    fs::write(
        &csv_path,
        "agent_type,rule,max_role_components,max_desired_outcomes,max_trade_off_configs,all_projects,project_ids\n\
         evaluator,strict,1,1,1,true,\n",
    )
    .unwrap();

    let v2 = watcher.current().clone();
    assert_eq!(v2.rules.len(), 1);
    assert!(v2.rule_for("evaluator").is_some());
}

/// Resolution path: agent_type is derived from the required scope so callers
/// can hand `meta:assigner` / `meta:evaluator` strings to the overlay directly.
#[test]
fn test_composition_rules_overlay_lookup_by_scope() {
    let overlay = CompositionRulesOverlay {
        rules: vec![CompositionRule {
            agent_type: "assigner".to_string(),
            rule: "balanced".to_string(),
            max_role_components: Some(2),
            max_desired_outcomes: Some(1),
            max_trade_off_configs: Some(1),
            all_projects: true,
            project_ids: vec![],
        }],
    };

    // Scope strings map to agent_type via the overlay helper.
    assert!(overlay.rule_for_scope("meta:assigner").is_some());
    assert!(overlay.rule_for_scope("meta:evaluator").is_none());
    assert!(overlay.rule_for_scope("task").is_none());
}
