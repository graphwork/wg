# Worker-owned universal completion recovery exit

**Decision:** PASS — recovery mode exited on 2026-08-05.

**Candidate source:** `4aca437f135721d17fec977dffd017f1f3481797`
(`feat: worker-owned universal completion cutover (#61)`). The merge tree is identical
to reviewed PR #61 head `06859c23`.

**Normative protocol:**
[`design-worker-owned-universal-review.md`](../design-worker-owned-universal-review.md)

**Cutover plan:**
[`simple-worker-owned-lean-convergence.md`](../plans/simple-worker-owned-lean-convergence.md)

## Clean-room exit canary

The permanent scenario
[`worker_owned_completion_canary.sh`](../../tests/smoke/scenarios/worker_owned_completion_canary.sh)
was run directly against `target/debug/wg` built from the candidate above. It created a
fresh Git repository, graph, `HOME`, config root, deterministic Pi executable, and daemon.
It did not use the development graph, installed `wg`, user credentials, or a global
service.

Command:

```bash
CARGO_BUILD_JOBS=1 cargo build --locked --bin wg
WG_SMOKE_CANDIDATE_BIN="$PWD/target/debug/wg" \
  bash tests/smoke/scenarios/worker_owned_completion_canary.sh
```

Result:

```text
PASS worker-owned-completion-canary workers=10
```

The ten source workers comprised two Land, five Report, and three Explore contracts. The
two Land workers started from the same main revision. Exactly one won the first landing
compare-and-fast-forward; the losing worker retained ownership, integrated moved main,
rebuilt its manifest, reran FLIP and eval, and landed on a later attempt. The scenario
asserted:

- all ten tasks reached Done through a resolvable immutable completion manifest;
- every completion carried exact manifest-bound FLIP and eval receipts;
- both Land commits were reachable from main;
- Report and Explore objects remained digest-resolvable without Git worktrees;
- at least three Land submissions occurred, proving same-worker moved-main repair;
- exactly ten source-agent directories existed, proving no replacement source spawn;
- no `.flip-*` or `.evaluate-*` graph children were created;
- no legacy `finalization/` or `worker-control/transactions/` authority was created.

## Failure and invariance matrix

The same candidate also passed the ten-attempt concurrent matrix. Six exact submissions
were accepted and completed; FLIP rejection, eval rejection, reviewer unavailability, and
incomplete evidence each left one submission visibly blocked rather than Done.

The final recovery gate additionally passed:

- `cargo fmt --check`;
- `cargo clippy` and `cargo clippy --bin worksgood`;
- manifest resolver, universal review valve, task projection, legacy-authority retirement,
  lifecycle conformance, SimpleLand conformance, and Rust/Lean oracle tests;
- the Lean proof-escape scan;
- `lake build` and `lake build simple-land-oracle`;
- the isolated real-daemon ten-worker scenario a second time.

These gates cover changed requirements invalidating review, digest mismatch and missing
output failing closed, protected `.wg` content rejection, exact-route reviewer failure as
Unavailable rather than Reject, terminal inertness, publication recovery traces, and the
absence of production CLI/daemon calls into legacy completion authority.

## Hosted verification

PR [#61](https://github.com/graphwork/wg/pull/61) merged at
`2026-08-05T18:38:21Z` with successful hosted checks for stable/nightly Rust, lint,
integration, Lean conformance, Windows installation, and the embedded Pi plugin.

## Evidence identity

The local full gate transcript was retained at
`target/recovery-evidence/final-recovery-exit-gates.log` with SHA-256:

```text
2162a9f942187d40ef354d7505898bf04e6aecda299275698a21d3c628c2793a
```

The tested debug candidate SHA-256 was:

```text
93732c7df221f5db5e9ef05aa3800a9883470850ac0dca35e838bc91babe715d
```

The temporary single-attended-session restriction was therefore removed from
`AGENTS.md` and `CLAUDE.md`. All later Land, Report, and Explore tasks remain subject to
the universal manifest, FLIP, eval, publication, and derived-Done protocol; the recovery
implementation remains the sole documented bootstrap exception.
