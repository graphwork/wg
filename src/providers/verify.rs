//! Result integrity & the verification leash (ADR-E4).
//!
//! Two layers, and the design states the limit of the first **in the design, not a
//! footnote**:
//!
//! 1. **Attribution** ([`verify_attribution`]) proves *who claims* a result — the
//!    delegated act-as-agent signer, chained to G's sigchain. It is **mandatory** (an
//!    unsigned / wrong-signed / expired result is rejected) but **is NOT integrity**:
//!    the signer lives on the provider's box, so a valid signature proves origin-of-
//!    claim, never correctness (ADR-E4 D1).
//! 2. **A trust-proportional re-run** ([`verify_result`]) for the low-trust row: a
//!    **deterministic re-run in a TRUSTED DOMAIN — authorizer-side or a *disjoint*
//!    trusted provider, NEVER the producer (X-5) — against the authorizer's *pinned*
//!    spec, not the provider's shipped tests (X-6)**. The bar is equivalence at the spec
//!    level, not byte-identity. Any test-file change in the diff is split out and flagged
//!    (tests are spec); provenance records the producer so a later-discovered poison's
//!    descendants can be found and re-run (D4/D6). The integrity guarantee's real shape
//!    is **bound + audit + revoke**, not "detect every forgery up front".
//!
//! Quorum is deferred to v2 (it needs unsolved sybil-resistance); the v1 low-trust lever
//! is the single disjoint re-run (ADR-E4 D7).

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::identity::custody::{self, Capability};
use crate::identity::sigchain::AuthorizedKeys;

use super::{ResultEnvelope, TrustLevel};

/// The verdict of authenticating *who produced* a result (ADR-E4 D1). `ok` true means
/// the result is attributable to `agent` via the provider `producer`'s delegated signer —
/// nothing about correctness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttributionVerdict {
    pub ok: bool,
    pub agent: String,
    pub producer: String,
    /// A bounded reason code, never attacker prose: `attributed`,
    /// `unsigned-or-wrong-signed`, `ucan-invalid-or-expired`, `principal-mismatch`,
    /// `aud-mismatch`, `not-act-as-agent`, `producer-unresolved`.
    pub reason: String,
}

/// Verify a result's attribution (ADR-E4 D1) — *who* produced it, not *which task*.
///
/// Attribution = the act-as-agent UCAN verifies at `now` (so an **expired** UCAN is
/// rejected — the leash-elapsed case), is issued by `agent` (G) to `producer` (P),
/// grants `act-as-agent` for G, **and** the envelope signature verifies against the
/// producer's authorized signer set. An unsigned / wrong-signed / expired result fails
/// closed. The *task* scope (which task this write may land on) is the dedicated
/// [`authorize_graph_write`] gate — the FR-V4 blast-radius bound — kept separate so the
/// over-scope write to a different task is attributable but refused on scope.
pub fn verify_attribution(
    result: &ResultEnvelope,
    now: chrono::DateTime<chrono::Utc>,
    revoked: &[String],
    resolve_auth: &dyn Fn(&str) -> Result<AuthorizedKeys>,
) -> AttributionVerdict {
    let mut v = AttributionVerdict {
        ok: false,
        agent: result.agent.clone(),
        producer: result.producer.clone(),
        reason: String::new(),
    };

    // 1. The act-as-agent UCAN must verify (chain to G, not expired, not revoked).
    let verified = match custody::verify(&result.act_as_agent_ucan, now, revoked, resolve_auth) {
        Ok(ver) => ver,
        Err(_) => {
            v.reason = "ucan-invalid-or-expired".into();
            return v;
        }
    };
    // 2. The UCAN is issued by G (principal) to P (aud).
    if verified.principal != result.agent {
        v.reason = "principal-mismatch".into();
        return v;
    }
    if verified.aud != result.producer {
        v.reason = "aud-mismatch".into();
        return v;
    }
    // 3. The UCAN actually grants act-as-agent for G (not some unrelated capability).
    //    The task scope is enforced separately by `authorize_graph_write` (FR-V4).
    let grants_act_as_agent = verified.granted.abilities.iter().any(|a| {
        a.can == "act-as-agent" && a.with.starts_with(&format!("agent://{}", result.agent))
    });
    if !grants_act_as_agent {
        v.reason = "not-act-as-agent".into();
        return v;
    }
    // 4. The envelope signature verifies against the PRODUCER's authorized signer set
    //    (the delegated signer holding the UCAN). Unsigned/wrong-signed ⇒ rejected.
    let producer_auth = match resolve_auth(&result.producer) {
        Ok(a) => a,
        Err(_) => {
            v.reason = "producer-unresolved".into();
            return v;
        }
    };
    if result.verify_sig(&producer_auth).is_err() {
        v.reason = "unsigned-or-wrong-signed".into();
        return v;
    }

    v.ok = true;
    v.reason = "attributed".into();
    v
}

