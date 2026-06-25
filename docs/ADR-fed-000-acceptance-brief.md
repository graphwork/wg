# ADR-fed-000 (WG-Fed): Wave-2 Acceptance Brief — the four ADRs, packaged for ratification

**Date:** 2026-06-25 · **Owner task:** `adr-fed-coherence` · **For:** Erik (ratification)
**Package status:** all four ADRs are **Proposed**. This brief is the one-page index Erik
reads to ratify **Proposed → Accepted** in one pass. **This brief is not itself an ADR and
sets no Status** — ratification is flipping each ADR's `**Status:** Proposed` line to
`Accepted`.

> **What this is.** A coherence pass + acceptance package over the four Wave-2 ADRs
> (`docs/ADR-fed-001..004-*.md`), which together formalize the federation-study decision
> memo (`docs/federation-study/06-decision-memo-and-roadmap.md`) into the commitments
> WG-Fed implementation will cite. It is **not** a fifth design — every decision below was
> made in the memo (and, for authority scope, in Erik's leash amendment). Per memo §5,
> **no federation code lands until ADR-001/002/003 are Accepted.**

---

## Coherence verdict (the consistency + faithfulness pass)

- **Internally consistent.** The four ADRs compose without contradiction. The key-model
  (ADR-001), custody (ADR-003), transport (ADR-002), and loadable-state (ADR-004)
  decisions interlock: ADR-002/003/004 take ADR-001's `wgid:` address, sigchain link set
  (`genesis`/`add_key`/`revoke_key`/`rotate_root`/`delegate`/`set_alias_proof`/`set_endpoints`),
  three-tier keys, and freshness attestation as given and never re-define them.
- **Shared terms verified identical** across all four: `wgid:<multibase-ed25519-pubkey>`
  spelling; the sigchain link set; UCAN as the (attenuating-only) delegation mechanism;
  `TrustLevel` enum (`Verified`/`Provisional`/`Unknown`) and the `trust_level` field;
  `WG_FED_COMPAT_VERSION` (defined in ADR-001 D7, used by ADR-002 D4, referenced by
  ADR-004 D4 — no literal version pinned in any ADR, so no version drift); the freshness
  Δ tiers (routine ≈ 24 h / high-value ≤ 15 min / ±5 min skew, defined once in ADR-001
  OQ4 and reused by ADR-002/003/004).
- **Faithful to the decision memo**, with **one authorized departure**: ADR-003 §D2
  carries **Erik's trust-default / leash-as-a-dial amendment**, which *reverses* the
  memo's HQ1/HQ11 "short-lived UCAN per session by default." This is a deliberate,
  flagged amendment — not drift — and ADR-003 argues at length why it reopens **no**
  Fatal finding (the hydra S-4 is killed by attenuation + `add_key`-lock, which are
  dial-independent; custody is unchanged). **Confirmed reflected:** broad/long-lived by
  birth · custody ≠ authority (custody costs no autonomy) · humans never leashed.
- **Two small drifts fixed in-place** (noted, not escalated — no real conflict):
  1. **ADR-001 D6 table** described the agent's day-to-day signer as "(short-lived)" — a
     descriptor copied from the memo's *pre-amendment* HQ9 table, now inconsistent with
     ADR-003's broad/long-lived default. Reconciled by deferring the cell to "scope/expiry
     per ADR-003's authority dial" and adding a note that ADR-003 §D2 sets the default to
     broad/long-lived by birth.
  2. **ADR-003** twice cited the freshness-attestation mechanism as "ADR-001 D7" — but
     freshness lives in ADR-001 **OQ4** (D7 is the compat handshake; ADR-002/004 cite OQ4
     correctly). Citation corrected to OQ4.
- **No ADR marked Accepted.** All four remain **Proposed** — that gate is Erik's.

---

## The four ADRs — decision + what still needs Erik's sign-off

### ADR-001 — Identity & key model (`wgid` + sigchain + 3-tier keys)
**Decision (one line):** Identity is a self-certifying `wgid:<multibase-ed25519-pubkey>`
backed by an append-only signed **sigchain** over a **three-tier key hierarchy**
(root/signer/encryption); the address is the **genesis root pubkey, stable under
rotation**; **verification is never central**; `did:web`/DNS is rejected as a root.

**Settled without Erik** (mechanical, the ADR closed them): OQ1 `wgid` encoding (=`did:key`
ed25519 body, base58btc canonical) and OQ2 `did:key` interop (accept liberally, emit on
request, never a substitute for the sigchain).

**Needs Erik's sign-off (tuning defaults only — mechanism is committed):**
- **Freshness Δ values (OQ4):** routine ≈ **24 h**, high-value ≤ **15 min**, clock-skew
  tolerance **±5 min** — and whether the high-value Δ is **operator-configurable per
  deployment**. (Shared knob — see "Set once" below.)
- **Guardian M-of-N at genesis (OQ3):** the *slot* is fixed here; the default **M/N**,
  **post-genesis mutability**, and **enrollment UX** are delegated to ADR-003 OQ4 (same
  knob — sign off there).

### ADR-002 — Transport (node store-and-forward default, untrusted, fallback ladder)
**Decision (one line):** Bytes move over a **pluggable fallback ladder** — default rung is
the **WG node's HTTP store-and-forward inbox**, escalating to **Iroh QUIC direct** when
both peers are online, plus **optional shared relays**; **no single relay/node is
mandatory** (losing one degrades reach, not correctness); the **transport — including your
own node — is untrusted** (end-to-end signed + optionally sealed); the handshake is
**authenticated** (S-7); email-speed, both-ends-offline-tolerant.

**Settled without Erik:** OQ1 P2P-leg library is *deliberately deferred* to the Wave-4 gate
against six fixed criteria (binding the wire now is the mistake the memo §5 guardrail
warns against).

**Needs Erik's sign-off:**
- **Poll/push (OQ2 — tuning):** the **default poll cadence** (tens of seconds → minutes)
  and whether nodes **ship the push firehose in v1 or defer** it. (Mechanism — poll
  mandatory + offline-bearing, push optional + latency-only — is committed.)
- **Storage/economics (OQ3 — policy):** **default retention windows** (inbox grace ≈ 30
  days; unpinned-blob GC) and **confirmation that multi-tenant billing/quota/paid-pinning
  economics are out of scope for v1** (memo §7.12). Default is *no quota mechanism;
  self-host or pin.*
- **Optional product call (OQ1):** whether to **pull the Wave-4 wire-library gate
  forward** or **force node-HTTP-only forever** — a call Erik *may* make, but the default
  is the disciplined deferral.

### ADR-003 — Custody, delegation & recovery (THE crux) + trust-default/leash amendment
**Decision (one line):** The portable identity **excludes the root private key, always**;
the agent root is **custodian-held** (node operator with a node; human owner node-less)
behind an ssh-agent-style "sign this digest" boundary, and **download confers no oracle
access**; authority is **attenuating-only UCAN** with `add_key`/`rotate_root` locked to
**root/M-of-N** (kills the hydra); **download onto host B = fork by default**; **recovery
is layered** by deployment mode; and — **the amendment** — **custody ≠ authority: default
authority is broad and long-lived; the short/scoped "leash" is environment-driven policy,
not the birth default; humans are never leashed.**

**The headline ratification item:** accepting ADR-003 **ratifies Erik's
trust-default/leash amendment** (§D2). Confirm: broad/long-lived by birth · the leash is a
deployment-set dial · humans sovereign/never leashed · the integrity invariants
(attenuation + `add_key`-lock) hold at every dial setting, so the amendment reopens no
Fatal finding.

**Needs Erik's sign-off:**
- **UCAN expiry defaults (OQ1 — tuning):** the default **sanity-ceiling** for a broad/long
  capability (e.g. 30 / 90 days vs literally "until revoked"); the **high-value short-Δ**
  the corporate profile uses (≤ 15 min, shared with ADR-001 OQ4); and **which scopes count
  as "high-value"** by default (`rotate_root`, large-scope delegation, cross-trust state
  load).
- **Revocation hosting (OQ2 — policy):** whether to **also expose an optional,
  non-authoritative CRL-style endpoint**, and the **default staleness Δ** for revocation
  checks (shared with ADR-001 OQ4). Mechanism (self-verifying, cascade-hosted,
  freshness-piggy-backed, fail-closed) is committed.
- **Default agent custodian in a multi-tenant/org node (OQ3 — governance):** when the node
  is operated by someone other than the agent's human owner, is the default custodian the
  **org node-operator** or the **human owner**? ADR recommends **owner-sovereign by
  default, org-custodian as an explicit org-profile policy**; the exact org default — and
  **whether an org may deny an owner the override key** — is Erik's governance call. (Ties
  directly to the §D2 amendment.)
