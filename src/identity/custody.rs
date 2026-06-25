//! UCAN-style capability delegation — the structural authority layer (Wave 6,
//! ADR-fed-003 §D3, doc 05 §7.1 "the best component in the study").
//!
//! A [`Capability`] is a signed, checkable, revocable, **expiring** certificate that
//! says *"agent X (`aud`) may act for principal Y (`iss`), scope S, until T"*
//! (FR-T4). It is the per-action authority token; the standing signer that wields it
//! is a separate, sigchain-`add_key`-authorized device key (see [`super::sigchain`]).
//! Capabilities are **off-chain** tokens (not appended to the append-only sigchain):
//! short-lived per-session caps would bloat the permanent chain and could not expire
//! cleanly. The chain authorizes the *keys*; the UCAN authorizes the *actions*.
//!
//! ## The integrity invariants (dial-independent — §D3, these are NOT the leash)
//!
//! - **Delegation never shares a private key** (FR-S1). A capability authorizes the
//!   `aud`'s *own* signer; it never hands over `iss`'s key. Custody is untouched.
//! - **Sub-delegation is attenuating-only** — a child capability's scope is always a
//!   subset of its parent's, and it **inherits the parent's expiry** (never extends
//!   it). This is what structurally kills the **hydra** (S-4): no chain of
//!   delegations can manufacture authority its root did not grant.
//! - **Revocation** is issuer-subtree-granular: revoking a capability kills it **and
//!   every capability delegated under it**, composed with by-expiry (the default) and
//!   sigchain `revoke_key`.
//! - **Accountability** — a capability records `iss`/`aud`, so an action is
//!   attributable to **both** the agent signer and the principal (NFR-7).
//!
//! ## The dial (the part §D2's amendment governs — the leash)
//!
//! [`LeashPolicy`] is the *scope-breadth* and *expiry* dial. Its **birth default is
//! broad and long** (agents and humans are first-class peers, not tools — Erik's
//! trust-default amendment): a freshly issued capability is broad-scope and
//! long-lived unless **environment policy** tightens it. Tightening (short expiries,
//! narrow scopes) is deployment-set policy, **never the birth default**, and
//! **humans are never leashed**. The integrity invariants above hold at *every* dial
//! setting — tightening only reduces blast radius / revocation latency; loosening
//! never reintroduces the hydra and never moves the root onto the worker.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::keys::{self, Custodian};
use super::sigchain::{AuthorizedKeys, KeyRole, KeyStatus};
use super::{ALG_ED25519, ENVELOPE_V, blake3_32, canonical_json, content_cid, signing_digest};

// ── Abilities & scope (the UCAN `{with, can}` attenuation lattice) ──────────────

/// One granted ability: an action (`can`) over a resource (`with`), the UCAN
/// `{with, can}` pair. The two abilities Wave 6 centres on are the **act-as-agent**
/// (`can = "act-as-agent"`, `with = "agent://<principal>"`) and **graph-write**
/// (`can = "graph/write"`, `with = "graph://<scope>"`) pair (ADR-fed-003 §D3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ability {
    /// Resource URI or prefix this ability is bound to (e.g. `graph://*`,
    /// `graph://task/abc`, `agent://wgid:…`). `*` is the unbounded resource.
    pub with: String,
    /// The action (e.g. `act-as-agent`, `graph/write`, `graph/read`, `msg/send`).
    /// `*` is the super-action; a trailing `/*` (e.g. `graph/*`) is a namespace.
    pub can: String,
}

impl Ability {
    pub fn new(can: &str, with: &str) -> Self {
        Self {
            with: with.to_string(),
            can: can.to_string(),
        }
    }
}

/// Whether a parent `can` subsumes a child `can` (parent ⊇ child).
///
/// `*` subsumes everything; an exact match subsumes itself; a namespace `ns/*`
/// subsumes every `ns/...` action.
fn can_subsumes(parent: &str, child: &str) -> bool {
    if parent == "*" || parent == child {
        return true;
    }
    if let Some(ns) = parent.strip_suffix("/*") {
        // "graph/*" subsumes "graph/write" and "graph/read".
        return child == ns || child.starts_with(&format!("{ns}/"));
    }
    false
}

/// Whether a parent resource subsumes a child resource (parent ⊇ child).
///
/// `*` (or a `<scheme>://*` wildcard) subsumes its sub-resources; an exact match
/// subsumes itself; otherwise the child must be a path *under* the parent.
fn resource_subsumes(parent: &str, child: &str) -> bool {
    if parent == "*" || parent == child {
        return true;
    }
    if let Some(prefix) = parent.strip_suffix('*') {
        // "graph://*" subsumes "graph://task/abc"; "graph://task/" subsumes deeper.
        return child.starts_with(prefix);
    }
    // Path-prefix containment: parent "graph://task" subsumes "graph://task/abc".
    child.starts_with(&format!("{parent}/"))
}

