# `worksgood` profile-first concierge trial

**Status:** isolated candidate only. This is not a full CLI rename, a release artifact, an installer change, or an alias for `wg`.

The complete advanced WorksGood CLI remains `wg`. This candidate is only a narrow lifecycle facade and does not install an alias or change existing command ownership.

## Build the candidate without installing it

Use one bounded target outside normal release/install paths:

```sh
CARGO_HOME="$(mktemp -d)" \
CARGO_TARGET_DIR=/tmp/worksgood-trial-target \
  cargo build --locked --features worksgood-trial --bin wg --bin worksgood

/tmp/worksgood-trial-target/debug/worksgood --help
```

`worksgood-trial` is not a default feature, and the existing installer/release configuration is unchanged. Do **not** run `cargo install` for an unmerged candidate. Remove the target and temporary `CARGO_HOME` after the trial.

The candidate resolves the physical `worksgood` executable and its sibling `wg` from that same isolated Cargo bundle. It never searches for, probes, or executes `wg` through `PATH`, `which`, `command -v`, a shell string, or a basename. A non-sibling WorksGood executable is accepted only with an absolute `WORKSGOOD_W_RECEIPT` JSON file binding `product`, canonical `executable`, and `sha256`; symlink candidates are refused.

## Lifecycle

In an attended terminal:

```text
worksgood              first run or returning fast path, then TUI
worksgood setup        profile selection/setup or resume; no TUI
worksgood setup --rollback
                       clear an uncommitted failed setup's exact selection/service effect
worksgood status       strictly read-only identity/readiness status
worksgood stop         graceful daemon stop; detached work is not killed
worksgood restart      explicit warning + confirmation, then authenticated restart
worksgood tui          setup-neutral existing TUI only
```

Bare non-TTY use fails with stable `ATTENDED_TTY_REQUIRED` and mutates nothing. `--dry-run` prints one immutable redacted plan and writes no graph, profile, history, journal, plugin/cache, service, or TUI state. The choice prompt defaults to cancel. The primary no-provider choice is **Continue without AI**; it selects no LLM route, runs no LLM service, and opens only the setup-neutral TUI.

The profile list reuses the project-profile catalog: current-project selection first, created reusable profiles before built-ins, quiet local frequency labels, exact Worker/chat and Agency/FLIP/evaluation routes, and honest handler-owned readiness. Core integrated choices are Pi/pi-codex, Codex, Claude, Nex/local, and OpenCode; specialized adapters remain in the advanced `wg` surface.

For Pi, the WorksGood picker asks Pi's own authenticated RPC model registry (`get_available_models`) and retains a manual exact-ID fallback. Worker/chat and Agency/FLIP/evaluation model plus effort are separate explicit choices. **Same as worker** is an explicit option and is never inferred. Every selected core profile persists explicit Worker/chat effort (default `high`) and Agency/FLIP/evaluation effort (default `low`) in a content-addressed reusable project profile; returning runs show both resolved routes and efforts. Pi maps those resolved values to its real `--thinking <level>` argv separately from model identity. Pi remains the auth owner; WorksGood only prepares its version-matched plugin after confirmation.

## Service reconciliation and identity

The concierge composes existing `wg init`, project-profile APIs, Pi plugin owner, `wg service`, and `wg tui`; it does not duplicate their config/process/TUI authority.

A healthy service is reused only when all of the following agree:

- canonical graph and graph digest;
- an authenticated absolute executable identity and stable SHA-256 **content build fingerprint** (not semantic version, pathname, inode, size, or mtime alone); identical-byte absolute aliases are equivalent and do not create restart loops;
- service identity protocol/compatibility identity;
- exact selected project-profile generation and effective merged-config/reasoning fingerprint;
- PID birth identity;
- exact project socket;
- state-file identity and live socket handshake identity.

Down starts and verifies; proven-dead PID state repairs then starts; compatible-build profile/config/reasoning generation changes reload and verify; binary content/build/protocol mismatch shows actual versus intended identity, confirms a controlled restart, and verifies the replacement before TUI. Same `0.1.0` version text never masks different bytes. An exact healthy match reuses. A foreign graph/executable identity, malformed/state-vs-socket mismatch, unresponsive handshake, or deleted/unverifiable running executable fails loudly **without signalling anything and without opening TUI**. A failed replacement may restore only an on-disk prior executable whose absolute path and startup content fingerprint still authenticate; stale TUI is never opened. Strict dry-run prints the action and exact reason without writes. Restart/stop never request `--kill-agents`; detached workers, agency one-shots, chats, and PTYs remain independent. Concurrent `worksgood` clients serialize only setup/reconcile, then open independent TUI clients against one daemon.

On TUI exit the service stays detached and the candidate prints concise re-entry, status, stop, setup, and TUI-only guidance. Continue-without-AI prints that no LLM service is running.

## Repository boundary and rollback limits

Resolution stops at the nearest physical Git repository/worktree root, including nested repos and `.git` worktree files. There is no `~/.wg` fallback. A legacy `.workgraph` blocks creation of a competing `.wg`. Dirty repositories are never committed, stashed, reset, or cleaned.

The one confirmed transaction initializes a missing graph first, writes a redacted `.wg/concierge-pending.json` recovery marker, prepares the selected integration, applies a preimage-guarded project profile, validates the exact route, reconciles the service, and atomically commits `.wg/concierge.json` before removing the marker and opening the TUI. Generated reusable profiles use create-new atomic writes; association failure removes only the exact generated bytes. External handler credentials remain owned by that handler and are never rollback targets.

If prerequisite/profile preparation fails after graph init, the graph and recovery marker are preserved and `worksgood setup` can resume with a new immutable plan. `worksgood setup --rollback` shows and confirms a bounded rollback: it clears only the still-matching project association and stops only a daemon started from that pending transaction's initially-down service state. It preserves initialized graph/agency files, generated reusable definitions, and handler-owned auth/plugin state; changed or already-committed state fails closed.
