# `worksgood` lifecycle concierge

**Status:** shipped by default alongside `wg` and `nex` on every supported platform.

`worksgood` is the attended human lifecycle surface. The complete expert task/tool CLI remains `wg`; this is not a full CLI rename, does not create a `worksg` alias, and does not change the `wg_*` protocol used by agent integrations.

## Install

A normal source install places all three commands in the same prefix:

```sh
cargo install --git https://github.com/graphwork/wg --locked
worksgood --help
wg --help
nex --help
```

The [native installers](guides/install.md) install the same binary set without requiring Rust. Existing receipt-owned installs upgrade and uninstall the set together; foreign commands are never overwritten.

The concierge resolves the physical `worksgood` executable and its sibling `wg` from that same installed bundle. It never searches for, probes, or executes `wg` through `PATH`, `which`, `command -v`, a shell string, or a basename. A non-sibling WorksGood executable is accepted only with an absolute `WORKSGOOD_W_RECEIPT` JSON file binding `product`, canonical `executable`, and `sha256`; symlink candidates are refused.

## Two deliberately separate paths

### Attended Pi chat (default)

In an attended terminal:

```sh
worksgood
```

Bare `worksgood` verifies the `pi` executable, ensures the compatible embedded WG plugin, initializes a route-free graph with no agency when needed, and opens the existing TUI. Choose **New chat → Pi**. The attended process receives only Pi session-lifecycle metadata; there is no WG `--provider`, `--model`, or `--thinking` override. Pi owns login, provider/model selection, and later model switching through its own UI.

This path does not read, select, create, or rewrite a named profile; it does not require worker/evaluator routes or reasoning; it does not inspect agency readiness; and it does not start, reconcile, or authenticate a dispatcher service. Existing repository automation settings remain untouched. A model change reported by the attended Pi plugin is scoped to that chat, not written back to worker/evaluator configuration. Missing Pi produces one install/login action before any model, profile, or service state is written.

Use `worksgood --without-ai` to initialize/open a graph without checking Pi. `worksgood tui` opens an existing graph without setup. Bare non-TTY use fails with stable `ATTENDED_TTY_REQUIRED` and mutates nothing; bare `--dry-run` reports the route-free attended plan and writes nothing.

### Unattended workers and evaluation (advanced)

Repository-wide automation is an explicit separate operation:

```sh
worksgood setup                         # interactive advanced setup; no TUI
M=pi:openrouter:deepseek/deepseek-v4-flash
worksgood setup --model "$M"             # exact one-model automation setup
worksgood --model "$M"                   # compatibility form: setup/reconcile, then TUI
```

These settings govern unattended dispatch only. They do **not** select or constrain the model a human chooses in an attended Pi chat. `--model` accepts only the exact handler-first shape `pi:<provider>:<model>` and never rewrites, infers, or falls back to another route. It copies that exact value to every unattended LLM role. Worker reasoning defaults to `high` and Eval/assign/FLIP reasoning defaults to `low`; `--strong-reasoning` and `--weak-reasoning` override those dimensions independently. Reasoning remains structured, content-addressed automation/service identity and is never encoded into the model route.

The generated automation profile is built from the bundled clean Pi starter, content-addressed, reusable, and selected only for the project. This advanced path retains fail-closed exact-route/effective-reasoning validation and starts/reloads only the authenticated paired service. `--profile`, `--strong-model`, and `--weak-model` remain available for the explicit two-route flow. **Same as worker** must be chosen explicitly and is never inferred.

Other lifecycle forms:

```text
worksgood setup --rollback
                       clear an uncommitted failed setup's exact selection/service effect
worksgood status       read-only automation/service identity status
worksgood stop         graceful daemon stop; detached work is not killed
worksgood restart      explicit warning + confirmation, then authenticated restart
worksgood tui          existing graph/TUI only; no setup or reconcile
```

An attended chat can optionally pin an exact model through the TUI's New-chat model editor. That pin belongs to the chat. It does not mutate repository automation routes.

## Service reconciliation and identity

The default attended path composes `wg init --no-agency`, the Pi plugin owner, and `wg tui`; chat creation/input use the existing bare-Pi PTY transport. Only explicit automation setup composes project-profile APIs and `wg service`. The concierge does not duplicate config/process/TUI authority.

A healthy service is reused only when all of the following agree:

- canonical graph and graph digest;
- an authenticated absolute executable identity and stable SHA-256 **content build fingerprint** (not semantic version, pathname, inode, size, or mtime alone); identical-byte absolute aliases are equivalent and do not create restart loops;
- service identity protocol/compatibility identity;
- exact selected project-profile generation and effective merged-config/reasoning fingerprint;
- PID birth identity;
- exact project socket;
- state-file identity and live socket handshake identity.

Down starts and verifies; proven-dead PID state repairs then starts; compatible-build profile/config/reasoning generation changes reload and verify; binary content/build/protocol mismatch shows actual versus intended identity, confirms a controlled restart, and verifies the replacement before TUI. Same `0.1.0` version text never masks different bytes. An exact healthy match reuses. A foreign graph/executable identity, malformed/state-vs-socket mismatch, unresponsive handshake, or deleted/unverifiable running executable fails loudly **without signalling anything and without opening TUI**. A failed replacement may restore only an on-disk prior executable whose absolute path and startup content fingerprint still authenticate; stale TUI is never opened. Strict dry-run prints the action and exact reason without writes. Restart/stop never request `--kill-agents`; detached workers, agency one-shots, chats, and PTYs remain independent. Automation setup/reconcile is guarded by the project lifecycle lock and service identity handshake.

On default attended TUI exit, the concierge states that no automation route or service was changed and points to `worksgood setup` only as an advanced action. When an explicit automation invocation opens the TUI, its authenticated service stays detached and the lifecycle guidance includes status/stop.

## Repository boundary and rollback limits

Resolution stops at the nearest physical Git repository/worktree root, including nested repos and `.git` worktree files. There is no `~/.wg` fallback. A legacy `.workgraph` blocks creation of a competing `.wg`. Dirty repositories are never committed, stashed, reset, or cleaned.

The following transaction applies only to explicit automation setup. It initializes a missing graph first, writes a redacted `.wg/concierge-pending.json` recovery marker, prepares the selected integration, applies a preimage-guarded project profile, validates the exact route and reasoning, reconciles the service, and atomically commits `.wg/concierge.json` before removing the marker and optionally opening the TUI. Generated reusable profiles use create-new atomic writes; association failure removes only the exact generated bytes. External handler credentials remain owned by that handler and are never rollback targets.

If prerequisite/profile preparation fails after graph init, the graph and recovery marker are preserved and `worksgood setup` can resume with a new immutable plan. `worksgood setup --rollback` shows and confirms a bounded rollback: it clears only the still-matching project association and stops only a daemon started from that pending transaction's initially-down service state. It preserves initialized graph/agency files, generated reusable definitions, and handler-owned auth/plugin state; changed or already-committed state fails closed.