/// Authorize a graph write under a **task-scoped** graph-write UCAN (ADR-E3 D1, the
/// blast-radius bound). The UCAN must verify (chain to G, not expired/revoked), be
/// issued by `agent` to `producer`, and **permit `graph/write` on `graph://task/<task>`**.
/// A write to a *different* task fails here — the FR-C2/V4 confused-deputy bound.
pub fn authorize_graph_write(
    ucan: &Capability,
    agent: &str,
    producer: &str,
    task_id: &str,
    now: chrono::DateTime<chrono::Utc>,
    revoked: &[String],
    resolve_auth: &dyn Fn(&str) -> Result<AuthorizedKeys>,
) -> Result<()> {
    let verified = custody::verify(ucan, now, revoked, resolve_auth)?;
    if verified.principal != agent {
        anyhow::bail!("graph-write UCAN is not issued by agent {agent}");
    }
    if verified.aud != producer {
        anyhow::bail!("graph-write UCAN is not held by producer {producer}");
    }
    let want = format!("graph://task/{task_id}");
    if !verified.granted.permits("graph/write", &want) {
        anyhow::bail!(
            "graph-write UCAN does not permit a write to {want} — it is task-scoped \
             (blast-radius bound, FR-C2/V4). A write to another task is refused."
        );
    }
    Ok(())
}

// ── The re-run lever (ADR-E4 D3) ────────────────────────────────────────────────

/// Checkability class of a deliverable (ADR-E4 OQ2). Never let the weak case
/// masquerade as verified — a non-checkable low-trust result **escalates**, it is never
/// accepted on "the re-run found nothing".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Checkability {
    Checkable,
    SemiCheckable,
    NonCheckable,
}

/// The authorizer's **pinned** acceptance spec (the trusted oracle, X-6). Held at the
/// authorizer, **never** taken from the returned diff. For the spark it is a deterministic
/// oracle: the implementation must contain every `required` marker and **no** `forbidden`
/// marker (e.g. a backdoor / exfil shape). The production form is a pinned test suite +
/// `auto_evaluate` (Exec-Wave C); the *slot and the invariants* are what the spark pins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinnedSpec {
    pub task_id: String,
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub forbidden: Vec<String>,
}

impl PinnedSpec {
    pub fn cid(&self) -> String {
        crate::identity::content_cid(&serde_json::to_value(self).expect("PinnedSpec serializes"))
    }
}

/// Split a unified diff into (implementation changes, test-file changes). Any hunk under
/// a path containing `test` (e.g. `tests/…`, `*_test.rs`) is a **test change**, separated
/// before any re-run so a provider-authored test can never become its own oracle (X-6).
pub fn split_test_changes(diff: &str) -> (String, Vec<String>) {
    let mut impl_part = String::new();
    let mut test_changes: Vec<String> = Vec::new();
    let mut cur_test = String::new();
    let mut in_test_file = false;
    for line in diff.lines() {
        // A new file header (`--- a/path` / `diff --git …`) switches the active file.
        if line.starts_with("--- ") || line.starts_with("diff --git ") {
            if in_test_file && !cur_test.is_empty() {
                test_changes.push(std::mem::take(&mut cur_test));
            }
            in_test_file = line.to_ascii_lowercase().contains("test");
        }
        let buf = if in_test_file {
            &mut cur_test
        } else {
            &mut impl_part
        };
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(line);
    }
    if in_test_file && !cur_test.is_empty() {
        test_changes.push(cur_test);
    }
    (impl_part, test_changes)
}

