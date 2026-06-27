# Production-Readiness Assessment — WG federation stack (synthesis)

**Task:** `audit-synth` · **Inputs:** `docs/prod-audit/audit-{fed,exec,safety,testops}.md`
**Subject:** WG-Fed (`src/identity/`), WG-Exec (`src/providers/`), WG-Review (`src/review/`),
their auto-wire seam (`src/trust.rs`), and the two e2e milestones.
**Date:** 2026-06-27 · **Mode:** synthesis only — no production code changed.

This is **the** production-readiness verdict and the prioritized v1 punch-list. It dedupes and
re-ranks every finding from the four component audits onto one severity scale, registers every
scaffold-without-silicon slot, and sequences the work to full production readiness with the
dependencies and the decisions only Erik can make.

---

## 0. The one-paragraph verdict

**The cryptographic substrate is production-grade; everything that turns it into a running,
multi-host product is a spark.** Across all three planes, one self-certifying identity algebra —
`wgid:` addressing, hash-linked sigchain with the hydra-lock, attenuating-only UCAN, per-recipient
sealed envelopes, tiered freshness, the fail-closed leash and depth matrices — is real, reused
verbatim with no second trust system, and adversarially unit-tested (125 lib tests in CI). What is
**not** production-ready is the shell around it: a network node any peer can OOM or flood; an
execution plane reachable only through a 10-verb manual CLI; "detectors" that are deterministic
substring matchers standing in for the LLM/re-run silicon; three of four content-safety ingest
seams unwired; near-zero observability; no ops story. **The smoke gates prove the plumbing and the
structural bounds compose — they do not prove detection, and they must not be read as "safe to
point at hostile input."** Shipping the stack as-is is safe only in the narrow sense that the
unsafe paths are mostly *not reachable from the product* yet; the moment they are wired (exec into
dispatch, the review gate onto import/accept), the blockers below go live.

---

## 1. Per-component production verdict

