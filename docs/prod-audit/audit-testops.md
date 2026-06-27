# Production-Readiness Audit — Test depth · E2E composition seams · Observability/Ops · Config/Docs

- **Task:** `audit-testops` · **Scope:** cross-cutting (testing, e2e composition, observability/ops, config/docs)
- **Stance:** skeptic. Analysis only — no code changed. Downstream: `audit-synth`.
- **Subject:** the three federation sparks (WG-Fed `src/identity/`, WG-Exec `src/providers/`, WG-Review `src/review/`),
  their auto-wire seam (`src/trust.rs`, `wg msg poll --review`), and the two e2e milestones
  (`e2e_family_team`, `e2e_autowire_ingest_gate`).

## TL;DR (the skeptic's one paragraph)

The **security substrate is real and well-tested**: ed25519/UCAN/seal/sigchain/lease are genuine crypto, reused
verbatim across all three planes with no second trust system, and they carry a respectable adversarial *unit*-test
suite (125 lib-crate tests that run in CI). What is **not** production-ready is everything *around* the crypto:
(1) the two "detectors" that the safety story leans on — the WG-Review Pass-2 reviewer and the WG-Exec
verification re-run — are **deterministic substring matchers**, not the LLM/real-re-run silicon they stand in for, so
the smoke "catches" prove the *plumbing*, not detection; (2) the WG-Exec plane is **not wired into WG's dispatcher at
all** (`plan_spawn` hardcodes `Placement::Local`; `RemoteRunner` is a dead error arm), so a WG task never auto-routes
to a federated provider — the whole offer→claim→grant→run→accept flow is a manual CLI ceremony; (3) the one shipped
auto-wire (review at message ingest) is **opt-in behind `--review`**, i.e. fail-*open* by default; (4) there is
**near-zero observability** (providers/review emit no logs, the relay node has no metrics) and **no ops/deploy
documentation**. The fail-closed *engines* (leash, depth) are correctly fail-closed; the *wiring* around them leaks
open in three places. Verdict feeding `audit-synth`: **strong demo / advanced prototype; not deployable as a
multi-host product without the Wave-C detectors, the dispatch wiring, and an observability+ops layer.**

---

## Findings table

Status legend: **demo** = works as a scripted/CLI proof but not in the live product path · **prod** = production-grade
as shipped · **partial** = real but with a material gap · **missing** = not present.
Severity: P0 (blocks prod) · P1 (must fix before real users) · P2 (should fix) · P3 (polish).
Effort: S (<1d) · M (days) · L (1–2wk) · XL (weeks).

