# Spark-stage feature documentation

Docs in this directory describe **spark-stage** subsystems of WorksGood: code
that is complete, smoke-pinned, compat-versioned, and deliberately *not* on the
operator's daily path (see `docs/survey/survey-synthesis-subtraction-plan.md`
§1 S7–S10).

They used to live inline in `AGENTS.md` / `CLAUDE.md` and were therefore loaded
into **every** agent prompt, even though the daily worker loop never touches
them. `AGENTS.md` / `CLAUDE.md` now keep a short stub that links here; the text
was moved verbatim, and the slimming rationale + the exact patch live in
`docs/design/agents-md-slimming.md`.

| Doc | Subsystem | Verbs | Compat const | Smoke owner |
|---|---|---|---|---|
| [`wg-fed.md`](wg-fed.md) | WG-Fed: identity, sigchain, transport, portable state, encryption=ACL, UCAN | `wg identity`, `wg fed-node`, `wg peer`, `wg msg --to wgid:` | `WG_FED_COMPAT_VERSION` | `federation_*` scenarios |
| [`wg-review.md`](wg-review.md) | WG-Review: the inbound-content review gate | `wg review` | — (rides WG-Fed envelopes) | `content_safety_spark` |
| [`wg-exec.md`](wg-exec.md) | WG-Exec: remote placement, lease fencing, disjoint verification | `wg provider` | `WG_EXEC_COMPAT_VERSION` | `exec_spark_borrowed_box` |
| [`e2e-family-team.md`](e2e-family-team.md) | The three sparks composing across two isolated instances | — | — | `e2e_family_team` |
| [`wg-pilot.md`](wg-pilot.md) | Turnkey family-team deploy (deploy/UX wrapper) | `wg pilot` | — | `pilot_dry_run` |

## Reading rules

1. **Read the doc before touching that code.** Each one carries the compat
   const, its smoke owner, and the "spark boundary" — what the code does *not*
   yet prove.
2. **Do not infer the daily contract from here.** The worker runtime,
   completion review, routing authority, TUI, and plugin are documented in
   `AGENTS.md` § Development / Service Configuration and `docs/`; a spark doc is
   never the authority for how a normal task is dispatched.
3. **Never widen a compat const to make a peer work.** Both consts fail *loudly*
   on mismatch on purpose (S-7); a bump is a wave decision, not a fix.
4. **The spark boundary is load-bearing.** "Deterministic stub" in a spark doc
   means the security *shape* is proven and the live-model/enclave half is a
   later wave. Do not read a passing spark smoke as evidence of a real LLM or a
   real enclave.
