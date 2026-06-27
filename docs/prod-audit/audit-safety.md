# Production-Readiness Audit — Content-Safety Review Gate (WG-Review)

**Task:** `audit-safety` · **Scope:** the cs-spark code (`src/review/`), its CLI
(`src/commands/review_cmd.rs`), the auto-wired ingest gate (`src/commands/identity_cmd.rs`
`run_poll` / `review_inbound_event`, `src/trust.rs`), and the inbound seams it is *meant*
to cover (task-import IC1, result-accept IC2, state-load IC3, message IC4).
**Verified against:** `docs/ADR-content-safety-001..003-*.md` and
`docs/content-safety-study/03-adversarial-evaluation.md` (+ `04` decision memo).
**Mode:** analysis only. No code changed. Skeptic's read.

> **Validation status:** all 26 `cargo test --lib review::` unit tests pass (confirmed
> this session). The code that exists *works*; the audit's quarrel is with **how much
> exists** vs. what the ADRs require for production, and with the **adversarial coverage
> of the deterministic stubs that stand in for the real silicon.**

---

## 1. Bottom line up front

WG-Review **is a faithful structural spark and a deliberate one.** Every load-bearing
design decision in ADR-CS1/2/3 has a *shape* in code: a fail-closed monotonic pipeline,
a trust×sensitivity depth matrix, a uniform accept/quarantine/reject verdict, the dual-LLM
no-scope reviewer bound, spotlight+normalize, a hash-linked verdict sigchain, digest-pinned
consumption, and a loud revoke leg. The smoke gate (`content_safety_spark.sh`,
`e2e_autowire_ingest_gate.sh`) exercises them end-to-end. **As a proof-of-correctness of
the *choice*, it succeeds.**

It is **not production-ready**, and — critically — it does not claim to be. The honest
one-line verdict:

- **The detection layer is a deterministic keyword/heuristic stand-in, not the reviewer
  the ADRs specify.** "Are the passes real?" — **No.** Pass 0/1/2 run; **Pass 2 is a
  hard-coded string classifier, not a weak-tier LLM** (`src/review/pass2_review.rs:29-36,
  82-83`); **Pass 3 (sandbox) and Pass 4 (human) do not execute at all** despite
  `depth.max_pass` advertising `3` (`src/review/depth.rs:97-103` vs `src/review/mod.rs:377`).
- **The gate is wired into exactly one of its four ingest seams.** IC4 (message) is
  auto-wired (`src/commands/identity_cmd.rs:1067`); **IC1 import, IC2 accept, and IC3
  load-state call `review_inbound` from nowhere** (grep: only `review_cmd.rs` and the IC4
  poll path call it). ADR-CS1 D1's headline is "four ingest seams"; three are unhooked.
- **The containment+audit+revoke leg — which doc 03 §5 says is the actual safety
  guarantee — is the most production-shaped part** and is genuinely good (digest-pin,
  sigchain, monotonic trust-lowering all real and tested). But **"quarantine pending
  human" has no human**: there is no escalation queue, no `release`/`approve` command, no
  Pass-4 gate (`src/cli.rs:3451-3510` — the `ReviewCommands` enum is Check/Depth/
  ReviewerScope/Log/Consume/Revoke, full stop).

So: **the skeleton is right, the muscle is papier-mâché, and three of four nerves aren't
connected.** This is exactly the spark boundary the decision memo §4.3 and the ADR Status
sections declare (Review-Wave C/D) — the audit's job is to make the gap precise and
severity-ranked, which §3–§6 do.

---

## 2. "Real today vs scaffold" — the one-glance table

