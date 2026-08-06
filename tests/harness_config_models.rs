//! Consolidated integration-test domain harness.
//!
//! Keep top-level cases as modules so Cargo links this domain once rather than
//! producing one debuginfo-heavy executable per source file.

#[path = "integration_bare_alias_contract.rs"]
mod integration_bare_alias_contract;
#[path = "integration_canonical_config.rs"]
mod integration_canonical_config;
#[path = "integration_config.rs"]
mod integration_config;
#[path = "integration_deprecated_provider_flag.rs"]
mod integration_deprecated_provider_flag;
#[path = "integration_dispatcher_config_roundtrip.rs"]
mod integration_dispatcher_config_roundtrip;
#[path = "integration_global_config.rs"]
mod integration_global_config;
#[path = "integration_login_openrouter.rs"]
mod integration_login_openrouter;
#[path = "integration_pi_sole_model_plane.rs"]
mod integration_pi_sole_model_plane;
#[path = "integration_pi_two_tier_profile.rs"]
mod integration_pi_two_tier_profile;
#[path = "integration_profile_tier_pinning.rs"]
mod integration_profile_tier_pinning;
#[path = "integration_provider_model_format.rs"]
mod integration_provider_model_format;
#[path = "integration_safety_interval_alias.rs"]
mod integration_safety_interval_alias;
#[path = "integration_setup.rs"]
mod integration_setup;
#[path = "integration_setup_routes.rs"]
mod integration_setup_routes;
#[path = "integration_tier_defaults.rs"]
mod integration_tier_defaults;
#[path = "smoke_model_pipeline.rs"]
mod smoke_model_pipeline;
#[path = "test_provider_health.rs"]
mod test_provider_health;
#[path = "test_unconfigured_cycle_breakin.rs"]
mod test_unconfigured_cycle_breakin;
