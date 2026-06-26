//! WG-Exec — the execution-federation plane (Exec-Wave B, the **Execution Spark**).
//!
//! This is the first execution-plane code. Where WG-Fed (`src/identity/`) proved
//! *"two graphs, one key, a third location"* (a downloaded identity cannot
//! impersonate), WG-Exec proves *"one graph, a borrowed box, a scoped leash"* (a
//! borrowed provider cannot exceed its lease). It is the minimum to pass the
//! six-step execution spark
//! (`docs/execution-federation-study/06-decision-memo-and-roadmap.md` §4) and is
//! the executable form of the four Exec-Wave A ADRs:
//!
//! - **ADR-E1 (placement)** — per-authorizer placement; push-default, a `Claim` is a
//!   *request* the authorizer's signed `RunGrant` decides; a hard filter (capability
//!   + trust-floor) then an advisory rank ([`placement`]).
//! - **ADR-E2 (confidentiality)** — three levers along the one leash dial (trust /
//!   minimize / attest); **confidential ⇒ attested-C or refuse, never A/B**;
//!   unlabeled fails closed ([`placement::leash`], [`bundle`]).
//! - **ADR-E3 (capability & lease)** — **two scoped attenuating UCANs, never the root
//!   key** (act-as-agent + graph-write-task-only); a signed lease fenced by a
//!   **monotonic epoch atomic-CAS** at the single canonical-write boundary
//!   ([`lease`], reusing WG-Fed's [`crate::identity::custody`]).
//! - **ADR-E4 (verification)** — attribution is mandatory but **is not integrity**; a
//!   low-trust result is re-run **in a trusted domain (never the producer) vs a
//!   *pinned* spec** ([`verify`]).
//!
//! **It invents no second trust system (NFR-4).** Identity (`wgid:`), the sigchain,
//! the custodian-held-root signing boundary, the attenuating UCAN, and the
//! per-recipient sealed envelope are all **WG-Fed's**, reused verbatim. What is *this*
//! module's to own is the **execution wire** (the five envelopes below), the
//! **lease-epoch fence**, and **how the leash dial wires UCAN scope/TTL + lease
//! term/cadence together per task**.

pub mod bundle;
pub mod lease;
pub mod placement;
pub mod verify;

use std::collections::HashMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::identity::sigchain::{AuthorizedKeys, KeyRole, KeyStatus};
use crate::identity::{ALG_ED25519, ENVELOPE_V, content_cid, keys, signing_digest};

pub use crate::graph::TrustLevel;
pub use bundle::SealedBundle;
pub use lease::Lease;

// ── Compat handshake (authenticated, loud-fail — HQ12 / ADR-E1 D6) ──────────────

/// WG-Exec wire/format compatibility version.
///
/// The execution wire's single source of truth, defined **once** here — the wire's
/// home, exactly as `WG_FED_COMPAT_VERSION` lives in `src/identity/mod.rs`. Peers
/// exchange it on first contact inside *signed* envelopes (so the handshake is
/// authenticated, not merely advertised) and **fail loud** on an incompatible
/// mismatch (no silent downgrade). Mirrors `WG_FED_COMPAT_VERSION` /
/// `WG_AGENCY_COMPAT_VERSION` / `WG_PI_PLUGIN_COMPAT_VERSION`.
pub const WG_EXEC_COMPAT_VERSION: &str = "0.1.0";

fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// Assert a peer's advertised `WG_EXEC_COMPAT_VERSION` is compatible with ours.
///
/// Pre-1.0 the wire is not frozen, so the **major AND minor** must match exactly; a
/// mismatch is a loud, hard error naming expected-vs-found — never a silent
/// downgrade. Because this field rides inside the signed offer/claim/grant
/// envelopes, a verified envelope's compat field is authenticated.
pub fn check_exec_compat(peer_version: &str) -> Result<()> {
    let ours =
        parse_semver(WG_EXEC_COMPAT_VERSION).expect("WG_EXEC_COMPAT_VERSION is valid semver");
    let theirs = match parse_semver(peer_version) {
        Some(v) => v,
        None => bail!(
            "WG-Exec compat handshake FAILED: peer advertised an unparseable version \
             {peer_version:?}; this build speaks {WG_EXEC_COMPAT_VERSION}"
        ),
    };
    let compatible = if ours.0 == 0 || theirs.0 == 0 {
        ours.0 == theirs.0 && ours.1 == theirs.1
    } else {
        ours.0 == theirs.0
    };
    if !compatible {
        bail!(
            "WG-Exec compat handshake FAILED (loud): peer speaks \
             WG_EXEC_COMPAT_VERSION={peer_version}, this build speaks \
             {WG_EXEC_COMPAT_VERSION}. Refusing to silently downgrade."
        );
    }
    Ok(())
}

// ── Shared signature helper (mirrors custody/envelope, kept local — no new crypto) ──

/// Verify `sig_hex` over `digest` against any key the issuer's sigchain authorizes
/// for signing (the active root or an active signer). The single primitive behind
/// every execution-envelope signature check; identical in spirit to
/// `custody::verify_against_authorized` (private there), rooted entirely in
/// `keys::verify_sig` — a pure local check, no central authority.
pub(crate) fn verify_sig_authorized(
    digest: &[u8; 32],
    sig_hex: &str,
    auth: &AuthorizedKeys,
    what: &str,
) -> Result<()> {
    let sig = decode_sig(sig_hex)?;
    if keys::verify_sig(&auth.active_root, digest, &sig) {
        return Ok(());
    }
    for k in &auth.keys {
        if k.role == KeyRole::Signer && k.status == KeyStatus::Active {
            if let Ok(pk) = decode_pub(&k.public) {
                if keys::verify_sig(&pk, digest, &sig) {
                    return Ok(());
                }
            }
        }
    }
    bail!("{what} signature does not verify against any key authorized by the issuer's sigchain")
}

fn decode_sig(sig_hex: &str) -> Result<[u8; 64]> {
    let b = hex::decode(sig_hex).map_err(|_| anyhow::anyhow!("signature is not valid hex"))?;
    if b.len() != 64 {
        bail!("signature is {} bytes, expected 64", b.len());
    }
    let mut out = [0u8; 64];
    out.copy_from_slice(&b);
    Ok(out)
}

fn decode_pub(pub_hex: &str) -> Result<[u8; 32]> {
    let b = hex::decode(pub_hex).map_err(|_| anyhow::anyhow!("public key is not valid hex"))?;
    if b.len() != 32 {
        bail!("public key is {} bytes, expected 32", b.len());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&b);
    Ok(out)
}

// ── Task-level descriptors (the leash inputs E2/E4 carry) ───────────────────────

/// Resolved sensitivity of a task (ADR-E2 D1/D4). An **`Unlabeled`** task is not a
/// silent `Normal`: it **fails closed** (refuse / route to C, never A — D-i).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    /// Explicitly + inferably normal — the only class eligible for the A (plaintext) tier.
    Normal,
    /// Explicitly elevated (but not confidential).
    High,
    /// Requires confidentiality — routes to attested-C or is refused (never A/B).
    Confidential,
    /// No label resolved — fails closed (D-i): never auto-routed to A.
    Unlabeled,
}

impl Sensitivity {
    pub fn parse(s: &str) -> Sensitivity {
        match s.trim().to_ascii_lowercase().as_str() {
            "normal" => Sensitivity::Normal,
            "high" => Sensitivity::High,
            "confidential" => Sensitivity::Confidential,
            _ => Sensitivity::Unlabeled,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Sensitivity::Normal => "normal",
            Sensitivity::High => "high",
            Sensitivity::Confidential => "confidential",
            Sensitivity::Unlabeled => "unlabeled",
        }
    }
}

