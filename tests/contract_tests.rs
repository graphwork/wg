//! Contract/schema tests for LLM response parsing.
//!
//! Tests response parsing and schema validation with fixture data,
//! without calling any LLM. Verifies that parsers handle expected formats,
//! edge cases, and malformed responses correctly.

use std::collections::HashMap;
use workgraph::json_extract::extract_json;

// ---------------------------------------------------------------------------
// Response structs (mirrors the private types in evaluate.rs and evolve.rs)
// ---------------------------------------------------------------------------

/// Mirrors the EvalOutput struct in commands/evaluate.rs.
#[derive(serde::Deserialize, Debug)]
struct EvalOutput {
    score: f64,
    #[serde(default)]
    dimensions: HashMap<String, f64>,
    #[serde(default)]
    notes: String,
}

/// Mirrors the EvolverOperation struct in commands/evolve.rs.
#[derive(serde::Deserialize, Debug)]
struct EvolverOperation {
    op: String,
    #[serde(default)]
    entity_type: Option<String>,
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default)]
    add_component_id: Option<String>,
    #[serde(default)]
    remove_component_id: Option<String>,
    #[serde(default)]
    new_outcome_id: Option<String>,
    #[serde(default)]
    new_tradeoff_id: Option<String>,
    #[serde(default)]
    new_name: Option<String>,
    #[serde(default)]
    new_description: Option<String>,
    #[serde(default)]
    new_content: Option<String>,
    #[serde(default)]
    new_category: Option<String>,
    #[serde(default)]
    new_success_criteria: Option<Vec<String>>,
    #[serde(default)]
    new_acceptable_tradeoffs: Option<Vec<String>>,
    #[serde(default)]
    new_unacceptable_tradeoffs: Option<Vec<String>>,
    #[serde(default)]
    selection_method: Option<String>,
    #[serde(default)]
    new_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, alias = "skills")]
    component_ids: Option<Vec<String>>,
    #[serde(default, alias = "desired_outcome")]
    outcome_id: Option<String>,
    #[serde(default)]
    role_id: Option<String>,
    #[serde(default)]
    tradeoff_id: Option<String>,
    #[serde(default)]
    acceptable_tradeoffs: Option<Vec<String>>,
    #[serde(default)]
    unacceptable_tradeoffs: Option<Vec<String>>,
    #[serde(default)]
    rationale: Option<String>,
    #[serde(default)]
    ideation_prompt: Option<String>,
}

/// Mirrors the EvolverOutput struct in commands/evolve.rs.
#[derive(serde::Deserialize, Debug)]
struct EvolverOutput {
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    target: Option<serde_json::Value>,
    operations: Vec<EvolverOperation>,
    #[serde(default)]
    deferred_operations: Vec<EvolverOperation>,
    #[serde(default)]
    summary: Option<String>,
}

// ============================================================================
// extract_json tests
// ============================================================================

#[test]
fn extract_json_plain_object() {
    let input = r#"{"score": 0.85, "notes": "Good work"}"#;
    let result = extract_json(input).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["score"], 0.85);
}

#[test]
fn extract_json_with_whitespace() {
    let input = "   \n  {\"score\": 0.7}  \n   ";
    let result = extract_json(input).unwrap();
    assert!(result.contains("0.7"));
}

#[test]
fn extract_json_markdown_fences() {
    let input = "```json\n{\"score\": 0.9, \"notes\": \"great\"}\n```";
    let result = extract_json(input).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["score"], 0.9);
}

#[test]
fn extract_json_markdown_fences_no_lang() {
    let input = "```\n{\"score\": 0.6}\n```";
    let result = extract_json(input).unwrap();
    assert!(result.contains("0.6"));
}

#[test]
fn extract_json_with_preamble_text() {
    let raw = include_str!("fixtures/eval_response_with_preamble.txt");
    let result = extract_json(raw).unwrap();
    let parsed: EvalOutput = serde_json::from_str(&result).unwrap();
    assert!((parsed.score - 0.78).abs() < f64::EPSILON);
}

