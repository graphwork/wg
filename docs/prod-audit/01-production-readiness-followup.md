# Production-Readiness Follow-up — closing verification

**Task:** `prod-verify` (the closing gate) · **Depends on:** `cross-task-poison`, `fed-harden`, `ops-and-tests`
**Subject:** the same stack as `00-production-readiness-assessment.md` — WG-Fed (`src/identity/`),
WG-Exec (`src/providers/`), WG-Review (`src/review/`), the auto-wire trust seam (`src/trust.rs`),
observability (`src/obs.rs`), and the two e2e milestones.
**Date:** 2026-06-28 · **Mode:** re-test + verify. One production-code line changed (a broken bin
unit-test caller, found and fixed during this pass — see §4).

This document re-runs the full systematic test, then walks **every** B*/M* item from the audit
punch-list and marks it **closed (with commit + code + re-test evidence)**, **open (with reason)**, or
**deferred-by-decision**. It is deliberately skeptical: the six threat-model attacks the contract
calls out were re-executed and confirmed to **fail** (the attack is blocked), not taken on faith.

---

## 0. The one-paragraph updated verdict

**Every BLOCKER in the audit is closed with re-tested evidence, and 25 of 30 MAJORs are closed; the
five residuals are all declared spark boundaries or Erik-only decisions, not hidden defects.** The
node is no longer an open DoS surface (write-auth + bounded reads + CID-verify + quota/GC/timeouts);
the exec epoch fence is crash-safe and *refuses* a corrupt ledger instead of silently resetting it;
`accept` now gates on the integrity re-run; the "detectors" are a real decode-then-detect engine
plus a weak→strong model path and a real executable exec re-run, and they catch the evasion corpus
the old keyword stub let through; the review gate is wired into all four ingest seams **default-on
and enforcing**; the trust dials are split fail-closed; and the worst-ranked cross-task-poison threat
(TC8) now has all three structural defenses. The stack has moved from **"advanced prototype"** to
**"v1-ready for the configured-peer, non-confidential-remote profile, pending the operator/scope
decisions below."** The honest residual risk is concentrated and named: (a) the production *detection
generalization past the seed corpus* rests on the weak-tier LLM path, which the credential-free smoke
gate exercises only through its deterministic fallback — the live model is proven by a *scheduled*
eval, not by CI; (b) confidential-remote-exec, a human quarantine-release operator, decentralized
discovery, review-flood DoS, and independent-quorum-at-scale are explicitly deferred. **Trust the
crypto and the structural bounds (re-tested below); scope v1 to what the deferred list excludes.**

---

## 1. Systematic test re-run (all green)

Run on `wg/agent-5891/prod-verify` at `HEAD` of the dependency chain
(`22ac5105 ops-and-tests` ← `6e853fdd cross-task-poison` ← `0e8939f3 fed-harden` ←
`ff52d70c exec-harden` ← `957e208b wave0-fixes` …), toolchain pinned by `rust-toolchain.toml` (1.96.0).

| Gate | Command | Result |
|---|---|---|
| Build | `cargo build --bin wg` | **OK** |
| Format | `cargo fmt --check` | **clean** (exit 0) |
| Lint (CI invocation) | `cargo clippy` | **clean** (exit 0; warnings only, CI runs no `-D warnings`) |
| Lint (all targets) | `cargo clippy --all-targets` | **clean after §4 fix** (was exit 101 — a real broken bin test) |
| Unit tests | `cargo test --lib` | **2713 passed**, 0 failed, 0 ignored |
| Doc tests | `cargo test --doc` | **8 passed**, 0 failed |
| Bin unit tests | `cargo test --bin wg` (peer module) | **10 passed** after §4 fix |
| Always-on wire (M29) | `cargo test --test integration_fed_wire` | **7 passed** |
| Failure injection (M22) | `cargo test --test integration_failure_injection` | **11 passed** |
| Exec/Fed integration | `integration_provider/placement/exec/context_scope` | **24 / 12 / 17 / 21 passed** |

**Smoke scenarios** — every federation/exec/review scenario (the audit subject) was run with a
freshly-built pinned binary in an isolated env. **All 18 PASS (exit 0); none SKIPped:**