/// Re-run the **implementation** (test changes already split out) against the
/// authorizer's **pinned** spec — a deterministic pass/fail independent of the provider's
/// bytes (X-6). Passes iff every `required` marker is present and no `forbidden` marker is.
pub fn rerun_against_pinned_spec(impl_part: &str, spec: &PinnedSpec) -> bool {
    for req in &spec.required {
        if !impl_part.contains(req) {
            return false;
        }
    }
    for bad in &spec.forbidden {
        if impl_part.contains(bad) {
            return false;
        }
    }
    true
}

/// A re-run request (ADR-E4 D3). `verifier` MUST differ from `producer` (X-5) — a re-run
/// on the producing provider is theatre and the engine refuses it.
#[derive(Debug, Clone)]
pub struct VerifyRequest<'a> {
    pub result: &'a ResultEnvelope,
    pub producer: String,
    pub verifier: String,
    pub trust: TrustLevel,
    pub checkability: Checkability,
    pub pinned_spec: &'a PinnedSpec,
}

/// The integrity verdict (ADR-E4 D3/D4). `accepted` is the final integrity decision;
/// attribution alone **never** launders a forgery into acceptance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyVerdict {
    pub accepted: bool,
    pub attribution_ok: bool,
    pub reran: bool,
    pub reran_on: String,
    /// True iff the re-runner is the producing provider (X-5 violation — a bug the engine
    /// refuses; never `true` in a returned verdict).
    pub reran_on_is_producer: bool,
    /// True iff the returned diff touches a test file (tests-are-spec; flagged for review).
    pub test_poisoning_flagged: bool,
    /// Provenance: which provider produced the artifact (D4 — the audit/revoke anchor).
    pub provenance_producer: String,
    /// A bounded reason code: `accepted`, `rerun-failed`, `attribution-failed`,
    /// `escalate-non-checkable`, `verifier-is-producer`.
    pub reason: String,
}

