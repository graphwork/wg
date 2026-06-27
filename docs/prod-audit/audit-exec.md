# Production-Readiness Audit — WG-Exec (execution-federation plane)

**Task:** `audit-exec` · **Scope:** `src/providers/{mod,placement,lease,bundle,verify}.rs`,
the `wg provider` CLI (`src/commands/exec_fed_cmd.rs`), the dispatch touch-points
(`ExecutorKind::RemoteRunner` + `Placement` in `src/dispatch/plan.rs`, `spawn_task.rs`,
`service/llm.rs`), the canonical trust resolver (`src/trust.rs`), and the smoke proof
(`tests/smoke/scenarios/exec_spark_borrowed_box.sh`).
**Verified against:** `docs/ADR-exec-e1..e4-*.md`, `docs/ADR-exec-000-acceptance-brief.md`,
`docs/execution-federation-study/05-adversarial-evaluation.md`,
`docs/execution-federation-study/06-decision-memo-and-roadmap.md`.
**Stance:** skeptic. Analysis only — **no code changed** except this document.
**Date:** 2026-06-27.

---

## 0. TL;DR verdict

WG-Exec is a **clean, honest Exec-Wave B spark** whose *cryptographic substrate is real and
production-grade* (it reuses WG-Fed identity / UCAN / sealed-envelope verbatim — no second
trust system) and whose *security invariants are individually correct and well-tested at the
library + smoke level*. The five envelopes, the fail-closed leash, the epoch fence, the
scoped-UCAN issuance, and the disjoint-re-run engine all exist and do what the ADRs say.

But it is a **PoC reachable only through a 10-verb manual CLI**, not an integrated execution
path, and three structural gaps make it **not production-ready as-is**:

1. **The epoch CAS is atomic in memory but the ledger is persisted with an unlocked,
   non-atomic `fs::write`** → the X-4 file-level TOCTOU the design claims to close is
   re-opened under any concurrency, and a corrupt/half-written ledger silently **fails open**.
2. **`accept` never runs the integrity re-run** — `verify_result` is a *separate manual
   command*, so the leash's `verification_depth` is never enforced at the canonical write
   boundary. A corrupted result is committed unless a human remembers to run `wg provider
   verify` first.
3. **The single worst threat in doc 05 (TC8 cross-task poison) is essentially undefended** —
   only provenance + trust-lowering exist; the design's structural answer (tier-by-graph-
   position + descendant re-run) is unbuilt, and a code comment overstates that descendants
   are surfaced.

Plus the three named big deferrals are confirmed and sized below: **attestation slot = empty
(confidential work can only be *refused*, never *run*)**, **B verified-overflow tier =
unbuilt**, **quorum = unbuilt (correctly deferred)**.

**Real today:** the library invariants + the manual six-step `wg provider` flow.
**Scaffold:** remote execution itself (the worker is a deterministic stub), liveness/lease
enforcement, accounting, and the entire integration into the live dispatcher.

---

## 1. Findings table

Severity: **Crit** (blocks safe prod) · **High** · **Med** · **Low** / **Nit**.
Effort: **S** (<1d) · **M** (days) · **L** (1–2wk) · **XL** (research / multi-wave).
Class: **prod** (production-grade) · **demo** (works but stub/deterministic) · **missing**.

