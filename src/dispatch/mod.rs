//! Dispatch — single source of truth for what runs when a task is spawned.
//!
//! Historically, two competing decision-makers (the daemon's executor-config
//! and per-task spawn-argv builders) read the merged config independently and
//! could disagree (`executor=claude` in the daemon log vs `executor=native`
//! in the spawn metadata). This module unifies that into a `SpawnPlan` built
//! by exactly one function (`plan_spawn`). Every spawn site calls it; nobody
//! else picks the executor.

pub mod handler_for_model;
pub mod plan;
pub mod profile;

pub use handler_for_model::handler_for_model;
pub use plan::{
    ExecutorKind, Placement, ResolvedModelSpec, SpawnPlan, SpawnProvenance, plan_spawn,
};
pub use profile::{ProfileCache, effective_config_for_task, effective_config_owned};
