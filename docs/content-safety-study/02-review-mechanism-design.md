# Content-Safety Study 2/4 — The Inbound-Content Review Gate (Design)

> **The deliverable.** Doc 01 (`01-threat-and-prior-art.md`) framed the problem and
> gathered prior art: a federated WG consumes **inbound content from other
> identities** (tasks/prompts, code/artifacts, loadable state, messages — classes
> **IC1–IC4**), any of which can carry an adversarial payload, and **every residual
> in the threat matrix lives on the one axis WG does not defend today — the
> *semantics of the content itself*.** This document **designs the review gate that
> owns that axis**: where it sits, the review pipeline (one or several review
> tasks), how its depth is set by the existing trust dial, what it screens for, the
> residual it cannot close, and how it is modelled and surfaced in `wg`.
>
> Wave 1, task **2 of 4** (the *design* phase). Downstream consumers: **`safety-
> adversarial` (3/4)** — attacks this gate (inject-the-reviewer, trusted-actor-
> turned-bad) — and **`safety-decision` (4/4)** — the decision memo + content-safety
> spark + roadmap. This is a **design**, not a build; no review code lands here.
>
> **The one-sentence design.** The review gate is **not a new system** — it is the
> ADR-fed-004 **S-5 load pipeline** ("loaded state is untrusted input → gate it
> through a fixed, fail-closed pipeline whose depth is set by `trust_level`, with a
> human-in-loop for the uncertain band") **generalized from one content class (IC3
> loadable state) to all four**, and wired into the two ingest paths S-5 never
> touched: inbound **tasks/prompts/messages** (IC1/IC4) and the WG-Exec
> **`ResultEnvelope`** code/artifact accept path (IC2). It reuses `trust_level`, the
> provenance/attribution layer, the WG-Exec verification leash, and the S-5 human
> gate **verbatim**. It invents **no second trust vocabulary.**

---

## 0. Design principle — generalize S-5, do not parallel it

Doc 01's central finding (its §1, §4 law 4) is that WG already defends the **who /
authority / correctness / containment** axes — `trust_level`, provenance, the HQ2
verification leash, UCAN blast-radius, worktree isolation — and that **the only
uncovered axis is content-semantics.** ADR-fed-004 already built a content-semantics
gate **for one class**: its **D6 load pipeline** treats every loaded `StateSnapshot`
as untrusted input and runs it through CAS → signature → freshness → model-binding →
kind-dispatch → **AI-input-safety scan (OQ1)** → **provenance-gate by `trust_level` +
human-in-loop (OQ2)**, fail-closed at each step.

The design discipline of this document is therefore stated up front, because it
governs every choice below and is the explicit instruction of the task:

> **Generalize, never parallel.** The review gate is the **D6 pipeline lifted to a
> content-class-generic primitive** and hooked at the other three ingest points. Its
> depth dial is **the same `leash(trust_level, sensitivity)` engine** WG-Exec defines
> (HQ11), with one new output face. Its uncertain-band escalation is **the same S-5
> human gate**. Its audit trail is **the same sigchain**. Its containment is **the
> same worktree/UCAN bound**. **No new `TrustLevel`, no new reputation, no new human-
> gate, no new wire crypto, no new dispatch path** — those exist and are reused.

What is genuinely *new* is small and bounded: (a) the gate is hooked at **IC1/IC4
ingest** and the **IC2 `ResultEnvelope` accept path**, not only the IC3 load path;
(b) the OQ1 scan grows **per-class check sets** for instruction-text and code/artifact
content (it currently specifies only state kinds); (c) the three-valued verdict
**accept / quarantine / reject** is made the gate's uniform output across all classes
and surfaced in `wg`. Everything else is reuse.

