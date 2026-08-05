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

## Service restart and control-plane recovery

The committed tree was installed with `cargo install --path . --locked`. The first
repository-service start then failed closed before daemon launch because the durable
control-plane identity recorded inode `3016162`, while the `.wg` path had been replaced
by inode `4197807`. The replacement contained no graph or config, so blindly adopting it
would have discarded the development graph.

The latest external control-plane snapshot was therefore recovered from Git common-dir
storage:

```text
wg-control-snapshot:v1:blake3:d7ac31604d04714af3d756c663a483fc97a424f79bbabb4c17dcbbf6487b75ee
created: 2026-08-05T08:37:05.680208246+00:00
reason: create-worktree:agent-1109:convergence-cutover-owner
```

All 451 receipt entries, including the 5,464,156-byte graph and config, were verified
against their exact BLAKE3 digests before restore. The replaced directory was retained,
not deleted, and an operator recovery receipt plus both identities were written under:

```text
.git/wg-control-plane/recovery/20260805T201259Z/
```

The restored snapshot predated the attended cutover and exposed the superseded planner
cutover chain as open, including one ready source task. Those six legacy cutover tasks
were explicitly Abandoned with a reason naming PR #61; abandonment remained non-success
and did not satisfy their dependency edges. `fix-pi-clean-live-log`, whose equivalent
commit `b2839707` is already in the merged cutover, was likewise retained as explicitly
superseded rather than automatically respawned. No tasks remained ready.

Because agency definitions were not among the snapshot's graph/config/chat/message
payload, `wg agency init` recreated the standard five agent definitions and the service
configuration was reloaded. The repository dispatcher then started with chat spawning
disabled and reported:

```text
Dispatch authority: direct fail-stop (no PlannerStore)
Dispatcher: enabled, max_agents=4
No tasks ready
No agents registered
```

The installed executable SHA-256 is
`5ae7100ab14a9f68e8985cadee05c779ffc8b5dfef8a497a54e1a4e21ea8ca0d`.
The service retained all restored graph history while spawning no obsolete source work.

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