- **Guardian M-of-N UX (OQ4 — UX/policy, shared with ADR-001 OQ3):** the exact default
  **M/N** (proposed **2-of-3**), whether the **guardian set is mutable post-genesis** (and
  under what authority), and **enrollment/recovery UX polish**.

### ADR-004 — Loadable-state format + AI-input safety (loaded state = untrusted input)
**Decision (one line):** One stable `StateSnapshot` envelope with a tagged, evolvable
`payload_kind` slot (`conv-cache-v1`/`summary-v1`/`opaque-blob-v1`/future), **signed +
BLAKE3-CAS + incremental** (`prev`), a `model_binding` wrong-model guard, graceful unknown-
kind degradation; **FR-S1 becomes runtime-containment for opaque kinds** (S-1); and —
load-bearing — **loaded state is treated as UNTRUSTED INPUT** (a signature proves *who
wrote* it, never that it is *safe to load*), gated through a fail-closed load pipeline
(provenance + `model_binding` + AI-safety scan + `trust_level` gate + human-in-loop for
cross-trust loads, S-5).

**Settled without Erik:** OQ3 — opaque payloads are **always sealed** (forced by S-1 + S-5).

**Needs Erik's sign-off:**
- **The AI-input-safety scan ruleset (OQ1 — living policy):** the **categories** (four for
  transparent kinds; containment for opaque) and the **fail-closed/escalate-on-soft-hit
  posture** are committed, but the **prompt-injection signature set and the block-vs-
  escalate confidence thresholds** are a **maintained policy surface** (like an antivirus
  signature set). Erik (or a designated security owner) should **own its maintenance and
  set the initial thresholds**.
