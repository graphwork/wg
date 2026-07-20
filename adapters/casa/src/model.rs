//! Casa product schemas for the credential-free adapter spark.
//!
//! This is intentionally not an identity, trust, ACL, capability, transport, or
//! execution module. Every authority-bearing handle below is a reference to an
//! already-authenticated WG-Fed/Review/Exec record.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const CHANNEL_SCHEMA: &str = "casa.channel-envelope.v1";
pub const ROSTER_SCHEMA: &str = "casa.household.v1";
pub const PROJECTION_SCHEMA: &str = "casa.projection.v2";
pub const INTENT_SCHEMA: &str = "wg.delivery-intent.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChannelEnvelope {
    pub schema: String,
    pub message_kind: MessageKind,
    pub origin: String,
    /// Domain-separated digest of protected native channel ids. Evidence only.
    pub native_evidence_cid: String,
    /// Stable connector/product dedupe key. Never an identity or capability.
    pub src_id: String,
    pub device_label: String,
    /// Household-local ISO date supplied by the deterministic connector fixture.
    pub local_date: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageKind {
    Request,
    Report,
}

impl ChannelEnvelope {
    pub fn validate(&self) -> Result<()> {
        if self.schema != CHANNEL_SCHEMA {
            bail!(
                "unsupported Casa channel schema {:?}; expected {CHANNEL_SCHEMA}",
                self.schema
            );
        }
        if !matches!(self.origin.as_str(), "telegram" | "casa-web") {
            bail!("unsupported channel origin {:?}", self.origin);
        }
        if !self.native_evidence_cid.starts_with("b3:")
            || !self.src_id.starts_with("casa-src:v1:b3:")
        {
            bail!("channel evidence/srcId is malformed");
        }
        if self.device_label.trim().is_empty() || self.device_label.len() > 80 {
            bail!("device label must be non-empty and at most 80 bytes");
        }
        chrono::NaiveDate::parse_from_str(&self.local_date, "%Y-%m-%d")
            .map_err(|_| anyhow::anyhow!("localDate must be YYYY-MM-DD"))?;
        if self.text.trim().is_empty() || self.text.len() > 16 * 1024 {
            bail!("message text must be non-empty and at most 16KiB");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HouseholdRoster {
    pub schema: String,
    pub household_id: String,
    pub members: Vec<HouseholdMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HouseholdMember {
    pub wgid: String,
    pub alias: String,
    #[serde(default)]
    pub domains: Vec<String>,
}

impl HouseholdRoster {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let roster: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        if roster.schema != ROSTER_SCHEMA {
            bail!(
                "unsupported Casa roster schema {:?}; expected {ROSTER_SCHEMA}",
                roster.schema
            );
        }
        if roster.household_id.trim().is_empty() {
            bail!("householdId must not be empty");
        }
        for member in &roster.members {
            worksgood::identity::keys::pubkey_from_wgid(&member.wgid).map_err(|_| {
                anyhow::anyhow!("roster member is not a canonical wgid: {}", member.wgid)
            })?;
            if member.alias.trim().is_empty() {
                bail!("roster alias must not be empty");
            }
        }
        Ok(roster)
    }

    /// Casa policy: route meal-plan/dinner asks to exactly one declared owner.
    /// The result is a WG principal, never a bot/name/provider id.
    pub fn elect(&self, text: &str) -> Result<&HouseholdMember> {
        let normalized = text.to_ascii_lowercase();
        let domain = if ["dinner", "meal", "nutrition", "recipe"]
            .iter()
            .any(|word| normalized.contains(word))
        {
            "meal-planning"
        } else {
            "household"
        };
        let candidates: Vec<_> = self
            .members
            .iter()
            .filter(|m| m.domains.iter().any(|d| d == domain))
            .collect();
        match candidates.as_slice() {
            [owner] => Ok(owner),
            [] => bail!("Casa election found no owner for domain {domain}"),
            _ => bail!("Casa election refused ambiguous owners for domain {domain}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedEvent {
    pub schema: String,
    pub event_cid: String,
    pub author_wgid: String,
    pub review_record_cid: String,
    pub content_cid: String,
    /// Exact bytes-as-text pinned by WG-Review. Rebuild re-checks this value; it may
    /// not be reconstructed from parsed JSON because whitespace is digest-significant.
    pub reviewed_body: String,
    pub envelope: ChannelEnvelope,
    pub duplicate_of: Option<String>,
    pub owner_wgid: Option<String>,
    pub owner_alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IngressReceipt {
    pub schema: String,
    pub event_cid: String,
    pub recipient_wgid: String,
    pub state: String,
    pub review_record_cid: String,
    pub content_cid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryIntent {
    pub schema: String,
    pub id: String,
    pub event_cid: String,
    pub destination_id: String,
    pub render_profile: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryAttempt {
    pub schema: String,
    pub intent_cid: String,
    pub attempt: u32,
    pub state: String,
    pub native_message_id: Option<String>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionRow {
    pub schema: String,
    pub event_cid: String,
    pub author_wgid: String,
    pub recipient_wgids: Vec<String>,
    pub review_record_cid: String,
    pub direction: String,
    pub channel: String,
    pub src_id: String,
    pub in_reply_to: Option<String>,
    pub delivery_state: Option<String>,
    pub text: String,
    pub display: ProjectionDisplay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionDisplay {
    pub sender: String,
    pub persona: Option<String>,
    pub device: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_schema_major_fails_closed() {
        let env = ChannelEnvelope {
            schema: "casa.channel-envelope.v2".into(),
            message_kind: MessageKind::Request,
            origin: "telegram".into(),
            native_evidence_cid: format!("b3:{}", "0".repeat(64)),
            src_id: format!("casa-src:v1:b3:{}", "1".repeat(64)),
            device_label: "your iPhone".into(),
            local_date: "2026-07-22".into(),
            text: "Plan dinner".into(),
        };
        assert!(
            env.validate()
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
    }

    #[test]
    fn election_requires_exactly_one_wgid_owner() {
        let key = worksgood::identity::keys::gen_ed25519().unwrap();
        let wgid = worksgood::identity::keys::wgid_from_pubkey(&key.public);
        let roster = HouseholdRoster {
            schema: ROSTER_SCHEMA.into(),
            household_id: "fixture-household".into(),
            members: vec![HouseholdMember {
                wgid: wgid.clone(),
                alias: "Nora".into(),
                domains: vec!["meal-planning".into()],
            }],
        };
        assert_eq!(roster.elect("Plan Wednesday dinner").unwrap().wgid, wgid);
    }
}