#[test]
fn extract_json_with_markdown_fences_and_commentary() {
    let raw = include_str!("fixtures/eval_response_with_fences.txt");
    let result = extract_json(raw).unwrap();
    let parsed: EvalOutput = serde_json::from_str(&result).unwrap();
    assert!((parsed.score - 0.82).abs() < f64::EPSILON);
    assert_eq!(parsed.dimensions.len(), 4);
}

#[test]
fn extract_json_returns_none_for_no_json() {
    assert!(extract_json("no json here at all").is_none());
}

#[test]
fn extract_json_returns_none_for_empty_string() {
    assert!(extract_json("").is_none());
}

#[test]
fn extract_json_returns_none_for_malformed_json() {
    assert!(extract_json("{score: 0.5, incomplete").is_none());
}

#[test]
fn extract_json_nested_braces() {
    let input = r#"{"outer": {"inner": 42}, "key": "value"}"#;
    let result = extract_json(input).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["outer"]["inner"], 42);
}

// ============================================================================
// EvalOutput deserialization tests
// ============================================================================

#[test]
fn eval_output_valid_fixture() {
    let raw = include_str!("fixtures/eval_response_valid.json");
    let parsed: EvalOutput = serde_json::from_str(raw).unwrap();
    assert!((parsed.score - 0.85).abs() < f64::EPSILON);
    assert_eq!(parsed.dimensions.len(), 4);
    assert!((parsed.dimensions["correctness"] - 0.9).abs() < f64::EPSILON);
    assert!((parsed.dimensions["completeness"] - 0.8).abs() < f64::EPSILON);
    assert!((parsed.dimensions["efficiency"] - 0.85).abs() < f64::EPSILON);
    assert!((parsed.dimensions["style_adherence"] - 0.82).abs() < f64::EPSILON);
    assert!(parsed.notes.contains("test coverage"));
}

#[test]
fn eval_output_minimal_fixture() {
    let raw = include_str!("fixtures/eval_response_minimal.json");
    let parsed: EvalOutput = serde_json::from_str(raw).unwrap();
    assert!((parsed.score - 0.75).abs() < f64::EPSILON);
    assert!(parsed.dimensions.is_empty());
    assert!(parsed.notes.is_empty());
}

#[test]
fn eval_output_score_zero() {
    let raw = include_str!("fixtures/eval_response_edge_zero.json");
    let parsed: EvalOutput = serde_json::from_str(raw).unwrap();
    assert!((parsed.score - 0.0).abs() < f64::EPSILON);
    for val in parsed.dimensions.values() {
        assert!((*val - 0.0).abs() < f64::EPSILON);
    }
}

#[test]
fn eval_output_score_perfect() {
    let raw = include_str!("fixtures/eval_response_edge_perfect.json");
    let parsed: EvalOutput = serde_json::from_str(raw).unwrap();
    assert!((parsed.score - 1.0).abs() < f64::EPSILON);
    for val in parsed.dimensions.values() {
        assert!((*val - 1.0).abs() < f64::EPSILON);
    }
}

#[test]
fn eval_output_extra_fields_ignored() {
    let json = r#"{"score": 0.8, "dimensions": {}, "notes": "ok", "extra_field": "ignored", "another": 42}"#;
    let parsed: EvalOutput = serde_json::from_str(json).unwrap();
    assert!((parsed.score - 0.8).abs() < f64::EPSILON);
}

#[test]
fn eval_output_extra_dimensions_accepted() {
    let json = r#"{"score": 0.8, "dimensions": {"correctness": 0.9, "creativity": 0.7, "custom_dim": 0.5}}"#;
    let parsed: EvalOutput = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.dimensions.len(), 3);
    assert!(parsed.dimensions.contains_key("creativity"));
}