/// Whether a parent ability subsumes a child ability (both `can` and `with`).
fn ability_subsumes(parent: &Ability, child: &Ability) -> bool {
    can_subsumes(&parent.can, &child.can) && resource_subsumes(&parent.with, &child.with)
}

/// The scope of a capability: a set of granted abilities. Attenuation is defined
/// over scopes — a child scope is valid iff **every** child ability is subsumed by
/// **some** parent ability (the child can only narrow, never widen).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Scope {
    #[serde(default)]
    pub abilities: Vec<Ability>,
}

impl Scope {
    pub fn new(abilities: Vec<Ability>) -> Self {
        Self { abilities }
    }

    /// The broad birth-default scope (the leash amendment's slack end): act as the
    /// principal anywhere and write the whole graph. Environment policy narrows this.
    pub fn broad_default(principal: &str) -> Self {
        Self::new(vec![
            Ability::new("act-as-agent", &format!("agent://{principal}")),
            Ability::new("graph/*", "graph://*"),
            Ability::new("msg/send", "msg://*"),
        ])
    }

    /// Does `self` (a parent) subsume `child` — i.e. is `child` a valid attenuation?
    /// Every child ability must be subsumed by some ability of `self`.
    pub fn subsumes(&self, child: &Scope) -> bool {
        child
            .abilities
            .iter()
            .all(|c| self.abilities.iter().any(|p| ability_subsumes(p, c)))
    }

    /// Does this scope authorize `(can, with)` for an actor? Used by a relying party
    /// after verifying the chain ("present the cap, then act").
    pub fn permits(&self, can: &str, with: &str) -> bool {
        let want = Ability::new(can, with);
        self.abilities.iter().any(|p| ability_subsumes(p, &want))
    }

    /// Intersect this scope (a request) with a policy `ceiling` — the **meet** of the
    /// two lattices. For each comparable (request, ceiling) pair the *narrower*
    /// ability is kept (a request within the ceiling stays; a request broader than the
    /// ceiling is capped to the ceiling); incomparable pairs grant nothing. The result
    /// never exceeds either input, so a tightened [`LeashPolicy`] can only narrow a
    /// request, never widen it.
    fn clamp_to(&self, ceiling: &Scope) -> Scope {
        let mut out: Vec<Ability> = Vec::new();
        for r in &self.abilities {
            for c in &ceiling.abilities {
                let meet = if ability_subsumes(c, r) {
                    Some(r.clone()) // request within ceiling → keep the narrower request
                } else if ability_subsumes(r, c) {
                    Some(c.clone()) // request broader than ceiling → cap to the ceiling
                } else {
                    None // incomparable → no grant
                };
                if let Some(a) = meet {
                    if !out.contains(&a) {
                        out.push(a);
                    }
                }
            }
        }
        Scope::new(out)
    }
}

// ── The leash dial (ADR-fed-003 §D2 — broad/long by birth, env-tightenable) ─────

/// Default broad/long expiry: 90 days (the first-class-peer birth default — NOT a
/// per-session leash). The dial tightens this; it is never the default.
pub const BROAD_DEFAULT_TTL_SECS: i64 = 90 * 24 * 60 * 60;

/// The authority-scope dial (§D2). **Default = broad scope + long expiry** (slack);
/// environment policy may set a `max_ttl_secs` ceiling and/or a `scope_ceiling` to
/// tighten. Custody (root-key safety) is unchanged at every setting; this governs
/// only *delegated* authority breadth/lifetime — humans (who self-hold their root)
/// are never leashed.
#[derive(Debug, Clone)]
pub struct LeashPolicy {
    /// The TTL applied to a capability when the issuer does not request one. Broad by
    /// default (`BROAD_DEFAULT_TTL_SECS`).
    pub default_ttl_secs: i64,
    /// An upper bound on any issued/requested TTL. `None` = no ceiling (the slack
    /// default); `Some(t)` is the leash — a requested longer TTL is clamped down.
    pub max_ttl_secs: Option<i64>,
    /// A scope ceiling the issued scope is intersected with. `None` = no ceiling
    /// (slack); `Some(s)` is the leash — abilities outside the ceiling are dropped.
    pub scope_ceiling: Option<Scope>,
}

impl Default for LeashPolicy {
    fn default() -> Self {
        Self::birth_default()
    }
}

impl LeashPolicy {
    /// The amendment's **birth default**: broad scope, long expiry, no ceiling. A
    /// newly-created agent is a first-class peer, not a tool on a per-session leash.
    pub fn birth_default() -> Self {
        Self {
            default_ttl_secs: BROAD_DEFAULT_TTL_SECS,
            max_ttl_secs: None,
            scope_ceiling: None,
        }
    }

