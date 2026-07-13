# Post Review Requests Audit - 2026-07-11

Task: `post-review-requests`

Repository: `graphwork/wg`

Scope: refreshed Luca Pinello PRs #49-#57 and posted only GitHub comments/reviews
authorized by the task. No branches were pushed, merged, closed, or otherwise
modified.

## Refreshed PR State

- #49 `7010e69117f362768bf8cafc69fc22eb0ffa3e65`: open, unstable; stable jobs
  green, `Build & Test (nightly)` failing at `cargo test`, run
  `29106848961`, job `86409344900`.
- #50 `842cdf94038d13141f8c3c167aef75d9094bbafd`: open, clean, all checks
  green. No comment needed for this task.
- #51 `f17867a8b3e89d87d34e478077ac0391df01b587`: open, clean, all checks
  green; still affected by sender-to-confirmed-binding and #49 coordination
  finding.
- #52 `85281b4939d746c96785d220cabbfd19267020ea`: open, clean, all checks
  green. No comment needed for this task.
- #53 `040325282048bc13fa3b31833933237d329a611b`: open, clean, all checks
  green. No comment needed for this task.
- #54 `43755390842fcb8cefff82445b1aadf7371dd76f`: open, clean, all checks
  green; still affected by the `WG_DELIVERABLE_PREFLIGHT_OVERRIDE` bypass.
- #55 `019b470f1bb6b776d3f3d63b9b6f34a21131d53d`: open, unstable; stable jobs
  green, `Build & Test (nightly)` failing at `cargo test`, run
  `29133597339`, job `86493568720`.
- #56 `c7950a28c4a7404042f327bd38453f7de3a6e7ea`: open, clean, all checks
  green; still affected by untagged durable `wg add` from disposable scope.
- #57 `87233bb9d4be45457f33272fa4796da23c27b640`: open, unstable;
  `Build & Test (stable)` failing at `cargo test (unit tests)`, run
  `29137074236`, job `86503262216`; still affected by missing per-command
  validation timeouts.

## Duplicate Check

Before posting, `gh pr view --json comments,reviews` returned no existing
comments or reviews on #49, #51, #54, #55, #56, or #57. Therefore all required
feedback was posted once.

## Posted Links

- #49 nightly CI comment:
  https://github.com/graphwork/wg/pull/49#issuecomment-4947096213
- #51 request-changes review:
  https://github.com/graphwork/wg/pull/51#pullrequestreview-4678151476
- #54 request-changes review:
  https://github.com/graphwork/wg/pull/54#pullrequestreview-4678151473
- #55 nightly CI comment:
  https://github.com/graphwork/wg/pull/55#issuecomment-4947096215
- #56 request-changes review:
  https://github.com/graphwork/wg/pull/56#pullrequestreview-4678151480
- #57 request-changes review:
  https://github.com/graphwork/wg/pull/57#pullrequestreview-4678151479

## Validation

- Current PR head/check state refreshed before commenting.
- Required feedback posted once on every still-affected PR.
- No branches pushed and no PRs merged or closed.
- Exact GitHub links recorded above.