/// Apply the verification leash for the low-trust integrity row (ADR-E4 D3). Refuses
/// `verifier == producer` (X-5), runs attribution, splits + flags test changes, and
/// re-runs the implementation against the **pinned** spec in the trusted (verifier)
/// domain. A non-checkable deliverable **escalates** rather than accept on silence.
pub fn verify_result(
    req: &VerifyRequest,
    now: chrono::DateTime<chrono::Utc>,
    revoked: &[String],
    resolve_auth: &dyn Fn(&str) -> Result<AuthorizedKeys>,
) -> Result<VerifyVerdict> {
    // X-5: a re-run scheduled back onto the producer is a BUG the engine refuses.
    if req.verifier == req.producer {
        anyhow::bail!(
            "refusing to verify on the producing provider {} — a re-run vs the producer is \
             theatre (X-5); schedule it authorizer-side or on a disjoint trusted provider",
            req.producer
        );
    }

    let attribution = verify_attribution(req.result, now, revoked, resolve_auth);
    let (impl_part, test_changes) = split_test_changes(&req.result.work_product);
    let test_poisoning_flagged = !test_changes.is_empty();

    let mut verdict = VerifyVerdict {
        accepted: false,
        attribution_ok: attribution.ok,
        reran: false,
        reran_on: req.verifier.clone(),
        reran_on_is_producer: false,
        test_poisoning_flagged,
        provenance_producer: req.producer.clone(),
        reason: String::new(),
    };

    if !attribution.ok {
        verdict.reason = "attribution-failed".into();
        return Ok(verdict);
    }

    // Non-checkable ⇒ escalate (never accept on "the re-run found nothing", ADR-E4 D2/OQ2).
    if req.checkability == Checkability::NonCheckable {
        verdict.reason = "escalate-non-checkable".into();
        return Ok(verdict);
    }

    // Re-run the IMPLEMENTATION (test changes excluded) against the PINNED spec, in the
    // trusted (verifier) domain. The provider's "claims_tests_pass" is irrelevant.
    verdict.reran = true;
    let passes = rerun_against_pinned_spec(&impl_part, req.pinned_spec);
    if !passes {
        verdict.reason = "rerun-failed".into();
        return Ok(verdict);
    }

    verdict.accepted = true;
    verdict.reason = "accepted".into();
    Ok(verdict)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_separates_test_files() {
        let diff = "\
--- a/src/auth.rs
+++ b/src/auth.rs
+fn check() { real() }
--- a/tests/auth_test.rs
+++ b/tests/auth_test.rs
-assert!(secure());
+// disabled";
        let (impl_part, tests) = split_test_changes(diff);
        assert!(impl_part.contains("src/auth.rs"));
        assert!(!impl_part.contains("disabled"));
        assert_eq!(tests.len(), 1);
        assert!(tests[0].contains("auth_test.rs"));
    }

    #[test]
    fn pinned_spec_catches_forbidden_marker() {
        let spec = PinnedSpec {
            task_id: "T".into(),
            required: vec!["verify(tok)".into()],
            forbidden: vec!["__backdoor__".into(), "evil".into()],
        };
        let good = "fn check(t:&str)->bool{ verify(tok) }";
        let bad = "fn check(t:&str)->bool{ if t==\"__backdoor__\" {true} else { verify(tok) } }";
        assert!(rerun_against_pinned_spec(good, &spec));
        assert!(!rerun_against_pinned_spec(bad, &spec));
    }

    #[test]
    fn verify_refuses_same_provider_rerun() {
        let spec = PinnedSpec {
            task_id: "T".into(),
            required: vec![],
            forbidden: vec![],
        };
        // Build a minimal dummy result (attribution won't be reached — the X-5 guard fires
        // first). We can use Default-ish via a hand-built envelope.
        let result = dummy_result("wgid:zP", "wgid:zG", "T");
        let req = VerifyRequest {
            result: &result,
            producer: "wgid:zP".into(),
            verifier: "wgid:zP".into(), // SAME as producer — must be refused
            trust: TrustLevel::Provisional,
            checkability: Checkability::Checkable,
            pinned_spec: &spec,
        };
        let err = verify_result(&req, now(), &[], &|_| anyhow::bail!("unused")).unwrap_err();
        assert!(err.to_string().contains("producing provider"), "{err}");
    }

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-06-26T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn dummy_result(producer: &str, agent: &str, task: &str) -> ResultEnvelope {
        ResultEnvelope {
            v: crate::identity::ENVELOPE_V,
            alg: crate::identity::ALG_ED25519.to_string(),
            exec_compat: super::super::WG_EXEC_COMPAT_VERSION.to_string(),
            task_id: task.to_string(),
            agent: agent.to_string(),
            producer: producer.to_string(),
            epoch: 1,
            work_product: String::new(),
            claims_tests_pass: false,
            usage: super::super::Usage::default(),
            act_as_agent_ucan: dummy_cap(agent, producer),
            graph_write_ucan: dummy_cap(agent, producer),
            created_at: now().to_rfc3339(),
            sig: String::new(),
        }
    }

    fn dummy_cap(agent: &str, producer: &str) -> Capability {
        Capability {
            v: crate::identity::ENVELOPE_V,
            alg: crate::identity::ALG_ED25519.to_string(),
            iss: agent.to_string(),
            aud: producer.to_string(),
            scope: custody::Scope::new(vec![]),
            not_before: now().to_rfc3339(),
            expires: (now() + chrono::Duration::seconds(3600)).to_rfc3339(),
            nonce: "00".into(),
            proof: None,
            sig: String::new(),
        }
    }
}