    /// Whether this policy is the slack birth default (no environment tightening).
    pub fn is_slack(&self) -> bool {
        self.max_ttl_secs.is_none() && self.scope_ceiling.is_none()
    }

    /// Read the leash from the environment (the §D2 "environment-driven policy").
    /// Unset env ⇒ the broad/long birth default (slack). Recognized:
    ///
    /// - `WG_FED_LEASH_MAX_TTL_SECS` — clamp every capability TTL to at most this.
    /// - `WG_FED_LEASH_SCOPE` — a `;`-separated `can@with` allowlist ceiling; an
    ///   issued scope is intersected with it (abilities outside are dropped).
    ///
    /// This is the *only* place the default value of the dial is moved — and it is
    /// moved by deployment, never at birth.
    pub fn from_env() -> Self {
        let mut p = Self::birth_default();
        if let Ok(v) = std::env::var("WG_FED_LEASH_MAX_TTL_SECS") {
            if let Ok(secs) = v.trim().parse::<i64>() {
                if secs > 0 {
                    p.max_ttl_secs = Some(secs);
                }
            }
        }
        if let Ok(v) = std::env::var("WG_FED_LEASH_SCOPE") {
            let abilities: Vec<Ability> = v
                .split(';')
                .filter_map(|tok| {
                    let tok = tok.trim();
                    if tok.is_empty() {
                        return None;
                    }
                    tok.split_once('@')
                        .map(|(can, with)| Ability::new(can.trim(), with.trim()))
                })
                .collect();
            if !abilities.is_empty() {
                p.scope_ceiling = Some(Scope::new(abilities));
            }
        }
        p
    }

    /// Apply the dial to an issuance request, returning the effective `(scope, ttl)`.
    ///
    /// - `humans_never_leashed`: a human principal self-holds its root and is
    ///   sovereign — the dial governs only *delegated* (agent) authority, so a human
    ///   subject is returned untouched (§D2).
    /// - Otherwise: TTL is clamped to `max_ttl_secs` (if set) and the requested (or
    ///   default-broad) scope is intersected with `scope_ceiling` (if set). With the
    ///   slack default both are no-ops — the request passes through broad/long.
    pub fn apply(
        &self,
        requested_scope: Scope,
        requested_ttl_secs: Option<i64>,
        subject_is_human: bool,
    ) -> (Scope, i64) {
        let ttl = requested_ttl_secs.unwrap_or(self.default_ttl_secs);
        if subject_is_human {
            // Humans are never leashed — return the request verbatim (§D2).
            return (requested_scope, ttl);
        }
        let ttl = match self.max_ttl_secs {
            Some(max) if ttl > max => max,
            _ => ttl,
        };
        let scope = match &self.scope_ceiling {
            Some(ceiling) => requested_scope.clamp_to(ceiling),
            None => requested_scope,
        };
        (scope, ttl)
    }
}

// ── The capability certificate ──────────────────────────────────────────────────

/// A UCAN-style capability certificate (ADR-fed-003 §D3). Signed by an authorized
/// signer of `iss`; verified offline against `iss`'s sigchain-authorized key set.
/// `proof` embeds the parent capability for a sub-delegation (the chain travels with
/// the token, UCAN-style), or is `None` for a root grant by the principal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub v: u16,
    pub alg: String,
    /// Issuer `wgid:` — the principal (root grant) or a delegate (sub-delegation).
    pub iss: String,
    /// Audience `wgid:` — the agent/delegate receiving the authority.
    pub aud: String,
    /// The granted scope (subset of the parent's, if any).
    pub scope: Scope,
    /// RFC3339 instant before which the capability is not yet valid.
    pub not_before: String,
    /// RFC3339 expiry. A child's expiry never exceeds its parent's (§D3).
    pub expires: String,
    /// Uniqueness nonce (so two caps with identical fields get distinct CIDs).
    pub nonce: String,
    /// The parent capability for a sub-delegation (embedded, UCAN `prf`-style), or
    /// `None` for a root grant signed directly by the principal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<Box<Capability>>,
    /// ed25519 signature over the canonical capability (sig removed) by an authorized
    /// signer of `iss`, hex.
    #[serde(default)]
    pub sig: String,
}

impl Capability {
    fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("Capability serializes")
    }

    /// Content id (`b3:<hex>`) — the stable id a revocation names.
    pub fn cid(&self) -> String {
        content_cid(&self.to_value())
    }

    fn sign(&mut self, custodian: &Custodian, signer_kid: &str) -> Result<()> {
        let digest = signing_digest(&self.to_value());
        self.sig = hex::encode(custodian.sign_digest(signer_kid, &digest)?);
        Ok(())
    }

    /// The depth of the delegation chain (1 = a root grant).
    pub fn chain_len(&self) -> usize {
        1 + self.proof.as_ref().map(|p| p.chain_len()).unwrap_or(0)
    }
}