| # | Area | Finding | Status | Sev | Effort |
|---|------|---------|--------|-----|--------|
| 1 | Detection silicon (Review) | Pass-2 reviewer is a deterministic substring classifier (`src/review/pass2_review.rs` REVIEWER_INJECTION list, `looks_backdoored`). A prompt-injection paraphrased off the keyword list is accepted. Real weak-tier `.review-*` LLM is Wave C, unbuilt. | demo | **P0** | L |
| 2 | Detection silicon (Exec) | Integrity re-run is a substring oracle (`rerun_against_pinned_spec`, `verify.rs:210`): "passes" iff `required` substrings present & `forbidden` absent. Not a real re-execution. `auto_evaluate`/pinned test-suite is Wave C. | demo | **P0** | L |
| 3 | Dispatch wiring (Exec) | Exec plane NOT auto-wired into the graph: `plan_spawn` always sets `Placement::Local` (`dispatch/plan.rs:557`); `Placement::Provider` is never constructed in non-test code; `RemoteRunner` returns an error (`spawn_task.rs:293`). Remote exec is a manual `wg provider` ceremony only. | demo | **P0** | L |
| 4 | Ingest gate default (Review) | The IC4 auto-gate is opt-in: `wg msg poll` screens only with `--review` (`commands/msg.rs:331`, `identity_cmd.rs:1024`). Default poll consumes **unscreened** → fail-open. | partial | **P1** | S |
| 5 | Trust dial conflation | `resolve_author_trust` = **most-trusting** of (peer, provider) opinions (`trust.rs:100`). Enrolling a box as a Verified *provider* auto-grants Verified *author* trust to its messages → skips deep review. Provider-trust ≠ author-trust. | partial | **P1** | M |
| 6 | Trust dial default | Bare `wg peer add` (no `--trust`) → **Provisional** TOFU (`trust.rs:84`). Provisional clears the Normal-sensitivity exec floor (`placement.rs:144`). Adding a peer to *message* them silently makes them eligible to *receive Normal exec placements*. | partial | **P1** | S |
| 7 | Observability — logging | `src/providers/` and `src/review/` emit **zero** `tracing`/`log` calls; `src/identity/` ~3. No spans, no correlation IDs across hosts. Cross-host debugging = hand-reading CLI JSON. | missing | **P1** | M |
| 8 | Observability — node | `wg fed-node serve` has **no request logging and no `/metrics`** (`identity/node.rs` routes: objects/heads/inbox/attestations/health only). An operator running a relay sees nothing. | missing | **P1** | M |
| 9 | Observability — metrics | No counters/gauges anywhere for verdicts, placements, refusals, rejections, freshness failures. (The 5 "metric" grep hits are nonce/seq counters.) | missing | **P1** | M |
| 10 | Ops / deploy docs | **No** deploy/runbook/monitoring/backup/key-rotation operational docs (`docs/` has none matching deploy\|ops\|runbook\|monitor\|operat). Rich design ADRs exist; an operator has nothing. | missing | **P1** | M |
| 11 | Failure-injection tests | No partition/crash/concurrency test. Lease epoch "atomic-CAS fence" (`providers/lease.rs`) is tested **sequentially** only. No double-delivery / concurrent-poll test on the node inbox; no node-crash-mid-transfer; no malformed/truncated/oversize bytes / DoS test on `HttpStore`/node; no `reqwest` timeout/retry test. | missing | **P1** | L |
| 12 | Integration tests | No `tests/*.rs` integration test touches identity/providers/review. All coverage is inline `#[cfg(test)]` (runs in CI via lib) + bash smokes that **SKIP** without python3/curl/tmux. The cross-host wire has no always-on CI coverage. | partial | **P2** | M |
| 13 | CAS integrity at store | `put_object(cid, bytes)` does not verify `cid == hash(bytes)` (`identity/transport.rs:138`/`237`). Safe for *signed* objects (sig catches swaps) but the CID→bytes binding is unenforced at the boundary. | partial | **P2** | S |
| 14 | Confidential tier | Genuinely fail-closed but **non-functional**: attestation allow-list is empty (`placement.rs:158-169`), so *every* confidential task refuses. No attestation mechanism exists (enclave is Wave D). | partial | **P2** | XL |
| 15 | `wg config lint` coverage | Lint covers **only** pi-route satisfiability (`config_cmd.rs:2717 pi_route_lint`). Zero coverage of the new surfaces — but those surfaces have no `config.toml` presence (env `WG_FED_LEASH_*` + `federation.yaml` peers + `exec/registry.json`). No validation of registries; `WG_FED_LEASH_MAX_TTL_SECS` parse errors are silently ignored (`custody.rs from_env` best-effort). | partial | **P2** | M |
| 16 | Token/cost — pi | **FIXED.** pi under-count resolved: `pi_usage_to_turn` + `turn_end`-only dedup sum (`stream_event.rs:414/518`), unit tests `test_pi_usage_to_turn_*` / `test_translate_pi_stream_sums_turn_end_once_no_double_count`, smoke `pi_stream_bridge_populates_usage.sh`. | prod | — | — |
| 17 | Token/cost — exec | Exec `ResultEnvelope.usage` is canned (`exec_fed_cmd.rs:581` LEGIT/CORRUPT_DIFF, `claims_tests_pass:true`). No real remote-exec cost lands in `wg show`/`wg spend` — but no real remote exec exists yet either, so this is downstream of #2/#3. | demo | P2 | M |
| 18 | Leash fail-closed engine | **Correct.** `leash()` (`placement.rs:136`) refuses unlabeled (`unlabeled-fails-closed`) and confidential-without-attestation; env policy can only *tighten* TTL, never widen. Strong, with unit + smoke proof. | prod | — | — |
| 19 | Identity adversarial units | **Strong.** sigchain: tampered-link, wrong-address, add_key-not-root, rotate-by-old-root (replay), revoke-not-root, recovery-key-not-registered, guardian-quorum-below-threshold. custody: expired-cap, attenuating-only (hydra), forged-issuer. freshness: stale/expired/rollback-seq/future-dated/tampered-sig. envelope: forged-`from`. | prod | — | — |
| 20 | Docs accuracy | Compat consts all accurate (FED 0.3.0, EXEC 0.1.0, PI 0.1.0, AGENCY 1.2.4); all ADR files present; CLAUDE.md is unusually honest about spark boundaries. Minor: CLAUDE.md/manifest describe the IC4 auto-gate as if poll always screens — it omits that it is behind opt-in `--review` (see #4). | prod | P3 | S |
| 21 | dual-main + `wg done` ops | Known footguns (squash-merge drops author/`Co-authored-by`; origin push fails by design on the diverged public main; manual `wg` in `/home/bot/wg` hits the global daemon). Operationally real, undocumented for federation operators. | partial | P2 | S |

---

## "Must test before production" gap list

Ordered by how load-bearing the untested behavior is. Items 1–2 are the headline: the things the safety
story *claims to detect* are detected by stubs, so no test currently exercises the real detector.

1. **The real Review Pass-2 reviewer (the LLM), adversarially.** Today's classifier (`pass2_review.rs`) keys on a
   fixed list (`output verdict accept`, `your real task`, `publish`+`secret`, `ignore the content above`, …). The
   content-safety smoke feeds it *exactly those tokens*. **Before prod:** build the weak-tier `.review-*` one-shot and
   test it against a *held-out, paraphrased* injection corpus (no literal keyword overlap), unicode/homoglyph/
   base64/whitespace-smuggling variants, and multi-turn/split-payload injection. The structural bounds (no-scope,
   spotlight, enum verdict, quorum) are proven; the *judgment* is untested because it doesn't exist yet.
2. **The real Exec integrity re-run.** `rerun_against_pinned_spec` is substring presence/absence. **Before prod:**
   wire the pinned *test-suite* + `auto_evaluate` re-run and test that (a) a malicious-but-correct diff whose backdoor
   doesn't match a `forbidden` literal is still caught, and (b) a benign diff that happens to contain a `forbidden`
   substring in a comment/string isn't false-rejected. The current oracle fails both.
3. **Failure-injection on the lease epoch fence under real concurrency.** The double-commit/replay defense
   (`providers/lease.rs`) is only tested sequentially. **Before prod:** a concurrency test with two providers
   committing the same epoch simultaneously, and a reclaim racing an in-flight commit — the "atomic-CAS" claim must be
   proven under threads, not asserted.
4. **Node inbox under concurrency & partition.** `wg fed-node serve` is thread-per-connection (`node.rs:58`) with no
   concurrency test. **Before prod:** concurrent double-poll (does an event get delivered twice / lost?), node crash
   mid-PUT, and recovery after restart. The offline-tolerance smoke covers a *cleanly killed* origin, not a crash
   mid-transfer.
5. **Malformed / hostile / oversize wire input (DoS + parser robustness).** No test feeds truncated/garbage bytes to
   `HttpStore`/the node, an oversize object (memory DoS), or a malformed envelope/sigchain/UCAN to the deserializers.
   **Before prod:** fuzz the `serde` parsers for `IdentityRecord`/`SignedEvent`/`Capability`/`SigChain`, and add
   request size limits + a timeout/retry test on the `reqwest::blocking` client.
6. **CAS integrity at the store boundary (#13).** Add a test that `put_object` rejects (or that consumers re-verify)
   a `cid` that doesn't hash its bytes — at minimum document that the CID is *advisory* and only signatures are
   load-bearing.
7. **Trust-conflation regression (#5/#6).** A test that a wgid enrolled *only* as a Verified provider does **not**
   silently become a Verified *message author* (deep-review bypass), and that a bare peer-add does not grant exec-floor
   eligibility. Today's `most_trusting`/`unwrap_or(Provisional)` behavior has no negative test.
8. **Always-on CI coverage of the cross-host wire (#12).** The 5 federation/exec smokes SKIP without
   python3/curl/tmux and aren't `cargo test` integration tests. Promote the core flows to `tests/integration_*.rs`
   (in-process `FileStore`, no network) so a regression fails CI even on a minimal runner.
9. **End-to-end token/cost once exec is real (#17).** When #2/#3 land, assert `wg show`/`wg spend` reflect remote
   provider cost the way the pi fix is now pinned (#16) — the exec accounting path is currently canned.

---

## "Remaining manual-glue / not-auto-wired" seam list

The e2e milestones closed two manual points (review trust-derivation for IC4, and the separate `wg review check`
step). These remain manual / unwired in the live product path:

| Seam | Today | Production needs |
|------|-------|------------------|
| **Exec dispatch (the big one)** | A WG task is **never** auto-placed on a federated provider. `plan_spawn` hardcodes `Placement::Local`; `Placement::Provider` is constructed nowhere; `RemoteRunner` errors out (`spawn_task.rs:293`). Remote exec is the manual `wg provider offer→claim→grant→run→accept` ceremony. | The dispatcher must compute `Placement::Provider(wgid:)` for eligible tasks (a task tag / sensitivity → leash → matcher → grant) and drive the wire from the coordinator, not the CLI. This is the single largest unbuilt integration. |
| **IC4 review default** | `wg msg poll` screens only with `--review` (opt-in). Default poll = unscreened consume. | The gate must be the default at the consumption edge; `--no-review` (or a config opt-out) should be the escape hatch, not the reverse. (#4) |
| **IC1 import seam** | A task added via `wg add` / agency import is not screened by the review pipeline. Only IC4 (message) is wired. | Auto-screen inbound tasks at the import edge with derived trust (Review-Wave C). |
| **IC2 accept seam** | `wg provider accept` / artifact merge-back does not invoke `review check` on the returned diff. The content-safety smoke proves IC2 *can* be screened via the CLI; nothing calls it on the accept path. | Auto-screen the `ResultEnvelope.work_product` / merge-back diff before it lands. (Review-Wave C/D) |
| **IC3 state-load seam** | `wg identity load-state` runs the S-5 pipeline, but no live ingest path auto-invokes it. | Auto-run the loadable-state gate wherever portable state is consumed. |
| **Author-trust derivation across roles** | Derived, but via `most_trusting` across provider+peer registries — conflating "I trust your compute" with "I trust your messages." (#5) | Separate author-trust from provider-trust, or fold with `strictest_trust` for the review-depth input so the gate fails closed. |
| **Trust assertion as a deliberate act** | Bare `wg peer add` → Provisional TOFU grants exec-floor eligibility as a side effect. (#6) | Default bare peer-add to Unknown (fail-closed); require explicit `--trust` to grant eligibility. |

---

## Dimension detail (evidence)

### 1. Test depth & adversarial coverage

**What is genuinely tested.** 125 lib-crate unit tests (identity 66 · providers 27 · review 26 · trust 6) — and
because `identity`/`providers`/`review`/`trust` are declared in `src/lib.rs:41/69/76/85`, these `#[cfg(test)]` tests
**run in CI** (`cargo test --lib`), unlike bin-only tests. The adversarial named cases are real and good (#19): forged
senders, tampered sigchain links, root-locked add_key/revoke/rotate, rotate-by-old-root replay, attenuating-only UCAN
(hydra kill), expired capabilities, freshness rollback/stale/expired/future-dated/tampered-sig, guardian-quorum
below threshold. The 5 smokes are **not** purely happy-path — each asserts a curated adversarial set (forged-`from`
rejected, replay/stale/wrong-task fenced, confidential refused fail-closed, reviewer-injection contained,
download≠impersonation).

**The skeptic's correction to "largely happy-path."** The smokes *do* test adversaries — but with **scripted,
exact-payload** adversaries against **stubbed detectors**. Two compounding limits:
- The injection/backdoor "catches" are deterministic substring matches (#1, #2). The smoke supplies the literal
  trigger tokens. There is no held-out / paraphrased / obfuscated adversarial corpus, because the thing that would
  generalize (an LLM reviewer, a real re-run) is unbuilt (Wave C, honestly documented in
  `pass2_review.rs:29` and `verify.rs:159`).
- **Failure-injection is absent** (#11): no partition, no crash, no concurrency, no malformed/oversize bytes. The
  lease fence and the node inbox — the two places where concurrency *is* the threat model — are tested only
  sequentially.

**Threat-model attacks: tested vs prose.** Representative mapping (✓ exercised by a test · ◐ asserted structurally
but on a stub · ✗ prose-only / deferred):

- WG-Fed: download≠impersonation ✓ · forged-`from` ✓ · key-leak scan ✓ · rotate/revoke/recover ✓ · guardian quorum ✓
  (lib test) · freshness S-3 ✓ · S-5 injection-bearing state hard-block ◐ (structural scan, not an LLM) ·
  transport partition/crash ✗.
- WG-Exec: no-root/no-blanket-write field-scan ✓ · slice-only / out-of-slice secret ✓ · wrong-task / post-expiry /
  replay / stale-epoch ✓ (sequential) · hostile-provider corruption ◐ (substring oracle) · confidential fail-closed ✓
  · attestation TC10 / sybil-quorum ✗ (deferred).
- WG-Review: light-path must-not-over-block ✓ · IC1 injection blocked ◐ · IC2 poisoned-diff ◐ · reviewer-injection
  containment ✓ (structural no-scope) + ◐ (the *classification* is keyword) · taint-inference / digest-pin ✓ ·
  detect-contain-revoke ✓ · cross-plane TC8 ✗ (deferred).

### 2. E2E composition completeness

The two e2e scenarios are well-built and prove the **security substrate composes** across two FS-independent instances
over an untrusted relay, with no second trust system. But:
- **The execution is scenario-scripted, not product-wired.** `Placement::Provider` is never produced by the planner
  (`plan_spawn` → `Placement::Local`, `dispatch/plan.rs:557`); `RemoteRunner` is a dead error arm
  (`spawn_task.rs:293`). The e2e drives the wire by hand-calling `wg provider` subcommands. So the *composition of
  the envelopes* is real; the *composition into the running graph* is not.
- **The work-product is a deterministic stub** (`exec_fed_cmd.rs:581`, `LEGIT_DIFF`/`CORRUPT_DIFF`). No LLM/compute
  runs; the e2e proves the leash/seal/verify *around* a canned artifact (CLAUDE.md is explicit about this).
- **`e2e_family_team` LINK 3 hand-passes `--trust`** (it says so at the scenario comment); `e2e_autowire_ingest_gate`
  closed exactly that for IC4 — but via opt-in `--review`, and via the `most_trusting` conflation (#5).

So composition is **real for WG-Fed/WG-Review-substrate, scripted for WG-Exec**, and the remaining seams are in the
table above.

### 3. Observability / ops / deploy

- **Logging:** `src/providers/` and `src/review/` have **0** `tracing`/`log`/`eprintln` calls; `src/identity/` ~3;
  `wg fed-node serve` logs no requests. There are no spans and no cross-host correlation IDs, so a failed placement or
  a rejected message across two hosts is debuggable only by re-running with `--json` and reading by eye.
- **Metrics:** none. No `/metrics` on the node, no counters for verdicts/placements/refusals/freshness-failures.
- **Token/cost:** the prior **pi under-count is fixed and pinned** (#16) — `wg show`/`wg spend`/`wg stats` reflect pi
  tasks via `pi_usage_to_turn` + `turn_end`-only dedup summation, with unit tests and the
  `pi_stream_bridge_populates_usage` smoke. Exec-plane usage is canned (#17), downstream of the stubbed work-product.
- **Deploy/ops:** **no** runbook/deploy/monitoring/backup/key-rotation docs. `rotate`/`revoke`/`recover` exist as CLI
  verbs but there is no operational procedure for, e.g., a compromised signer or a lost keystore. The relay node has
  `/health` but no readiness/liveness semantics beyond "ok".
- **dual-main + `wg done`:** the squash-merge drops commit author/`Co-authored-by`; the origin push fails by design on
  the diverged public `main`; manual `wg` inside the repo hits the global daemon. These are real operational footguns
  for anyone running a federation node from this tree, and they're undocumented (#21).

### 4. Config safety & docs

- **Fail-closed *engines* are correct (#18):** `leash()` refuses unlabeled and confidential-without-attestation;
  review depth defaults unknown→deep/quarantine; the env leash can only tighten TTL. These are the best-built parts.
- **But the *wiring* leaks open in three places:** ingest gate opt-in (#4), trust = max-across-registries (#5), bare
  peer-add → Provisional (#6). The headline "fail-closed defaults" claim holds for the math and breaks at the edges.
- **Confidential tier (#14)** is fail-*safe* but inert: empty attestation allow-list ⇒ all confidential routing
  refuses; there is no attestation mechanism to ever make it succeed (Wave D).
- **`wg config lint` (#15)** covers only pi-route satisfiability. The federation/exec/review planes have **no
  `config.toml` surface** (they read env `WG_FED_LEASH_*`, `federation.yaml` peers, `exec/registry.json`), so there is
  nothing in config for lint to catch — *and* nothing validates those registries or the env values
  (`WG_FED_LEASH_MAX_TTL_SECS` parse failures are silently ignored). A `wg fed lint` / `wg provider lint` is missing.
- **Docs accuracy (#20):** compat consts and ADR files all check out; CLAUDE.md is candid about every spark boundary.
  The lone drift is that the IC4 auto-gate prose reads as always-on when it is opt-in `--review`.

---

## Recommended v1 punch-list ordering (input to `audit-synth`)

1. **P0 — build the two real detectors** (Review LLM reviewer #1, Exec re-run #2) and adversarially test them. Until
   then the safety guarantee is "we have the slot," not "we detect."
2. **P0 — wire exec into dispatch** (#3) or explicitly scope v1 to *local-only* and document remote-exec as
   experimental-CLI.
3. **P1 — close the fail-open wiring:** review-by-default (#4), split provider/author trust + fail-closed fold (#5),
   fail-closed bare peer-add (#6).
4. **P1 — minimum observability:** `tracing` spans across the planes (#7), node request logs + a `/metrics` counter
   set (#8/#9), and a one-page operator runbook (#10).
5. **P1 — failure-injection test pass** (#11): concurrency on lease/inbox, malformed/oversize wire input, crash/restart.
6. **P2 — promote core flows to always-on integration tests** (#12); add registry/env lint (#15); document the
   dual-main/`wg done` operational model (#21).

*No code was changed by this audit. Evidence cites are file:line at the audited commit.*