| Capability | State | Where |
|---|---|---|
| Fail-closed monotonic pipeline (Pass 0→1→2, strictest-wins) | **REAL** | `mod.rs:303-404` |
| Trust×sensitivity depth matrix, fail-closed-on-unlabeled, sensitivity-floors-depth | **REAL** | `depth.rs:65-104` |
| Uniform accept/quarantine/reject verdict + `permits_consumption` | **REAL** | `mod.rs:138-168` |
| Taint-inference overrides self-asserted `low`→`high` | **REAL (seed inputs)** | `pass1_lint.rs:194-207`, `mod.rs:333-338` |
| Normalize-before-scan (zero-width, fullwidth, whitespace) | **REAL but narrow** | `pass1_lint.rs:55-73` |
| Pass-2 reviewer **no-scope bound** (pure fn, `act-as-reviewer` only) | **REAL (structural)** | `pass2_review.rs:44-51, 77-99` |
| Pass-2 spotlight nonce delimiter | **REAL** | `pass2_review.rs:65-72` |
| Pass-2 **as an actual classifier** (weak-tier LLM) | **SCAFFOLD** (deterministic keyword lists) | `pass2_review.rs:29-36, 108-119, 149-166` |
| Pass-1 ruleset | **SCAFFOLD** (one seed signature per category) | `pass1_lint.rs:96-157` |
| Diverse-reviewer quorum (N independent) | **SCAFFOLD** (2 hard-coded fn pointers, cycled) | `pass2_review.rs:82-97` |
| Model-strength escalation on high band (ADR-CS2 D4) | **MISSING** | — |
| Pass 3 sandbox detonation (IC2/opaque) | **MISSING** (label only; never runs) | `depth.rs:98,101` (`max_pass:3`+label) vs `mod.rs:377` (only `runs_pass2`) |
| Pass 4 human-in-loop gate / quarantine release | **MISSING** | `cli.rs:3451-3510` |
| Verdict sigchain (hash-linked, content-addressed) | **REAL** | `verdict.rs:104-173` |
| Digest-pinned consumption (RA-8 TOCTOU) | **REAL** | `verdict.rs:188-215` |
| Loud revoke: trace author, lower trust, name consumers | **REAL (single hand-wired consumer)** | `verdict.rs:235-267` |
| Canonical derived author-trust (peer registry ∪ exec pool) | **REAL** | `trust.rs:79-106` |
| **IC4 message** ingest auto-gate | **REAL (wired)** | `identity_cmd.rs:1024-1186` |
| **IC1 task/import** ingest gate | **MISSING (unwired)** | `agency_import.rs:627` (no call) |
| **IC2 result/accept** ingest gate | **MISSING (unwired)** | `exec_fed_cmd.rs:645` (no call) |
| **IC3 state-load** ingest gate (via `review_inbound`) | **N/A — separate pipeline** | `identity_cmd.rs:1664` uses `state_safety`, not `review` |
| Review-pass timeout/error fail-closed handling | **MISSING** (no fallible pass exists yet) | — |
| Rate-limit / flooder de-trust (RA-5 DoS) | **MISSING** | — |
| Config/operating-point surface (matrix, signatures) | **MISSING** (all hard-coded consts) | `depth.rs`, `pass1_lint.rs` |

---

## 3. Findings table (severity = production risk if shipped as-is; effort = to close)

Severity: **S1** ship-blocker · **S2** serious · **S3** moderate · **S4** minor.
Effort: **E-S** ≤1d · **E-M** days · **E-L** weeks · **E-XL** multi-wave/cross-plane.