/// Parse an RFC3339 instant or bail loudly.
fn parse_ts(s: &str, what: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|e| anyhow::anyhow!("{what} {s:?} is not RFC3339: {e}"))
}

/// A fresh 16-byte hex nonce. `seed` varies the value per call site (no `rand`).
fn nonce_from(seed: &str) -> Result<String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|e| anyhow::anyhow!("CSPRNG unavailable: {e}"))?;
    // Mix the seed in so a caller can make it deterministic in a test if needed.
    let mixed = blake3_32(&[seed.as_bytes(), &b].concat());
    Ok(hex::encode(&mixed[..16]))
}

/// Issue a **root** capability: principal `iss` grants `aud` a scope, applying the
/// leash dial (`policy`). Signed by an authorized signer of `iss` held in custody.
/// `subject_is_human` routes through the "humans are never leashed" branch (§D2).
#[allow(clippy::too_many_arguments)]
pub fn issue_root(
    custodian: &Custodian,
    signer_kid: &str,
    iss: &str,
    aud: &str,
    requested_scope: Scope,
    requested_ttl_secs: Option<i64>,
    now: chrono::DateTime<chrono::Utc>,
    policy: &LeashPolicy,
    subject_is_human: bool,
) -> Result<Capability> {
    let (scope, ttl) = policy.apply(requested_scope, requested_ttl_secs, subject_is_human);
    let expires = now + chrono::Duration::seconds(ttl);
    let mut cap = Capability {
        v: ENVELOPE_V,
        alg: ALG_ED25519.to_string(),
        iss: iss.to_string(),
        aud: aud.to_string(),
        scope,
        not_before: now.to_rfc3339(),
        expires: expires.to_rfc3339(),
        nonce: nonce_from(&format!("{iss}->{aud}"))?,
        proof: None,
        sig: String::new(),
    };
    cap.sign(custodian, signer_kid)?;
    Ok(cap)
}

/// Sub-delegate (attenuate) `parent` to a new audience with a narrowed scope.
///
/// Enforced structurally (§D3): the new scope MUST be a subset of the parent's
/// (attenuating-only — refused otherwise), and the child's expiry is **clamped to
/// the parent's** (a child can never outlive its parent). The new issuer is the
/// parent's audience (the delegate sub-delegating its own grant), signed by *that*
/// identity's signer in custody — never the parent's key.
#[allow(clippy::too_many_arguments)]
pub fn delegate(
    custodian: &Custodian,
    signer_kid: &str,
    parent: &Capability,
    new_aud: &str,
    requested_scope: Scope,
    requested_ttl_secs: Option<i64>,
    now: chrono::DateTime<chrono::Utc>,
    policy: &LeashPolicy,
) -> Result<Capability> {
    // The delegator is the parent's audience.
    let iss = parent.aud.clone();
    // Attenuating-only: child ⊆ parent (the structural hydra kill, S-4).
    if !parent.scope.subsumes(&requested_scope) {
        bail!(
            "refusing to delegate: requested scope is NOT a subset of the parent \
             capability's scope — sub-delegation is attenuating-only (it can narrow, \
             never widen; the structural hydra kill, ADR-fed-003 §D3)"
        );
    }
    let (scope, ttl) = policy.apply(requested_scope, requested_ttl_secs, false);
    // Inherit (clamp to) the parent's expiry — a child never outlives its parent.
    let parent_expires = parse_ts(&parent.expires, "parent.expires")?;
    let mut expires = now + chrono::Duration::seconds(ttl);
    if expires > parent_expires {
        expires = parent_expires;
    }
    let mut cap = Capability {
        v: ENVELOPE_V,
        alg: ALG_ED25519.to_string(),
        iss,
        aud: new_aud.to_string(),
        scope,
        not_before: now.to_rfc3339(),
        expires: expires.to_rfc3339(),
        nonce: nonce_from(&format!("{}->{new_aud}", parent.aud))?,
        proof: Some(Box::new(parent.clone())),
        sig: String::new(),
    };
    cap.sign(custodian, signer_kid)?;
    Ok(cap)
}

// ── Revocation (issuer-subtree, §D3) ────────────────────────────────────────────