#[test]
fn eval_output_missing_score_fails() {
    let json = r#"{"dimensions": {"correctness": 0.9}, "notes": "no score"}"#;
    let result: Result<EvalOutput, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn eval_output_wrong_score_type_fails() {
    let json = r#"{"score": "high", "notes": "score is a string"}"#;
    let result: Result<EvalOutput, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn eval_output_null_dimensions_uses_default() {
    let json = r#"{"score": 0.5, "dimensions": null}"#;
    // serde(default) means if the field is null, it should fail (null != missing)
    // Actually, with serde's default, null will fail. Let's verify.
    let result: Result<EvalOutput, _> = serde_json::from_str(json);
    // serde(default) only applies to missing fields, not null values
    // null will cause deserialization error for HashMap
    assert!(result.is_err());
}

// ============================================================================
// EvolverOperation deserialization tests
// ============================================================================

#[test]
fn evolver_output_mutation_fixture() {
    let raw = include_str!("fixtures/evolver_output_mutation.json");
    let parsed: EvolverOutput = serde_json::from_str(raw).unwrap();
    assert_eq!(parsed.run_id.as_deref(), Some("evolve-run-001"));
    assert_eq!(parsed.operations.len(), 1);
    assert_eq!(parsed.operations[0].op, "wording_mutation");
    assert_eq!(parsed.operations[0].new_name.as_deref(), Some("Senior Builder"));
    assert!(parsed.operations[0].rationale.is_some());
    assert!(parsed.deferred_operations.is_empty());
}

#[test]
fn evolver_output_multi_operations_fixture() {
    let raw = include_str!("fixtures/evolver_output_multi_ops.json");
    let parsed: EvolverOutput = serde_json::from_str(raw).unwrap();
    assert_eq!(parsed.run_id.as_deref(), Some("evolve-run-002"));
    assert_eq!(parsed.operations.len(), 3);
    assert_eq!(parsed.operations[0].op, "create_role");
    assert_eq!(parsed.operations[1].op, "modify_motivation");
    assert_eq!(parsed.operations[2].op, "component_substitution");
    assert_eq!(parsed.deferred_operations.len(), 1);
    assert_eq!(parsed.deferred_operations[0].op, "retire_role");
    assert!(parsed.summary.is_some());
}

#[test]
fn evolver_output_all_operation_types() {
    let raw = include_str!("fixtures/evolver_output_all_op_types.json");
    let parsed: EvolverOutput = serde_json::from_str(raw).unwrap();
    assert_eq!(parsed.operations.len(), 15);

    let op_types: Vec<&str> = parsed.operations.iter().map(|o| o.op.as_str()).collect();
    assert!(op_types.contains(&"create_role"));
    assert!(op_types.contains(&"modify_role"));
    assert!(op_types.contains(&"retire_role"));
    assert!(op_types.contains(&"create_motivation"));
    assert!(op_types.contains(&"modify_motivation"));
    assert!(op_types.contains(&"retire_motivation"));
    assert!(op_types.contains(&"wording_mutation"));
    assert!(op_types.contains(&"component_substitution"));
    assert!(op_types.contains(&"config_add_component"));
    assert!(op_types.contains(&"config_remove_component"));
    assert!(op_types.contains(&"config_swap_outcome"));
    assert!(op_types.contains(&"config_swap_tradeoff"));
    assert!(op_types.contains(&"random_compose_role"));
    assert!(op_types.contains(&"random_compose_agent"));
    assert!(op_types.contains(&"bizarre_ideation"));
}

#[test]
fn evolver_operation_create_role_fields() {
    let json = r#"{"op": "create_role", "new_name": "Tester", "new_description": "Runs tests", "component_ids": ["testing", "ci"], "outcome_id": "All tests pass"}"#;
    let parsed: EvolverOperation = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.op, "create_role");
    assert_eq!(parsed.new_name.as_deref(), Some("Tester"));
    assert_eq!(parsed.component_ids.as_ref().unwrap().len(), 2);
    assert_eq!(parsed.outcome_id.as_deref(), Some("All tests pass"));
}

#[test]
fn evolver_operation_component_substitution_fields() {
    let json = r#"{"op": "component_substitution", "target_id": "role-abc", "remove_component_id": "old", "add_component_id": "new"}"#;
    let parsed: EvolverOperation = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.op, "component_substitution");
    assert_eq!(parsed.target_id.as_deref(), Some("role-abc"));
    assert_eq!(parsed.remove_component_id.as_deref(), Some("old"));
    assert_eq!(parsed.add_component_id.as_deref(), Some("new"));
}

