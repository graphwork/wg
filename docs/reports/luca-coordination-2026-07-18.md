# Luca coordination: upstream batches and Casa federation boundary

**Date:** 2026-07-18

**Task:** `engage-luca-on`

**Posting identity:** `ekg` (Erik Garrison), authenticated GitHub maintainer

## Verification immediately before posting

The posts were guarded by live GitHub API/ref checks immediately before creation. Posting aborted unless all expected values matched.

| Surface | Verified state |
|---|---|
| Upstream `graphwork/wg` `main` | `8391f46a36be05f6bf06113766bcd76a73a9826c` |
| Upstream PR [#57](https://github.com/graphwork/wg/pull/57) | Open, head `6509e4cf553b0deeb6b11f17b7f3958a4b5bf045`, `CHANGES_REQUESTED`; no revised head after the existing browser-escape review |
| Casa integration branch | [`64f2cba83bf91e4e0203f09957e655ff635226ed`](https://github.com/lucapinello/workgraph/commit/64f2cba83bf91e4e0203f09957e655ff635226ed) |
| Fork PR [#2](https://github.com/lucapinello/workgraph/pull/2) | Closed, unmerged, head `78e099787b3ec9bbf889b793e542d552de46aca2`; closure note says gateway-side `claw3d-bridge` `ledger.mjs` superseded it |
| Proposed core candidates | [`9a595528`](https://github.com/lucapinello/workgraph/commit/9a5955288dc8419f2efff10986cfe25d8f3bc1a9), [`02204b19`](https://github.com/lucapinello/workgraph/commit/02204b19ce5d9f15a7706febcda43cd67a84d9a0), and [`d25cdd59`](https://github.com/lucapinello/workgraph/commit/d25cdd594dba4789eba310891cc2caed2ce1975d) all resolved |
| Public planning links | The exact-revision [core batch report](https://github.com/graphwork/wg/blob/8391f46a36be05f6bf06113766bcd76a73a9826c/docs/reports/luca-core-fix-batches-2026-07-18.md) and [federation overlap report](https://github.com/graphwork/wg/blob/9e33da3e1a7dc26ef66e5d552529ad88ea0dc6f2/docs/reports/luca-casa-wgfed-overlap-2026-07-18.md) both resolved |

The fork has issues and discussions disabled, so the broader coordination went to a clearly scoped upstream issue. PR #57 received only its exact outstanding review request. Both posts were made with `GH_BROWSER` and `BROWSER` unset. No approval, merge, branch mutation, or promise of upstream inclusion was made.

## Broader coordination post

**URL:** https://github.com/graphwork/wg/issues/58

**Created:** 2026-07-18T10:03:28Z

**Title:** `Coordination: Casa upstream split and federation adapter boundary`

Exact posted body:

> @lucapinello — I reviewed the refreshed Casa stream through [`integration/casa-pinello` at `64f2cba8`](https://github.com/lucapinello/workgraph/commit/64f2cba83bf91e4e0203f09957e655ff635226ed). It is a broad, coherent product effort, and I have not previously given you a clear upstream-vs-Casa coordination plan. Sorry for that gap.
>
> Could you confirm which current changes you intend for upstream WG and which are Casa-only? I do not want to ask for the whole integration history or mix household product policy into unrelated core reviews.
>
> My proposed dependency-aware upstream batches are:
>
> 1. **Eval rescue lifecycle:** preserve [`9a595528`](https://github.com/lucapinello/workgraph/commit/9a5955288dc8419f2efff10986cfe25d8f3bc1a9) with attribution, then complete persisted handler-first route, pipeline, verdict, CAS, and repair semantics. The predicate fix alone is a safety slice, not the full lifecycle fix.
> 2. **Provider-failure provenance:** retain the diagnosis/tests from [`02204b19`](https://github.com/lucapinello/workgraph/commit/02204b19ce5d9f15a7706febcda43cd67a84d9a0), but classify structured completion outcomes and handler/wire/endpoint identity rather than free-form `wg done` text.
> 3. **Detached background jobs:** preserve [`d25cdd59`](https://github.com/lucapinello/workgraph/commit/d25cdd594dba4789eba310891cc2caed2ce1975d) as the portable-launch baseline, then add process-group/tree cleanup, safe redirection and persisted identity, and explicit Windows behavior.
>
> Proposed order: the eval predicate slice, provider provenance, then the remaining eval lifecycle; detached jobs can proceed independently. [Detailed current-main batch plan](https://github.com/graphwork/wg/blob/8391f46a36be05f6bf06113766bcd76a73a9826c/docs/reports/luca-core-fix-batches-2026-07-18.md).
>
> For Casa, the proposed boundary is: WG-Fed owns principals/signatures/recipient ACLs/UCANs; WG-Review gates authenticated inbound before election, composition, task creation, or fast writes; WG-Exec owns remote grant/lease/result acceptance. Casa keeps roster/persona/election/conversation/feed/ledger/report-back policy as an adapter. Shared secrets, nonces, Telegram IDs, sender labels, and `srcId` remain channel evidence or projection keys, not a second trust root. The two generic seams worth designing once in WG are signed channel binding/assertion and a durable delivery outbox/receipt. [Detailed overlap and adapter study](https://github.com/graphwork/wg/blob/9e33da3e1a7dc26ef66e5d552529ad88ea0dc6f2/docs/reports/luca-casa-wgfed-overlap-2026-07-18.md).
>
> Finally, [fork PR #2](https://github.com/lucapinello/workgraph/pull/2) is closed at `78e09978` with your note that `claw3d-bridge`'s `ledger.mjs` superseded it. Is that still the current direction, and is there a newer reviewable branch/repo/PR for that gateway work or anything else superseding `64f2cba8`?
>
> If this split looks right, please reply with the upstream/Casa-only classification and links you want us to use. We can then work in small, reviewable branches rather than a single catch-all request.

## PR #57 post

**URL:** https://github.com/graphwork/wg/pull/57#issuecomment-5010848333

**Created:** 2026-07-18T10:03:29Z

Exact posted body:

> Thanks, Luca. I rechecked the exact PR state for this coordination pass: the head is still `6509e4cf553b0deeb6b11f17b7f3958a4b5bf045`, unchanged since the last review.
>
> The remaining blocker is still narrow: `gh pr view|list --web` / `-w` can invoke an inherited `GH_BROWSER` or `BROWSER` executable, which may escape the validation command's timeout/process-group cleanup. Please push a revised head with explicit read-only option allowlists for those `gh` subcommands and the public-path no-side-effect proof-browser regression requested in the review, or confirm if you prefer a different direction for this PR.
>
> I will not poll the unchanged head again; please ping when there is a revision or a direction decision. I am not approving or merging this head.

## Post-write verification

The GitHub API returned both posts with author `ekg`; their returned bodies matched the submitted text (apart from the API extraction adding a final display newline). After posting:

- PR #57 remained open at exact head `6509e4cf553b0deeb6b11f17b7f3958a4b5bf045`, with review decision `CHANGES_REQUESTED`.
- Fork PR #2 remained closed at exact head `78e099787b3ec9bbf889b793e542d552de46aca2`.
- `integration/casa-pinello` remained at `64f2cba83bf91e4e0203f09957e655ff635226ed`.
- Upstream `main` remained at `8391f46a36be05f6bf06113766bcd76a73a9826c`.
