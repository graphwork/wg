use std::fs;
use std::path::Path;

use workgraph::agency::{
    ComponentCategory, ContentRef, DesiredOutcome, Lineage, PerformanceRecord, RoleComponent,
    TradeoffConfig,
};

fn empty_performance_yaml() -> &'static str {
    "performance:\n  task_count: 0\n  avg_score: null\n"
}

#[test]
fn test_primitive_loads_quality_domain_scope() {
    let yaml = format!(
        "id: component-1\n\
         name: Schema aware component\n\
         description: Accepts agency primitive schema metadata.\n\
         quality: '91'\n\
         domain_specificity: '42'\n\
         domain:\n\
         - software\n\
         - analysis\n\
         scope: meta:evaluator\n\
         origin_instance_id: 018f3e10-0000-7000-8000-000000000000\n\
         parent_content_hash: parent-1\n\
         category: novel\n\
         content: !inline Accepts agency primitive schema metadata.\n\
         {}\
         lineage:\n\
         \x20\x20parent_ids: []\n\
         \x20\x20generation: 1\n\
         \x20\x20created_by: evolver-run-123\n\
         \x20\x20created_at: 2026-05-04T00:00:00Z\n\
         \x20\x20reframing_potential: 0.25\n\
         access_control:\n\
         \x20\x20owner: local\n\
         \x20\x20policy: open\n",
        empty_performance_yaml()
    );

    let component: RoleComponent = serde_yaml::from_str(&yaml).unwrap();

    assert_eq!(component.quality, 91);
    assert_eq!(component.domain_specificity, 42);
    assert_eq!(component.domain, vec!["software", "analysis"]);
    assert_eq!(component.scope.as_deref(), Some("meta:evaluator"));
    assert_eq!(
        component.origin_instance_id.as_deref(),
        Some("018f3e10-0000-7000-8000-000000000000")
    );
    assert_eq!(component.parent_content_hash.as_deref(), Some("parent-1"));
    assert_eq!(component.lineage.created_by, "evolver");
    assert_eq!(component.lineage.reframing_potential, Some(0.25));
}

#[test]
fn test_missing_agency_schema_fields_default() {
    let yaml = format!(
        "id: outcome-1\n\
         name: Legacy outcome\n\
         description: Loads without agency v1.2.4 metadata.\n\
         success_criteria: []\n\
         {}\
         lineage:\n\
         \x20\x20created_by: bundled-starter-v0.1.0\n\
         \x20\x20created_at: 2026-05-04T00:00:00Z\n\
         access_control:\n\
         \x20\x20owner: local\n\
         \x20\x20policy: open\n",
        empty_performance_yaml()
    );

    let outcome: DesiredOutcome = serde_yaml::from_str(&yaml).unwrap();

    assert_eq!(outcome.quality, 100);
    assert_eq!(outcome.domain_specificity, 0);
    assert!(outcome.domain.is_empty());
    assert!(outcome.scope.is_none());
    assert_eq!(outcome.lineage.created_by, "import");
}

#[test]
fn test_agency_schema_fields_serialize_deserialize_byte_equal() {
    let tradeoff = TradeoffConfig {
        id: "tradeoff-1".to_string(),
        name: "Schema fields".to_string(),
        description: "Round-trips non-default agency schema fields.".to_string(),
        quality: 97,
        domain_specificity: 58,
        domain: vec!["strategy".to_string(), "management".to_string()],
        scope: Some("meta:assigner".to_string()),
        origin_instance_id: Some("018f3e10-0000-7000-8000-000000000001".to_string()),
        parent_content_hash: Some("parent-hash".to_string()),
        acceptable_tradeoffs: vec!["More metadata".to_string()],
        unacceptable_tradeoffs: vec!["Losing provenance".to_string()],
        performance: PerformanceRecord::default(),
        lineage: Lineage {
            parent_ids: vec!["parent-hash".to_string()],
            generation: 2,
            created_by: "agent_creator".to_string(),
            created_at: "2026-05-04T00:00:00Z".parse().unwrap(),
            reframing_potential: Some(0.5),
        },
        access_control: Default::default(),
        domain_tags: vec!["schema".to_string()],
        metadata: Default::default(),
        former_agents: vec![],
        former_deployments: vec![],
    };

    let first = serde_yaml::to_string(&tradeoff).unwrap();
    let loaded: TradeoffConfig = serde_yaml::from_str(&first).unwrap();
    let second = serde_yaml::to_string(&loaded).unwrap();

    assert_eq!(first, second);
}

#[test]
fn test_existing_wg_agency_primitives_deserialize() {
    let primitives = Path::new(".wg/agency/primitives");
    if !primitives.exists() {
        return;
    }

    for path in yaml_files(primitives) {
        let contents = fs::read_to_string(&path).unwrap();
        match path
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
        {
            Some("components") => {
                let _: RoleComponent = serde_yaml::from_str(&contents)
                    .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            }
            Some("outcomes") => {
                let _: DesiredOutcome = serde_yaml::from_str(&contents)
                    .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            }
            Some("tradeoffs") => {
                let _: TradeoffConfig = serde_yaml::from_str(&contents)
                    .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            }
            _ => {}
        }
    }
}

fn yaml_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "yaml") {
                paths.push(path);
            }
        }
    }

    paths
}

#[test]
fn test_default_agency_schema_fields_skip_when_serializing() {
    let component = RoleComponent {
        id: "component-2".to_string(),
        name: "Defaults".to_string(),
        description: "Default agency schema fields stay additive.".to_string(),
        quality: 100,
        domain_specificity: 0,
        domain: vec![],
        scope: None,
        origin_instance_id: None,
        parent_content_hash: None,
        category: ComponentCategory::Novel,
        content: ContentRef::Inline("Default agency schema fields stay additive.".to_string()),
        performance: PerformanceRecord::default(),
        lineage: Lineage::default(),
        access_control: Default::default(),
        domain_tags: vec![],
        metadata: Default::default(),
        former_agents: vec![],
        former_deployments: vec![],
    };

    let yaml = serde_yaml::to_string(&component).unwrap();

    assert!(!yaml.contains("quality:"));
    assert!(!yaml.contains("domain_specificity:"));
    assert!(!yaml.contains("origin_instance_id:"));
}