#[test]
fn evolver_operation_bizarre_ideation_fields() {
    let json = r#"{"op": "bizarre_ideation", "entity_type": "component", "new_name": "Quantum Debug", "new_content": "Debug via quantum states", "ideation_prompt": "Invent debugging"}"#;
    let parsed: EvolverOperation = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.op, "bizarre_ideation");
    assert_eq!(parsed.entity_type.as_deref(), Some("component"));
    assert_eq!(parsed.new_content.as_deref(), Some("Debug via quantum states"));
    assert_eq!(parsed.ideation_prompt.as_deref(), Some("Invent debugging"));
}

#[test]
fn evolver_operation_random_compose_fields() {
    let json = r#"{"op": "random_compose_agent", "role_id": "r1", "tradeoff_id": "t1", "selection_method": "uniform_random"}"#;
    let parsed: EvolverOperation = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.op, "random_compose_agent");
    assert_eq!(parsed.role_id.as_deref(), Some("r1"));
    assert_eq!(parsed.tradeoff_id.as_deref(), Some("t1"));
    assert_eq!(parsed.selection_method.as_deref(), Some("uniform_random"));
}

#[test]
fn evolver_operation_legacy_skills_alias() {
    let json = r#"{"op": "create_role", "skills": ["a", "b"]}"#;
    let parsed: EvolverOperation = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.component_ids.as_ref().unwrap(), &["a", "b"]);
}

#[test]
fn evolver_operation_legacy_desired_outcome_alias() {
    let json = r#"{"op": "create_role", "desired_outcome": "Everything works"}"#;
    let parsed: EvolverOperation = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.outcome_id.as_deref(), Some("Everything works"));
}

#[test]
fn evolver_output_empty_operations() {
    let json = r#"{"operations": [], "summary": "Nothing to do."}"#;
    let parsed: EvolverOutput = serde_json::from_str(json).unwrap();
    assert!(parsed.operations.is_empty());
    assert!(parsed.run_id.is_none());
    assert_eq!(parsed.summary.as_deref(), Some("Nothing to do."));
}

#[test]
fn evolver_output_missing_operations_fails() {
    let json = r#"{"summary": "no operations field"}"#;
    let result: Result<EvolverOutput, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn evolver_output_missing_op_field_fails() {
    let json = r#"{"operations": [{"new_name": "Missing op field"}]}"#;
    let result: Result<EvolverOutput, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

// ============================================================================
// End-to-end: extract_json + parse pipeline
// ============================================================================

#[test]
fn pipeline_extract_and_parse_eval_from_fenced() {
    let raw = include_str!("fixtures/eval_response_with_fences.txt");
    let json_str = extract_json(raw).unwrap();
    let parsed: EvalOutput = serde_json::from_str(&json_str).unwrap();
    assert!(parsed.score >= 0.0 && parsed.score <= 1.0);
    assert!(!parsed.dimensions.is_empty());
}

#[test]
fn pipeline_extract_and_parse_eval_from_preamble() {
    let raw = include_str!("fixtures/eval_response_with_preamble.txt");
    let json_str = extract_json(raw).unwrap();
    let parsed: EvalOutput = serde_json::from_str(&json_str).unwrap();
    assert!(parsed.score >= 0.0 && parsed.score <= 1.0);
}

#[test]
fn pipeline_extract_and_parse_evolver_output() {
    let raw = format!(
        "Here are my proposed evolution operations:\n```json\n{}\n```\nLet me know if these look good.",
        include_str!("fixtures/evolver_output_mutation.json")
    );
    let json_str = extract_json(&raw).unwrap();
    let parsed: EvolverOutput = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed.operations.len(), 1);
    assert_eq!(parsed.operations[0].op, "wording_mutation");
}