/// The isolation-class ladder (HQ8). Ordered: a provider's class must be ≥ the task's
/// minimum. For confidential routing the class must be **attested**, not merely
/// self-advertised at this rung (ADR-E2 D2 / TC10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IsolationClass {
    Process,
    Container,
    Vm,
    /// The top rung — a trusted execution environment; the only class that can carry
    /// a *verified attestation* for confidential routing.
    Tee,
}

impl IsolationClass {
    pub fn parse(s: &str) -> Result<IsolationClass> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "process" => IsolationClass::Process,
            "container" => IsolationClass::Container,
            "vm" => IsolationClass::Vm,
            "tee" => IsolationClass::Tee,
            other => bail!("unknown isolation class {other:?} (process|container|vm|tee)"),
        })
    }
    pub fn as_str(self) -> &'static str {
        match self {
            IsolationClass::Process => "process",
            IsolationClass::Container => "container",
            IsolationClass::Vm => "vm",
            IsolationClass::Tee => "tee",
        }
    }
}

/// The pool class — three operating points of the *one* placement mechanism (ADR-E1
/// D4). Only `trust_level` + the applied leash change across these; never the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PoolClass {
    Private,
    Cooperative,
    Market,
}

/// Parse a [`TrustLevel`] from a CLI string (`graph::TrustLevel` is the one trust
/// dial — no second system). Numeric ordering for the trust-floor compare lives in
/// [`trust_rank`].
pub fn parse_trust(s: &str) -> Result<TrustLevel> {
    Ok(match s.trim().to_ascii_lowercase().as_str() {
        "verified" => TrustLevel::Verified,
        "provisional" => TrustLevel::Provisional,
        "unknown" => TrustLevel::Unknown,
        other => bail!("unknown trust level {other:?} (verified|provisional|unknown)"),
    })
}

/// `wgid:`-readable trust string (kebab-case, matching `graph::TrustLevel`'s serde).
pub fn trust_str(t: TrustLevel) -> &'static str {
    match t {
        TrustLevel::Verified => "verified",
        TrustLevel::Provisional => "provisional",
        TrustLevel::Unknown => "unknown",
    }
}

/// A monotone rank for the trust-floor compare: `Verified(2) > Provisional(1) >
/// Unknown(0)`. `provider.trust_level ≥ floor` ⇔ `trust_rank(provider) ≥
/// trust_rank(floor)`.
pub fn trust_rank(t: TrustLevel) -> u8 {
    match t {
        TrustLevel::Unknown => 0,
        TrustLevel::Provisional => 1,
        TrustLevel::Verified => 2,
    }
}

// ── The provider's signed capability advertisement (FR-R4) ──────────────────────

/// What a provider advertises it can run: a model/handler and an isolation class,
/// plus whether that class is **attestation-backed** (verified quote) rather than
/// self-advertised. The authorizer stores the *latest* of these in the registry and
/// **never** takes the provider's trust from it — trust is the authorizer's to assert.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityAd {
    pub model: String,
    pub isolation: IsolationClass,
    /// `true` only when the isolation/TEE class is bound to the provider's `wgid:` by a
    /// **verified attestation quote**. v1's attestation slot has an empty allow-list, so
    /// this is `false` for every spark provider — which is exactly why a confidential
    /// task refuses (ADR-E2 D5, the loud degradation).
    #[serde(default)]
    pub attested: bool,
}

// ── ProviderEntry + ProviderRegistry (ADR-E1 D6, the authorizer's known pool) ───

/// One enrolled provider in the authorizer's pool. **Liveness is the
/// authorizer's observation of accepted signed `LeaseRenewal`s** (ADR-E3 D5), never
/// the provider's self-report / a local PID — `is_process_alive()` is meaningless
/// across a host boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderEntry {
    pub wgid: String,
    /// The authorizer's *local* trust assertion — never self-certified by the provider.
    pub trust_level: TrustLevel,
    /// The provider's last signed capability advertisement (FR-R4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<CapabilityAd>,
    /// The highest lease epoch for which this provider has produced an accepted signed
    /// renewal — the cross-host liveness signal (`is_live()`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_renewal_epoch: Option<u64>,
    /// RFC3339 of the last accepted renewal, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<String>,
}

