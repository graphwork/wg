use serde::Serialize;
use sha2::{Digest, Sha256};

use super::types::{ComponentCategory, ContentRef};

/// Default number of hex characters for short display of content hashes.
pub const SHORT_HASH_LEN: usize = 8;

/// Return the first `SHORT_HASH_LEN` hex characters of a full hash for display.
pub fn short_hash(full_hash: &str) -> &str {
    &full_hash[..full_hash.len().min(SHORT_HASH_LEN)]
}

/// Compute the Agency-compatible SHA-256 hash for a primitive description.
///
/// Agency v1.2.4 hashes the raw description string bytes directly. This is
/// intentionally not a serialized YAML envelope.
pub fn description_hash(description: &str) -> String {
    let digest = Sha256::digest(description.as_bytes());
    format!("{:x}", digest)
}

/// Compute the SHA-256 content hash for a RoleComponent.
/// Hashed fields: description.
pub fn content_hash_component(
    description: &str,
    _category: &ComponentCategory,
    _content: &ContentRef,
) -> String {
    description_hash(description)
}

/// Compute the SHA-256 content hash for a DesiredOutcome.
/// Hashed fields: description.
pub fn content_hash_outcome(description: &str, _success_criteria: &[String]) -> String {
    description_hash(description)
}

/// Compute the SHA-256 content hash for a TradeoffConfig (formerly Motivation).
/// Hashed fields: description.
pub fn content_hash_tradeoff(
    _acceptable_tradeoffs: &[String],
    _unacceptable_tradeoffs: &[String],
    description: &str,
) -> String {
    description_hash(description)
}

/// Compute the SHA-256 content hash for a Role composition.
/// Hashed fields: sorted component_ids, outcome_id.
pub fn content_hash_role(component_ids: &[String], outcome_id: &str) -> String {
    #[derive(Serialize)]
    struct Input<'a> {
        component_ids: Vec<&'a str>,
        outcome_id: &'a str,
    }
    let mut sorted: Vec<&str> = component_ids.iter().map(|s| s.as_str()).collect();
    sorted.sort();
    let input = Input {
        component_ids: sorted,
        outcome_id,
    };
    let yaml = serde_yaml::to_string(&input).expect("serialization of hash input cannot fail");
    let digest = Sha256::digest(yaml.as_bytes());
    format!("{:x}", digest)
}

/// Compute the SHA-256 content hash for an Agent composition.
/// Hashed fields: role_id, tradeoff_id.
pub fn content_hash_agent(role_id: &str, tradeoff_id: &str) -> String {
    #[derive(Serialize)]
    struct Input<'a> {
        role_id: &'a str,
        #[serde(rename = "motivation_id")]
        tradeoff_id: &'a str,
    }
    let input = Input {
        role_id,
        tradeoff_id,
    };
    let yaml = serde_yaml::to_string(&input).expect("serialization of hash input cannot fail");
    let digest = Sha256::digest(yaml.as_bytes());
    format!("{:x}", digest)
}