| # | Capability / gap | Class | ADR/doc decision | Sev | Effort |
|---|---|---|---|:--:|:--:|
| F1 | Pass 2 is a deterministic keyword classifier, **not** the weak-tier `.review-*` LLM the ADR mandates | demo | ADR-CS2 D1; memo §4.3 | **S1** | E-L |
| F2 | IC1 (import) and IC2 (accept) seams **unwired** — gate never runs on task or artifact ingest | missing | ADR-CS1 D1 ("four ingest seams") | **S1** | E-M |
| F3 | Pass 3 sandbox + Pass 4 human gate **do not execute**; "quarantine pending human" has no human / no release path | missing | ADR-CS1 D2 Pass 3/4; ADR-CS3 D5 | **S1** | E-XL |
| F4 | Pass-1 / Pass-2 signatures are tiny seed lists; trivially evaded by paraphrase (RA-2) | demo | ADR-CS1 OQ1 (living policy = Wave D) | **S2** | E-L |
| F5 | Normalize covers only zero-width + fullwidth; no base64/hex/rot13/homoglyph/leet (RA-2) | demo | doc 03 RA-2 (residual real) | **S2** | E-M |
| F6 | IC2 test-rewrite heuristic `weakens_assert` matches the bare word `assert` → **every** test-touching diff hard-rejected (massive FP) | bug | doc 03 RA-6 (FP-cost dial) | **S2** | E-S |
| F7 | No model-strength escalation on the high band; quorum is 2 cycled copies of 2 fn pointers (not independent models/prompts) | demo | ADR-CS2 D4 | **S2** | E-M |
| F8 | No review-pass timeout/error path → ADR-CS3 D2 "loud recorded SKIP that fails closed" is unimplemented (no fallible pass exists yet, but the production LLM call will need it) | missing | ADR-CS3 D2 | **S2** | E-M |
| F9 | RA-5 review-fatigue / flood DoS: no rate-limit, no flooder de-trust, no blast-radius-prioritized queue | missing | doc 03 RA-5; ADR-CS3 D1 | **S2** | E-L |
| F10 | IC4 gate is **advisory** at the CLI: the message body is printed *before and regardless of* the verdict; "refuse consumption" = a JSON `consumable:false` the caller must honor, not withheld bytes | partial | ADR-CS1 D1 (received ≠ consumed) | **S2** | E-M |
| F11 | Verdict-record append is read-whole-file → modify → atomic-write (`verdict.rs:125-173`); two concurrent recorders race (lost record / broken hash-link); `find_by_cid` is O(n) full-parse per consume | bug/scale | ADR-CS3 D2 | **S3** | E-M |
| F12 | `resolve_author_trust` returns the **most-trusting** of peer-registry vs exec-pool opinions (`trust.rs:53-58, 100-106`) — a single stale Verified vouch in either home wins; trust-inflation surface | risk | ADR-CS1 D3 / D5 (one dial) | **S3** | E-S |
| F13 | RA-11 verdict-channel: `reason` is correctly an enum (good!), but `wg show`/`wg review log` rendering is **not** spotlighted-as-data per MUST-3's render requirement (today only the *type* is bounded, not the render contract) | partial | ADR-CS2 MUST-3; doc 03 RA-11 ★ | **S3** | E-S |
| F14 | RA-4 / TC8 cross-task slow-poison: revoke names **one** hand-wired `consumer_task`; no graph-walk of `--after` consumers, no cross-plane D-iii re-run | missing | ADR-CS3 D4 (cross-plane, flagged) | **S2** | E-XL |
| F15 | RA-12 opaque-blob: `ContentClass` has no opaque kind; IC3 Pass-1 is a "conservative placeholder" text scan (`pass1_lint.rs:81-85`); the "opaque ⇒ never light regardless of trust" rule is not encoded | missing | doc 03 RA-12; ADR-CS1 D5 | **S3** | E-M |
| F16 | No config surface: depth matrix + signature lists are compile-time `const`/`match`; no operating-point file, no `wg config lint` depth integration | missing | ADR-CS1 OQ2; ADR-CS2 OQ1 (Wave C) | **S3** | E-M |
| F17 | Manual paths (`wg review check`, and any future IC1/IC2 hook) still **hand-pass `--trust`** (`review_cmd.rs:48-79`); only the IC4 poll path derives trust canonically | partial | CLAUDE.md "auto-wire the four ingest seams" | **S2** | E-M |

---

## 4. Dimension 1 — Completeness