impl ProviderEntry {
    /// Cross-host liveness (ADR-E3 D5): a provider is live iff the authorizer has
    /// accepted a signed renewal at or beyond `current_epoch`. A provider that *claims*
    /// alive but has produced no accepted renewal for the current epoch is **not** live.
    pub fn is_live(&self, current_epoch: u64) -> bool {
        matches!(self.last_renewal_epoch, Some(e) if e >= current_epoch)
    }
}

/// The authorizer's known-provider pool, keyed by `wgid:` (ADR-E1 D6). Persisted as
/// JSON by the CLI layer; the private-pool case needs **zero** central nodes (NFR-6).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderRegistry {
    #[serde(default)]
    pub providers: HashMap<String, ProviderEntry>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enroll (or update) a provider at an authorizer-asserted trust level + advertised
    /// capability. **Trust is the authorizer's to set**, never the provider's.
    pub fn enroll(&mut self, wgid: &str, trust: TrustLevel, capability: Option<CapabilityAd>) {
        let e = self
            .providers
            .entry(wgid.to_string())
            .or_insert_with(|| ProviderEntry {
                wgid: wgid.to_string(),
                trust_level: trust,
                capability: None,
                last_renewal_epoch: None,
                last_seen: None,
            });
        e.trust_level = trust;
        if capability.is_some() {
            e.capability = capability;
        }
    }

    pub fn get(&self, wgid: &str) -> Option<&ProviderEntry> {
        self.providers.get(wgid)
    }

    /// The authorizer's local trust assertion for a provider; an unknown provider is
    /// `TrustLevel::Unknown` (fail-safe — a stranger is never silently trusted).
    pub fn trust_of(&self, wgid: &str) -> TrustLevel {
        self.get(wgid)
            .map(|e| e.trust_level)
            .unwrap_or(TrustLevel::Unknown)
    }

    /// Record an accepted signed renewal (the liveness signal).
    pub fn record_renewal(&mut self, wgid: &str, epoch: u64, at: &str) {
        if let Some(e) = self.providers.get_mut(wgid) {
            e.last_renewal_epoch = Some(e.last_renewal_epoch.map_or(epoch, |p| p.max(epoch)));
            e.last_seen = Some(at.to_string());
        }
    }

    /// Lower a provider's trust after a caught defection (ADR-E4 D5/D4 revoke leg): its
    /// next item then takes the deeper verification path.
    pub fn lower_trust(&mut self, wgid: &str) {
        if let Some(e) = self.providers.get_mut(wgid) {
            e.trust_level = match e.trust_level {
                TrustLevel::Verified => TrustLevel::Provisional,
                TrustLevel::Provisional | TrustLevel::Unknown => TrustLevel::Unknown,
            };
        }
    }
}

// ── The five execution wire envelopes (ADR-E1 D6) ───────────────────────────────

/// **(1) `PlacementOffer`** — authorizer → provider: an offer to run task T, carrying
/// the required model/handler, isolation minimum, sensitivity label, the
/// authorizer-decided trust-floor + lease epoch. Signed by the authorizer (the
/// principal whose root it custodies). A provider acts on it only by *claiming*; the
/// offer authorizes nothing by itself (ADR-E1 D2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementOffer {
    pub v: u16,
    pub alg: String,
    pub exec_compat: String,
    pub task_id: String,
    /// `wgid:` of the authorizer / principal G.
    pub authorizer: String,
    /// `wgid:` of the provider this offer is pushed to.
    pub provider: String,
    pub required_model: String,
    pub min_isolation: IsolationClass,
    pub sensitivity: Sensitivity,
    /// The authorizer-decided trust-floor from the fail-closed leash (ADR-E1 D3).
    pub trust_floor: TrustLevel,
    pub lease_epoch: u64,
    pub created_at: String,
    #[serde(default)]
    pub sig: String,
}