- **One UX cell (OQ2 — UX tuning only):** for a `Verified ∧ cross-self ∧ transparent ∧
  scan-clean` load, does it **auto-load with no human glance** or **always surface a
  one-line confirmation** ("loaded N turns from Nora [Verified]")? The matrix and the
  escalate-on-flag rule are committed; only this one happy-path cell's friction is Erik's.

---

## Shared knobs (set once, applied everywhere)

Two of the "needs sign-off" items are the **same** tunable surfacing in multiple ADRs —
set each once and it propagates:

- **High-value freshness Δ (≤ 15 min) + skew (±5 min).** Defined in ADR-001 OQ4; reused as
  the freshness gate by ADR-002 D5, ADR-003 OQ1/OQ2/D6, and ADR-004 pipeline step 3.
- **Guardian M-of-N default (proposed 2-of-3) + post-genesis mutability + UX.** Flagged in
  both ADR-001 OQ3 and ADR-003 OQ4; **ADR-003 owns it** (ADR-001 only owns the genesis
  *slot*). Sign off in ADR-003.

---

## How to ratify

1. Confirm the **trust-default/leash amendment** (ADR-003 §D2) — it is the one substantive
   departure from the memo and the highest-order decision in the package.
2. Set (or bless the proposed defaults for) the **tuning knobs** above; none require
   reopening any design — each ADR commits the *mechanism* and flags only the *value*.
3. Flip each ADR's `**Status:** Proposed` → `Accepted`. (Memo §5: ADR-001/002/003 must be
   Accepted before any federation code lands; ADR-004's S-5 safety layer is implemented in
   Wave 5 but is designed-in here.)

---

## References

- `docs/ADR-fed-001-identity-key-model.md` · `docs/ADR-fed-002-transport.md` ·
  `docs/ADR-fed-003-custody-delegation-recovery.md` ·
  `docs/ADR-fed-004-loadable-state-safety.md` — the four ADRs this brief packages.
- `docs/federation-study/06-decision-memo-and-roadmap.md` — the decision memo all four
  formalize (§1 the decision, §3 HQ1–HQ12, §5 waves + guardrails, §6 ADR stubs, §7
  non-goals, §8 open-question hand-off). ADR-003 §D2 amends its HQ1/HQ11 default.
- `docs/federation-study/01–05` — prior-art, baseline, requirements/hard-questions,
  candidate architectures, and adversarial evaluation underlying the memo.