/// A signed revocation of a capability by CID (ADR-fed-003 §D3). Revoking a
/// capability kills it **and every capability delegated under it** (issuer-subtree).
/// Signed by `revoked_by` — which must be the issuer of the named capability.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Revocation {
    pub v: u16,
    pub alg: String,
    /// CID of the capability being revoked.
    pub cap_cid: String,
    /// `wgid:` of the revoker — must be the `iss` of the revoked capability.
    pub revoked_by: String,
    pub at: String,
    #[serde(default)]
    pub sig: String,
}

impl Revocation {
    fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("Revocation serializes")
    }

    pub fn cid(&self) -> String {
        content_cid(&self.to_value())
    }

    /// Build + sign a revocation of `cap` by `cap.iss` (held in custody).
    pub fn issue(
        custodian: &Custodian,
        signer_kid: &str,
        cap: &Capability,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Self> {
        let mut rev = Revocation {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            cap_cid: cap.cid(),
            revoked_by: cap.iss.clone(),
            at: now.to_rfc3339(),
            sig: String::new(),
        };
        let digest = signing_digest(&rev.to_value());
        rev.sig = hex::encode(custodian.sign_digest(signer_kid, &digest)?);
        Ok(rev)
    }

    /// Verify the revocation's signature against `revoker_auth` (the authorized signer
    /// set of `revoked_by`). A revocation the verifier cannot authenticate is ignored.
    pub fn verify_signature(&self, revoker_auth: &AuthorizedKeys) -> Result<()> {
        if self.revoked_by != keys::wgid_from_pubkey(&revoker_auth.root_pub) {
            bail!("revocation.revoked_by does not match the resolved sigchain root");
        }
        let digest = signing_digest(&self.to_value());
        verify_against_authorized(&digest, &self.sig, revoker_auth, "Revocation")
    }
}

// ── Verification ────────────────────────────────────────────────────────────────

/// The result of verifying a capability chain.
#[derive(Debug, Clone, PartialEq)]
pub struct Verified {
    /// The effective granted scope (the leaf scope — a subset of every ancestor).
    pub granted: Scope,
    /// The leaf audience (the agent ultimately authorized).
    pub aud: String,
    /// The root principal the authority descends from (the top-of-chain `iss`).
    pub principal: String,
    /// The chain depth.
    pub chain_len: usize,
}

/// Verify a capability chain **offline** (ADR-fed-003 §D3). For each link, root →
/// leaf:
///
/// 1. its signature verifies against an authorized signer of its `iss` (resolved via
///    `resolve_auth`, which returns the issuer's sigchain-authorized key set);
/// 2. attenuation holds — a child scope is a subset of its parent (refused otherwise);
/// 3. the child's `iss` equals the parent's `aud` (the chain is connected);
/// 4. the child's expiry does not exceed the parent's;
/// 5. `now` is within `[not_before, expires]` (not-yet-valid / expired ⇒ refused —
///    this is what makes a stolen signer **near-worthless after expiry**);
/// 6. neither this capability's CID nor any ancestor's CID is in `revoked` (the
///    issuer-subtree revocation — killing a parent kills the whole subtree).
///
/// Returns the effective granted scope (the leaf's, already ⊆ every ancestor).
/// `resolve_auth` is a closure so verification stays pure-local — it can read from a
/// cache, a `FedStore`, or a fixed map, but it never contacts a central authority.
pub fn verify(
    leaf: &Capability,
    now: chrono::DateTime<chrono::Utc>,
    revoked: &[String],
    resolve_auth: &dyn Fn(&str) -> Result<AuthorizedKeys>,
) -> Result<Verified> {
    verify_inner(leaf, now, revoked, resolve_auth)?;
    // Walk to the root to report the principal.
    let mut cur = leaf;
    while let Some(p) = cur.proof.as_deref() {
        cur = p;
    }
    Ok(Verified {
        granted: leaf.scope.clone(),
        aud: leaf.aud.clone(),
        principal: cur.iss.clone(),
        chain_len: leaf.chain_len(),
    })
}