/// **(2) `Claim`** — provider → authorizer: a *request* to run T, carrying the
/// provider's signed capability advertisement (ADR-E1 OQ3 eligibility proof). It does
/// **not** assert the provider's trust — trust is the authorizer's. The authorizer's
/// signed `RunGrant`, not this claim, is the placement decision (ADR-E1 D2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Claim {
    pub v: u16,
    pub alg: String,
    pub exec_compat: String,
    pub task_id: String,
    pub provider: String,
    /// The provider's signed capability advertisement (model + isolation + attested).
    pub capability: CapabilityAd,
    pub created_at: String,
    #[serde(default)]
    pub sig: String,
}

/// **(3) `RunGrant`** — authorizer → provider: the placement decision. Carries the
/// **two scoped attenuating UCANs** (act-as-agent + graph-write-task-only — **never the
/// root key, never a blanket graph write**), the **sealed context bundle** (the
/// minimal `ContextScope` slice sealed to the provider), and the signed lease. Signed
/// by the authorizer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunGrant {
    pub v: u16,
    pub alg: String,
    pub exec_compat: String,
    pub task_id: String,
    pub authorizer: String,
    pub provider: String,
    /// "run task T as agent G" — the impersonation surface, intent-bound + expiring.
    pub act_as_agent_ucan: crate::identity::custody::Capability,
    /// "graph/write on graph://task/T only" — the integrity surface; the structural
    /// blast-radius cap on a forged result (FR-V4). NEVER blanket graph write.
    pub graph_write_ucan: crate::identity::custody::Capability,
    pub bundle: SealedBundle,
    pub lease: Lease,
    pub created_at: String,
    #[serde(default)]
    pub sig: String,
}

/// What a field-scan of a `RunGrant`'s bytes finds (ADR-E3 D1, the spark step-1
/// assertion). The bytes must carry **no root key** and **no blanket graph-write**.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrantScan {
    /// True iff any custody-tagged private-key material appears in the serialized grant.
    pub contains_private_key_material: bool,
    /// True iff the graph-write UCAN grants graph write on `graph://*` (blanket) rather
    /// than a single task subtree.
    pub has_blanket_graph_write: bool,
    /// The exact resource the graph-write UCAN is scoped to (must be `graph://task/<T>`).
    pub graph_write_resource: String,
    /// The resource the act-as-agent UCAN is bound to (the intent binding).
    pub act_as_agent_resource: String,
}

/// **(4) `LeaseRenewal`** — provider → authorizer: a signed heartbeat carrying the
/// lease epoch the worker holds. Signed by the worker's delegated signer (the
/// act-as-agent UCAN's `aud`), so liveness is unforgeable by a relay (ADR-E3 D5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LeaseRenewal {
    pub v: u16,
    pub alg: String,
    pub exec_compat: String,
    pub task_id: String,
    pub epoch: u64,
    pub provider: String,
    pub created_at: String,
    #[serde(default)]
    pub sig: String,
}

/// Token/cost usage carried on a result so it is **not a bare "done" bit** (FR-V3).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cost_usd: f64,
}

/// **(5) `ResultEnvelope`** — provider → authorizer: the work product + usage +
/// **provenance** (which provider produced it, FR-V3/D4) + the lease epoch, **signed by
/// the worker's delegated act-as-agent signer** and carrying that UCAN as proof.
/// Attribution proves *who claims*, never *correctness* (ADR-E4 D1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultEnvelope {
    pub v: u16,
    pub alg: String,
    pub exec_compat: String,
    pub task_id: String,
    /// `wgid:` of the agent G the result is attributed to (the principal).
    pub agent: String,
    /// `wgid:` of the provider P that produced it — provenance (D4, the audit/revoke
    /// + cross-task-poison leg).
    pub producer: String,
    pub epoch: u64,
    /// The work product (diff / artifacts).
    pub work_product: String,
    /// The provider's *claim* that its tests pass — believed only after verification.
    #[serde(default)]
    pub claims_tests_pass: bool,
    pub usage: Usage,
    /// The act-as-agent UCAN proving the signer may act as G for this task (the
    /// attribution chain root is G's sigchain).
    pub act_as_agent_ucan: crate::identity::custody::Capability,
    /// The task-scoped graph-write UCAN the worker presents to authorize the write back
    /// (ADR-E3 D1). The accept boundary checks it permits `graph://task/<task_id>`; a
    /// write aimed at a *different* task is refused (the FR-C2/V4 blast-radius bound).
    pub graph_write_ucan: crate::identity::custody::Capability,
    pub created_at: String,
    #[serde(default)]
    pub sig: String,
}