### 4.1 Are the review passes real?  **No — deterministic stand-ins; two passes don't run.**

The pipeline (`src/review/mod.rs:303-404`) is genuinely fail-closed and monotonic and runs
**Pass 0 (provenance), Pass 1 (lint), Pass 2 (reviewer)**. But:

- **Pass 2 is a pure deterministic function over hard-coded keyword lists**, not an LLM.
  `review()` (`pass2_review.rs:77-99`) calls two `fn` pointers `reviewer_signature` /
  `reviewer_intent` (`:82-83`) whose entire logic is `n.contains("...")` against frozen
  arrays (`:108-119`, `:149-166`). The module header is explicit and honest: *"The reviewer
  here is a deterministic semantic classifier, not a live weak-tier LLM call"* (`:29-36`).
  This is the **single biggest "passes real?" answer: the detection silicon is absent.**
  The *structural* bound it proves (no-scope, pure-function, spotlight) is real and valuable;
  the *classifier* is not.
- **Pass 1 is one seed signature per category** (`pass1_lint.rs:96-157`), explicitly a
  living-policy placeholder (ADR-CS1 OQ1, Wave D). Real as a mechanism, demo as a ruleset.
- **Pass 3 (sandbox) and Pass 4 (human) never execute.** `review_depth` returns
  `max_pass: 3` and the label `"Pass 1+2+3, quarantine-by-default (unknown)"` for Unknown
  (`depth.rs:97-103`), but `review_inbound` only gates on `depth.runs_pass2()` and never
  has a `runs_pass3()`/Pass-4 branch (`mod.rs:377`). The advertised depth **overstates**
  what runs — an Unknown item is labeled "1+2+3" but only 1+2 execute. Minor honesty bug
  in the label; major capability gap underneath.

### 4.2 Trust-proportional depth — **implemented, and the best-realized completeness item.**

`depth.rs:65-104` is a real matrix: Verified+Low ⇒ light (Pass 1 only, accept-default);
Verified+High / Provisional ⇒ Pass 1+2; Unknown ⇒ quarantine-default + quorum 2. The two
coherence rules (fail-closed-on-unlabeled via the `Unlabeled⇒High` fold at `mod.rs:336`;
sensitivity-floors-depth via the `(Verified, _)` high row at `depth.rs:75`) are present and
unit-tested (`depth.rs:128-149`). This faithfully implements ADR-CS1 D3. **No gap of
substance** beyond it being non-configurable (F16).

### 4.3 Verdicts + human escalation.

accept/quarantine/reject is a real `Ord`-derived enum where strictest-wins = `max`
(`mod.rs:138-168`) — clean and correct. **Human-in-loop escalation is the hole:**
`quarantine` is documented as "held pending escalation / human" (`mod.rs:144-147`) and the
CLI even prints *"held un-consumed until review releases"* (`review_cmd.rs:131`) — **but
there is no release.** No `wg review release`, no approval queue, no Pass-4 gate
(`cli.rs:3451-3510`). Quarantine is a terminal dead-letter, not a reversible hold with an
operator path. ADR-CS3 D5 / ADR-CS1 D2 Pass 4 (the reused S-5 human gate) is unbuilt here.

### 4.4 Which inbound paths are actually covered?  **One of four.**

ADR-CS1 D1 names four ingest seams. Reality (grep of `review_inbound` call sites):

| Seam | ADR intent | Wired? | Evidence |
|---|---|---|---|
| **IC1** task/prompt (graph-import / placement) | Pass 0→4 gate | **NO** | `agency_import.rs:627` `run_import` never calls `review_inbound` |
| **IC2** result/artifact (`ResultEnvelope` accept) | Pass 0→4 gate | **NO** | `exec_fed_cmd.rs:645` `run_accept` never calls `review_inbound` |
| **IC3** state-load | reuse S-5 pipeline | **separate** | `identity_cmd.rs:1664` `run_load_state` uses `state_safety`, not `review` (defensible per ADR, but not unified) |
| **IC4** message (inbox poll) | IC1 pipeline | **YES** | `identity_cmd.rs:1067` `run_poll(... review=true)` → `review_inbound_event` (`:1157`) |