fn verify_inner(
    cap: &Capability,
    now: chrono::DateTime<chrono::Utc>,
    revoked: &[String],
    resolve_auth: &dyn Fn(&str) -> Result<AuthorizedKeys>,
) -> Result<()> {
    if cap.alg != ALG_ED25519 {
        bail!("capability alg {:?} unsupported", cap.alg);
    }
    // 6 (this link). Revocation — the named CID kills this cap and (via the recursion
    // below) anything delegated under it.
    let cid = cap.cid();
    if revoked.iter().any(|r| r == &cid) {
        bail!(
            "capability {cid} is REVOKED (issuer-subtree revocation, ADR-fed-003 §D3) \
             — refused"
        );
    }
    // 1. Signature by an authorized signer of `iss`.
    let iss_auth = resolve_auth(&cap.iss)
        .map_err(|e| anyhow::anyhow!("resolving issuer {} of capability: {e}", cap.iss))?;
    if cap.iss != keys::wgid_from_pubkey(&iss_auth.root_pub) {
        bail!("capability.iss does not match the resolved sigchain root");
    }
    let digest = signing_digest(&cap.to_value());
    verify_against_authorized(&digest, &cap.sig, &iss_auth, "Capability")?;

    // 5. Temporal validity (the expiry that makes a stolen signer worthless).
    let nbf = parse_ts(&cap.not_before, "not_before")?;
    let exp = parse_ts(&cap.expires, "expires")?;
    if now < nbf {
        bail!(
            "capability is not yet valid (not_before {})",
            cap.not_before
        );
    }
    if now > exp {
        bail!(
            "capability EXPIRED at {} — refused (a stolen signer is near-worthless \
             after expiry, ADR-fed-003 §D3)",
            cap.expires
        );
    }

    // Recurse into the parent (root grant has no parent).
    if let Some(parent) = cap.proof.as_deref() {
        // 3. Connected chain: child.iss == parent.aud.
        if cap.iss != parent.aud {
            bail!(
                "broken delegation chain: capability.iss {:?} != parent.aud {:?}",
                cap.iss,
                parent.aud
            );
        }
        // 2. Attenuating-only: child scope ⊆ parent scope.
        if !parent.scope.subsumes(&cap.scope) {
            bail!(
                "capability widens its parent's scope — sub-delegation is \
                 attenuating-only (the hydra kill, ADR-fed-003 §D3); refused"
            );
        }
        // 4. Child never outlives parent.
        let parent_exp = parse_ts(&parent.expires, "parent.expires")?;
        if exp > parent_exp {
            bail!("capability outlives its parent (expiry exceeds parent's); refused");
        }
        verify_inner(parent, now, revoked, resolve_auth)?;
    }
    Ok(())
}

