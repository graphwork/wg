<!-- provenance: extracted verbatim from AGENTS.md (sections now stubbed there) by
     task `doc-archive-policy`. Text is unchanged apart from heading level; the
     stub in AGENTS.md/CLAUDE.md links here. See docs/archive/POLICY.md. -->

# WG end-to-end — the family-team integration

*Extracted from `AGENTS.md` § "WG end-to-end — the family-team integration (`tests/smoke/scenarios/e2e_family_team.sh`)".*

The three sparks (WG-Fed, WG-Review, WG-Exec) each pass in **isolation**. The **family-team
e2e** (`e2e_family_team.sh`, `owners = [e2e-family-team]`) is the milestone that proves they
**COMPOSE** into one continuous flow across **two FS-independent instances** — distinct `$HOME`
keystore + distinct `--dir` graph, **no shared filesystem**, whose ONLY channel is a dumb,
untrusted HTTP relay node (`wg fed-node serve`; every byte self-verifying). It reuses the
existing identity/UCAN/seal substrate with **no second trust system** and adds no new compat
const. Cast: instance A (the family home) = **Sara** (human requester) + **Luca** (operates the
borrowed compute box, the WG-Exec Provider P); instance B (the chef host) = **Bruno** (chef
agent, principal/authorizer — root custodied on B, never leaves it; accepts + verifies) +
**Nora** (dietitian agent — the disjoint integrity verifier Q ≠ producer); plus **Mallory** (a
stranger adversary).

The continuous chain (each link a falsifiable assertion): **(1) identity** — mint the four
family `wgid:`s, cross-publish + cross-fetch + OFFLINE-verify across the wall, no private key in
any published byte; **(2) cross-graph task** — Sara sends "plan Wednesday dinner" sealed to Bruno
via `wg msg --to wgid:`, Mallory plants a hostile variant, Bruno polls his node inbox, both
authenticate by key, a forged "from Sara" is rejected; **(3) review gate on the way IN**
(received ≠ consumed) — Bruno screens each inbound BEFORE consuming: Sara's legit task is accepted
on the light path and becomes the exec input while Mallory's planted hostile variant is
quarantined/rejected and **never consumed** (no exec offer is made for it); **(4) remote exec** —
Bruno places the reviewed task on the OTHER instance's compute (Luca's borrowed box on A) under
**two scoped attenuating UCANs** (act-as-agent + graph-write scoped to `graph://task/wed-dinner`)
— never his root, never a blanket write — and the box opens ONLY its `task` slice; **(5) signed
result back** — Bruno (authorizer) accepts + verifies the signed result against HIS sigchain
(attributed to Bruno; wrong-signed rejected), the borrowed box cannot exceed its lease
(wrong-task / post-expiry / replay / stale-after-reclaim all fenced), a corrupted plan is caught
by Nora's disjoint re-run vs the pinned spec, a confidential task to the non-attested box is
refused fail-closed, and the signed completion crosses the wall **back** to Sara on instance A,
authenticated as Bruno.

**Seam finding.** The three modules compose cleanly because they share one substrate (the e2e
required **no production-code change**). The one real seam the isolated sparks could not surface
is **auto-wiring**: the review gate's author-trust is hand-passed (`--trust`) and the gate is
invoked manually between `wg msg poll` and the exec offer. Production must (a) derive author-trust
canonically from the federation peer/sigchain `graph::TrustLevel` (the same dial the exec pool
reads) and (b) auto-run the review pipeline at the live ingest edge so "received ≠ consumed" holds
with no manual step. **This is now SHIPPED for the IC4 (message) ingest path** (the `auto-wire-the`
build): `src/trust.rs::resolve_author_trust` is the canonical trust resolver unifying the federation
peer registry (`wg peer add --trust`) with the WG-Exec provider pool, and `wg msg poll --review`
auto-screens each authenticated inbound through the review pipeline with that *derived* trust,
refusing consumption of a non-accept verdict (no `--trust` flag, no separate `wg review check`).
It is pinned by `tests/smoke/scenarios/e2e_autowire_ingest_gate.sh` (`owners = [auto-wire-the]`).
The remaining import / accept / state-load ingest seams stay Review-Wave C/D. **Spark boundary**:
the exec result work-product is the exec spark's deterministic stub (the
real weak-tier LLM is a later wave); the e2e proves the **composition + the security bounds at
every seam**, not the silicon.