`real_review_evasion` · `exec_harden_wire` · `exec_real_run_silicon` · `exec_ledger_crash_safe` ·
`harden_node_network` · `fed_harden_residuals` · `wire_review_seams` · `content_safety_spark` ·
`cross_task_poison` · `e2e_autowire_ingest_gate` · `e2e_family_team` · `exec_spark_borrowed_box` ·
`federation_spark_two_graphs` · `federation_node_inbox_cross_graph` · `federation_recovery_portable_state` ·
`federation_acl_ucan_delegation` · `ops_observability` · **`bin_test_target_compiles`** (new, §4).

> **Scope note (honest):** the manifest has 185 scenarios; the ~167 not listed above are pre-existing
> TUI/chat/nex/pi/codex scenarios that require live LLM credentials or a tmux/PTY and **loud-SKIP** in
> this credential-free environment. They are unrelated to the federation stack and to the one
> test-only code change in this pass. The lib/doc/integration test suites above *do* exercise every
> `src/identity`, `src/providers`, `src/review`, `src/trust`, `src/obs` module.

---

## 2. Threat-model re-tests — the six attacks the contract names (all BLOCKED)

Each was re-executed through its smoke scenario against the real binary and confirmed to **fail
closed**. These are the falsifiable "the attack is now blocked" proofs, not code-reading.

| # | Attack | Re-test | Observed (attack blocked) |
|---|---|---|---|
| 1 | **Injection/evasion corpus** walks the old keyword stub | `real_review_evasion` + `detect::tests::evasion_corpus_before_after` | base64 / hex / homoglyph / leet / separator / paraphrase obfuscations **all BLOCKED** (`reject, injection-signature`); clean content still **accepted** (no over-block). "all 6 evasions blocked at the live review seam." |
| 2 | **Corrupted remote result** committed at `accept` | `exec_harden_wire` step B, `exec_real_run_silicon` | low-trust result with no pinned spec ⇒ `verification-required` (fail-closed); a genuine result + spec ⇒ **accepted** (no over-block); a corrupted result ⇒ `integrity-rerun-failed` / `review-reject`, **write refused, producer trust lowered → unknown**. The executable re-run on a disjoint domain Q catches a runtime-built `__backdoor__` invisible to any substring oracle. |
| 3 | **Double-commit / replay**, even under a **corrupt ledger** | `exec_ledger_crash_safe`, `exec_spark_borrowed_box` step 4 | a partial/corrupt `leases.json` ⇒ mutating reclaim **REFUSES (`REFUSING to reset`)** and leaves bytes unclobbered (never the old silent `unwrap_or_default()` empty reset); a replay ⇒ `replay-already-committed`; a stale-after-reclaim write ⇒ `stale-epoch`. The fence holds under crash, not just on the happy path. |
| 4 | **Unauthenticated node write** | `harden_node_network` step 2 | the owner's real head with its `sig` **stripped is REJECTED (403)**; the owner-signed head re-accepted (200). Plus: chosen-CID squat ⇒ 409, 4 GiB-declared body ⇒ 413 (no pre-alloc), inbox flood ⇒ 507. |
| 5 | **Withheld revocation** (an untrusted node serves a stale head to hide a revoke) | `fed_harden_residuals` M10, `federation_acl_ucan_delegation` | `revoke-cap` publishes a signed, **monotonic-seq, freshness-windowed** `RevocationHead`; `verify-cap` re-fetches + freshness-gates it and reports the cap **REVOKED** — a node serving an older `seq` is caught as rollback (fail-closed). |
| 6 | **Verified provider auto-clears author review** | `wire_review_seams` M18 | a Verified *provider* never added as an author peer resolves to author `effective_trust=unknown` ⇒ its message takes the **deep path and is WITHHELD**; a bare `wg peer add` (no `--trust`) ⇒ `unknown`, **not** Provisional. The dials are split; the provider dial still governs the exec leash + IC2 directly. |

---

## 3. Per-item verdict table (every B*/M*)

Evidence cites the closing commit (short hash), the load-bearing code, and the re-test that proves it.

### 3.1 BLOCKERS — all CLOSED with evidence