So the only live production hook is `wg msg poll --review` / `wg identity poll --review`.
The smoke scenarios for IC1/IC2 (`content_safety_spark.sh`) drive the gate through the
**manual** `wg review check` CLI, not through `run_import` / `run_accept` — i.e. the
scenario proves the *pipeline*, not that the import/accept code paths *invoke* it. CLAUDE.md
itself concedes this ("The remaining import / accept / state-load ingest seams stay
Review-Wave C/D"). The audit confirms it in code: F2.

---

## 5. Dimension 2 — Security / adversarial (each doc-03 attack, checked in code)

Legend: **Defended** (mechanism present + effective at spark scope) · **Structural-only**
(the containment bound is real, detection is stubbed) · **Partial** · **Undefended/Deferred**.

| Attack | In-code status | Evidence & skeptic's note |
|---|---|---|
| **RA-1 inject-the-reviewer** | **Structural-only — the bound is real, the detector is a stub** | The no-scope bound is genuinely enforced: `review()` is `fn(ContentClass,&str)->Verdict` with no graph/net/fs handle, scope is the literal `["act-as-reviewer"]` (`pass2_review.rs:44-51`), field-scan asserted (`:216-224`). So a flipped reviewer ⇒ wrong **verdict**, never wrong **action** — **this part is production-grade.** BUT the "won't be flipped" claim rests on a deterministic classifier that treats lure strings as *suspicion-raising data* (`:108-126`). A real weak-tier LLM (Wave C) is exactly the injectable surface doc 03 calls Fatal-as-prevention; the spark cannot prove the LLM resists RA-1b/RA-1c because there is no LLM. **The smoke "inject-the-reviewer contained" assertion proves the structural bound, not silicon resistance.** |
| **RA-1b spotlight-escape** | **Defended (structural)** | Nonce delimiter is content-derived BLAKE3, payload can't predict it (`pass2_review.rs:65-72`); a forged `---END UNTRUSTED---` stays inside the span and is itself a reject signal (`:116-119`). Sound *as a framing*; again untested against a real model. |
| **RA-2 obfuscation/encoding** | **Partial — narrow** | `normalize` strips zero-width/tag chars + folds fullwidth + collapses whitespace + lowercases (`pass1_lint.rs:55-73`). **Does NOT** handle base64, hex, rot13, URL-encoding, homoglyph (Cyrillic а/о), leetspeak, or split-across-fields. doc 03 RA-2 scores this **2/5, "residual real"** — the code matches that: a paraphrase ("disregard the prior directives") or a Cyrillic homoglyph walks straight through the seed lists. F4/F5. |
| **RA-3 trusted-actor-turned-bad** | **Defended-as-detect-revoke (per design)** | The light path *is* the bypass by construction (Verified+Low ⇒ Pass 1 only, `depth.rs:68`). Mitigations present: sensitivity-floors-depth (`depth.rs:75`) and taint-inference upgrade (`pass1_lint.rs:194-207`) bound *how bad* a light-path miss is; the loud revoke leg + monotonic trust-lowering (`verdict.rs:235-302`) catch it after. This faithfully implements the "bound + revoke, don't prevent the first shot" stance (doc 03 §6.2). **Gap:** no trust *decay* over time, no anomaly-on-light-path detector — both named as the RA-3 fix and both absent. |
| **RA-4 / TC8 cross-task slow-poison** | **Deferred (flagged, not closed)** | Per-task review is structurally blind to cross-task activation. revoke records a single `consumer_task` and re-runs *it* (`verdict.rs:265`), but there is no `--after` graph walk and no cross-plane D-iii. Matches ADR-CS3 D4's "cross-plane, flagged, not falsely closed." F14, **S2/E-XL — the one residual the whole stack jointly owns.** |
| **RA-8 TOCTOU (check vs use)** | **Defended — the strongest leg** | `digest_pin_consume` re-hashes presented bytes and permits only a CID matching an `accept` record (`verdict.rs:188-215`); a mutated byte ⇒ no match ⇒ refused, unit-tested (`:361-379`). doc 03 scores RA-8 **4/5**; the code earns it. **Caveat (doc 03's own):** this only holds where consumption *calls* `digest_pin_consume`. Today only the explicit `wg review consume` does; IC1/IC2 consumers don't (because they aren't wired — F2), so the TOCTOU close is **available but not yet enforced at the seams**. |
| **RA-9 leash/routing manipulation** | **Defended (fail-closed)** | Unlabeled⇒High fold (`mod.rs:336`), taint-inference overriding self-asserted low (`mod.rs:333-338`, `pass1_lint.rs:194-207`), tested (`mod.rs:476-490`). Failure direction is over-review (FP), per D-i. Seed input list is incomplete (taint never provably complete — doc 03 owns this). |
| **RA-11 verdict-channel 2nd-order** | **Partial — half-built (the novel finding)** | The *type* defense is done: `reason` is `ReasonCode` enum, never free-form (`mod.rs:177-227`), so a payload can't be laundered into prose. **But MUST-3 also requires the render be spotlighted-as-data**; `wg review log` prints `r.reason.tag()` plainly (`review_cmd.rs:204-214`) and there's no spotlight-on-render contract. Since it's an enum the *current* re-injection risk is ~nil, but the moment any free-text field (e.g. a future evidence excerpt) is added, the render path is unprotected. F13. |
| **RA-5 review-fatigue DoS** | **Undefended** | No rate-limit, no flooder de-trust, no prioritized queue. A P-FLOOD adversary submitting borderline items has no mechanical backpressure. F9. |
| **RA-7 supply-chain depth** | **Deferred** | No dep-tree resolve-and-scan; Pass 3 (the detonation leg) doesn't exist. Out of spark scope; note for completeness. |
| **RA-10 sandbox evasion** | **Deferred** | Pass 3 sandbox unbuilt; containment-as-cage argument can't be exercised. |
| **RA-12 opaque-blob** | **Undefended in code** | No opaque `ContentClass`; IC3 Pass-1 is a placeholder text scan (`pass1_lint.rs:81-85`); the "opaque ⇒ never light regardless of trust" rule (doc 03's explicit demand) is not encoded. F15. |
| **X-6 test-file rewrite** | **Defended but wildly over-broad** | `touches_test && weakens_assert` hard-rejects (`pass1_lint.rs:139-147`). Catches the backdoor-hides-behind-disabled-test case — but `weakens_assert` includes the bare substring `"assert"`, so **any legitimate diff that adds an assertion to a test file is hard-rejected.** Correct-direction, but the FP rate is ~100% on normal test development. F6. |
| **X-5 backdoor-shaped code** | **Defended (narrow)** | Requires a backdoor token *and* a fetch (`pass1_lint.rs:148-161`, `pass2_review.rs:201-209`). `eval(remote)` without an `http`/`fetch(` literal slips; obfuscated exec slips. Seed-grade. |

**Net adversarial read:** the *containment* columns (RA-1 no-scope bound, RA-8 digest-pin,
RA-3 revoke) are real and good — exactly the columns doc 03 §5 says carry the safety load.
The *detection* columns (RA-1 classifier, RA-2 encoding, RA-4 cross-task, RA-12 opaque) are
stubbed or deferred — exactly the columns doc 03 scores 1–2. The code's strengths and
weaknesses line up precisely with the study's worst-case scorecard, which is itself a sign
the build is honest. The danger is **mis-reading the green smoke as "adversarially
covered"** when what's proven is the *structural bound*, not the *classifier*.

---

## 6. Dimension 3 — The residual-risk boundary ("mitigate, don't eliminate")

**Implemented, and the most production-faithful part of the spark.** ADR-CS3 D5's stance —
*safety = containment + audit + revoke, not detection* — is realized:

- **Containment:** quarantine = zero-consumed held state (`mod.rs:165`,
  `permits_consumption` only on `accept`); the reviewer's no-scope bound (RA-1).
- **Audit:** every verdict (incl. rejects and the provenance-missing quarantine) is recorded
  on the hash-linked content-addressed sigchain (`verdict.rs:104-173`); nothing silently
  dropped (the IC4 path records best-effort, `identity_cmd.rs:1178`).
- **Revoke:** trace-author → lower-trust (monotonic, persisted to `trust_overrides.json`) →
  name-consumer, with the next item provably taking the deep path
  (`verdict.rs:235-267`, tested `:382-405`; folded strictest-wins at `review_cmd.rs:69`,
  `identity_cmd.rs:1162`).

**Where the stance is *assumed* rather than *implemented*:** the human-in-loop end of the
boundary (Pass 4) — the place doc 03 explicitly parks the RA-1/RA-3/RA-5 residuals ("the
residual is human, not mechanical") — **has no mechanism** (F3). The design says "quarantine,
then a human decides"; the code quarantines and there is no human surface to decide. The
residual-risk boundary is therefore **half-implemented**: the containment+audit+revoke 75% is
real; the human-escalation 25% it leans on for the irreducible tail is a stub. That's the
single most important "assumed, not built" item for an honest mitigate-don't-eliminate claim.

---

## 7. Dimension 4 — Failure modes

- **Fail-open vs fail-closed on pass failure/timeout.** The *decision* face is fail-closed:
  missing provenance ⇒ quarantine (`mod.rs:317-329`), unlabeled ⇒ High ⇒ never-light, errors
  in the depth/verdict computation can't occur (pure, total functions). **But the ADR-CS3 D2
  failure path — "endpoint-unreachable / credential-missing / **timeout** ⇒ a loud recorded
  SKIP that fails closed" — is unimplemented because no fallible (LLM/network/sandbox) pass
  exists yet.** When Pass 2 becomes a real weak-tier call (Wave C), this path must be built;
  today there is nothing to fail. The IC4 auto-gate does the right *partial* thing: a
  sigchain-record failure is swallowed (`let _ = store.record`, `identity_cmd.rs:1178`) **but
  the gate decision still stands** — i.e. an audit-write failure does not fail the gate open.
  Correct posture; note it means a verdict can be *enforced but unlogged* under disk failure
  (silent loss of the audit row, not of the block).
- **Review-fatigue DoS (RA-5).** Undefended (F9) — covered in §5.
- **Concurrency.** The verdict append is read-whole-file → push → `write_atomic`
  (`verdict.rs:162-172`). `write_atomic` makes each *write* atomic but the **read-modify-write
  is not serialized**: two agents recording concurrently (entirely possible — many workers,
  one `--dir`) can interleave so one record is lost and the `prev` hash-link is broken. For a
  tamper-evident audit chain this is a real integrity bug at production concurrency (F11).
- **Scale.** `load_chain` parses the entire JSONL on every `record`, `find_by_cid`,
  `digest_pin_consume`, and `revoke` (`verdict.rs:104-121`). O(n) per operation, O(n²) to
  build a chain of n. Fine for the spark, not for a long-lived gate (F11).

---

## 8. Dimension 5 — Test depth · observability · config safety

- **Test depth:** 26 lib unit tests, all green; each covers a real assertion (light path,
  hostile block, test-rewrite reject, taint override, unlabeled quarantine, spotlight nonce,
  reviewer-lure containment, quorum strictest-wins, hash-link, digest-pin reject, revoke
  lowers+names, monotonic lower). Plus two smoke scenarios (`content_safety_spark.sh`,
  `e2e_autowire_ingest_gate.sh`). **Good for the spark.** The gap is *adversarial breadth*:
  tests prove the *seed* attacks are caught, never that *paraphrased/encoded* variants are —
  there is no negative-corpus / evasion test (e.g. "disregard the prior directives" should
  also flag, but isn't asserted and in fact wouldn't). A production gate needs a maintained
  adversarial corpus, not 6 happy-path-of-the-attack cases.
- **Observability:** verdict sigchain + `wg review log` give a real audit surface; JSON
  output on every command. **Missing:** per-class FP/FN telemetry (ADR-CS3 OQ1), any metric
  for quarantine-queue depth / flood, and the depth integration into `wg show` / `wg config
  lint` (acknowledged Wave C, `depth.rs:27-29`).
- **Config safety:** there is **no config surface at all** — the depth matrix (`depth.rs`),
  the signature lists (`pass1_lint.rs`), and the reviewer scope (`pass2_review.rs`) are
  compile-time constants. **Upside:** nothing to mis-set, no insecure default to footgun.
  **Downside:** the security owner's "signed operating point" (ADR-CS1 OQ2, ADR-CS2 OQ3,
  ADR-CS3 OQ1) cannot be set without a recompile; no `HANDLER_FIRST`-style release flag; no
  lint. F16.

---

## 9. What to build for v1 (ranked, feeds `audit-synth`)

1. **F2 — wire IC1 (`run_import`) and IC2 (`run_accept`) to `review_inbound`** with derived
   trust. Highest value/effort ratio: the pipeline already exists; this connects two of the
   three dead nerves. (S1/E-M)
2. **F1/F7 — make Pass 2 a real weak-tier `.review-*` one-shot** behind the existing
   no-scope structural bound, with the timeout⇒fail-closed-SKIP path (F8) and a genuinely
   independent quorum. The structural slot is ready; drop in the silicon. (S1/E-L)
3. **F3 — build the Pass-4 human gate + quarantine-release surface** (`wg review release`/
   `approve`, a queue). Without it "quarantine" is a dead-letter and the residual-risk stance
   is half-assumed. (S1/E-XL — but a minimal release command is E-M and unblocks a lot.)
4. **F6 — fix the `weakens_assert` FP** (require a *removed/disabled* assertion, not the word
   `assert`). One-line-ish, prevents the gate from rejecting all honest test edits. (S2/E-S)
5. **F4/F5 — promote signatures + normalization to a maintained policy file** with an
   adversarial evasion corpus in CI. (S2/E-L)
6. **F11 — serialize verdict-chain appends** (lock/append-only-open) and index `find_by_cid`.
   (S3/E-M)
7. **F10/F13/F12/F17** — enforce the IC4 verdict (withhold bytes, not advisory); spotlight the
   render path; reconsider most-trusting vs strictest in trust resolution; route the remaining
   manual `--trust` callers through the canonical resolver.
8. **F14/F15** — TC8 cross-plane re-run (joint with WG-Exec) and opaque-class handling — the
   acknowledged multi-wave residuals.

---

## 10. Verdict for the synthesis task

**WG-Review is a correct, honest, well-tested *spark* and a *non-production* gate.** Its
containment/audit/revoke spine is production-shaped and genuinely strong (digest-pin,
sigchain, revoke, no-scope bound). Its detection layer is deterministic scaffolding by
design, and three of four ingest seams are unhooked. **The two ship-blockers are F1 (no real
reviewer) and F2 (gate not on the import/accept paths); the third (F3, no human gate) makes
the residual-risk claim half-assumed.** Nothing here is *wrong* — it is *unfinished exactly
where the ADRs say Review-Wave C/D finishes it*. The risk to manage is **organizational, not
technical: do not let the green smoke gate be read as "content safety is done."** It proves
the choice is buildable; it does not yet make a system safe to point at hostile inbound
content.
