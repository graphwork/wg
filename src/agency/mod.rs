mod agency_bridge;
pub mod composition_rules;
pub mod constraint_fidelity;
mod eval;
pub mod evolver;
mod hash;
mod lineage;
mod output;
mod prompt;
pub mod run_mode;
pub(crate) mod starters;
mod store;
mod types;

/// Agency federation compatibility surface implemented by this wg build.
pub const WG_AGENCY_COMPAT_VERSION: &str = "1.2.4";

// Re-export everything at the agency:: level for backward compatibility
pub use agency_bridge::*;
pub use constraint_fidelity::*;
pub use eval::*;
pub use evolver::*;
pub use hash::*;
pub use lineage::*;
pub use output::*;
pub use prompt::*;
pub use run_mode::*;
pub use starters::*;
pub use store::*;
pub use types::*;