| # | Verdict | Evidence |
|---|---|---|
| **B1** Node zero write-auth | **CLOSED** | `harden-node` `214ecd39`. `node.rs::put_head_authed`/`put_attestation_authed` reconstruct + verify the wgid's sigchain (`reconstruct_authorized`) and refuse a wrong/absent `sig`; inbox quota + GC. Re-test: `harden_node_network` (403 on unsigned head, 507 on flood). |
| **B2** Unbounded `Content-Length` pre-alloc | **CLOSED** | `harden-node` `214ecd39`. `node.rs` caps `max_body` (8 MiB, `WG_FED_NODE_MAX_BODY`) and answers `413` **before** reading/allocating — never `vec![0u8; content_length]`. Re-test: `harden_node_network` step 4. |
| **B3** Exec ledger fails open under crash/concurrency | **CLOSED** | `wave0-fixes` `957e208b`. `lease.rs`: atomic temp-file+fsync+rename (`atomic_file::write_atomic`), an exclusive advisory lock across the whole read-modify-write (`open_locked`/`LedgerLock`), and **refuse — never reset — on a corrupt parse**. Re-test: `exec_ledger_crash_safe`. |
| **B4** `accept` never runs the integrity re-run | **CLOSED** | `exec-harden` `ff52d70c`. `exec_fed_cmd.rs::run_accept` consults `verification_depth` at the canonical-write boundary: a low-trust result with no pinned spec is `verification-required` (fail-closed); a corrupted one is rejected by the trusted-domain re-run. Re-test: `exec_harden_wire` step B. |
| **B5** Detectors are stubs, not silicon | **CLOSED** (nuance) | Review: `real-review` `b6987832` — `detect.rs` decode-then-detect (base64/hex/homoglyph/leet/zero-width/rot13) + `reviewer.rs` weak→strong model path; Exec: `exec-real-run` `e9c0bd81` — real subprocess worker + **executable-test** re-run (`verify.rs`, not substring); Fed S-5 reuses the same `detect` engine. **Nuance:** credential-free CI exercises the strong *deterministic* floor (which now catches the evasion corpus); the production weak-tier LLM is exercised by a *scheduled* eval, not the smoke gate. Re-tests: `real_review_evasion`, `exec_real_run_silicon`, `content_safety_spark`. |
| **B6** Review wired into 1 of 4 ingest seams | **CLOSED** | `wire-review` `3fe2b9c0`. IC1 (`trace_import.rs`), IC2 (`exec_fed_cmd.rs::screen_accept_artifact`), IC4 (`identity_cmd.rs` poll) are **default-on + enforcing** (bytes withheld on non-accept); IC3 is the S-5 load gate. Re-test: `wire_review_seams` (all four edges). |
| **B7** Cross-task poison (TC8) undefended | **CLOSED** | `cross-task-poison` `6e853fdd`. `cross_task.rs`: (1) tier-by-graph-position floors a *foundational* task to Verified; (2) `WorkGraph::transitive_descendants` + descendant re-queue (done→open) on verify-reject/review-revoke; (3) cross-trust input re-verification gate at grant. Re-test: `cross_task_poison` (all 3 defenses). |

### 3.2 MAJORS