| # | Capability | Class | Decision ref | Sev | Effort | File:line |
|---|-----------|-------|--------------|-----|--------|-----------|
| F1 | **Epoch CAS — durability/concurrency**: `try_commit` is a correct single-method compare-and-set *in memory*, but the CLI persists the ledger via `load → mutate → fs::write` with **no flock and no atomic temp+rename**. Concurrent `accept`/`reclaim`/`offer` processes re-introduce the read-modify-write TOCTOU X-4 says must not exist. The "one in-process check-and-set" guarantee holds only for a single serialized writer; the spark is sequential so it never exercises this. | **demo** (sound for 1 writer; unsafe for N) | ADR-E3 D6 / X-4 (doc 05 §X-4, doc 06 line 652) | **High** | M | `lease.rs:188`, `exec_fed_cmd.rs:678-682,74-82` |
| F2 | **Corrupt ledger fails OPEN**: `load_ledger` swallows any parse error to `unwrap_or_default()` → a corrupt/half-written `leases.json` silently becomes an **empty** ledger; every task reverts to `NoPlacement` and a re-offer restarts the epoch at 1 (`committed=false`). Combined with non-atomic `fs::write`, a crash mid-write **resets the fence** (fail-open for replay/double-commit). (Registry corruption fails *safe* — trusts nobody — so only the ledger is dangerous.) | **demo** | ADR-E3 D6 (fence is the integrity backstop) | **High** | M | `exec_fed_cmd.rs:67-72,381-386` |
| F3 | **`accept` does not gate on the integrity re-run**: `run_accept` does attribution + graph-write-scope + epoch-CAS, then **commits** — it never calls `verify_result`. The leash computes `verification_depth = ReRunInTrustedDomain` for low-trust but it is **never consulted at the write boundary**. The re-run is the decoupled, manual `wg provider verify`. A corrupted result from a Provisional/Verified provider that clears attribution+scope+epoch is accepted with **no integrity check**. | **missing** (engine exists, not wired into accept) | ADR-E4 D2/D3, placement.rs `VerificationDepth` | **High** | M | `exec_fed_cmd.rs:645-710` vs `:795`; `placement.rs:219-226` |
| F4 | **Cross-task poison (TC8 / D-iii) — the worst-ranked threat — largely undefended**: only provenance (`ResultEnvelope.producer`) + `lower_trust` on a caught defection exist. The design's structural defense — **tier-by-graph-position** (foundational⇒A/C, leaf⇒B) + **re-verify-inputs-across-trust-boundaries** + **surface poisoned descendants to re-run** — is **not built**. `leash()`/`evaluate_placement` take **no graph-topology input**. The verify-path comment "surface the descendants to re-run" is **not implemented** (no descendant enumeration; only trust is lowered). | **missing** | doc 05 §4 / D-iii (lines 461,518,673); ADR-E4 D4/D6 | **High** | L | `exec_fed_cmd.rs:798-803`; `placement.rs:266-325` (no topology input); `mod.rs:534` |
| F5 | **Attestation slot is empty — confidential work can only be *refused***: `CapabilityAd.attested` is hardcoded `false` in every claim, and the design's measurement allow-list is empty. So confidentiality today = **fail-closed refuse**, never **attest-and-run**. `IsolationClass::Tee` exists but nothing verifies a quote, nothing seals-to-quote. **The single biggest prod gap for any confidential remote workload.** (For a v1 that does *not* run confidential work remotely, the refuse is a *safe* gap.) | **missing** (slot present, payload absent) | ADR-E2 D2/D5 (C-or-refuse); doc 06 Wave D (lines 879-889) | **High** (Crit if confidential remote exec is in v1 scope) | XL | `exec_fed_cmd.rs:302`; `mod.rs:271-281`; `placement.rs:158-169` |
| F6 | **B verified-overflow tier unbuilt**: only the A (trusted-pool) tier + the fail-closed refuse exist. The `verify_result` re-run/eval-gate machinery is scaffolded but (a) not gated into accept (F3) and (b) there is no vouched/attested *overflow pool* distinct from the trusted pool, and no `auto_evaluate` eval-gate integration (the pinned-spec oracle is a deterministic substring stub). So "B" today is "A + a decoupled manual verify", not a tier. | **missing** | doc 06 Wave C (lines 864-877); ADR-E4 | **Med** | L | `verify.rs:210-222` (stub oracle); `placement.rs:227` (pool class informational) |
| F7 | **Quorum unbuilt** — the low-trust lever is the single disjoint re-run. Code + comments are honest that quorum waits on unsolved sybil-resistance. **Correctly deferred**; not a near-term prod gap *provided* the single re-run is actually wired (it isn't — see F3). | **missing** (intentional) | ADR-E4 D7; doc 06 (lines 1035-1036) | **Low** | XL+research | `verify.rs:20-21` |
| F8 | **Plane is CLI-only, not wired into the dispatcher**: `Placement::Provider` is **never constructed** (`plan_spawn` always sets `Placement::Local`); `ExecutorKind::RemoteRunner` **errors** in the spawn-task handler and **degrades to claude-haiku** in agency dispatch. So the coordinator never places a task remotely — remote exec is reachable *only* through the manual 10-verb `wg provider` CLI. | **missing** (type seams present; no driver) | ADR-E1 D6 (placement field); NFR-3 | **Med** (High to actually use it) | L–XL | `plan.rs:557,293`; `spawn_task.rs:293-301`; `service/llm.rs:361-366` |
| F9 | **Liveness/lease enforcement is data-model-only**: `LeaseRenewal` is a defined-but-**unused** wire type (built at `mod.rs:692`, **never constructed/sent**); `verify_renewal_sig` is **uncalled + untested**; there is **no `wg provider renew`, no heartbeat loop, no auto-reclaim-on-timeout**. `record_renewal` runs only as a side-effect of `accept`. So lease `term_secs`/`renew_cadence_secs`/`is_live()` are inert metadata; reclaim is fully manual. | **demo** (model present, no runtime) | ADR-E3 D5 (signed-renewal liveness) | **Med** | L | `mod.rs:497-508,692`; `lease.rs:236`; CLI has no `renew` verb (`main.rs:4018-4136`) |
| F10 | **`wg provider run` is a deterministic stub, not a worker**: it ignores the opened slice and emits a hardcoded `LEGIT_DIFF`/`CORRUPT_DIFF` constant; it never invokes a model handler. Adversary-simulation flags `--corrupt`/`--target-task`/`--scope-probe` ship in the **production** CLI. (Not a privilege hole — a hostile provider can always emit a hostile diff — but it confirms `run` is a test harness.) | **demo** | doc 06 §4.3 (deterministic-stub spark boundary) | **Med** | L | `exec_fed_cmd.rs:581-587,950-976`; `cli.rs` `ProviderCommands::Run` |
| F11 | **Accounting disconnected (pi-handler-class under-count)**: `Usage` on the result is a hardcoded stub (1200/340/$0.012); `accept` emits it to stdout JSON but **never records it into `task.token_usage` / `wg spend` / `wg stats`**. Remote-exec spend is invisible to every accounting surface — the exact failure class the pi-handler had (a non-canonical usage schema that never reaches accounting). | **missing** (bridge) | FR-V3 (non-bare usage) | **Med** | M | `exec_fed_cmd.rs:600-604,689-708` |
| F12 | **Grant drops sensitivity — gate not bound in the chain**: `run_grant` recomputes the leash with **`sensitivity: Sensitivity::Normal` hardcoded** ("a granted task cleared the offer's sensitivity gate"). The `Claim` envelope carries **no** sensitivity field and grant reads the *claim*, not the *signed offer*. So the fail-closed confidential/unlabeled gate is **structurally absent at grant**; it relies entirely on the offer phase having refused first. Belt-without-braces (X-1 ordering). | **demo** (composes in the happy CLI flow only) | ADR-E2 D-i / X-1 (floor before context) | **Med** | S | `exec_fed_cmd.rs:368`; `mod.rs:440-452` (Claim has no sensitivity) |
| F13 | **Observability gap**: a remote task emits no `stream.jsonl`/events and is invisible to `wg show` / the TUI events pane; only `wg provider show`/`providers` surface it (and they recompute the leash for *display* only). Ops cannot watch a remote task the way a local one is watched. | **missing** | ADR-E3 Consequences (surface applied leash) | **Med** | M | `exec_fed_cmd.rs:851-937` |
| F14 | **Reclaim default provider is a placeholder string** `"wgid:reassigned"` — a real reassignment must pass `--new-provider`; the default leaves a non-resolvable wgid on the lease. Minor ops foot-gun. | **demo** | ADR-E3 D6 | **Low** | S | `exec_fed_cmd.rs:742` |
| F15 | **Compat handshake checked pre-authentication**: `check_exec_compat` runs on raw envelope bytes *before* `verify_sig` in claim/grant/run/accept/verify. The ADR calls the handshake "authenticated"; in practice the version is read before the signature, so a forged incompatible version triggers the loud refuse (DoS-grade only — it would refuse anyway, no security impact). | **prod** (nit) | ADR-E1 D6 (authenticated handshake) | **Nit** | S | `exec_fed_cmd.rs:283,288,350,545,653,771` |
| F16 | **`field_scan` private-key detection is a substring heuristic**: scans the serialized grant for `ed25519:`/`x25519:`/`"seed"`/`"private"`. Correct today (it caught the real custody tags), but it is a string scan over JSON, not a structural type guarantee — a future field storing key bytes under a different tag would slip past it. Belt-and-braces raw-hex scan in the smoke partially compensates. | **prod** (with caveat) | ADR-E3 D1 (no-root assertion) | **Low** | S | `mod.rs:597-636` |

---

## 2. ADR-exec security properties — checked **in code**

Each of the five properties the task names, verified against the implementation (not the prose).

### 2.1 Two scoped UCANs never carry the root key — ✅ **HOLDS (prod)**
- `run_grant` issues both capabilities with `custody::issue_root(&cust, &signer, …)` using the
  **delegated signer kid**, never the root; the custody boundary (`Custodian::sign_digest`)
  never returns private key material (`exec_fed_cmd.rs:401-422`).
- Scopes are correctly attenuated: `act-as-agent` bound to `agent://<G>/task/<T>` and
  `graph/write` bound to `graph://task/<T>` only — never `graph://*` (`:393-400`).
- `RunGrant::field_scan` asserts no key material + no blanket write (`mod.rs:597-636`); the
  blanket-write probe is a genuine two-resource `permits()` test (`:614-621`).
- Smoke step 1 + a raw private-key-hex scan over the grant bytes confirm empirically
  (`exec_spark_borrowed_box.sh:127-148`).
- **Residual:** F16 (substring heuristic). Mechanism is otherwise production-grade because it
  rides the real WG-Fed UCAN.

### 2.2 Lease-epoch atomic CAS at the canonical-write boundary (X-4) — ⚠️ **PARTIAL**
- *In memory*: `try_commit` is a correct single-method compare-and-set — rejects stale (`<`),
  future-epoch (`>`, "authorizer is the only minter"), and already-committed
  (`lease.rs:188-209`). This matches ADR-E3 D6 / X-4 **for a single in-process writer**.
- *On disk*: **the guarantee does not survive the persistence layer.** Each CLI verb reloads
  the ledger, mutates, and `fs::write`s it back with no lock and no atomic rename
  (`exec_fed_cmd.rs:678-682`). Two concurrent writers race (**F1**); a corrupt file fails open
  (**F2**). The design's own words — "exactly one canonical writer … one in-process
  check-and-set" — are an *assumption the spark satisfies by being sequential*, not an
  enforced property. Production needs either a single in-process ledger actor (the daemon) or
  an OS advisory lock + atomic write.

### 2.3 Fail-closed leash (unlabeled ⇒ refuse/C never A; confidential ⇒ attested-C or refuse, D-i) — ✅ **HOLDS (prod), with a chain gap**
- `leash()` refuses unlabeled (`unlabeled-fails-closed`) and confidential-to-unattested
  (`no-eligible-confidential-provider`) **before any context/seal decision** (X-1 ordering),
  and `evaluate_placement` re-asserts the leash first even when capability is missing
  (`placement.rs:151-169,275-278`).
- `run_offer` writes **no offer file** on refusal and the smoke asserts the file's absence
  (`exec_fed_cmd.rs:201-217`; `exec_spark_borrowed_box.sh:320`). Strong, well-tested (unit +
  smoke steps 6).
- **Gap (F12):** the gate is enforced *only at the offer phase*. `run_grant` re-runs the leash
  with sensitivity **hardcoded to Normal** and reads the claim (which carries no sensitivity),
  so grant **cannot independently refuse** a confidential/unlabeled placement. The gates
  compose only because the CLI always runs offer first against the same registry. Sensitivity
  should be carried (signed) into the claim/offer-ref and re-derived at grant.

### 2.4 Trusted-domain re-run vs a pinned spec, never the producer/its tests (X-5/X-6) — ✅ **engine HOLDS (prod-shaped)** / ❌ **not gated into accept**
- `verify_result` refuses `verifier == producer` (X-5, `verify.rs:267-273`); `split_test_changes`
  separates test-file hunks so a provider-authored test can't be its own oracle (X-6,
  `:178-205`); `rerun_against_pinned_spec` checks the authorizer's pinned markers, never the
  shipped tests (`:210-222`); non-checkable ⇒ escalate (`:296-299`). All correct.
