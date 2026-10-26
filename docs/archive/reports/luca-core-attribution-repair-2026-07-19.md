# Luca core attribution repair

**Repair task:** `repair-luca-core`

**Audit date:** 2026-07-19

**History policy:** additive only; no published commit is amended, rebased, replaced, or force-pushed

## Decision

Repair the lost credit without rewriting published `main` in two durable, additive forms:

1. Keep this exact provenance record in the repository, including full object names and stable patch IDs.
2. Put `Co-authored-by: Luca Pinello <lucapinello@gmail.com>` on the additive repair commit. That trailer restores repository-level credit for the U1/U3 contribution whose original attribution the two squash commits discarded. It does not claim that Luca implemented the squash-merger repair.

For future landings, `wg done` now reads every source commit in `HEAD..<worktree-branch>` before squashing. The oldest source author becomes the squash commit author. Every other distinct source author and every valid `Co-authored-by` trailer becomes a deduplicated `Co-authored-by` trailer on the squash commit. The actor who performs the landing remains the Git committer. If source attribution cannot be read or the selected identity cannot be committed, landing fails loudly instead of emitting an unattributed squash.

This is deliberately narrower than changing merge topology: WG retains its one-commit-per-task squash history, but the squash no longer discards contributor identity.

## Exact retained and lost provenance

All ancestry statements below were checked against both local `main` and `origin/main` at `05a94228487ba3df5694a313d7f6cfe671d741c4`. Full hashes are used so the record remains useful if short prefixes later collide.

### U1 — detached background process

| Role | Commit | Attribution on the object | Main ancestry |
|---|---|---|---|
| Luca baseline retained on source branch `wg/agent-513/integrate-luca-detached` | `8cf7a682883cfde7aa5f5a046c5c8f3950e80cc9` | author Luca Pinello `<lucapinello@gmail.com>`; `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; cherry-pick provenance to `d25cdd594dba4789eba310891cc2caed2ce1975d` | **not** an ancestor |
| Integration hardening on that branch | `f68de662f3146d5d122a761ac472f655b88431f7` | author Erik Garrison `<erik.garrison@gmail.com>` | **not** an ancestor |
| Grow-only manifest resolution on that branch | `31819fbbff21a47cd31c1d558288d2d1ae526226` | author Erik Garrison `<erik.garrison@gmail.com>` | **not** an ancestor |
| Published squash landing | `604076593e9d6fb8e2dfd0529ac2f83806063c9f` | author and committer Erik Garrison `<erik.garrison@gmail.com>`; **no Luca author identity and no Luca co-author trailer** | ancestor of `main` and `origin/main` |

The source range is based at the landing's parent, `3027b825c802c10277c5364185afe4c49dbf6b10`. The stable patch ID of both `3027b825..60407659` and `3027b825..31819fbb` is `1c187f28726d2c66ccd1ac61376e0319f0e3fd34`; the landing tree and source-branch tip tree are equal. The implementation landed, but Luca's commit identity did not.

### U2 — FailedPendingEval rescue

| Role | Commit | Attribution on the object | Main ancestry |
|---|---|---|---|
| Luca source | `9a5955288dc8419f2efff10986cfe25d8f3bc1a9` | author and committer Luca Pinello `<lucapinello@gmail.com>`; Claude co-author trailer | **not** an ancestor (cherry-picked object) |
| Published current-main commit | `9176849cac11a93134aa21f94db879923ffa002e` | **author Luca Pinello** `<lucapinello@gmail.com>`; committer Erik Garrison; Claude co-author trailer; explicit `(cherry picked from commit 9a595528...)` line | ancestor of `main` and `origin/main` |

The two commits have the same stable patch ID, `7f4f6039a4cc948e12664cd78d524c3211bee8c3`. U2 therefore retained Luca's author credit even though the cherry-pick necessarily produced a new commit object.

### U3 — provider failure provenance

| Role | Commit | Attribution on the object | Main ancestry |
|---|---|---|---|
| Reviewed integration on source branch `wg/agent-511/integrate-luca-provider` | `2a440c7141d15f445df7b078c267e1821dd07a93` | author Erik Garrison; `Co-authored-by: Luca Pinello <lucapinello@gmail.com>` and Claude co-author trailer; body records reimplementation of Luca diagnosis `02204b19` | **not** an ancestor |
| Published squash landing | `3027b825c802c10277c5364185afe4c49dbf6b10` | author and committer Erik Garrison `<erik.garrison@gmail.com>`; **no Luca co-author trailer** | ancestor of `main` and `origin/main` |

The source range is based at `8391f46a36be05f6bf06113766bcd76a73a9826c`. The stable patch ID of both `8391f46a..2a440c71` and the U3 delta applied by `76a2d996..3027b825` is `86e3ac50218db0e3d213038977174b6ca4d86497`. The reviewed implementation landed, but its trailers did not.

## Why history is not rewritten

`60407659`, `3027b825`, and `9176849c` are already reachable from published `origin/main` and have many descendants. Replacing either squash would invalidate those descendants and collaborators' refs for a metadata-only repair. This change creates only new descendants. It uses no amend, rebase, reset of `main`, replacement ref, or force push.

## Regression coverage

`src/commands/done.rs` contains repository-backed tests for both lost-attribution shapes:

- a multi-commit source range keeps Luca as the squash author, converts the other source author to one co-author trailer, retains Claude's existing trailer, and deduplicates a repeated Luca identity;
- a single integration commit authored by Erik retains both its Luca and Claude co-author trailers.

Existing push, unavailable-remote, no-remote, conflict, uncommitted-work, and idempotent merge tests continue to exercise the same merge path.

## Scope boundary

This repair changes only Git provenance handling, tests for that handling, and this audit record. It introduces no product behavior, authority model, roster, household workflow, messaging rule, federation rule, provider-health policy, or evaluation policy. The functional U1/U2/U3 audit remains in `docs/reports/validate-integrated-luca-core-2026-07-18.md`.