| # | Verdict | Evidence / reason |
|---|---|---|
| **M1** At-rest root-key protection | **CLOSED** | `fed-harden` `0e8939f3`. `keys.rs`: XChaCha20-Poly1305 KEK-wrapping of every stored seed when `WG_FED_KEYSTORE_PASSPHRASE`/OS-keyring is present; a loud once-warning + `wg secret backend show` surface when not. (Out-of-process signer remains a future hardening.) |
| **M2** Wire the S-7 compat handshake | **CLOSED** | `wave0-fixes` `957e208b`. `transport.rs:605` calls `check_compat` on the peer's advertised version (a real non-test caller). |
| **M3** Enforce `cid == hash` on get/PUT | **CLOSED** | `harden-node` `214ecd39`. `node.rs` `cid_mismatch()` (409) on `PUT /objects` and the get path. Re-test: `harden_node_network` step 3. |
| **M4** Node timeouts + thread bound + inbox GC | **CLOSED** | `harden-node` `214ecd39`. Read/write timeouts, inbox cap + delete-after-ack GC. Re-test: `harden_node_network` steps 5–7. |
| **M5** Wire exec into the dispatcher | **CLOSED (implemented)** | `exec-harden` `ff52d70c`. `dispatch/plan.rs` `ExecutorKind::RemoteRunner` + `Placement::Provider`; the `exec-provider:<wgid>` tag drives `wg provider place`. Re-test: `exec_harden_wire` step A. *(The v1 in/out-scope call for remote exec remains Erik's — Decision 1 below — but the wiring is built and gated behind B3/B4/B5, which are now sound.)* |
| **M6** Attestation enclave for confidential remote | **DEFERRED-BY-DECISION** | `placement.rs` `ContextSeal::SealedToAttestation` slot + **fail-closed refuse** for a confidential task with no attested provider (correct, never plaintext). Real TEE quote verification = Exec-Wave D; audit-recommended v1 default is "document confidential remote as unsupported." Erik Decision 2. |
| **M7** S-5 `model_binding` + real state consumer | **CLOSED** | `fed-harden` `0e8939f3`. `state_safety.rs` `ModelBindingVerdict` compares the binding to the runtime model and **fails closed on mismatch**; `consume_payload` actually decodes `conv-cache-v1`/`summary-v1` into a `LoadedState` (no longer prints "LOADED"). Re-test: `fed_harden_residuals` (S13/M7). |
| **M8** Sealed-sender outer envelope unauthenticated | **CLOSED** | `fed-harden` `0e8939f3`. `envelope.rs`: the inner sealed payload carries a commitment over the visible outer fields; `open` re-derives it and **refuses a mismatch** (malleable routing metadata caught). |
| **M9** Envelope-layer replay/dedup at consume edge | **CLOSED** | `fed-harden` `0e8939f3`. `dedup.rs` consume-edge `DedupStore`; a re-polled event authenticates (idempotent) but is flagged `replayed`, body withheld. Re-test: `fed_harden_residuals` M9. |
| **M10** Revocation propagation withholdable | **CLOSED** | `fed-harden` `0e8939f3`. `custody.rs::RevocationHead` (signed, monotonic-seq, freshness-windowed); `verify-cap` fails closed on stale/rollback/withheld. Re-test: `fed_harden_residuals` M10. |
| **M11** Recovery key unrevocable + unwindowed | **CLOSED** | `fed-harden` `0e8939f3`. `sigchain.rs` `SetRecovery` link + `recovery_not_before`/`recovery_expires` window; recovery outside the window fails closed. Re-test: `fed_harden_residuals` M11. |
| **M12** No equivocation/fork detection | **CLOSED** | `fed-harden` `0e8939f3`. `equivocation.rs` (302 lines) — signed head gossip / fork-history detection. Re-test: lib tests + `fed_harden_residuals`. |
| **M13** UCAN depth-cap + transport size-cap | **CLOSED** | `wave0-fixes` `957e208b`. `custody.rs` `MAX_CAP_CHAIN_DEPTH=64` with **iterative** `chain_len` (no recursion overflow); `transport.rs` `MAX_RESPONSE_BYTES=64 MiB` bounded read. |
| **M14** Pass-3 sandbox + Pass-4 human / quarantine-release | **DEFERRED-BY-DECISION** | Quarantine is a terminal fail-closed block (`review_cmd.rs`); there is **no `wg review release`/approve** queue yet. Gated on Erik Decision 3 (is a human operator in v1?). A minimal release command remains cheap insurance. Review-Wave D. |
| **M15** Bridge remote-exec usage into accounting | **CLOSED** | `exec-harden` `ff52d70c`. `exec_fed_cmd.rs::bridge_usage_into_graph` flows real usage into `task.token_usage` → `wg show`/`spend`/`stats`. Re-test: `exec_harden_wire` step A (`usage_accounted_to_graph=true`). |
| **M16** Liveness/lease enforcement | **CLOSED** | `exec-harden` `ff52d70c`. `lease.rs` `accept_renewal` + `sweep_expired` (auto-reclaim on timeout) + `verify_renewal_sig`; `wg provider renew`/`sweep`. Re-test: `exec_harden_wire` steps D/E. |
| **M17** Grant drops sensitivity (hardcoded Normal) | **CLOSED** | `exec-harden` `ff52d70c`. Sensitivity is carried into claim/grant and **grant re-derives the real label** from the authorizer's ledger (`Sensitivity::Unlabeled` default fails closed). Re-test: `exec_harden_wire` step C (High→Provisional refused at grant). |
| **M18** Trust-dial conflation | **CLOSED** | `wire-review` `3fe2b9c0`. `trust.rs::resolve_author_trust` splits the dials and **min-folds fail-closed** (provider trust can only tighten); bare peer-add ⇒ Unknown. Re-test: `wire_review_seams` M18. Resolves Erik Decision 4 toward split-dials-min-folded. |
| **M19** Make IC4 verdict enforcing | **CLOSED** | `wire-review` `3fe2b9c0`. Non-accept ⇒ body withheld (`body_withheld=true`, `consumable=false`), not printed-then-flagged. Re-test: `wire_review_seams` IC4. |
| **M20** Observability | **CLOSED** | `ops-and-tests` `22ac5105`. `obs.rs` `FedMetrics` (atomics + Prometheus render) + correlation IDs; node `/metrics` + per-request access log. Re-test: `ops_observability`. |
| **M21** Ops/deploy runbook | **CLOSED** | `ops-and-tests` `22ac5105`. `docs/ops/runbook.md` (deploy/monitor/backup/key-rotation + dual-main/`wg done` footguns). Re-test: `ops_observability` step 5. |
| **M22** Failure-injection test pass | **CLOSED** | `ops-and-tests` `22ac5105`. `tests/integration_failure_injection.rs` (11): lease epoch-CAS + node inbox + verdict-chain concurrency, crash/restart recovery, malformed/oversize wire, serde fuzz. |
| **M23** Serialize verdict-chain appends + index | **CLOSED** | `ops-and-tests` `22ac5105`. `verdict.rs` `ChainLock` exclusive append lock (same class as B3). |
| **M24** Adversarial evasion corpus in CI | **CLOSED** | `real-review` `b6987832`. `detect::tests::evasion_corpus_before_after` (must catch ≥85%) + the always-on `real_review_evasion` smoke. |
| **M25** Review timeout/error → fail-closed SKIP | **CLOSED** | `real-review` `b6987832`. `reviewer.rs` `REVIEW_TIMEOUT_SECS` + `ReviewSource::FailClosed` (a timeout/error/unparseable reply blocks ≥ quarantine). |
| **M26** Review-fatigue / flood DoS | **OPEN (deferred)** | No rate-limit / flooder-de-trust / blast-radius-prioritized queue in `src/review/` yet. Declared Review-Wave C/D; low live risk while Pass-2 is the deterministic in-process engine (no per-item network cost). Becomes important once Pass-2 is a real network call at volume. |
| **M27** FP bug — fix `weakens_assert` | **CLOSED** | `real-review` `b6987832`. `detect.rs` requires a *removed/disabled* assertion (`-assert && !+assert`), not the bare word `assert`; an assertion-*adding* diff is accepted. Lib tests pin both directions. |
| **M28** DHT/Iroh discovery | **DEFERRED-BY-DECISION** | `federation.rs` `ResolveSource::Dht` defined, deferred past Wave 4; resolution is cached-signed-record → directory-hint (manual config). Erik Decision 5 (does v1 claim "decentralized" vs "configured-peer"?). |
| **M29** Always-on cross-host wire CI | **CLOSED** | `ops-and-tests` `22ac5105`. `tests/integration_fed_wire.rs` (7) over in-process `FileStore`+node — runs on a minimal runner with no python/curl/tmux. |
| **M30** Model-strength escalation + independent quorum | **PARTIAL** | `real-review` `b6987832`. weak→strong **escalation** is built (`reviewer.rs`) and the **strictest-wins structural quorum** slot exists (`pass2_review.rs`); a genuinely-independent **N-distinct-model** quorum *at scale* + model-strength-by-depth are Review-Wave C (need the real weak/strong silicon online). The structural bound holds today; the model diversity is the deferred half. |

### 3.3 Summary

- **BLOCKERS: 7/7 CLOSED** with re-tested evidence.
- **MAJORS: 25/30 CLOSED**, **1 PARTIAL** (M30), **1 OPEN-deferred** (M26), **3 DEFERRED-BY-DECISION**
  (M6, M14, M28).
- **MINORS** (audit §2.3) and the **scaffold register S1–S18** track the same waves: the Wave-0/1/2/4/5
  silicon and wiring landed (S1 reviewer slot real + decode floor, S4/S5 real exec re-run+worker, S11
  fed scanner shares `detect`, S12/S13 binding+consumer, S15 handshake, S16 recovery window, S17
  equivocation); the genuinely-deferred scaffolds are **S2/S3** (Pass-3 sandbox + Pass-4 human, =M14),
  **S6** (TEE enclave, =M6), **S8** (sybil-resistant quorum, research), **S14** (DHT, =M28), **S18**
  (MLS forward-secrecy, correctly-scoped non-goal).

---

## 4. New finding closed during this pass — broken bin unit test (CI blind spot)

Being skeptical surfaced one real defect the green CI could not see. The M18/`wire-review` work grew
`peer::run_add` a 7th parameter (`trust: Option<&str>`) but left all **seven `#[cfg(test)]` callers**
in `src/commands/peer.rs` at six arguments. This shipped **green** because CI runs only
`cargo test --lib` / `--doc` / `--test integration_*` and **never compiles the binary crate's
`#[cfg(test)]` modules** — the documented lib-vs-bin blind spot. `cargo clippy --all-targets` exits
**101** on it; plain `cargo clippy` (CI) and `cargo test --lib` are blind.

- **Fixed:** the seven callers now pass `None` (matching bare-peer-add). `cargo test --bin wg` peer
  module: **10 passed**; `cargo clippy --all-targets`: clean; `cargo fmt --check`: clean.
- **Guarded permanently:** added `tests/smoke/scenarios/bin_test_target_compiles.sh`
  (`owners = [prod-verify]`) — it runs `cargo test --bin wg --no-run` and FAILS the smoke gate if the
  bin test target stops compiling, so this class of regression is caught at the gate instead of rotting.
  It loud-SKIPs when cargo/the workspace is absent. **PASS.**

This is the one production-code change in this verification pass (plus the new smoke + manifest entry).

---

## 5. Decisions still only Erik can make (unchanged from the audit, now the only gating items)

The blocker/major engineering is done; what remains for a GA call is policy/scope, not unsafe code:

1. **Does v1 include remote execution, or is it local-only?** The wiring (M5) is built and its
   preconditions (B3/B4/B5/B7) are closed, so remote-exec is now *safe to enable* — but whether it is
   *in v1* is the pivot. Local-only moves M6/M14(exec)/M16/M30(exec) out of v1 scope.
2. **If remote: confidential workloads remotely?** Recommended default **no** — keep the fail-closed
   *refuse* (M6/S6); don't let the empty TEE slot read as a capability.
3. **A human operator in v1?** Gates M14/S3 (Pass-4 + `wg review release`). If no, v1 quarantine is a
   terminal reject ("block, don't triage"); a minimal release command is cheap insurance.
4. **One min-merged trust dial vs per-plane dials?** Implemented as **split + fail-closed-min-fold**
   (M18) — confirm this is the intended API.
5. **Claim "decentralized" or "configured-peer" federation for v1?** (M28) Configured-peer defers
   DHT/Iroh cleanly and is a documented limitation, not a gap.

---

## 6. Bottom line

- **The audit's blocker clusters are all closed and re-tested:** node DoS (B1/B2), exec fence+accept
  (B3/B4), stubbed detectors + unwired seams (B5/B6), and the cross-plane TC8 (B7). The six named
  threat-model attacks were re-run and **fail closed**.
- **The crypto and the structural bounds are production-grade and adversarially tested** (2713 lib +
  92 fed/exec integration [provider 24 · context_scope 21 · exec 17 · placement 12 · failure_injection
  11 · fed_wire 7] + 18 federation smokes green). Trust the verification code.
- **The honest residual is detection *generalization* and five named deferrals,** not an unsafe
  reachable path. The credential-free gates prove plumbing + the deterministic detection floor + every
  structural bound; the production weak-tier-LLM detection is proven by a *scheduled* eval, which is
  the right place to keep watching as the corpus and the models evolve.
- **Verdict: v1-ready for the configured-peer, non-confidential-remote, block-don't-triage profile**,
  pending the five scope decisions above. Everything outside that profile is explicitly deferred, not
  silently missing.

*Verification pass — one production-code line changed (the bin-test caller fix in §4) plus a new smoke
scenario + manifest entry; all gates green.*
