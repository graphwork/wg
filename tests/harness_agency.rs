//! Consolidated integration-test domain harness.
//!
//! Keep top-level cases as modules so Cargo links this domain once rather than
//! producing one debuginfo-heavy executable per source file.

#[path = "agency_schema_fields.rs"]
mod agency_schema_fields;
#[path = "evaluation_recording.rs"]
mod evaluation_recording;
#[path = "flip_role_model_routing.rs"]
mod flip_role_model_routing;
#[path = "integration_agency.rs"]
mod integration_agency;
#[path = "integration_agency_csv_roundtrip.rs"]
mod integration_agency_csv_roundtrip;
#[path = "integration_agency_edge_cases.rs"]
mod integration_agency_edge_cases;
#[path = "integration_agency_federation.rs"]
mod integration_agency_federation;
#[path = "integration_agency_hash.rs"]
mod integration_agency_hash;
#[path = "integration_agency_import.rs"]
mod integration_agency_import;
#[path = "integration_agency_lineage.rs"]
mod integration_agency_lineage;
#[path = "integration_agency_loop.rs"]
mod integration_agency_loop;
#[path = "integration_agency_pipeline.rs"]
mod integration_agency_pipeline;
#[path = "integration_agency_scope_rules.rs"]
mod integration_agency_scope_rules;
#[path = "integration_agency_stats.rs"]
mod integration_agency_stats;
#[path = "integration_auto_assignment.rs"]
mod integration_auto_assignment;
#[path = "integration_auto_evolver.rs"]
mod integration_auto_evolver;
#[path = "integration_evolver_pipeline.rs"]
mod integration_evolver_pipeline;
#[path = "integration_lazy_evaluation.rs"]
mod integration_lazy_evaluation;
#[path = "integration_llm_autopoiesis.rs"]
mod integration_llm_autopoiesis;
#[path = "skill_resolution.rs"]
mod skill_resolution;