This is the structural mirror of how WG-Exec composed onto WG-Fed ("a *consumer*, not
a peer; reads `trust_level`, defines no identity; reuses the UCAN, invents no second
delegation system") — the review gate is a **consumer of WG-Fed + WG-Exec**, reading
their dials and hooking their seams.

---

## 1. Where the gate sits — a *consumption* gate at four ingest seams

The trust boundary is crossed the moment an agent consumes something authored by
another `wgid:` identity (doc 01 §1). The gate sits **on the consumption edge of each
of the four ingest seams** — content may be *received, stored, signed, and attributed*
freely (that is the existing `who` layer's job), but it is **held un-consumed until
review grants consumption.** This is the dual-LLM quarantine law (doc 01 PA-6, law 3)
made structural: *received ≠ consumed.*

| Inbound class | Ingest seam (where content crosses the boundary) | Existing hook the gate reuses | Gate runs *before* |
|---|---|---|---|
| **IC1 task/prompt/instruction** | WG-Fed graph import / WG-Exec **placement** (`PlacementOffer`→`Claim`); a `wg msg` carrying directives | placement path (`src/providers/placement.rs`); message ingest (ADR-fed-002) | …the task becomes `ready` / its text enters a consuming agent's context |
| **IC2 code / artifact (incl. remote result)** | WG-Exec **`ResultEnvelope`** accept path; pulled deps/files a task references | `src/providers/verify.rs` (the HQ2 leash) + artifact store | …merge-back **and** before any downstream `--after` consumer reads the artifact (TC8) |
| **IC3 loadable state** | WG-Fed **`StateSnapshot`** load | **ADR-fed-004 D6** pipeline (`src/identity/state_safety.rs`) | …the payload decodes into the agent's working state |
| **IC4 message** | WG-Fed message/inbox transport | ADR-fed-002 message delivery | …the message reaches an agent's context **or** seeds a task (the A-4.4 inject→IC1 path) |

**The placement of each hook, concretely:**

1. **IC1 — inbound task/prompt.** A federated task pulled into the local graph, or
   placed by a remote authorizer, is **not marked `ready`** until its review task
   resolves. The review runs on the **task description + any embedded directives**
   *before* a worker reads it as working context. This is the seam WG-Exec already
   guards on the *authority* axis (placement is capability-gated, `trust_level`-
   floored); the gate adds the *content* axis at the same seam. Inbound text is
   **spotlighted/delimited** (PA-6) on the way in, so even an accepted task is
   presented to the consuming agent as *data-with-provenance*, not as instructions.

2. **IC2 — remote result / artifact.** The WG-Exec `ResultEnvelope` accept path
   (`verify.rs`) already runs the **HQ2 verification leash** ("is this result
   *correct* against the pinned spec, re-run in a trusted domain — X-5/X-6"). The
   gate runs **on the same accept path, as a sibling check on a different axis**: the
   leash asks *"is it correct,"* the gate asks *"is it malicious-but-correct"* — the
   exact TC8 hole doc 01 flags as "most under-defended." Both must pass before
   merge-back, and — critically — **before any honest downstream `--after` consumer
   reads the artifact**, so a check-passing semantic poison cannot launder into the
   graph (TC8/X-6).

3. **IC3 — loadable state.** **The gate *is* the ADR-fed-004 D6 pipeline**, already
   designed and located in `src/identity/state_safety.rs`. Steps 1–3 (CAS / signature
   / freshness) are the provenance precondition (§2 Pass 0); step 6 (the OQ1 scan) is
   Pass 1; step 7 (provenance-gate + human-in-loop) is Passes 3–4. This document does
   **not** re-design IC3 — it **adopts D6 as the canonical template** the other three
   classes are generalized from, and notes the one extension D6 already anticipated:
   the OQ1 scan's "exact heuristic ruleset … is a living policy surface," which is
   where the cross-class Pass-1/Pass-2 check sets (§4) plug in.

4. **IC4 — message.** Messages are gated at inbox delivery (ADR-fed-002 transport,
   which already proves *who sent it*). Because A-4.2/A-4.4 make a message an
   **injection vector into IC1**, a message that *contains directives* or *seeds a
   task* is routed through the **same IC1 pipeline** — the message gate and the
   task gate are one pipeline with two entry points, not two systems.

**Why a consumption gate and not an admission gate.** Admission ("refuse the bytes at
the door") throws away the provenance and audit substrate WG paid for — a rejected
artifact you never stored cannot be traced to its author later, and an honest-but-
unusual task wrongly rejected is simply lost. A *consumption* gate keeps the bytes
(signed, CAS-addressed, attributed on the sigchain), **quarantines** them by default
when uncertain, and makes every accept/reject an auditable, reversible event (PA-9,
doc 01 law 8). This is the same posture as S-5: *verify and store, then gate the
decode.*

---

## 2. The review pipeline — one fail-closed pipeline of cheap→expensive passes

The pipeline generalizes the D6 7-step pipeline and orders passes **cheap
deterministic first, expensive judgment last** (doc 01 laws 1–2; PA-5/PA-7/PA-8). It
is **fail-closed at every pass** (a pass that errors, times out, or cannot run
escalates one level stricter — never silently accepts), and **monotonic in suspicion**
(any soft hit moves the verdict one level stricter — the OQ2 escalate-on-flag rule
generalized). The verdict is always one of three values (§2.5).

```
                    inbound content (IC1 | IC2 | IC3 | IC4)
                                   │
   ┌───────────────────────────────┼───────────────────────────────┐
   │ PASS 0 — provenance precondition  (REUSED, not new)            │
   │   CAS integrity → signature/attribution → freshness            │   tamper/forgery
   │   (WG-Fed FR-V1 / ADR-fed-004 D6 steps 1–3 / WG-Exec FR-V1)    │ ── reject ──►
   │   establishes WHO. Necessary, NEVER sufficient (the S-5 point) │
   └───────────────────────────────┬───────────────────────────────┘
                                   │ clean → continue
   ┌───────────────────────────────┼───────────────────────────────┐
   │ PASS 1 — fast deterministic lint/classifier  (near-zero cost)  │   hard hit
   │   per-class scanners (§4): injection-signature lint, secret    │ ── reject ──►
   │   scan, typosquat/dep diff, test-file-change flag (X-6),       │   soft hit
   │   OQ1 structural/secret/lineage checks                         │ ── escalate ─┐
   └───────────────────────────────┬───────────────────────────────┘              │
                  clean & light-leash → ACCEPT                                     │
                                   │ leash says "go deeper" ───────────────────────┤
   ┌───────────────────────────────┼───────────────────────────────┐              │
   │ PASS 2 — quarantined LLM-review pass  (weak-tier one-shot)     │◄─────────────┘
   │   the dual-LLM judge: classifies semantic INTENT               │   real-threat
   │   (injection / goal-hijack / social-eng / malicious-but-correct│ ── reject ──►
   │   code / exfil). NO privileged tools. Input spotlighted.       │   uncertain
   └───────────────────────────────┬───────────────────────────────┘ ── escalate ─┐
                  clean → ACCEPT                                                    │
                                   │ IC2 unknown-code / IC3 opaque ────────────────┤
   ┌───────────────────────────────┼───────────────────────────────┐              │
   │ PASS 3 — sandboxed detonation  (expensive, IC2/opaque-IC3 only)│   bad behavior
   │   run in worktree/UCAN-bounded, egress-restricted sandbox;     │ ── reject ──►
   │   observe net/fs/exfil (PA-8). REUSES WG-Exec isolation ladder │   uncertain
   └───────────────────────────────┬───────────────────────────────┘ ── escalate ─┐
                  clean → ACCEPT                                                    │
                                   │ high-impact / uncertain band ─────────────────┤
   ┌───────────────────────────────┼───────────────────────────────┐              │
   │ PASS 4 — human-in-loop escalation  (REUSED S-5 human gate)     │◄─────────────┘
   │   the ADR-fed-004 OQ2 cross-trust human decision, applied to   │
   │   the inbox/accept path. Human: accept / quarantine / reject.  │
   └───────────────────────────────┬───────────────────────────────┘
                                   ▼
                   ACCEPT  ·  QUARANTINE  ·  REJECT   (recorded: §2.5)
```

### 2.0 Pass 0 — provenance precondition (reused, the `who` layer)

Pass 0 is **not part of the new gate** — it is the existing `who` layer (WG-Fed
provenance/attribution FR-V1, WG-Exec result-attribution, D6 steps 1–3) run as a
precondition: recompute the CAS digest, verify the signature against the author's
sigchain, check freshness on cross-trust/high-value loads (fail-closed on stale, ADR-
fed-001 OQ4). It answers **who authored this and is it unmodified**. Doc 01's whole
thesis (and S-5's) is that **passing Pass 0 is necessary but never sufficient** — a
perfectly-signed task from a `Verified` identity can still carry a prompt injection.
Pass 0 gates out tampering and forgery; Passes 1–4 are the content-semantics axis.

### 2.1 Pass 1 — fast deterministic lint / classifier (the cheap 80%)

The cheapest, fastest layer — deterministic, no LLM — run on **every** inbound item
regardless of trust (doc 01 law 2; the smoke-gate-before-`wg done` idiom). It is the
generalization of the **OQ1 scan** to all four classes; the per-class check sets are
§4. A **hard hit** (committed private key, known-malware signature, a kind-tag that
disagrees with the bytes) → **reject**; a **soft hit** (an injection-shaped phrase, a
test-file change, a typosquat-distance dep) → **escalate** to Pass 2; **clean** →
accept *iff the leash for this source is light* (§3), else continue. This pass catches
the cheap, known, and deterministically-detectable attacks at near-zero cost and is
the layer that makes the gate affordable to run on every item.

### 2.2 Pass 2 — the quarantined LLM-review pass (the content-semantics core)

The deeper pass is a **weak-tier agency one-shot** — the same `.evaluate-*`/`.flip-*`
machinery (`resolve_agency_dispatch`, the cheap `tiers.fast` model), now a `.review-*`
role — that reads the (spotlighted) content and **classifies its semantic intent**:
prompt-injection, goal-hijack, social-engineering/unusual-request, malicious-but-
correct code, exfiltration/escalation intent. It returns `{verdict, reason,
confidence}`. This is the **content-semantics axis doc 01 says WG lacks** — the
judgment layer that asks *"does this instruction try to hijack my goal"* / *"does this
diff contain a backdoor that still passes the tests,"* which Pass 0's attribution and
the HQ2 correctness leash structurally cannot answer.

**Two hard constraints make Pass 2 safe to run on hostile input** (and pre-empt study
3's "inject-the-reviewer" attack — the full adversarial analysis is handed there):

- **Dual-LLM discipline (PA-6).** The reviewer is **not** the consuming agent and runs
  with **no privileged tools, no graph-write, no network** — it holds only the
  *act-as-reviewer* scope. Its only output is a constrained verdict enum + reason
  string. So even if the content injects *the reviewer*, the blast radius is "the
  reviewer emits a wrong verdict," not "the reviewer takes an action" — and a wrong
  *accept* is still caught downstream by Pass 0/3 attribution, sandbox containment,
  and the audit trail.
- **Spotlighting (PA-6).** The content is delimited/encoded as untrusted data in the
  reviewer's prompt, so the reviewer is structurally biased to *classify* it rather
  than *obey* it.

Pass 2's confidence drives the verdict: high-confidence-threat → **reject**; clean →
**accept** (subject to the leash); **uncertain** → **escalate** (to Pass 3 for code/
opaque, else Pass 4 human). This is content-moderation tiering (PA-7) mapped onto WG's
weak tier + S-5 human gate, exactly as doc 01 law 6 prescribes.

### 2.3 Pass 3 — sandboxed detonation (IC2 / opaque-IC3, the expensive last resort)

For **unknown code/artifacts** (IC2) and **opaque loadable state** (IC3 `opaque-blob-
v1`, un-scannable by construction — A-3.3), static inspection cannot decide intent, so
the gate **detonates the artifact in a sandbox and watches behaviour** (PA-8) — network
egress, filesystem writes, exfil attempts. This pass **reuses the WG-Exec isolation
ladder (HQ8) and worktree/UCAN blast-radius bound (FR-V4) verbatim** as the sandbox:
egress-restricted/allow-listed, scope-bounded, both-directions-isolated. It is the
expensive tier, reserved for the uncertain band that Passes 1–2 could not clear, and
it doubles as **containment** — even a payload it fails to detect runs *inside* the
bound, so the blast radius is the sandbox, not the host. For opaque IC3, this is D5's
"we cannot read it, so we contain it" posture.

### 2.4 Pass 4 — human-in-loop escalation (the reused S-5 gate)

The uncertain/high-impact tail escalates to a **human decision — the *same* ADR-fed-
004 OQ2 cross-trust human-in-loop gate**, now also reachable from the IC1/IC4 inbox and
the IC2 accept path, not only the IC3 load path. The human renders the same three-
valued verdict. This is doc 01 law 6 ("tiered human-in-loop, mapped to the S-5 gate")
— WG already has the escalation primitive; the gate **routes the uncertain band
through it rather than inventing a new one.** Automation (Passes 1–3) handles volume;
the human handles the ambiguous and the high-impact, and the human's verdict feeds
reputation (lowering a defector's `trust_level`, doc 01 law 8).

### 2.5 The three verdicts — uniform across all classes, recorded on the sigchain

Every pass emits one of three values; the pipeline's verdict is the strictest any pass
reached (monotonic):

| Verdict | Meaning | Effect | Reversible? |
|---|---|---|---|
| **accept** | consumption permitted | content enters the consuming agent's context / merges / loads | n/a |
| **quarantine** | held, **not consumed**, pending escalation/more-context/human | content stays stored + attributed but un-consumed; surfaced in `wg` for a human | **yes** — re-review can release or reject |
| **reject** | refused | content not consumed; author `trust_level` may be lowered; if already propagated, downstream `--after` consumers are re-run (TC8) | logged, traceable |

**Quarantine is the default for the unknown/unlabeled** (the fail-closed posture, §3)
— it is *not* a third "maybe" the human must always resolve immediately; it is a safe
holding state that bounds risk to zero (un-consumed) while preserving the content for
audit and possible later release. This is the App-Store/content-moderation review-
queue shape (PA-3/PA-7) and the S-5 "pause for a human decision" behaviour.

**Every verdict is recorded** as `{verdict, reason, content_class, deciding_pass,
confidence, provenance(author wgid + sigchain position)}` — appended to the **same
sigchain** WG already keeps (PA-9, doc 01 law 8). This is the audit/revoke leg: a
later-discovered poison is traced to its author, the author's `trust_level` is lowered,
and TC8 downstream consumers are found and re-run. **No SKIP/uncertain verdict is ever
silently dropped** — it is surfaced loudly (the smoke-gate discipline, doc 01 law 7).

---

## 3. Trust-proportional review — the same leash, a new output face

This is the **"one coherent dial, not a new one"** requirement, and it is the heart of
"reuse, don't parallel." Review depth is **not a new threshold** — it is an additional
output of the **existing `leash()` policy engine** (WG-Exec HQ11):

```
leash(provider_trust, task_sensitivity, pool_class, env_config) →
    { delegation, context, isolation, verification, lease,
      review{ depth, default_verdict } }          ← the one new output face
```

The `review.depth` dial selects how far down the §2 pipeline an item must go before it
can be accepted, and `review.default_verdict` is the fail-closed floor for the
unlabeled case. **The inputs are the ones WG already has** — the author's
`trust_level` (`Verified | Provisional | Unknown`, `src/graph.rs:1920`) and the
content's sensitivity/class — so there is **no new trust vocabulary**.

**The review-depth matrix** (the OQ2 trust matrix, generalized from IC3 to all classes;
the rows are the author's trust relative to the consumer, because the threat is *whose*
content you consume):

| Author `trust_level` × content | Review depth | Default verdict on clean |
|---|---|---|
| **Verified**, low-sensitivity, transparent | **Pass 1 only** (light lint) | **accept** |
| **Verified**, high-sensitivity **or** code/opaque | **Pass 1 + 2** (LLM review); +Pass 3 for unknown code/opaque | accept on clean; **human (Pass 4) on soft hit** |
| **Provisional** (TOFU default for federated peers) | **Pass 1 + 2**, **human-in-loop on any flag** | accept on clean; **human** on flag |
| **Unknown** | **Pass 1 + 2 + 3**, **quarantine-by-default** | **quarantine**; human (Pass 4) to release |
| **Unlabeled sensitivity** (the fail-closed cell) | **deep** (treated as Unknown/high) | **quarantine**, never light — the WG-Exec D-i rule |
| **same-self** (IC3 resume of *my own* continuous self) | **Pass 1 scan only**, no human gate | accept on clean (the S-5 happy path — preserve the resume UX) |

Two rules make the dial coherent with its two siblings:

- **Fail-closed on unlabeled (WG-Exec D-i).** An item whose sensitivity is unlabeled or
  whose author is `Unknown` gets **deep review and a quarantine default** — never the
  light path. This is *literally* the leash engine's "unlabeled ⇒ refuse/C, never A"
  rule, applied to the review axis: the gate **cannot** emit "light review" for
  `author_trust < floor`.
- **Monotonic escalate-on-flag (S-5 OQ2).** Any soft hit at any pass moves the verdict
  one level stricter (accept → human, human → quarantine/reject). The dial only ever
  tightens under suspicion.

**The three faces of one dial — stated explicitly so study 4 can verify coherence.**
`trust_level` now drives three trust-proportional gates that are **one engine with
three output faces**, not three engines:

| Gate | Question it answers | Output face | Where |
|---|---|---|---|
| **WG-Exec HQ2 verification leash** | is this result *correct* vs the pinned spec? | `verification{depth}` | `verify.rs` |
| **ADR-fed-004 S-5 provenance-gate** | is it safe to *load* this state? | the OQ2 load decision | `state_safety.rs` |
| **This review gate** | is it safe to *consume* this content's semantics? | `review{depth, default_verdict}` | the gate (§6) |

Same `trust_level` input, same fail-closed-on-unlabeled default, same monotonic
escalate-on-flag, same S-5 human-in-loop for the uncertain band, same `wg`-surfaced
output, same sigchain audit. A `Verified` peer's normal task gets a lint and goes; an
`Unknown` author's code gets lint + LLM-review + sandbox + quarantine-by-default. **One
dial, three faces — the leash doc 01 law 5 names ("the same dial, a new axis").**

---

## 4. What the gate screens for — per content class

The per-class check sets that populate Pass 1 (deterministic) and Pass 2 (LLM-review).
These are the **OQ1 scan categories generalized** beyond state kinds; ADR-fed-004
already flagged the "exact heuristic ruleset … is a living, maintained policy surface"
(like an antivirus signature set) — these are that surface's seed, tunable without
reopening the design.

| Class | Screened for (attacks from doc 01 §2) | Pass 1 deterministic | Pass 2 LLM-review | Pass 3 sandbox |
|---|---|---|---|---|
| **IC1 task/prompt** | injection (A-1.1), jailbreak (A-1.2), goal-hijack (A-1.3), social-eng/unusual-request (A-1.4), indirect/2nd-order (A-1.5) | injection-signature lint ("ignore previous…", role-confusion, tool-invocation strings in *data* positions); **spotlight/delimit always** | semantic intent: does this re-point the consumer's goal? is the ask one the principal would refuse? | — |
| **IC2 code/artifact** | malicious code (A-2.1), poisoned/typosquat dep (A-2.2), test-poison (A-2.3/X-6), check-passing semantic poison (A-2.4/TC8), build/exec payload (A-2.5) | secret scan (gitleaks-class), known-bad/SAST-lite patterns, **typosquat/dep-manifest diff**, **test-file-change flag (X-6)**, install-script/suspicious-call scan | review the diff for a backdoor that *still passes the tests* (the TC8 hole) | **detonate** unknown code: watch net/fs/exfil (A-2.5 build-time payload) |
| **IC3 loadable state** | stored injection (A-3.1), poisoned summary (A-3.2), opaque smuggling (A-3.3), model-binding mismatch (A-3.4) | **the OQ1 four categories verbatim**: structural/type-confusion, embedded-secret/key scan (FR-S1), prompt-injection heuristics, provenance/lineage consistency | the OQ1 category-3 escalation (lower-confidence injection shapes) | **contain** opaque kinds (un-scannable — D5): sandbox-only load |
| **IC4 message** | phishing (A-4.1), social-eng of agent (A-4.2), social-eng of human (A-4.3), inject→task (A-4.4) | sender-auth (reused, ADR-fed-002) + injection-signature lint on directive-bearing messages | manipulation intent; **route directive/task-seeding messages through the IC1 pipeline** (A-4.2/A-4.4) | — |

The cross-cutting screens — **spotlight/delimit all instruction-bearing text (IC1/IC4)**
so it is consumed as data-with-provenance, and **never let unscreened content drive a
privileged tool** — are the dual-LLM law (PA-6, doc 01 law 3) and apply to every class
that carries instructions.

---

## 5. The inherent-risk boundary — what is screened vs accepted as residual

Doc 01's governing stance (its §0, §5) is **the npm reality: review reduces, never
eliminates.** Stated plainly and operationally, so this design does not over-promise:

**What the gate *screens* (reduces — raises attacker cost, raises catch probability):**

- **Known/cheap attacks** — known injection signatures, known-malware packages,
  committed secrets, typosquatted deps, test-file rewrites (X-6), kind-tag/structure
  mismatches, and anything deterministically detectable (Pass 1, near-zero cost).
- **Plausible-but-detectable semantic attacks** — goal-hijack, social-engineering, and
  malicious-but-correct code that an LLM-review pass can recognize (Pass 2).
- **Behaviour-revealing payloads** — exfil/escalation that only shows up at runtime, by
  detonating unknown code/opaque state in the bound sandbox (Pass 3).

**What is *accepted as residual* (NOT eliminated — the irreducible tail, doc 01 §0):**

- **Novel injection / jailbreak** that evades both the signature lint and the LLM
  reviewer — guaranteed false-negatives for unseen attacks (doc 01 §0 reason 2).
- **Check-passing semantic poison (TC8 / A-2.4)** — a backdoor that passes the pinned
  tests *and* the sandbox *and* reads clean to the reviewer. Doc 01 names this "most
  under-defended, residual real." The gate **reduces** its probability; it cannot close
  it.
- **Opaque-blob smuggling (A-3.3)** — un-scannable by construction; **contained, never
  screened** (sealed, sandbox-only, mandatory trust gate — D5). The residual is
  inherent and disclosed.
- **The compromised/`Verified`-but-defecting author (P6)** — a trusted actor who turns
  bad sends clean-looking content the light path waves through. The gate's structural
  answer is random spot-check re-review on the fungible middle + audit-after-the-fact;
  the **full adversarial treatment is `safety-adversarial` (3/4)'s "trusted-actor-
  turned-bad" case** — handed off, not closed here.
- **Social-engineering of the *human* (A-4.3)** that the human approves at Pass 4 — the
  gate routed it to a human; if the human is fooled, the gate did its job and the
  residual is human, not mechanical.

**How the tail is caught (since it is real):** the gate's last two layers are exactly
doc 01 law 8 — **containment** (every consumption is worktree/UCAN blast-radius-bounded,
so a *missed* payload is contained, FR-V4) and **audit + revoke** (every accept is
logged to the sigchain; a later-discovered poison is traced to its author, the author's
`trust_level` lowered, and **TC8 downstream `--after` consumers found and re-run**).
**Quarantine + human escalation + provenance/after-the-fact audit catch what the passes
miss** — they do not prevent the miss, they bound and reverse it.

**The non-goal, stated so studies 3–4 can hold this design to it:** *the gate does not
certify inbound content as safe.* A gate that claimed to would be a liability — it
would license agents to consume inbound content without the skepticism that is the real
last line of defense (doc 01 §0 reason 4). Success is **a smaller, well-understood,
contained, auditable residual at an acceptable false-positive cost** — *mitigate, don't
eliminate.* The false-positive/false-negative cut-offs are a **tuned, surfaced policy**
(doc 01 law 7), flagged for the security owner like the OQ1 ruleset, not a silent
constant.

---

## 6. WG surface — how a review task is modelled, hooked, and shown

### 6.1 A review task is a WG task that gates consumption

The task's phrase "a review task, maybe several" is **literal**: each pass is a **WG
task on a dependency edge in front of the consuming task**, so the gate is built from
the same primitive everything else is — a node in the live graph, dispatched by the
coordinator, visible in `wg show`.

- The consuming task is `--after` its review task(s): the consumer **cannot become
  `ready`** until review resolves `accept`. A `reject`/`quarantine` verdict holds the
  consumer (and surfaces it for a human), exactly as a failed dependency does today.
- **Pass 1 (deterministic lint)** is a cheap inline check — a deterministic scanner on
  the ingest path, not even an LLM (the cheapest tier, like the smoke gate).
- **Pass 2 (LLM-review)** is a **weak-tier agency one-shot** — a new `.review-<target>`
  role resolved by `resolve_agency_dispatch` off the active profile's **weak tier**
  (`tiers.fast`), exactly like `.evaluate-*`/`.flip-*` today. It is cheap, recoverable,
  and runs with **no privileged scope** (§2.2). Cost stays on the weak tier, per the
  agency-dispatch design.
- **Pass 3 (sandbox detonation)** is a **worktree-isolated task** (`isolation:
  'worktree'` analog), reusing the WG-Exec isolation ladder.
- **Pass 4 (human-in-loop)** is the **existing S-5 human gate** — no new mechanism.

A pipeline is therefore "one review task or several" by composition:
`ingest → .review-lint(T) → .review-llm(T) → [.review-sandbox(T)] → [human] → T`,
a `--after` chain whose depth is set by the §3 leash (a `Verified` low-sensitivity
source collapses to just `.review-lint`; an `Unknown` author expands to the full
chain). This is the standard WG pipeline pattern, not a bespoke control flow.

### 6.2 Where it hooks (concrete seams, all reused)

| Class | Hook (existing seam) | Module |
|---|---|---|
| IC1 task/prompt | placement / graph-import; gate before `ready` | `src/providers/placement.rs` |
| IC2 code/artifact | the **`ResultEnvelope` accept path**, alongside the HQ2 leash, before merge-back + before `--after` consumers | `src/providers/verify.rs` |
| IC3 loadable state | the **ADR-fed-004 D6 load pipeline** (steps 6–7) | `src/identity/state_safety.rs` |
| IC4 message | inbox delivery; directive/task-seeding messages → the IC1 pipeline | ADR-fed-002 transport |

The gate's own logic lives in a small new module — `src/review/` (the content-axis
analog of `src/providers/` for the execution axis and `src/identity/` for the identity
axis), or equivalently the generalization of `state_safety.rs` — holding the per-class
Pass-1 check sets, the Pass-2 reviewer prompt/scope, the leash `review{}` output face,
and the verdict recorder. It is a **living policy surface** (like an AV signature set),
not a write-once check (ADR-fed-004 Consequences).

### 6.3 What `wg` shows

The verdict is surfaced exactly as the applied leash is surfaced today (WG-Exec HQ11:
"the applied leash is surfaced in `wg show <task>` / `wg providers`, mirroring the
handler-first `wg status` rendering"):

- **`wg show <task>`** renders the **review verdict + reason + content-class +
  deciding-pass + confidence + provenance** (author `wgid` + sigchain position) — so a
  quarantined/rejected consumer shows *why* and *who*, at a glance, the way `wg show`
  already renders attribution and token usage.
- **A quarantine queue** (`wg review` / a `wg list` filter on quarantined items)
  surfaces the held band awaiting human escalation — the App-Store/moderation review
  queue (PA-3/PA-7), loud, never silent (doc 01 law 7).
- **The applied review depth** rides the same leash-surfacing + **`wg config lint`**
  leash-lint that already exists, so a mis-configured (too-loose) review policy is
  visible the way a too-loose leash is.
- A `WG_REVIEW_COMPAT_VERSION`-style constant is **not** introduced — the gate reuses
  WG-Fed/WG-Exec wire envelopes and adds only the verdict record, so it rides their
  existing compat handshakes (NFR-4, no forked crypto/wire).

---

## 7. Compose contract — what this design reuses, and what is new

Stated explicitly (the WG-Exec "compose contract" convention) so studies 3–4 and any
implementer can confirm no parallel trust system was invented:

| Primitive | Source | This gate's use |
|---|---|---|
| `TrustLevel` (`Verified`/`Provisional`/`Unknown`) | `src/graph.rs:1920` (WG-Fed) | **read** as the review-depth input — no new enum |
| Provenance / attribution (FR-V1, sigchain) | WG-Fed / WG-Exec | Pass 0 precondition; verdict record |
| **S-5 D6 load pipeline + OQ1 scan + OQ2 matrix** | ADR-fed-004 (`state_safety.rs`) | **the canonical template** the gate generalizes (IC3 is unchanged) |
| HQ2 verification leash (`verify.rs`) | WG-Exec | the sibling check on the IC2 accept path (correctness ∥ content) |
| `leash()` policy engine (HQ11) | WG-Exec | **+1 output face** `review{depth, default_verdict}` — no new dial |
| S-5 human-in-loop gate (OQ2) | ADR-fed-004 | Pass 4 — reused verbatim |
| Worktree / UCAN blast-radius (HQ8 / FR-V4) | WG-Exec | Pass 3 sandbox + containment of the miss |
| Weak-tier agency one-shot (`resolve_agency_dispatch`) | WG (`src/service/llm.rs`) | Pass 2 `.review-*` role on `tiers.fast` |
| Sigchain audit + `trust_level` lowering | WG-Fed / WG (PA-9) | verdict log + audit/revoke the miss (TC8 re-run) |
| `wg show` / `wg config lint` surfacing | WG | verdict + reason + applied-depth rendering |

**Genuinely new (and bounded):** the IC1/IC4-ingest and IC2-accept **hooks** (D6 only
covered IC3); the **per-class Pass-1/Pass-2 check sets** (§4); the uniform **accept /
quarantine / reject** verdict surfaced across all classes; the `.review-*` agency role;
the `review{}` leash output face. Everything else is reuse.

---

## 8. Hand-off to studies 3 and 4

- **`safety-adversarial` (3/4)** should attack this gate directly. Two named surfaces
  this design deliberately leaves for it: **(a) inject-the-reviewer** — Pass 2 consumes
  hostile content; §2.2's dual-LLM/no-privileged-scope/spotlight constraints are the
  *intended* defense, and 3/4 should test whether they hold (can a payload make the
  reviewer emit a false `accept`, and is the downstream containment enough when it
  does?). **(b) trusted-actor-turned-bad (P6)** — the light path for a `Verified`
  author is the soft underbelly; §5 names the structural answer (spot-check + audit +
  trust-lowering) but 3/4 owns the adversarial evaluation.
- **`safety-decision` (4/4)** should verify the **one-dial coherence** (§3's three-faces
  table) holds, define the **content-safety spark** (the minimal end-to-end proof — by
  analogy to the WG-Fed and Exec sparks: *one inbound poisoned task, one review
  pipeline, a verdict that quarantines it and an audit record that traces it*), and set
  the roadmap wave (it sequences after the ADR-fed-004 Wave-5 safety layer, which it
  generalizes).

---

### Provenance of this document

- **Builds directly on** `docs/content-safety-study/01-threat-and-prior-art.md` — its
  threat matrix (IC1–IC4, A-*), prior art (PA-1…PA-10), the eight design laws (§4), and
  the mitigate-don't-eliminate stance (§0, §5).
- **Generalizes** `docs/ADR-fed-004-loadable-state-safety.md` — the D6 load pipeline,
  the OQ1 per-kind scan categories, and the OQ2 `trust_level` + human-in-loop matrix —
  from IC3 (loadable state) to all four inbound-content classes.
- **Reuses, does not fork** `docs/execution-federation-study/06-decision-memo-and-
  roadmap.md` — HQ2 (the verification leash + `ResultEnvelope` accept path, X-5/X-6,
  TC8), HQ8 (isolation ladder), HQ11 (the `leash()` engine + fail-closed-on-unlabeled +
  surfaced/linted), FR-V4 (UCAN blast-radius).
- **Existing primitives referenced:** `TrustLevel` (`src/graph.rs:1920`); `Agent.
  trust_level` (`src/agency/types.rs:521`); the weak-tier agency one-shot
  (`resolve_agency_dispatch`, `Config::weak_tier_spec()`); the S-5 state-safety module
  (`src/identity/state_safety.rs`); the WG-Exec provider modules (`src/providers/
  placement.rs`, `verify.rs`).
- **Downstream:** `safety-adversarial` (3/4) attacks the gate (§8); `safety-decision`
  (4/4) writes the decision memo + content-safety spark + roadmap (§8).