// ── Envelope sign/verify (each follows the WG-Fed pattern: digest = canonical-minus-sig) ──

macro_rules! signed_envelope {
    ($t:ty, $what:literal) => {
        impl $t {
            fn to_value(&self) -> serde_json::Value {
                serde_json::to_value(self).expect(concat!($what, " serializes"))
            }
            /// Content id (`b3:<hex>`) of the canonical envelope.
            pub fn cid(&self) -> String {
                content_cid(&self.to_value())
            }
            /// Sign the envelope (sig field excluded from the digest) with a custody key.
            pub fn sign(
                &mut self,
                custodian: &crate::identity::keys::Custodian,
                signer_kid: &str,
            ) -> Result<()> {
                let digest = signing_digest(&self.to_value());
                self.sig = hex::encode(custodian.sign_digest(signer_kid, &digest)?);
                Ok(())
            }
            /// Verify the signature against an authorized signer of `auth` (the signer's
            /// sigchain). Pure local check, no central authority.
            pub fn verify_sig(&self, auth: &AuthorizedKeys) -> Result<()> {
                let digest = signing_digest(&self.to_value());
                verify_sig_authorized(&digest, &self.sig, auth, $what)
            }
        }
    };
}

signed_envelope!(PlacementOffer, "PlacementOffer");
signed_envelope!(Claim, "Claim");
signed_envelope!(RunGrant, "RunGrant");
signed_envelope!(LeaseRenewal, "LeaseRenewal");
signed_envelope!(ResultEnvelope, "ResultEnvelope");

impl RunGrant {
    /// Field-scan the grant's bytes for the step-1 assertion (ADR-E3 D1): **no root key,
    /// no blanket graph write**. Structural — inspects the two UCANs' scopes and scans
    /// the serialized grant for custody-tagged private-key material.
    pub fn field_scan(&self) -> GrantScan {
        let bytes = serde_json::to_string(self).unwrap_or_default();
        // The custody boundary tags every stored private key "ed25519:" / "x25519:"
        // (`keys.rs`); a "seed"/"private" field would likewise be a leak. None may appear.
        let contains_private_key_material = bytes.contains("ed25519:")
            || bytes.contains("x25519:")
            || bytes.contains("\"seed\"")
            || bytes.contains("\"private\"");
        let graph_write_resource = self
            .graph_write_ucan
            .scope
            .abilities
            .iter()
            .find(|a| a.can.starts_with("graph/") || a.can == "graph" || a.can == "*")
            .map(|a| a.with.clone())
            .unwrap_or_default();
        // Blanket graph write = the UCAN would permit a write to ANY task subtree.
        let has_blanket_graph_write = self
            .graph_write_ucan
            .scope
            .permits("graph/write", "graph://task/__probe_other__")
            && self
                .graph_write_ucan
                .scope
                .permits("graph/write", "graph://task/__probe_yet_another__");
        let act_as_agent_resource = self
            .act_as_agent_ucan
            .scope
            .abilities
            .iter()
            .find(|a| a.can == "act-as-agent")
            .map(|a| a.with.clone())
            .unwrap_or_default();
        GrantScan {
            contains_private_key_material,
            has_blanket_graph_write,
            graph_write_resource,
            act_as_agent_resource,
        }
    }
}

