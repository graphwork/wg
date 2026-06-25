# Content-Safety Study 1/4 — Inbound-Content Threat Surface & Prior-Art

> **The question.** In a federated WG, an agent does not only consume content it
> authored or that its own principal authored. It consumes **inbound content from
> other identities** — tasks, prompts, instructions, code/artifacts (including
> results computed on a remote provider), loadable identity state, and messages.
> Any of these can carry an **adversarial payload**: prompt-injection / jailbreak
> guidance, social-engineering ("unusual requests we don't want to consume"),
> malicious code, poisoned dependencies, and supply-chain-style attacks — *"the
> issue with node/npm."* We need a **review mechanism** (a review task, maybe
> several) that **screens inbound content before an agent consumes it**.
>
> Wave 1, task **1 of 4** (the *frame + gather* phase). This document is a
> **threat survey + prior-art comparison**, not a design. Downstream consumers:
> **`safety-design` (2/4)** — designs the inbound-content review gate (the review
> task(s)) — and the `.flip-safety-threat` evaluator.
>
> **Substrate.** This study sits on top of the WG-Fed identity/capability decision
> (`docs/federation-study/06-decision-memo-and-roadmap.md`) and the WG-Exec
> execution-federation decision
> (`docs/execution-federation-study/06-decision-memo-and-roadmap.md`). The fixed
> points it reuses: every actor is a self-certifying `wgid:<pubkey>`; agents act
> under **short-lived, scoped, attenuating UCANs**, never a root key;
> **`trust_level`** (`TrustLevel` enum, `graph.rs:1920` — `Verified | Provisional
> | Unknown`) is the
> **single dial** gating placement / leash / context / verification; **a signature
> proves *who*, never *safe-to-think*** (the S-5 finding, ADR-fed-004); and the
> execution plane defends result integrity with **attribution + a
> trust-proportional verification leash** (WG-Exec HQ2). This study asks the one
> question those decisions *named but did not build*: **how do we screen the
> *content itself* — its semantics, not its signer — before an agent consumes
> it?** WG has **no AI-input-safety / content-screening layer today**
> (federation-study doc 02 §2.4; ADR-fed-004 S-5 "requires an AI-input-safety
> layer WG does not have today").

---

## 0. The thesis up front — inherent risk, mitigate don't eliminate (the npm stance)

**Stating the conclusion before the evidence, because it governs every choice
downstream:** inbound content from another identity **cannot be made safe**. A
federated WG that consumes other identities' tasks, code, state, and messages
**accepts an irreducible residual risk**, exactly as every developer who runs
`npm install` accepts that a transitive dependency *might* be malicious. The job
of the review gate (task 2/4) is **not to eliminate the risk** — that is
impossible — but to **raise the attacker's cost, raise the probability of catch,
bound the blast radius, and make consumption auditable and revocable after the
fact.** Success is a *better filter with a smaller, well-understood residual*, not
a proof of safety.

**Why the residual is irreducible — four reasons it cannot be designed away:**

1. **Signature ≠ safety (the S-5 generalization).** Every WG-Fed/WG-Exec defense
   that exists today answers *who* (attribution, `trust_level`, provenance) or
   *how much damage* (UCAN scope, blast-radius bound). **None answers *is the
   content adversarial*.** A perfectly-signed task from a `Verified` identity can
   still carry a prompt injection; a validly-attributed result can still be
   backdoored code. ADR-fed-004 fixed this for *loadable state* (S-5: "a signature
   proves *who wrote* state, never that it is *safe to load*"); this study
   generalizes the same gap to **all four inbound content classes**.

2. **Semantic undecidability.** "Is this instruction a jailbreak?" / "does this
   diff contain a backdoor?" / "is this a social-engineering request?" are
   *judgment* problems with no decision procedure. A classifier can score them; it
   cannot certify them. False negatives are guaranteed for novel attacks, false
   positives for unusual-but-benign content.

3. **The supply-chain analogy is exact.** npm/PyPI **cannot** prove a published
   package is benign — they scan, sign, attest provenance, run reputation, and
   *still* ship Shai-Hulud-style worms, `event-stream`, `ua-parser-js`,
   typosquats, and protestware. The ecosystem's posture is **layered mitigation
   with a known residual**, not elimination. WG inherits both the structure of the
   problem (untrusted code/artifacts crossing a trust boundary at scale) *and* the
   only honest stance available.

4. **The adversary adapts.** Any published gate is a target. Spotlighting,
   classifiers, and deny-lists are all evadable by an attacker who knows them
   (this is why prompt-injection remains an open problem in the literature). A gate
   that *claimed* to eliminate risk would be a liability — it would license
   agents to consume inbound content *without* the skepticism that is the real
   last line of defense.

**Operationally, "mitigate, don't eliminate" means the gate commits to four
measurable things, not a guarantee:**

| Lever | What it buys | WG primitive it reuses |
|---|---|---|
| **Raise attacker cost** | known-bad / cheap-to-detect attacks are caught at the door; novel attacks cost real effort | scan-first layering (CI/SAST/secret/malware-signature analogs) |
| **Raise catch probability** | layered, diverse detectors; reputation accrues on caught defectors | content-moderation tiering + WoT reputation (`trust_level`) |
| **Bound blast radius** | even a *missed* payload is contained | UCAN task-scoping (WG-Exec FR-V4), worktree isolation, sandboxed eval |
| **Audit + revoke after the fact** | consumption is logged, attributable, reversible | signed provenance + sigchain + WG's existing `wg show` / revocation |

The explicit **non-goal**, stated so task 2/4 does not over-promise: *the gate does
not certify inbound content as safe.* It downgrades trust, screens, contains, and
surfaces — and a residual always remains.

---

## 1. The trust boundary — where inbound content enters the WG-Fed / WG-Exec pipeline

Today WG consumes only content inside **one trust domain**: tasks the local
principal authored, code in the local repo, state the local agent wrote. Every
inbound-content threat is created by federation **crossing a trust boundary** —
the moment an agent consumes something authored by *another* `wgid:` identity. The
four entry points, mapped to the existing pipeline:

| Entry point (where it crosses the boundary) | Inbound content class | Pipeline location |
|---|---|---|
| **Pull / receive a federated task or instruction** from another identity's graph; a remote authorizer places work; a `wg msg` carries directives | **IC1 task/prompt/instruction** | WG-Fed import / WG-Exec placement (`Claim` / push); message queue |
| **Accept a remote-computed result or artifact** (`ResultEnvelope` diff/files); pull code/deps a federated task references | **IC2 code/artifact** | WG-Exec result-acceptance path (`verify.rs`, eval-gate); artifact store |
| **Load a portable `StateSnapshot`** to "resume a continuous self" authored elsewhere | **IC3 loadable state** | WG-Fed loadable-state load path (ADR-fed-004) |
| **Receive a message** (human↔agent, agent↔agent) across the federation | **IC4 message** | WG-Fed message/inbox transport (ADR-fed-002) |

**The defenses that already guard these entry points — and the one they all
share-blind.** WG-Fed/WG-Exec already put real machinery on this boundary. The
study's central finding is *what kind* of machinery it is:

| Existing defense | What it actually checks | Axis |
|---|---|---|
| **`trust_level` single dial** (FR-R2, FR-T3; `graph.rs:1920`) | *who* — gates placement/leash/context/verification by the author's trust | **who / authority** |
| **Provenance / attribution** (FR-V1: sig → identity; sigchain) | *who* — an unsigned/wrong-signed input is rejected; result attributes to agent G | **who** |
| **S-5 provenance-gate** (ADR-fed-004 D-step 7) | *who + when* — auto-load vs human-in-loop vs refuse, by author `trust_level` + cross-trust | **who / human-gate** |
| **Exec verification leash** (WG-Exec HQ2) | *correctness* — attribution + trust-proportional **re-run in a trusted domain** / **eval-gate** scoring against `## Validation` | **correctness of a result** |
| **Test-poisoning gate** (WG-Exec X-6) | *spec integrity* — re-run against the authorizer's **pinned** test; flag test-file changes for review | **correctness oracle** |
| **UCAN blast-radius bound** (FR-V4) | *damage* — worst-case forged result is bounded to its own task's scope, revocable/auditable | **authority / blast radius** |
| **Sandbox / worktree isolation** (today: per-agent worktree) | *containment* — an agent's writes are isolated to its worktree until merge-back | **containment** |

**The gap, stated sharply:** every existing defense is on the **who / authority /
correctness / containment** axes. **Not one is on the *content-semantics* axis** —
"is this inbound *content* adversarial in intent?" The verification leash (HQ2) is
the closest neighbor, but it checks *"is this result correct against the spec,"*
**not** *"does this inbound instruction try to hijack my goal"* or *"does this diff
contain a backdoor that still passes the tests"* (precisely the TC8 / X-6 hole:
a check-passing semantic poison). The provenance-gate (S-5) is the *only* place WG
has even *named* content-screening ("sandbox/scan") — and ADR-fed-004 explicitly
records that the scan layer **does not exist yet**. **This study's gate fills that
axis.**

---

## 2. The inbound-content threat matrix

Each content class below lists: the **attacks** it carries across the boundary,
**which existing defense partly covers it** (and on which axis), and the
**residual gap** the new gate must close. Stable IDs (`IC*` / `A-*`) so task 2/4
and the FLIP can reference findings.

### IC1 — Tasks / prompts / instructions from another identity

A federated task description, a placed instruction, or a directive embedded in a
message is **read by an agent as part of its working context** — i.e. it can act
as *instructions*, not merely data.

| ID | Attack | Mechanism |
|---|---|---|
| **A-1.1** | **Prompt injection** | inbound text contains "ignore previous instructions / your real task is…" that re-points the consuming agent |
| **A-1.2** | **Jailbreak / guardrail-bypass guidance** | instructions craft the agent into a persona or chain-of-thought that evades its own safety posture |
| **A-1.3** | **Goal-hijack** | the task *looks* like normal work but its real objective is the attacker's (exfiltrate a secret, escalate scope, mutate another task) |
| **A-1.4** | **Social-engineering "unusual request"** | a plausibly-framed ask the principal would not want consumed ("publish this key to debug," "disable the smoke gate," "approve this merge") — the human/agent phishing case applied to *work* |
| **A-1.5** | **Indirect / second-order injection** | payload hides in a *referenced* artifact (a linked file, a dep README, a tool output) that the agent fetches while doing the task |

- **Partly covered by:** `trust_level` (refuse/down-rank tasks from `Unknown`
  authors) + provenance (the task is attributable). **Axis: who.** The S-5
  human-in-loop pattern is directly reusable for cross-trust task acceptance.
- **Residual gap:** nothing inspects the *semantics* of the instruction text. A
  `Verified` identity (compromised, or behave-then-defect — WG-Exec P6) sends a
  clean-looking task with A-1.3/A-1.4 inside, and the agent consumes it. **This is
  the prompt-injection problem and it is unsolved by attribution alone.**

### IC2 — Code / artifacts (including results from a remote provider)

Diffs, files, build artifacts, and dependency trees authored elsewhere — the
direct **supply-chain** surface, and the one the task names as *"the issue with
node/npm."*

| ID | Attack | Mechanism |
|---|---|---|
| **A-2.1** | **Malicious code** | a diff/artifact contains a backdoor, data-exfil call, or destructive payload |
| **A-2.2** | **Poisoned dependency** | the artifact pulls a malicious/typosquatted dep (the npm/PyPI vector, transitively) |
| **A-2.3** | **Test-poisoning** | the deliverable edits its **own** acceptance tests so verification passes (WG-Exec **X-6** — the oracle is rewritten) |
| **A-2.4** | **Check-passing semantic poison → cross-task propagation** | a backdoor that passes per-task checks **launders into honest downstream `--after` consumers** (WG-Exec **TC8** — the graph-integrity attack) |
| **A-2.5** | **Build/exec-time payload** | code that is benign to read but malicious to *run* (install scripts, codegen, macros) — the detonation surface |

- **Partly covered by:** the **exec verification leash (HQ2)** — re-run in a
  *trusted domain* + eval-gate against `## Validation` (**correctness axis**);
  the **X-6 test-poison gate** — pin the spec, flag test-file diffs for review
  (**oracle axis**); the **UCAN blast-radius bound (FR-V4)** — a forged result
  corrupts only its own task's output (**damage axis**); worktree isolation
  (**containment**).
- **Residual gap:** HQ2 checks *"is the result correct,"* not *"is the code
  malicious-but-correct."* A backdoor that passes the pinned tests (A-2.4) is
  exactly the **TC8** finding the exec study flags as *"most under-defended"* and
  *"residual real."* Dependency poisoning (A-2.2) and build-time payloads (A-2.5)
  are entirely outside the correctness check. **These need code/dep scanning +
  sandboxed detonation — a content-axis layer that does not exist.**

### IC3 — Loadable identity state

A portable `StateSnapshot` loaded to "resume a continuous self" — the
federation **S-5** finding, AI-substrate-specific with no Nostr/Keybase/atproto
analogue.

| ID | Attack | Mechanism |
|---|---|---|
| **A-3.1** | **Stored prompt-injection** | a validly-signed "conversation cache" carries an injection the next host inherits on load |
| **A-3.2** | **Poisoned summary** | the summary payload subtly re-points goals/beliefs of the resuming agent (persistence + lateral movement) |
| **A-3.3** | **Opaque-blob smuggling** | an un-introspectable `payload_kind` hides a payload (or, per S-1, exfiltrates a key) under a valid signature |
| **A-3.4** | **Model-binding mismatch** | state crafted for a different model version mis-loads into the current one |

- **Partly covered by:** this is the **best-specified** class — ADR-fed-004 D-step
  7 already mandates **provenance-gate by `trust_level`** + **`model_binding`
  enforcement** + **human-in-loop for cross-trust loads**, and explicitly *treats
  loaded state as untrusted input.* **Axes: who + model-binding + human-gate.**
- **Residual gap:** ADR-fed-004 *names* "sandbox/scan" as the content-axis
  mitigation but records it **does not exist yet** ("requires an AI-input-safety
  layer WG does not have today"). Opaque `payload_kind`s (A-3.3) are
  **un-scannable by construction** — the residual here is inherent and must be
  contained by runtime sandboxing, not screened. **This study's gate is the scan
  layer ADR-fed-004 deferred.**

### IC4 — Messages (human↔agent, agent↔agent)

Federation-transported messages between humans and agents — the
phishing/social-engineering surface.

| ID | Attack | Mechanism |
|---|---|---|
| **A-4.1** | **Phishing** | a message impersonates a trusted party (or rides a compromised-but-`Verified` identity) to induce an action |
| **A-4.2** | **Social engineering of an agent** | directives in a message (the IC1 overlap) push an agent toward an unwanted action — `wg msg` is consumed as instructions |
| **A-4.3** | **Social engineering of a human** | a message manipulates the human operator (approve, reveal, install, override the gate) |
| **A-4.4** | **Injection via message → task** | a message seeds a poisoned task/instruction that re-enters as IC1 |

- **Partly covered by:** transport authenticity (ADR-fed-002: messages are signed
  → *who*), `trust_level` down-ranking, and the WG-Fed anti-abuse / consent gates
  + reputation noted in the federation memo. **Axis: who + transport.**
- **Residual gap:** authenticity proves the *sender's key*, not the *message's
  intent*. A `Verified`-but-compromised sender phishes freely (A-4.1); message
  *content* is not screened for manipulation, and the A-4.2/A-4.4 overlap means
  messages are an **injection vector into IC1**. **Needs content classification +
  the same human-in-loop escalation as S-5, applied to the inbox.**

### Threat-matrix summary

| Class | Headline attacks | Existing defense (axis) | Residual gap the gate must close |
|---|---|---|---|
| **IC1 task/prompt** | injection, jailbreak, goal-hijack, social-eng | `trust_level` + provenance (**who**) | content-semantics of instructions — *unsolved* |
| **IC2 code/artifact** | malicious code, poisoned dep, test-poison (X-6), TC8 cross-task poison | HQ2 verification leash (**correctness**), X-6 gate (**oracle**), UCAN bound (**damage**) | malicious-but-correct code, deps, build-time payload |
| **IC3 loadable state** | stored injection, poisoned summary, opaque smuggling | S-5 provenance-gate + `model_binding` + human-in-loop (**who/model/human**) | the scan layer S-5 deferred; opaque = contain-only |
| **IC4 message** | phishing, social-eng (human+agent), inject→task | signed transport + `trust_level` (**who**) | message-content intent; injection vector into IC1 |

**The one-sentence finding:** WG already defends the **who / authority /
correctness / containment** axes well; **every residual in this matrix lives on
the same uncovered axis — the *semantics of the content itself* — and that is the
axis the review gate (task 2/4) must own.**

---

## 3. Prior art (≥8 systems, mapped to WG)

For each family: **what it catches**, its **false-positive / false-negative
posture**, **automatable vs human-in-loop**, and **fit for WG** (which `IC*` class
it best informs and the pattern WG should borrow). These are the systems the
review gate should be assembled *from* — none is sufficient alone, which is itself
the lesson (§4).

### PA-1 — npm / PyPI malware scanning + typosquat detection
*(Socket, npm audit / GitHub malware scanning, PyPI/OpenSSF, `pip-audit`, Guarddog)*
- **Catches:** known-malicious packages, suspicious install scripts, exfil
  patterns, typosquatted names (edit-distance to popular packages), dependency
  confusion.
- **FP/FN:** moderate FP on aggressive heuristics (legit install scripts flagged);
  **high FN on novel/obfuscated malware** — caught the headline incidents only
  *after* publication. This is the canonical "mitigate, don't eliminate" system.
- **Automatable:** highly — runs in CI / on publish; reputation + heuristics need
  no human. Human escalation only for ambiguous flags.
- **Fit:** **IC2 (A-2.1/A-2.2).** The *direct* analog — WG's code/artifact +
  dependency screening is "npm scanning for federated diffs." **Borrow:** scan +
  typosquat + reputation + provenance, accept residual; run at the boundary on
  publish/accept.

### PA-2 — Package signing + provenance (SLSA, sigstore/cosign, npm provenance attestations)
- **Catches:** **tampering and mis-attribution** — proves *who built what, from
  which source, on which builder*. Catches a swapped/forged artifact; does
  **nothing** about a *signed-but-malicious* one.
- **FP/FN:** near-zero FP (cryptographic); **structural FN on intent** — a
  malicious author signs their malware perfectly. This is the **S-5/IC-wide
  lesson in the supply-chain world: provenance ≠ safety.**
- **Automatable:** fully (verify on accept).
- **Fit:** **all classes — the *who* axis WG already has.** Confirms WG's
  provenance/attribution is necessary but *not* the content gate. **Borrow:** the
  *transparency-log + attestation* pattern for after-the-fact audit (it pairs with
  scanning, never replaces it).

### PA-3 — App-store / browser-extension review (Apple App Review, Chrome Web Store, VS Code Marketplace)
- **Catches:** policy violations, malware, privacy abuse, capability over-reach —
  via **automated static/dynamic scan + a human review tier + post-publish
  telemetry/kill-switch**.
- **FP/FN:** notable FP (false rejections, slow appeals); FN real (malicious
  extensions and apps repeatedly slip through, caught later by telemetry →
  **remote removal**).
- **Automatable:** **hybrid by design** — cheap automated pre-screen, human review
  for higher-risk submissions, *and* an after-the-fact revocation channel.
- **Fit:** **IC2 + IC1.** The closest *organizational* analog to a "review task."
  **Borrow:** the **tiered automated-then-human + post-acceptance kill-switch**
  shape — maps onto WG's revoke/audit-after-the-fact (UCAN revocation, sigchain).

### PA-4 — Email spam + phishing filtering (SpamAssassin, Gmail, DMARC/SPF/DKIM, Bayesian + reputation)
- **Catches:** spam, phishing, spoofing — via **layered scoring**: sender-auth
  (SPF/DKIM/DMARC = *who*), content Bayesian/ML classifier (*what*), URL/attachment
  reputation, and IP/domain reputation.
- **FP/FN:** **tuned, never zero** — the entire discipline is FP/FN *threshold
  management* (a legit mail in spam is costly; a phish in inbox is costly). Decades
  of adversarial co-evolution.
- **Automatable:** highly; user-facing "report" loop feeds reputation. Human (the
  recipient) is the final arbiter on borderline.
- **Fit:** **IC4 (and IC1).** The *direct* analog for the message/phishing surface.
  **Borrow:** **sender-auth + content-classifier + reputation, layered, with an
  explicit FP/FN tuning policy** — and a user-report loop that updates
  `trust_level`. This is the template for the message gate.

### PA-5 — CI / secret / SAST scanners (Semgrep, CodeQL, gitleaks / trufflehog, Dependabot)
- **Catches:** known vulnerability/anti-patterns, committed secrets, vulnerable
  deps — **deterministic pattern + dataflow** detectors.
- **FP/FN:** SAST is **FP-heavy** (noisy, alert-fatigue); secret scanners are
  **precise/low-FP** on known formats; both **FN on novel logic bugs / backdoors**
  that match no rule.
- **Automatable:** **fully** — the cheapest, fastest first layer; already WG's
  idiom (the smoke gate is exactly a deterministic CI gate).
- **Fit:** **IC2.** **Borrow:** **run the cheap deterministic scanners *first***
  (secrets, known-bad patterns, suspicious calls) before any expensive LLM-judge —
  catch the easy 80% at near-zero cost, escalate the rest. Mirrors WG's
  smoke-gate-before-`wg done` ordering.

### PA-6 — Prompt-injection defenses (spotlighting, delimiting, dual-LLM, input classifiers, guardrails)
*(Microsoft spotlighting, the Simon Willison **dual-LLM / quarantine** pattern,
Llama Guard, Lakera Guard, NeMo Guardrails, Prompt Shields)*
- **Catches:** injected instructions in untrusted text — by **marking untrusted
  spans** (delimiting/spotlighting), **quarantining** untrusted content from the
  privileged/action-taking LLM (dual-LLM), or **classifying** input for known
  injection/jailbreak patterns.
- **FP/FN:** classifiers have **real FN** (novel injections evade) and FP (benign
  text flagged); **the structural patterns (delimiting, dual-LLM) reduce *blast
  radius* even when detection fails** — the most robust of the set because they
  don't rely on catching the attack.
- **Automatable:** fully (they *are* LLM/code layers); the dual-LLM pattern needs
  no human but trades capability.
- **Fit:** **IC1 + IC4 (and IC3 stored-injection).** The *direct* analog for the
  prompt/instruction surface. **Borrow two things:** (a) **spotlight/delimit all
  inbound IC1 text** so the consuming agent treats it as data-with-provenance, not
  instructions; (b) the **quarantine/dual-LLM** discipline — never let unscreened
  inbound content reach a privileged tool/action without a trust-downgrade step.
  **This is the single most important pattern for the content-semantics axis** —
  and the literature is candid that it is *mitigation, not solution*, reinforcing §0.

### PA-7 — Content-moderation pipelines (Perspective API, OpenAI moderation, platform Trust & Safety)
- **Catches:** abusive/harmful/policy-violating content — via a **multi-stage
  pipeline**: cheap classifier → confidence threshold → human-review queue for the
  uncertain band → appeal/feedback loop.
- **FP/FN:** explicitly **threshold-managed**; the human tier exists *because* the
  automated tier's FP/FN is unacceptable alone on high-stakes calls.
- **Automatable:** **hybrid by design** — automation handles volume, humans handle
  the ambiguous/high-impact tail, feedback retrains.
- **Fit:** **IC1 + IC4.** **Borrow:** the **tiered cheap-classifier → confidence
  band → human-in-loop escalation** architecture — and map "human-in-loop" onto
  WG's existing **S-5 cross-trust human gate**. The cheap classifier maps onto
  WG's **weak-tier agency one-shot** (`.evaluate-*` style); the human tier onto the
  cross-trust escalation.

### PA-8 — Allow/deny-list + reputation + sandboxed eval (Cuckoo/CAPE detonation, VirusTotal, capability allowlists)
- **Catches:** known-bad (deny-list, instant), known-good (allow-list, instant),
  and **unknown-by-behavior** — *detonate the artifact in a sandbox and watch what
  it does* (network, fs, exfil) instead of trying to read intent statically.
- **FP/FN:** deny/allow-lists are **precise but brittle** (zero FP on listed
  items, total FN on anything new); **sandboxing catches behavior static analysis
  misses** but is evadable (sandbox-detection, time-bombs) and costly.
- **Automatable:** fully; detonation is the expensive tier reserved for unknown
  artifacts.
- **Fit:** **IC2 + IC3 (incl. the opaque-blob A-3.3 that is un-scannable
  statically).** **Borrow:** **allow/deny-list as the cheap first cut; sandboxed
  detonation as the expensive last resort for unknown code/artifacts and opaque
  state** — and reuse WG's **worktree/UCAN containment** as the sandbox, so even a
  missed payload is blast-radius-bounded (FR-V4).

### PA-9 — Certificate Transparency / transparency-log audit (CT, Sigstore Rekor, binary transparency)
- **Catches:** **nothing at admission time** — instead it makes mis-issuance /
  malicious-publication **publicly detectable after the fact**, so attacks are
  *caught and revoked* rather than *prevented*.
- **FP/FN:** N/A at the gate; it is an **audit + accountability** layer. Its value
  is deterrence (you *will* be seen) and forensic revocation.
- **Automatable:** fully (append-only log + monitors).
- **Fit:** **all classes — the audit/revoke leg of "mitigate, don't eliminate."**
  **Borrow:** log every inbound-content acceptance to an append-only, attributable
  record (WG's sigchain is the substrate) so a *later*-discovered poison can be
  traced to its author, the author's `trust_level` lowered, and downstream `--after`
  consumers (TC8) found and re-run. **This is how WG copes with the inevitable
  miss.**

### PA-10 — Antivirus / EDR (signature + heuristic + behavioral detection)
- **Catches:** known malware (signatures), suspicious structure (heuristics), and
  **malicious runtime behavior** (EDR) — the mature template for "screen untrusted
  executable content."
- **FP/FN:** signatures = low-FP/known-only; heuristics + behavioral = higher FP,
  catch more novel. The whole industry is a **layered, signature+heuristic+behavior
  stack with a permanent residual** — a 30-year proof that elimination is not on
  offer.
- **Automatable:** fully; SOC human tier for incident response.
- **Fit:** **IC2.** **Borrow:** the **layered signature→heuristic→behavioral**
  composition and, again, the candid residual posture — AV has never claimed to
  eliminate malware, only to raise cost and catch probability. Exactly WG's stance.

### Prior-art comparison table

| # | System | Catches | FP/FN posture | Automatable | Best-fit class | Pattern WG borrows |
|---|---|---|---|---|---|---|
| PA-1 | npm/PyPI malware + typosquat | known-bad pkgs, typosquats, exfil scripts | mod-FP / **high-FN novel** | auto + escalate | IC2 | scan+typosquat+reputation, accept residual |
| PA-2 | Signing + provenance (SLSA/sigstore) | tamper, mis-attribution | ~0-FP / **structural-FN on intent** | full | all (who) | provenance ≠ safety; pair with scan |
| PA-3 | App-store / extension review | malware, over-reach, policy | FP-appeals / FN-slips | **hybrid** | IC2/IC1 | tiered auto→human + kill-switch |
| PA-4 | Email spam/phishing | spam, phish, spoof | **tuned, never zero** | auto + report loop | IC4/IC1 | sender-auth + classifier + reputation, layered |
| PA-5 | CI/secret/SAST | secrets, known vulns/patterns | secret:low-FP / SAST:**FP-heavy**, FN-novel | full | IC2 | cheap deterministic scanners **first** |
| PA-6 | Prompt-injection defenses | injected instructions, jailbreaks | classifier-FN / **struct. patterns bound radius** | full | IC1/IC4/IC3 | **spotlight/delimit + dual-LLM quarantine** |
| PA-7 | Content-moderation pipelines | harmful/policy content | **threshold-managed** | **hybrid** | IC1/IC4 | cheap-classifier→band→human escalation |
| PA-8 | Deny/allow-list + sandboxed eval | known-bad/good + behavior | brittle-precise / sandbox-evadable | auto (detonate=costly) | IC2/IC3 | allow/deny first; **sandbox detonation** last |
| PA-9 | Transparency-log / CT audit | (nothing at admit) after-the-fact mis-publication | N/A — audit layer | full | all (audit) | append-only log → trace+revoke the miss |
| PA-10 | Antivirus / EDR | known + heuristic + behavioral malware | sig:low-FP / behavior:higher-FP | auto + SOC | IC2 | layered sig→heuristic→behavior, candid residual |

---

## 4. Synthesis — what the prior art teaches WG's review gate (hand-off to 2/4)

The ten systems agree on a small set of design laws. These are the inputs
`safety-design` (2/4) should build the gate from:

1. **No single layer is sufficient — defense-in-depth is the only posture that
   works.** Every mature system (npm = scan+sign+provenance+reputation; email =
   SPF+DKIM+Bayes+reputation; AV = sig+heuristic+behavior) is *layered*. The gate
   is a **pipeline of cheap→expensive detectors**, not one classifier.

2. **Order by cost: cheap deterministic first, expensive judgment last** (PA-5,
   PA-7, PA-8). Run secret/known-bad/typosquat/deny-list scanners (near-zero cost)
   before any LLM-judge; reserve the expensive sandbox-detonation and human tier
   for the residual uncertain band. **This maps directly onto WG's idioms** — the
   smoke-gate-before-`wg done` ordering, and the **weak-tier agency one-shot** as
   the cheap LLM classifier.

3. **Quarantine before you trust (the dual-LLM law, PA-6).** The most robust
   prompt-injection defense doesn't *detect* the attack — it **structurally
   prevents** untrusted content from reaching a privileged action without a
   trust-downgrade. The gate must **spotlight/delimit all inbound IC1/IC4 text**
   and forbid unscreened inbound content from driving privileged tools.

4. **Provenance and content are *orthogonal* layers — WG has the first, lacks the
   second** (PA-2, PA-9 vs PA-1, PA-6). Signing tells you *who*; scanning/sandboxing
   tells you *what*. WG's `trust_level` + attribution + sigchain are the *who*
   layer and are **already built**; the gate is the missing *what* layer. **Do not
   re-solve provenance; build on it.**

5. **Reuse the trust-proportional dial (PA-3, PA-4, and WG-Exec's own leash).**
   Verification depth should scale with the author's `trust_level` — a `Verified`
   peer's task gets cheap screening; an `Unknown` author's code gets the full
   scan + sandbox + human gate. This is **literally WG-Exec's HQ2 leash applied to
   content instead of correctness** — the same dial, a new axis.

6. **Tiered human-in-loop, mapped to the S-5 gate (PA-3, PA-7).** Automation
   handles volume; the ambiguous/high-impact tail escalates to a human. WG already
   has the escalation primitive — the **S-5 cross-trust human-in-loop** — and
   should route the gate's uncertain band through it rather than inventing a new
   one.

7. **FP/FN is a *policy*, tuned and surfaced, never silent (PA-4, PA-7).** A
   too-strict gate blocks honest federated work; a too-loose one consumes poison.
   The threshold is a stated, inspectable policy (like the visible leash in
   `wg status`), and — borrowing WG's smoke-gate discipline — **a SKIP/uncertain
   verdict is surfaced loudly, never silently dropped.**

8. **Contain the miss, then audit and revoke it (PA-8, PA-9, FR-V4).** Because the
   residual is real (§0), the gate's last two layers are **containment** (worktree
   / UCAN blast-radius bound / sandboxed eval, so a missed payload is contained)
   and **audit** (append-only, attributable acceptance log on the sigchain, so a
   later-discovered poison is traced to its author, `trust_level` lowered, and
   **TC8 downstream `--after` consumers found and re-run**).

---

## 5. The stance, restated and made operational

The npm/supply-chain analogy is not decorative — it is the **governing model**.
WG's federation deliberately re-creates npm's situation: untrusted code, state,
instructions, and messages crossing a trust boundary at scale, consumed by an
automated agent. npm's two-decade answer is the only honest one available: **layer
the mitigations, tune the false-positive/false-negative trade-off, contain and
audit the inevitable miss, and never claim the risk is gone.**

Concretely, the review gate that task 2/4 designs **commits to**:

- **screen** inbound content on the content-semantics axis WG lacks today (the
  layered, cheap→expensive pipeline of §4),
- **scale** the screening depth by `trust_level` (the existing leash, new axis),
- **quarantine** untrusted content from privileged actions (the dual-LLM law),
- **escalate** the uncertain band to the S-5 human-in-loop,
- **contain** every consumption in a blast-radius-bounded sandbox (UCAN/worktree),
- **audit** every acceptance to the sigchain so a later-found poison is traceable,
  the author's trust is lowered, and downstream consumers are re-run (TC8),

and **explicitly does not** certify that inbound content is safe. The residual is
inherent (§0); the gate's success metric is **a smaller, well-understood,
contained, auditable residual at an acceptable false-positive cost** — *mitigate,
don't eliminate.*

---

### Provenance of this document

- **Threat axes & defenses mapped from:** WG-Fed identity/trust decision
  (`docs/federation-study/06-decision-memo-and-roadmap.md`), the S-5 loadable-state
  finding (`docs/federation-study/05-adversarial-evaluation.md` §S-5;
  `docs/ADR-fed-004-loadable-state-safety.md`), and the WG-Exec result-integrity
  decision + cross-task-poison/test-poison findings
  (`docs/execution-federation-study/06-decision-memo-and-roadmap.md` HQ2;
  `…/05-adversarial-evaluation.md` X-5, X-6, TC8).
- **Existing primitives referenced:** `Agent.trust_level` (`TrustLevel` enum,
  `graph.rs:1920` — `Verified|Provisional|Unknown`); provenance/attribution (FR-V1); UCAN
  blast-radius bound (FR-V4); the verification leash (HQ2); worktree isolation.
- **Downstream:** `safety-design` (2/4) consumes §2 (the threat matrix), §3–§4 (the
  prior-art + design laws), and §5 (the stance) to design the review task(s).
