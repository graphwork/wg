# Upstream Pi patch packages

This directory contains ready-to-submit patches for
[`earendil-works/pi`](https://github.com/earendil-works/pi). They touch no WG
runtime source. Each patch's directory or companion document states its own
gating and validation status.

`PI_NO_TUI` is the original optional Phase-5 patch described below. The
production bug fix under `output-guard-epipe/` makes JSON/RPC output tolerate a
consumer closing stdout and is independently applicable.

## Contents

| File | What it is |
|------|------------|
| `PI_NO_TUI.patch` | The change as a unified diff against `packages/coding-agent/src/main.ts`. Applies with `git apply` / `patch -p1`. |
| `PR.md` | The PR title + description (harness use case, minimal-surface argument, behavior matrix, suggested commit message). |
| `tui_guard_test.mjs` | Standalone truth-table test of the patched logic — `node tui_guard_test.mjs`, 11/11 PASS. |
| `output-guard-epipe/` | Pi 0.80.6 source patch, focused tests, executable source snapshot, and validation record for graceful closed-reader `EPIPE` handling. |

## Provenance

- Patch was generated against the **real upstream source**, fetched from
  `earendil-works/pi` → `packages/coding-agent/src/main.ts`
  (`resolveAppMode` at L98–109 at fetch time; npm package version line
  `@earendil-works/pi-coding-agent@0.79.x`).
- Upstream `main.ts` SHA-256 the diff was cut against:
  `f9993c7c44d2a22ac91847ea50ff8c6f88d4ac608b17731d188ae8317d9a4e1b`.
- The patch reuses pi's **own** `isTruthyEnvFlag()` helper (already used in
  `main.ts` for `PI_OFFLINE` and `PI_STARTUP_BENCHMARK`), so `PI_NO_TUI=0` /
  `false` are correctly treated as off — a small upstream-idiomatic refinement
  over the bare `process.env.PI_NO_TUI` truthiness check sketched in
  `docs/pi-integration/integration-plan.md` §3 / `executor-research.md` §4.2.
  Behavior for the set/unset cases is identical to the design spec; the helper
  only additionally rejects explicit falsey strings.

## How to apply / submit

```bash
# in a clean checkout of earendil-works/pi (or a fork)
git checkout -b pi-no-tui-escape-hatch
git apply /path/to/PI_NO_TUI.patch        # or: patch -p1 < PI_NO_TUI.patch
node /path/to/tui_guard_test.mjs          # sanity: 11/11 PASS
# build/format per pi's repo conventions, then open a PR using PR.md as the body
```

If the upstream source has drifted since this was cut (line numbers or
surrounding context), re-fetch `packages/coding-agent/src/main.ts` and re-cut
the one-hunk diff — the change itself is a fixed 7-line insertion at the top of
`resolveAppMode` and is trivial to re-apply by hand from `PR.md`.

## What the patch does (one paragraph)

`resolveAppMode` is the single function that decides whether pi runs its
full-screen interactive TUI (which grabs the terminal via raw mode). It returns
`interactive` **iff** no `--mode`/`-p` flag was passed **and** both stdin and
stdout are TTYs. This patch inserts a guard *above* the existing branch order:
if `PI_NO_TUI` is truthy **and** no mode was requested **and** `--print` was not
passed, return `print` instead. Net effect: a supervising harness can force pi
headless from the environment even under a both-TTY PTY, without editing argv —
while every existing path (explicit `--mode rpc`/`json`, `-p`, the
unset-env default) is byte-for-byte unchanged.

## Gating / status (read before submitting)

This patch is **Phase 5 (P5)** of the pi.dev integration plan
(`docs/pi-integration/integration-plan.md` §3) and is explicitly **optional and
belt-and-suspenders**:

- WG's terminal-takeover problem is **already fully solved from WG's side** with
  two flags (`--mode rpc` for chat, `-p`/`--mode json` for workers) plus piped
  stdio. That wrapper carries **zero** upstream dependency, which is the
  preferred posture (`executor-research.md` §4.1).
- This patch only matters **if** WG later pursues **Shape B** — embedding pi's
  *real* TUI through a both-TTY PTY *and* needing an external kill switch for the
  grab (open question #2 in the integration plan, §6).
- Per the plan, **do not submit this PR** unless Shape B is actually in scope.
  If Shape B is deferred, §3 stays documentation-only and this package remains a
  prepared-but-unsubmitted deliverable. Submitting it early would put a (tiny,
  but nonzero) maintenance ask on the pi maintainers for a feature WG isn't yet
  using.

In short: the patch is **prepared and verified**; the decision to open the PR is
a human, Shape-B-scoped call (handled by the downstream `.flip` gate).