- **But (F3):** this engine is invoked only by the manual `wg provider verify`. `wg provider
  accept` — the canonical write boundary — never calls it. So X-5/X-6 protect a result *only
  if a human runs verify*; the live accept path commits without them. The pinned-spec oracle
  is also a deterministic substring stub (real pinned-suite + `auto_evaluate` is Wave C).

### 2.5 Cross-task poison placement constraint (D-iii / TC8) — ❌ **LARGELY MISSING (F4)**
- doc 05 ranks TC8 the **"most under-defended" / "worst"** threat. The design's structural
  answer is tier-by-graph-position + provenance + re-verify-inputs-across-trust-boundaries.
- In code: provenance is recorded and a caught defection lowers trust (the audit/revoke leg).
  **Absent:** any graph-topology input to placement (a foundational task is *not* routed to a
  higher-trust tier than a leaf); any descendant enumeration (the `:798` comment "surface the
  descendants to re-run" is aspirational — the code only calls `lower_trust`); any input
  re-verification across a trust boundary. For the single threat the adversarial study calls
  most dangerous, the structural defense is unbuilt.

---

## 3. Failure modes

| Mode | Behavior today | Assessment |
|------|----------------|-----------|
| **Remote orphan / reclaim across hosts** | `reclaim` bumps the epoch; the fence makes a resurrected worker's late write stale (`lease.rs:165-175`). Correct **as a data model**. | But **nothing detects** an orphan (no heartbeat-timeout loop, F9); reclaim is fully manual, and its default new-provider is a placeholder (F14). Prefer-liveness is sound *only if the fence holds across processes* — see F1. |
| **Partition** | Documented prefer-liveness: reclaiming a live-but-partitioned worker costs ≤1 wasted re-run, never corruption — *because* the fence rejects the partitioned worker's late write. | Sound **conditional on F1/F2**. Under concurrent/ crash-corrupt persistence the fence can be defeated, so the "never a corrupt graph" claim is not yet unconditional. |
| **Hostile / buggy provider** | Over-scope write (different task) → `graph-write-scope-violation`; post-expiry → `attribution-failed`; replay → `replay-already-committed`; stale-after-reclaim → `stale-epoch`; corrupted diff → caught by `verify` (when run). | Envelope-level coverage is **strong and end-to-end tested** (smoke steps 4–5). The hole is F3 (accept doesn't auto-verify) + F4 (cross-task propagation). |
| **Malformed envelopes** | `read_json` bubbles a parse error and the verb refuses loudly. A malformed *result/offer/claim/grant* is rejected. | OK for envelopes. The dangerous silent-default is on the **ledger/registry** state files, not envelopes (F2). |

---

## 4. Test depth

**Inventory:** 27 unit tests across `src/providers/` + one six-step smoke
(`exec_spark_borrowed_box.sh`, owner `exec-spark`) + reuse in `e2e_family_team.sh`.

**Genuinely good:** the §4 adversarial assertions are exercised **end-to-end with real keys**,
not stubbed — over-scope (4i), post-expiry (4ii), replay + stale-after-reclaim (4iii),
corrupted-result + test-poison + same-provider-refusal (5), fail-closed-confidential +
unlabeled (6). The library unit tests cover the leash fail-closed gates, the trust-floor /
capability filter, rank determinism, the fence (stale/replay/reclaim), the sealed-bundle
ACL + minimization, and the X-5 refusal. For a spark this is above-bar.

**Gaps (what the green checkmark does *not* prove):**
- **F1 untested** — no concurrency/race test on `leases.json`; the spark is sequential so the
  file-level TOCTOU is never exercised.
- **F3 invisible** — there is no test asserting that `accept` *without* `verify` commits a
  corrupted result, so the decoupling reads as safe when it isn't. The smoke only catches the
  corrupt diff because step 5 *explicitly* calls `verify` on a known-bad result.
- **F2 untested** — no corrupt/half-written ledger durability test.
- **`verify_attribution` success path + its reason codes** (`principal-mismatch`,
  `aud-mismatch`, `not-act-as-agent`, `producer-unresolved`) are not unit-tested — only the
  X-5 guard has a unit test (`verify.rs:349-369`); the OK path is exercised only via the smoke.
- **F9 untested + unwired** — `verify_renewal_sig` / `LeaseRenewal` have no caller and no test.
- **F12 untested** — the grant-hardcodes-Normal sensitivity loss has no regression test.

---

## 5. Observability / accounting / ops / config safety

- **Accounting (F11):** the headline ops gap and an exact repeat of the pi-handler under-count
  class — usage is a stub and never bridged into `task.token_usage`/`wg spend`/`wg stats`.
  Remote spend is unaccounted.
- **Observability (F13):** no events/stream for a remote task; invisible to `wg show`/TUI.
- **Config safety:** **good direction.** Trust is authorizer-asserted and fail-safe-`Unknown`
  for strangers (`mod.rs:361-365`); a corrupt registry → empty → trusts nobody (fail-safe);
  the env leash (`LeashPolicy::from_env`) can only *tighten* TTL (`placement.rs:187,214-217`);
  the canonical `src/trust.rs` resolver folds the pool + peer registries into one dial. **The
  one fail-*open* state file is the ledger (F2)** — the asymmetry (registry fails safe, ledger
  fails open) is the thing to fix.
- **Compat (F15):** handshake works but is checked pre-authentication (nit).

---

## 6. Real today vs scaffold

| Layer | Real today (prod-grade) | Scaffold / stub |
|-------|------------------------|-----------------|
| **Crypto substrate** | ✅ WG-Fed identity / UCAN / per-recipient seal reused verbatim; scoped-UCAN issuance; sealed-bundle ACL + minimization; envelope sign/verify | — |
| **Placement & leash** | ✅ hard filter + fail-closed leash (unlabeled/confidential refuse); deterministic rank | Pool class is informational (`placement.rs:227`); no graph-topology input (F4) |
| **Lease / fence** | ✅ in-memory atomic CAS logic (stale/future/replay) | ❌ unlocked non-atomic persistence (F1/F2); ❌ no heartbeat/renew runtime (F9) |
| **Verification** | ✅ attribution; ✅ disjoint-re-run engine (X-5/X-6 logic) | ❌ not gated into accept (F3); deterministic substring oracle, not a pinned suite + eval-gate (F6); ❌ cross-task/descendant defense (F4) |
| **Confidentiality** | ✅ fail-closed *refuse* for confidential-to-unattested | ❌ no enclave / no attestation verify / no seal-to-quote (F5) — cannot *run* confidential work, only refuse it |
| **Worker / execution** | — | ❌ `wg provider run` emits a constant diff, never runs a model (F10); stub usage (F11) |
| **Integration** | ✅ type seams (`ExecutorKind::RemoteRunner`, `Placement::Provider`) defined | ❌ never constructed/driven; CLI-only PoC (F8); no observability (F13) |
| **Tests** | ✅ 27 unit + 6-step adversarial smoke | ❌ no concurrency / durability / accept-without-verify / renewal coverage (§4) |

---

## 7. Recommended v1 punch-list (for `audit-synth`)

Ranked by "blocks safe production" first.

1. **(F1+F2, High, M)** Make the ledger write **crash-safe and serialized**: atomic temp+rename
   + an OS advisory lock (or route all writes through one in-process daemon actor), and make
   `load_ledger` **refuse on parse error** instead of `unwrap_or_default()`. Add a concurrency
   regression test. *Without this the fence — the entire integrity backstop — is not sound off
   the happy path.*
2. **(F3, High, M)** Gate `wg provider accept` on the leash's `verification_depth`: when it is
   `ReRunInTrustedDomain`/`Escalate`, run `verify_result` (on a disjoint verifier) **before**
   committing, and refuse to accept a low-trust result that hasn't been re-run. Add a test that
   accept-without-verify rejects a corrupted result.
3. **(F4, High, L)** Build the cross-task-poison defense doc 05 flags as worst: feed graph
   position into placement (foundational⇒higher tier), actually **enumerate and re-queue
   poisoned descendants** on a caught defection (make the `:798` comment true), and re-verify
   inputs across trust boundaries.
4. **(F5, High→Crit-if-in-scope, XL)** Decide explicitly whether v1 ships confidential remote
   exec. If yes, this is the long pole (real enclave/attestation behind the slot — Wave D). If
   no, **document that confidential remote work is unsupported (refused), not "secured."**
5. **(F11, Med, M)** Bridge accepted-result usage into the canonical token-accounting pipeline
   (mirror the pi-stream-bridge fix) so remote spend shows in `wg spend`/`wg show`.
6. **(F9, Med, L)** Wire liveness: a `renew` verb + heartbeat loop + auto-reclaim-on-timeout, or
   delete the unused `LeaseRenewal` type and document accept-implies-liveness as the only model.
7. **(F12, Med, S)** Bind (signed) sensitivity into the claim/offer-ref and re-derive it at
   grant so the fail-closed gate is enforced at *both* offer and grant.
8. **(F6/F7, Med/Low, L/XL)** B-tier and quorum: correctly deferred — keep them deferred, but
   the synth roadmap should state the *prerequisite* (F3 wired + sybil-resistance for quorum).
9. **(F8, Med, L–XL)** Integration: only attempt once F1–F3 land — driving `Placement::Provider`
   from the coordinator before the fence/verify are sound would expose the gaps at scale.
10. **(F13/F14/F15/F16, Low/Nit, S)** Observability for remote tasks; require `--new-provider`
    on reclaim; verify-after-authenticate ordering; harden `field_scan` toward a structural check.