/// Shared: verify `sig_hex` over `digest` against any active signer (or the active
/// root) of `auth`.
fn verify_against_authorized(
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

/// Parse a `can@with` ability token (CLI helper).
pub fn parse_ability(tok: &str) -> Result<Ability> {
    let (can, with) = tok
        .split_once('@')
        .ok_or_else(|| anyhow::anyhow!("ability {tok:?} must be in the form can@resource"))?;
    let can = can.trim();
    let with = with.trim();
    if can.is_empty() || with.is_empty() {
        bail!("ability {tok:?} has an empty can or resource");
    }
    Ok(Ability::new(can, with))
}

/// The canonical wire bytes (`canonical_json`) of a capability — for content-address
/// stable storage / transport.
pub fn capability_bytes(cap: &Capability) -> Vec<u8> {
    canonical_json(&cap.to_value())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::sigchain::{KeyEntry, add_key, genesis, verify as verify_chain};

    struct Party {
        wgid: String,
        cust: Custodian,
        signer_kid: String,
        auth: AuthorizedKeys,
    }

    fn mint(name: &str) -> Party {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        std::mem::forget(tmp);
        let cust = Custodian::with_keystore_dir(name, dir);
        let root = keys::gen_ed25519().unwrap();
        let root_kid = keys::kid_for(&root.public);
        cust.store_signing_key(&root_kid, &root.seed).unwrap();
        let wgid = keys::wgid_from_pubkey(&root.public);
        let g = genesis(&cust, &root.public, &root_kid, None).unwrap();
        let signer = keys::gen_ed25519().unwrap();
        let signer_kid = keys::kid_for(&signer.public);
        cust.store_signing_key(&signer_kid, &signer.seed).unwrap();
        let l1 = add_key(
            &cust,
            &g,
            &root.public,
            &root_kid,
            KeyEntry {
                kid: signer_kid.clone(),
                public: hex::encode(signer.public),
                role: KeyRole::Signer,
                scope: vec!["event".into()],
                status: KeyStatus::Active,
            },
        )
        .unwrap();
        let auth = verify_chain(&[g, l1], &wgid).unwrap();
        Party {
            wgid,
            cust,
            signer_kid,
            auth,
        }
    }

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-06-25T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    /// A resolver over a fixed set of parties (the offline cache, no store needed).
    fn resolver<'a>(parties: &'a [&'a Party]) -> impl Fn(&str) -> Result<AuthorizedKeys> + 'a {
        move |wgid: &str| {
            parties
                .iter()
                .find(|p| p.wgid == wgid)
                .map(|p| p.auth.clone())
                .ok_or_else(|| anyhow::anyhow!("unknown issuer {wgid}"))
        }
    }

    #[test]
    fn subsumption_lattice() {
        let parent = Scope::new(vec![Ability::new("graph/*", "graph://*")]);
        // narrower can + narrower resource ⊆ parent
        assert!(parent.subsumes(&Scope::new(vec![Ability::new(
            "graph/write",
            "graph://task/abc"
        )])));
        // a different namespace is NOT subsumed
        assert!(!parent.subsumes(&Scope::new(vec![Ability::new("msg/send", "msg://x")])));
        // widening the resource scheme wildcard is NOT subsumed
        assert!(
            !Scope::new(vec![Ability::new("graph/write", "graph://task/a")])
                .subsumes(&Scope::new(vec![Ability::new("graph/write", "graph://*")]))
        );
    }

    #[test]
    fn issue_and_verify_root() {
        let alice = mint("alice_cap");
        let agent = mint("agent_cap");
        let scope = Scope::broad_default(&alice.wgid);
        let cap = issue_root(
            &alice.cust,
            &alice.signer_kid,
            &alice.wgid,
            &agent.wgid,
            scope,
            Some(3600),
            now(),
            &LeashPolicy::birth_default(),
            false,
        )
        .unwrap();
        let v = verify(&cap, now(), &[], &resolver(&[&alice, &agent])).unwrap();
        assert_eq!(v.aud, agent.wgid);
        assert_eq!(v.principal, alice.wgid);
        assert!(v.granted.permits("graph/write", "graph://task/x"));
    }

    #[test]
    fn expired_capability_is_worthless() {
        let alice = mint("alice_exp");
        let agent = mint("agent_exp");
        let cap = issue_root(
            &alice.cust,
            &alice.signer_kid,
            &alice.wgid,
            &agent.wgid,
            Scope::broad_default(&alice.wgid),
            Some(60), // 60-second TTL
            now(),
            &LeashPolicy::birth_default(),
            false,
        )
        .unwrap();
        // Valid now…
        assert!(verify(&cap, now(), &[], &resolver(&[&alice, &agent])).is_ok());
        // …worthless 2 minutes later (the stolen-signer-after-expiry property).
        let later = now() + chrono::Duration::seconds(120);
        let err = verify(&cap, later, &[], &resolver(&[&alice, &agent]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("EXPIRED"), "{err}");
    }

    #[test]
    fn delegation_is_attenuating_only() {
        let alice = mint("alice_att");
        let agent = mint("agent_att");
        let sub = mint("sub_att");
        let cap = issue_root(
            &alice.cust,
            &alice.signer_kid,
            &alice.wgid,
            &agent.wgid,
            Scope::new(vec![Ability::new("graph/write", "graph://task/abc")]),
            Some(3600),
            now(),
            &LeashPolicy::birth_default(),
            false,
        )
        .unwrap();
        // Narrowing (same resource, no widening) is allowed.
        let narrowed = Scope::new(vec![Ability::new("graph/write", "graph://task/abc")]);
        let child = delegate(
            &agent.cust,
            &agent.signer_kid,
            &cap,
            &sub.wgid,
            narrowed,
            Some(3600),
            now(),
            &LeashPolicy::birth_default(),
        )
        .unwrap();
        assert!(verify(&child, now(), &[], &resolver(&[&alice, &agent, &sub])).is_ok());

        // WIDENING is structurally refused at delegate() time…
        let widened = Scope::new(vec![Ability::new("graph/*", "graph://*")]);
        assert!(
            delegate(
                &agent.cust,
                &agent.signer_kid,
                &cap,
                &sub.wgid,
                widened.clone(),
                Some(3600),
                now(),
                &LeashPolicy::birth_default(),
            )
            .is_err()
        );

        // …and even a hand-forged widened child fails verification (defence in depth).
        let mut forged = child.clone();
        forged.scope = widened;
        forged.sig = String::new();
        forged.sign(&agent.cust, &agent.signer_kid).unwrap();
        assert!(verify(&forged, now(), &[], &resolver(&[&alice, &agent, &sub])).is_err());
    }

    #[test]
    fn child_never_outlives_parent() {
        let alice = mint("alice_ttl");
        let agent = mint("agent_ttl");
        let sub = mint("sub_ttl");
        let cap = issue_root(
            &alice.cust,
            &alice.signer_kid,
            &alice.wgid,
            &agent.wgid,
            Scope::broad_default(&alice.wgid),
            Some(100), // parent expires in 100s
            now(),
            &LeashPolicy::birth_default(),
            false,
        )
        .unwrap();
        // Ask for a much longer child TTL — it must be clamped to the parent's expiry.
        let child = delegate(
            &agent.cust,
            &agent.signer_kid,
            &cap,
            &sub.wgid,
            Scope::broad_default(&alice.wgid),
            Some(10_000),
            now(),
            &LeashPolicy::birth_default(),
        )
        .unwrap();
        let child_exp = parse_ts(&child.expires, "x").unwrap();
        let parent_exp = parse_ts(&cap.expires, "x").unwrap();
        assert!(child_exp <= parent_exp, "child outlived parent");
    }

    #[test]
    fn revocation_kills_subtree() {
        let alice = mint("alice_rev");
        let agent = mint("agent_rev");
        let sub = mint("sub_rev");
        let cap = issue_root(
            &alice.cust,
            &alice.signer_kid,
            &alice.wgid,
            &agent.wgid,
            Scope::broad_default(&alice.wgid),
            Some(3600),
            now(),
            &LeashPolicy::birth_default(),
            false,
        )
        .unwrap();
        let child = delegate(
            &agent.cust,
            &agent.signer_kid,
            &cap,
            &sub.wgid,
            Scope::broad_default(&alice.wgid),
            Some(3600),
            now(),
            &LeashPolicy::birth_default(),
        )
        .unwrap();
        let parties = [&alice, &agent, &sub];
        // Both valid before revocation.
        assert!(verify(&child, now(), &[], &resolver(&parties)).is_ok());

        // Alice revokes the ROOT cap → the whole subtree (incl. child) dies.
        let rev = Revocation::issue(&alice.cust, &alice.signer_kid, &cap, now()).unwrap();
        assert!(rev.verify_signature(&alice.auth).is_ok());
        let revoked = [rev.cap_cid.clone()];
        assert!(verify(&cap, now(), &revoked, &resolver(&parties)).is_err());
        assert!(
            verify(&child, now(), &revoked, &resolver(&parties)).is_err(),
            "revoking the parent must kill the delegated subtree"
        );
    }

    #[test]
    fn leash_slack_by_default_tightenable_by_policy() {
        let alice = mint("alice_leash");
        let agent = mint("agent_leash");
        // Birth default: broad scope passes through, long TTL.
        let slack = LeashPolicy::birth_default();
        assert!(slack.is_slack());
        let (scope, ttl) = slack.apply(Scope::broad_default(&alice.wgid), None, false);
        assert_eq!(ttl, BROAD_DEFAULT_TTL_SECS);
        assert!(scope.permits("graph/write", "graph://anything"));

        // A tightened (environment) policy: short TTL ceiling + scope ceiling.
        let tight = LeashPolicy {
            default_ttl_secs: BROAD_DEFAULT_TTL_SECS,
            max_ttl_secs: Some(900),
            scope_ceiling: Some(Scope::new(vec![Ability::new(
                "graph/read",
                "graph://task/abc",
            )])),
        };
        assert!(!tight.is_slack());
        let (scope, ttl) = tight.apply(Scope::broad_default(&alice.wgid), Some(100_000), false);
        assert_eq!(ttl, 900, "TTL must be clamped to the leash ceiling");
        // The broad request is narrowed to only what the ceiling permits.
        assert!(!scope.permits("graph/write", "graph://anything"));
        assert!(scope.permits("graph/read", "graph://task/abc"));

        // Humans are NEVER leashed — even under the tight policy the request passes.
        let (hscope, httl) = tight.apply(Scope::broad_default(&alice.wgid), Some(100_000), true);
        assert_eq!(httl, 100_000);
        assert!(hscope.permits("graph/write", "graph://anything"));

        // Issue a real cap under the broad default → long-lived & broad.
        let cap = issue_root(
            &alice.cust,
            &alice.signer_kid,
            &alice.wgid,
            &agent.wgid,
            Scope::broad_default(&alice.wgid),
            None,
            now(),
            &slack,
            false,
        )
        .unwrap();
        let v = verify(&cap, now(), &[], &resolver(&[&alice, &agent])).unwrap();
        assert!(
            v.granted
                .permits("act-as-agent", &format!("agent://{}", alice.wgid))
        );
    }

    #[test]
    fn forged_issuer_signature_is_rejected() {
        let alice = mint("alice_forge");
        let agent = mint("agent_forge");
        let mallory = mint("mallory_forge");
        // Mallory tries to issue a cap CLAIMING alice as the issuer, signing with her
        // own key. Verification resolves alice's chain → mallory's sig fails.
        let mut cap = Capability {
            v: ENVELOPE_V,
            alg: ALG_ED25519.to_string(),
            iss: alice.wgid.clone(),
            aud: agent.wgid.clone(),
            scope: Scope::broad_default(&alice.wgid),
            not_before: now().to_rfc3339(),
            expires: (now() + chrono::Duration::seconds(3600)).to_rfc3339(),
            nonce: "deadbeef".into(),
            proof: None,
            sig: String::new(),
        };
        cap.sign(&mallory.cust, &mallory.signer_kid).unwrap();
        assert!(verify(&cap, now(), &[], &resolver(&[&alice, &agent, &mallory])).is_err());
    }
}