Scale: **PRODUCTION-READY** · **PARTIAL** (real core, material prod gaps) · **SCAFFOLD** (proof
shape; the load-bearing payload is a stub or the path isn't wired into the product).

| Component | Verdict | Headline gaps |
|---|---|---|
| **WG-Fed** (identity / crypto / custody / state / transport) | **PARTIAL** — *"the math is ready; the daemon is a demo."* The offline-verifiable identity algebra is production-grade and adversarially tested. | **Node = the blocker carrier:** zero write-auth + unbounded `Content-Length` alloc = trivial DoS on any networked deploy. Plus: at-rest root keys are plaintext-hex; the S-7 compat handshake is defined but **never invoked on the wire**; `get_object`/node-PUT don't enforce `cid==hash`; sealed-sender outer metadata is unauthenticated; no equivocation/fork detection; DHT discovery deferred (peers are manual config). The S-5 state gate's *policy* is real but its *mechanism* (content scanner, `model_binding` enforcement, actual state load) is stub/missing. |
| **WG-Exec** (providers / leash / lease / verify) | **SCAFFOLD** — a clean, honest PoC reachable **only** through the manual `wg provider` CLI; never constructed by the planner. The crypto substrate and the per-invariant logic are real and well-tested. | **Not wired into dispatch** (`Placement::Provider` never built; `RemoteRunner` errors). The **epoch fence — the whole integrity backstop — is unsound off the happy path** (ledger persisted with unlocked non-atomic `fs::write`; corrupt ledger silently fails *open*). **`accept` never runs the integrity re-run** (it's a decoupled manual command), so a corrupted result commits. **TC8 cross-task poison** (the worst-ranked threat) is structurally undefended. The **attestation slot is empty** (confidential work can only be *refused*). The worker is a constant-diff stub; usage is canned. |
| **WG-Review** (content-safety gate) | **SCAFFOLD / PARTIAL** — a faithful structural spark whose **containment+audit+revoke spine is genuinely production-shaped and strong**, but whose **detection layer is deterministic scaffolding** and which is wired into **one of four** ingest seams. | **Pass-2 is a keyword classifier, not a weak-tier LLM**; **Pass-3 (sandbox) and Pass-4 (human) never execute**, so "quarantine pending human" has no human and no release path. **IC1 (import) and IC2 (accept) seams are unwired**; IC4 (message) is wired but **opt-in behind `--review`** (fail-open default) and **advisory** (prints the body, returns `consumable:false` rather than withholding bytes). A FP bug hard-rejects ~every honest test diff. Strong, real: digest-pin TOCTOU close, verdict sigchain, monotonic revoke, the reviewer no-scope bound. |
| **Integration** (e2e `family_team` + `autowire_ingest_gate`) | **PARTIAL** — the **substrate composes cleanly** across two FS-independent instances over an untrusted relay with no second trust system (a real, valuable result that required no production-code change). | The **composition into the running graph is not built**: exec is hand-driven by CLI (the planner never produces `Placement::Provider`), the work-product is a deterministic stub, and the review trust is hand-passed (`--trust`) and **conflated** (most-trusting merge of provider-trust with author-trust). The e2e proves the *envelopes* compose and the security bounds hold at each seam — not that the *product* auto-wires them. |

**Cross-plane verdict:** the stack is an **advanced prototype**, not a deployable multi-host
product. It is internally honest — every gap below is a declared spark boundary (Exec-Wave C/D,
Review-Wave C/D, federation transport hardening), not a hidden defect. The risk to manage is
**organizational**: do not let green smokes read as "done."

---

## 2. The unified punch-list (deduped, severity-ranked)

Severity per the task contract: **BLOCKER** = unsafe / incorrect for production · **MAJOR** =
needed for v1 · **MINOR** = polish. Effort: **S** <1d · **M** days · **L** 1–2 wk · **XL**
weeks/multi-wave. Each row folds the duplicate findings across audits into one item with its
source IDs. Source-severity normalization: fed BLOCKER→BLOCKER, exec Crit/High→BLOCKER/MAJOR,
safety S1→BLOCKER S2→MAJOR, testops P0→BLOCKER P1→MAJOR.

### 2.1 BLOCKERS — unsafe or incorrect for production (fix before the path is reachable)

| # | Finding | Component | Effort | Sources |
|---|---|---|:--:|---|
| **B1** | **Node: zero auth on every write endpoint.** Any peer can `PUT /heads`, `/inbox`, `/attestations`, `/objects` for *any* wgid → inbox flood, head-squat/rollback, attestation overwrite, storage exhaustion. (Forged *identity* is still impossible; DoS/grief is trivial.) | WG-Fed | M | fed G5 |
| **B2** | **Node: unbounded `Content-Length` pre-allocation** (`vec![0u8; content_length]` before read) → one request OOMs the node. | WG-Fed | S | fed G6 |
| **B3** | **Exec ledger fails open under concurrency/crash.** `leases.json` is persisted via unlocked, non-atomic `fs::write`; a corrupt/half-written ledger silently `unwrap_or_default()`s to empty → the epoch fence (the integrity backstop) resets, re-enabling double-commit/replay. The "one in-process check-and-set" guarantee holds only for a single serialized writer. | WG-Exec | M | exec F1+F2, testops #11 |
| **B4** | **`accept` never runs the integrity re-run.** `verification_depth` is computed by the leash but never consulted at the canonical write boundary; `wg provider verify` is a *separate manual* command. A corrupted result that clears attribution+scope+epoch is committed with no integrity check. *(Becomes fully meaningful once B5/exec is real.)* | WG-Exec | M | exec F3 |
| **B5** | **The detectors are stubs, not silicon.** Review Pass-2 is a deterministic keyword classifier (not a weak-tier `.review-*` LLM); Exec integrity re-run is a substring presence/absence oracle (not a real re-execution); Fed S-5 content scanner is a ~10-phrase list. The "we detect injection/poison" guarantee is "we have the slot." A paraphrased/encoded payload walks through all three. | Cross-plane (Review/Exec/Fed) | L each | safety F1, exec F6/F10, fed F4, testops #1/#2 |
| **B6** | **Review gate wired into 1 of 4 ingest seams.** IC1 (import) and IC2 (accept) never call the pipeline; IC4 (message) is wired but **opt-in behind `--review`** (default poll consumes unscreened = fail-open). Tasks and artifacts ingest unscreened. | WG-Review | M | safety F2/F17, testops #3/#4 |
| **B7** | **Cross-task poison (TC8) structurally undefended** — the worst-ranked threat in the adversarial study. Only provenance + trust-lowering exist; tier-by-graph-position, descendant re-run, and input re-verification across trust boundaries are unbuilt (a code comment overstates that descendants are surfaced). **BLOCKER if v1 runs multi-task remote exec; MAJOR otherwise.** | Cross-plane (Exec+Review) | L–XL | exec F4, safety F14 |

> **Why B4/B7 are conditional:** WG-Exec is not in the product path today (see M5), so its blockers
> are not *live*. They become live the instant exec is wired into dispatch. Treat the exec blockers
> as **gating preconditions on M5**, not as shippable-today regressions.

### 2.2 MAJORS — needed for v1

| # | Finding | Component | Effort | Sources |
|---|---|---|:--:|---|
| **M1** | At-rest root-key protection: seeds stored plaintext-hex in a `0600` file, signing is in-process. Anything that reads the file/process gets the root. (OS keyring / HSM / at-rest AEAD; ideally out-of-process signer.) | WG-Fed | M | fed A3 |
| **M2** | Wire the **S-7 compat handshake**: `check_compat` has zero non-test callers; the node negotiates no version. Call it at fetch/poll/serve. | WG-Fed | S | fed A5 |
| **M3** | Enforce `cid == hash(bytes)` in `get_object` and on node `PUT /objects` — today the content-address invariant the design leans on is unchecked at the boundary. | WG-Fed | S | fed G2/G7, testops #13 |
| **M4** | Node hardening: socket read/write timeouts (slow-loris), thread bound (connection flood), inbox GC/retention/cursor + delete-after-ack (unbounded growth, O(n) re-poll). | WG-Fed | M | fed G8/G9 |
| **M5** | **Wire exec into the dispatcher** (`Placement::Provider` from the planner + drive the wire from the coordinator) **or explicitly scope v1 to local-only** and document remote-exec as experimental-CLI. *Gated on B3+B4+B5(exec) being sound first.* **→ Erik decision.** | WG-Exec | L–XL | exec F8, testops #3 |
| **M6** | Attestation slot empty → confidential remote work can only be *refused*. Decide whether v1 ships confidential remote exec (→ real enclave/attestation, XL) or documents it as unsupported. **→ Erik decision.** | WG-Exec | XL | exec F5, testops #14 |
| **M7** | S-5 mechanism: `model_binding` enforcement is presence-only (never compared to the runtime model); there is no real state consumer (`AutoLoad` only prints "LOADED"). | WG-Fed | M–L | fed F5/F6 |
| **M8** | Sealed-sender outer envelope unauthenticated (routing metadata malleable; AEAD uses no AAD). Sign an outer commitment or fold metadata into the seal + enforce equality on open. | WG-Fed | M | fed C3 |
| **M9** | Envelope-layer replay/dedup store at the consume edge — `verify()` does not dedup; `id` is a key no consumer tracks. | WG-Fed | M | fed C5 |
| **M10** | Revocation propagation is withholdable (no freshness gate on revocation discovery; an untrusted node can omit a revocation and a revoked-but-unexpired cap is honored). Freshness-gate a revocation head or mandate short cap TTLs. | WG-Fed | M | fed D5 |
| **M11** | Recovery key is unrevocable + unwindowed despite the doc claiming atproto-style windowed recovery. Add a signed recovery-window + a root-signed `SetRecovery` link. | WG-Fed | M | fed B8 |
| **M12** | No equivocation/fork-history detection — a malicious signer can show two validly-signed divergent chains to different peers. Transparency log or head gossip. | WG-Fed | L | fed B9 |
| **M13** | DoS hardening: depth-cap UCAN chains (recursive `verify`/deserialize → stack overflow) and size-cap transport responses (`resp.bytes()`/`read_exact` are unbounded). | WG-Fed | S | fed D6/G3 |
| **M14** | Pass-3 sandbox + Pass-4 human gate / **quarantine-release** are missing — "quarantine pending human" is a terminal dead-letter (no `wg review release/approve`, no queue). A minimal release command is E-M and unblocks the residual-risk stance. **→ Erik decision (is a human operator in v1?).** | WG-Review | E-M → XL | safety F3 |
| **M15** | Bridge remote-exec usage into `task.token_usage` / `wg spend` / `wg stats` (today `Usage` is canned and stdout-only — the exact pi-handler under-count class). | WG-Exec | M | exec F11, testops #17 |
| **M16** | Liveness/lease enforcement is data-model-only: `LeaseRenewal` is defined-but-unused; no `renew` verb, heartbeat, or auto-reclaim-on-timeout. Either build it or delete the type and document accept-implies-liveness. | WG-Exec | L | exec F9 |
| **M17** | Grant drops sensitivity (hardcoded `Normal`); the `Claim` carries no sensitivity → the fail-closed confidential/unlabeled gate is structurally absent at grant (composes only because the CLI runs offer first). Carry signed sensitivity into claim/grant. | WG-Exec | S | exec F12 |
| **M18** | **Trust-dial conflation.** `resolve_author_trust` returns the *most-trusting* of (peer, provider) opinions → enrolling a box as a Verified *provider* auto-grants Verified *author* trust (skips deep review). Bare `wg peer add` → Provisional TOFU clears the Normal exec floor. Split provider-trust from author-trust, fail-closed-fold (min) for the review-depth input, default bare peer-add to Unknown. **→ Erik decision (one min-merged dial vs per-plane dials).** | Cross-plane (`trust.rs`) | M | fed H4, safety F12, testops #5/#6 |
| **M19** | Make the IC4 verdict enforcing, not advisory: withhold the bytes on a non-accept verdict instead of printing the body then returning `consumable:false`. | WG-Review | M | safety F10 |
| **M20** | Observability: `providers/` and `review/` emit zero tracing/logs; the node has no request log or `/metrics`; no counters for verdicts/placements/refusals/freshness-failures. Add spans + correlation IDs + a node metrics endpoint. | Cross-plane | M | testops #7/#8/#9, exec F13 |
| **M21** | Ops/deploy story: no runbook, deploy, monitoring, backup, or key-rotation procedure; the dual-main / `wg done` footguns are undocumented for federation operators. Write a one-page operator runbook. | Cross-plane | M | testops #10/#21 |
| **M22** | Failure-injection test pass: concurrency on the lease fence / node inbox / verdict-chain (all tested only sequentially), crash-mid-PUT + restart recovery, malformed/truncated/oversize wire input, fuzz the `serde` parsers. | Cross-plane | L | testops #11, exec §4, safety F11 |
| **M23** | Serialize verdict-chain appends (read-modify-write is unguarded → concurrent recorders lose records / break the hash-link) and index `find_by_cid` (O(n) full-parse per op). Same class as B3 (ledger). | WG-Review | M | safety F11 |
| **M24** | Promote Pass-1/Pass-2 signatures + normalization from compile-time seed lists to a maintained policy file with an adversarial **evasion corpus in CI** (paraphrase / base64 / hex / homoglyph / leet — all currently walk through). | WG-Review | L | safety F4/F5 |
| **M25** | Build the review timeout/error → loud-recorded-SKIP-that-fails-closed path (ADR-CS3 D2) — required the moment Pass-2 becomes a real network call. | WG-Review | M | safety F8 |
| **M26** | Review-fatigue / flood DoS (RA-5): rate-limit, flooder de-trust, blast-radius-prioritized queue. | WG-Review | L | safety F9 |
| **M27** | **FP bug — fix `weakens_assert`** (matches the bare word `assert` → hard-rejects ~every honest test-touching diff). Require a *removed/disabled* assertion. Cheap, high-value. | WG-Review | S | safety F6 |
| **M28** | DHT/Iroh discovery deferred → the "decentralized" claim is unmet; resolution is manual config + cached records only. Bind a real discovery rung **or** drop the decentralized claim for v1. **→ Erik decision.** | WG-Fed | L | fed H2 |
| **M29** | Promote the 5 cross-host smokes (which SKIP without python3/curl/tmux) to always-on `tests/integration_*.rs` over in-process `FileStore` so a wire regression fails CI on a minimal runner. | Cross-plane | M | testops #12 |
| **M30** | Model-strength escalation on the high band + a genuinely independent quorum (today: 2 cycled copies of 2 fn pointers). | WG-Review | M | safety F7 |

### 2.3 MINORS — polish / hardening / known deferrals

Grouped; each is cheap (mostly S) and non-blocking.

- **WG-Fed:** canonical-JSON float/NaN normalization (A6); guardian-endorsement `prev`/expiry (B10);
  `SetEndpoints`/`SetAliasProof` are verify no-ops (B11); chain-length cap (B12); `open_multi` retry
  other wraps (C6); UCAN clock-skew tolerance (D7); sign/namespace the `seen_seq` tracker, per-device
  (E2); trusted time source (E3); fold freshness head-check into one gated API (E4); offline-send
  queue (G4); TLS or documented proxy requirement on the node (G10).
- **WG-Exec:** require `--new-provider` on reclaim (placeholder wgid today, F14); verify-after-authenticate
  ordering (F15, nit); harden `field_scan` toward a structural key check (F16).
- **WG-Review:** RA-11 spotlight-on-render contract for the verdict log (F13); opaque `ContentClass` +
  "opaque ⇒ never light" rule for IC3 (F15 — arguably MAJOR if IC3 is in v1); config/operating-point
  surface + lint (F16).
- **Cross-cutting:** `wg fed lint` / `wg provider lint` for registries + `WG_FED_LEASH_*` env (parse
  errors silently ignored today) (testops #15); IC4 doc drift — CLAUDE.md/manifest read as if poll
  always screens (testops #20).

---

## 3. The scaffold-vs-silicon register

Every place a slot / interface / type seam shipped without the real payload behind it, and what it
takes to make each real. This is the canonical "what is a demo" list — none of these are defects
(all are declared spark boundaries); they are the **production delta**.

| # | Slot shipped | What's there now | The silicon it needs | Effort | Wave |
|---|---|---|---|:--:|---|
| S1 | **Review Pass-2 reviewer** | Deterministic keyword classifier (`pass2_review.rs`) behind a real no-scope structural bound | Weak-tier `.review-*` LLM one-shot (via `resolve_agency_dispatch`) in the existing no-scope slot + timeout→fail-closed-SKIP + genuinely independent N-reviewer quorum + model-strength-by-depth | L | Review-C |
| S2 | **Review Pass-3 sandbox** | Label only (`max_pass:3`); never executes | Real container/VM detonation for IC2 + opaque artifacts; containment-as-cage | L | Review-C/D |
| S3 | **Review Pass-4 human gate** | None — quarantine is terminal, no release | `wg review release/approve` + escalation queue + the reused S-5 human gate | E-M→XL | Review-D |
| S4 | **Exec integrity re-run oracle** | Substring presence/absence (`rerun_against_pinned_spec`) | Real pinned test-suite + `auto_evaluate` re-execution in a trusted domain (≠ producer, ≠ provider's tests) | L | Exec-C |
| S5 | **Exec worker (`wg provider run`)** | Emits constant `LEGIT_DIFF`/`CORRUPT_DIFF`; never calls a model | Real model-handler-driven remote execution + canonical usage accounting (M15) | L | Exec-C |
| S6 | **Exec attestation enclave** | `attested` hardcoded `false`; empty measurement allow-list ⇒ confidential = refuse-only | Real TEE quote verification + seal-to-quote + measurement allow-list | XL | Exec-D |
| S7 | **Exec B verified-overflow tier** | Unbuilt; "B" today = "A + decoupled manual verify" | A vouched/attested overflow pool distinct from the trusted pool + eval-gate (depends on B4 wired) | L | Exec-C |
| S8 | **Exec quorum** | Unbuilt (single disjoint re-run is the lever) | N-verifier quorum — **blocked on unsolved sybil-resistance** | XL+research | deferred |
| S9 | **Exec dispatch integration** | `Placement::Provider` / `RemoteRunner` type seams defined, never constructed/driven | Planner computes placement (tag/sensitivity → leash → matcher → grant); coordinator drives the wire | L–XL | Exec-C |
| S10 | **Exec liveness runtime** | `LeaseRenewal` type defined-but-unused; reclaim is manual | `renew` verb + heartbeat loop + auto-reclaim-on-timeout | L | Exec-C |
| S11 | **Fed S-5 content scanner** | ~10-phrase static list (`state_safety.rs`) | The same real reviewer as S1 (shared silicon) | L | Review-C |
| S12 | **Fed `model_binding` enforcement** | Presence-only check, opaque kinds only | Compare binding to the consuming agent's runtime model; fail-closed on mismatch | M | Fed-hardening |
| S13 | **Fed state consumption** | `AutoLoad` prints "LOADED"; no payload decoded | A real conv-cache/portable-state consumer that loads the gated payload into a running agent | M–L | Fed-hardening |
| S14 | **Fed transport library** | Dumb `FileStore` + bespoke un-hardened HTTP node | Abuse-hardened transport (B1/B2/M3/M4) + DHT/Iroh discovery (M28) | L–XL | Fed-transport |
| S15 | **Fed S-7 compat handshake** | `check_compat` defined, zero wire callers | Invoke at fetch/poll/serve (M2) | S | Wave 0 |
| S16 | **Fed recovery window** | Doc claims windowed; no window/`SetRecovery` implemented | Signed recovery-window + revocable recovery slot (M11) | M | Fed-hardening |
| S17 | **Fed equivocation/fork detection** | None | Transparency log or head gossip (M12) | L | Fed-hardening |
| S18 | **Fed MLS / forward-secret groups** | Static recipient keys on the offline path; honestly documented | MLS/Double-Ratchet — **online/long-lived groups only; does not compose with send-to-offline (S-6)** | XL | deferred-by-decision |

> **S18 is a correctly-scoped non-goal, not a stub pretending to be FS.** It is listed for
> completeness so the register is exhaustive, not because it is v1 work.

---

## 4. Sequenced v1 roadmap to full production readiness

Ordered by dependency. Each wave names what it unblocks. The exec blockers (B3/B4/B7) are
**preconditions on the exec-integration decision (M5)**, so the whole right-hand column hinges on
the first Erik decision below.

```
Wave 0  (independent, cheap, do first) ──┬──► Wave 1 (detectors) ──► Wave 2 (wire the seams)
  node DoS hardening, ledger crash-safe   │                              │
  CID-verify, S-7 handshake, FP fix        │                              ▼
  UCAN/transport caps                      └──► Wave 3 (exec integration + confidentiality)
                                                         │
Wave 5 (ops/test/observability) ── runs in parallel ─────┴──► Wave 4 (cross-plane residuals + fed hardening)
```

### Wave 0 — Make the reachable paths safe *(no dependencies; mostly S; do immediately)*
Removes the unsafe-as-shipped behaviors on the one path that *is* exposed today (the node) and the
cheap correctness fixes. **Gate:** anything network-facing must not ship before this.
- **B2** Content-Length cap (S) · **B1** node write-auth + quotas/rate-limit (M)
- **M3** CID-verify on get/PUT (S) · **M4** node timeouts + thread bound + inbox GC (M)
- **B3** exec ledger atomic-temp+rename + advisory lock + refuse-on-corrupt-parse (M)
- **M13** UCAN depth-cap + transport size-cap (S) · **M2** wire S-7 handshake (S)
- **M27** `weakens_assert` FP fix (S)

### Wave 1 — Build the silicon (the long pole for the safety story) *(depends on: nothing structurally — slots are ready)*
Until this lands, "safe" means "we have the slot." Maps onto **Review-Wave C** + **Exec-Wave C**.
- **B5/S1** real weak-tier Pass-2 reviewer + **M25** timeout-fail-closed + **M30** independent quorum
- **B5/S4/S5** real exec integrity re-run (pinned suite + `auto_evaluate`) + real worker
- **S11** fed S-5 scanner reuses the Pass-2 reviewer (shared silicon)
- **M24** adversarial evasion corpus in CI (proves the new silicon generalizes past seed keywords)

### Wave 2 — Wire the seams fail-closed *(depends on: Wave 1 detectors being real)*
Connects the dead nerves so "received ≠ consumed" holds with no manual step.
- **B6** wire IC1 (import) + IC2 (accept) to the pipeline; make IC4 the **default** (not opt-in)
- **M19** withhold bytes on non-accept (enforcing, not advisory)
- **B4** gate exec `accept` on `verification_depth` (meaningful now that the re-run is real)
- **M18** split provider-trust / author-trust; fail-closed-fold; bare peer-add → Unknown
- **M23** serialize the verdict-chain append (same fix class as B3)

### Wave 3 — Exec integration + confidentiality *(depends on: Wave 0 fence sound + Wave 1 re-run real + Wave 2 accept-gates-verify; ERIK DECISION)*
Maps onto the back half of **Exec-Wave C** + the **Exec-Wave D** attestation decision.
- **M5** drive `Placement::Provider` from the planner (only after B3/B4/B5 — driving it sooner exposes the gaps at scale)
- **M6** attestation enclave **iff** confidential remote exec is in v1 (else document refuse) — **Exec-Wave D**
- **M16** liveness runtime (renew/heartbeat/auto-reclaim) · **M15** accounting bridge · **M17** sensitivity-in-grant · **S7** B-tier
- remote-task observability (folds into M20)

### Wave 4 — Cross-plane residuals + federation hardening *(depends on: Wave 3 for the exec half of TC8)*
- **B7/F14** TC8 cross-task poison: tier-by-graph-position + descendant enumeration/re-run + input re-verify across trust boundaries (joint Exec+Review)
- **M1** at-rest key protection · **M8** sealed-sender outer auth · **M9** envelope dedup · **M10** revocation freshness-gate
- **M11/S16** windowed+revocable recovery · **M12/S17** equivocation/fork detection · **M7/S12/S13** `model_binding` + real state consumer
- **M14/S2/S3** Pass-3 sandbox + Pass-4 human gate + quarantine-release · **M26** review-flood DoS
- **M28** DHT/Iroh discovery (iff "decentralized" is a v1 claim)

### Wave 5 — Ops, observability, test hardening *(no hard dependency; start alongside Wave 0, land before GA)*
- **M20** tracing spans + correlation IDs + node `/metrics` · **M21** operator runbook + key-rotation/backup procedures
- **M22** failure-injection test pass (concurrency / crash / malformed-oversize / fuzz)
- **M29** always-on integration CI for the cross-host wire · **MINORS** registry/env lint, doc-drift fixes

### Decisions only Erik can make (call these out before Wave 3)

1. **Does v1 include remote execution at all, or is v1 local-only?** This is the pivot. *Local-only*
   makes B3/B4/B7/M5/M6/M15/M16/M17/S4–S10 all **out of v1 scope** (document remote-exec as
   experimental-CLI) and collapses the roadmap to Fed + Review hardening. *Remote-exec-in-v1* makes
   the entire exec column a v1 requirement with the dependency chain above.
2. **If remote exec ships: does v1 run confidential workloads remotely?** If yes → the attestation
   enclave (M6/S6, XL, Exec-Wave D) is the long pole. **Recommended default: no** — document
   confidential remote work as unsupported (fail-closed *refuse*, which is already correct), not
   "secured." Don't let the empty slot read as a capability.
3. **Is a human operator part of v1?** Gates the Pass-4 / quarantine-release scope (M14/S3). If no,
   v1 quarantine is a terminal reject and the residual-risk stance is explicitly "block, don't
   triage." A *minimal* `wg review release` (E-M) is cheap insurance even if the full queue defers.
4. **One min-merged trust dial vs per-plane dials?** (M18) Determines whether provider-trust and
   author-trust are unified-but-min-folded or kept as separate dials. Either fixes the conflation;
   the choice is an API/UX call.
5. **Does v1 claim "decentralized federation" or "configured-peer federation"?** (M28) If
   configured-peer is acceptable for v1, DHT/Iroh defers cleanly and the manual-config cascade is a
   documented v1 limitation rather than a gap.

---

## 5. Bottom line

- **Ship-safe today only because the unsafe paths aren't wired.** The exposed surface is the node
  (B1/B2 — fix in Wave 0) and the opt-in review gate. Everything else dangerous is reachable only
  through manual CLI ceremonies.
- **The blockers cluster in three places:** the node (B1/B2), the exec fence + accept (B3/B4), and
  the stubbed detectors + unwired seams (B5/B6). B7 (TC8) is the cross-plane residual the whole
  stack jointly owns.
- **The single highest value/effort move** is wiring IC1/IC2 + making IC4 default (B6, M) on top of
  the existing pipeline — it connects two dead nerves cheaply. **The single longest pole** is the
  detector silicon (B5, L×3) — without it the safety story is "we have the slot," and the green
  smokes prove plumbing, not detection.
- **The crypto is done.** Trust the verification code to reject forgery, tampering, expiry,
  attenuation-widening, and below-threshold recovery. Do not trust the daemon, the detectors, the
  wiring, or the ops story until the waves above land.

*Synthesis only — no production code changed (git status: only `docs/prod-audit/`).*