// ── Envelope builders (stamp the compat version into every envelope) ─────────────

impl PlacementOffer {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        task_id: &str,
        authorizer: &str,
        provider: &str,
        required_model: &str,
        min_isolation: IsolationClass,
        sensitivity: Sensitivity,
        trust_floor: TrustLevel,
        lease_epoch: u64,
        created_at: &str,
    ) -> Self {
        Self {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            exec_compat: WG_EXEC_COMPAT_VERSION.to_string(),
            task_id: task_id.to_string(),
            authorizer: authorizer.to_string(),
            provider: provider.to_string(),
            required_model: required_model.to_string(),
            min_isolation,
            sensitivity,
            trust_floor,
            lease_epoch,
            created_at: created_at.to_string(),
            sig: String::new(),
        }
    }
}

impl Claim {
    pub fn build(
        task_id: &str,
        provider: &str,
        capability: CapabilityAd,
        created_at: &str,
    ) -> Self {
        Self {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            exec_compat: WG_EXEC_COMPAT_VERSION.to_string(),
            task_id: task_id.to_string(),
            provider: provider.to_string(),
            capability,
            created_at: created_at.to_string(),
            sig: String::new(),
        }
    }
}

impl LeaseRenewal {
    pub fn build(task_id: &str, epoch: u64, provider: &str, created_at: &str) -> Self {
        Self {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            exec_compat: WG_EXEC_COMPAT_VERSION.to_string(),
            task_id: task_id.to_string(),
            epoch,
            provider: provider.to_string(),
            created_at: created_at.to_string(),
            sig: String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compat_accepts_same_and_patch_rejects_minor() {
        assert!(check_exec_compat(WG_EXEC_COMPAT_VERSION).is_ok());
        assert!(check_exec_compat("0.1.99").is_ok());
        assert!(check_exec_compat("0.2.0").is_err());
        assert!(check_exec_compat("nope").is_err());
    }

    #[test]
    fn trust_rank_orders_verified_above_unknown() {
        assert!(trust_rank(TrustLevel::Verified) > trust_rank(TrustLevel::Provisional));
        assert!(trust_rank(TrustLevel::Provisional) > trust_rank(TrustLevel::Unknown));
    }

    #[test]
    fn registry_never_trusts_an_unknown_provider() {
        let reg = ProviderRegistry::new();
        assert_eq!(reg.trust_of("wgid:zStranger"), TrustLevel::Unknown);
    }

    #[test]
    fn registry_liveness_is_renewal_observation() {
        let mut reg = ProviderRegistry::new();
        reg.enroll("wgid:zP", TrustLevel::Verified, None);
        // No renewal yet ⇒ not live at epoch 1.
        assert!(!reg.get("wgid:zP").unwrap().is_live(1));
        reg.record_renewal("wgid:zP", 1, "2026-06-26T00:00:00Z");
        assert!(reg.get("wgid:zP").unwrap().is_live(1));
        // A stale renewal does not satisfy a bumped epoch.
        assert!(!reg.get("wgid:zP").unwrap().is_live(2));
    }

    #[test]
    fn lower_trust_steps_down_the_dial() {
        let mut reg = ProviderRegistry::new();
        reg.enroll("wgid:zP", TrustLevel::Verified, None);
        reg.lower_trust("wgid:zP");
        assert_eq!(reg.trust_of("wgid:zP"), TrustLevel::Provisional);
        reg.lower_trust("wgid:zP");
        assert_eq!(reg.trust_of("wgid:zP"), TrustLevel::Unknown);
    }

    #[test]
    fn sensitivity_unlabeled_is_not_normal() {
        assert_eq!(Sensitivity::parse("normal"), Sensitivity::Normal);
        assert_eq!(Sensitivity::parse(""), Sensitivity::Unlabeled);
        assert_eq!(Sensitivity::parse("garbage"), Sensitivity::Unlabeled);
        assert_eq!(
            Sensitivity::parse("confidential"),
            Sensitivity::Confidential
        );
    }
}
