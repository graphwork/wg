# WorksGood concierge recovery closeout

**Task:** `closeout-worksg-concierge`

**Recovery branch:** `wg/agent-676/closeout-worksg-concierge`

**Recovery base:** reviewed `main` at `459e030b659c68de9498b51e1363294a3dc46db4`

**Closed over candidate:** 2026-07-24

## Outcome

The preserved concierge was imported rather than reimplemented. The public trial surface is now coherently **WorksGood** / `worksgood`; `wg` remains the full expert CLI. There is no `worksg`, `workg`, `graphwork`, alias, compatibility shim, installer change, or release-path exposure.

The approved boundary remains intact: one reversible profile-first project selection, exact handler-owned routes, no provider/cross-system fallback, generic authenticated absolute-executable safety, explicit **Continue without AI**, strictly non-mutating dry-run, setup-neutral TUI entry, no implicit chat creation, and a detached service that survives TUI exit.

## Authoritative donor identity and preservation

The donor was treated as read-only throughout:

- worktree: `/home/bot/wg/.wg-worktrees/agent-642`
- branch: `wg/agent-642/implement-worksg-concierge`
- HEAD commit: `c4c1e211eda30b548831fdaad02d76317fb18baf`
- HEAD tree: `f494c9b8a57145dac9591f2ecb108704b4247726`
- HEAD author: Erik Garrison `<erik.garrison@gmail.com>`
- tracked working-tree patch: SHA-256 `1bf29bcb72eac0738b14fe09e1bdb7156beeda59cdb66be0f392537c9298d6b6` (`git diff --binary HEAD` over the nine tracked donor paths; 28,955 bytes)
- porcelain-v2 status SHA-256 before and after recovery: `8cb0522e368ad91aacdf3ea17cc942bbd7713ffb40b1dba9d5a9f733a7d83ae6`

The historical task log attributes the substantive uncommitted concierge implementation to `agent-646` at 2026-07-23T09:39:35Z. Later donor executions produced no additional file changes. That attribution, the exact donor branch/HEAD, patch digest, and per-file hashes below preserve the evidence that could not exist as a donor commit.

A final donor `git status --short` was identical to the initial inventory: nine modified files and five untracked files. No donor file was written, renamed, staged, committed, or deleted.

## Complete 14-path donor-to-recovery inventory

Hashes are SHA-256 of file bytes. Renamed recovery paths implement the approved public naming decision; historical task and branch IDs remain unchanged for provenance.

| # | Donor path | Donor SHA-256 | Recovery path | Recovery SHA-256 | Disposition |
|---:|---|---|---|---|---|
| 1 | `Cargo.lock` | `8d707f156a46a6545e05f3fbe2dc677c20ed7044df3072b7782231d664baaee1` | `Cargo.lock` | `8d707f156a46a6545e05f3fbe2dc677c20ed7044df3072b7782231d664baaee1` | exact import |
| 2 | `Cargo.toml` | `e81ba245070095061f8816b0c0f5db8db8d3bef1ae3ff121a01301cb7c19adea` | `Cargo.toml` | `cac8d35b043ebc6675c98162bcbdc23a6e99be73f1b701cc991c0efb495b5938` | imported; binary/path/feature renamed to `worksgood` |
| 3 | `src/commands/profile_cmd.rs` | `246d979bdaa1a19893d3caec65c5d0e1bf4c5bbf01819bc792e3cec5ae9713cc` | same | same | exact import |
| 4 | `src/commands/service/ipc.rs` | `c808cbc5b4948ad56d0b26d9cf2cd20cdca35aadaccd3300c868bc5c5333e34b` | same | `0f9cca3cce6e3c546279ef4a720a5d7232393aaa377a7ede8ac79b2aff3af1bd` | imported; reload handshake now advances exact profile generation |
| 5 | `src/commands/service/mod.rs` | `7c5244a61306beb96c6db959b880bd3567ac3a90362275bc8fb2320f43619c1a` | same | `7aa7a5bfa3cfdb531a90cbe78f67ed5ba31047a524fcfd365002ee7688da43d7` | donor identity additions, reviewed-main changes, and profile-generation status |
| 6 | `src/config.rs` | `7c9d42abadd7ad6df909161578ef24f94577e8c336acac9217799f5a31815aff` | same | same | exact import |
| 7 | `src/lib.rs` | `976c59af86e91d69d5a83a92ae539a5da750bd49fc8097794a065ce4f05afb49` | same | same | exact import |
| 8 | `src/profile/project.rs` | `79dd92c132bad536f1796d110efce0685ce095a26b48e691eb36dcb99f54defe` | same | same | exact import |
| 9 | `tests/smoke/manifest.toml` | `70c852100ac923833bf7d0cf712db21169ad92a5f762c3167f214fe3deb534de` | same | `19e23414c2cdf2dbc2b25f7e3e487397bd057c42dc1b61072da05cf889b2040d` | grow-only merge; renamed/strengthened scenario and retained historical owners |
| 10 | `docs/worksg-concierge-trial.md` | `602ee9edeb14aaa6b8df388e0ccb95beaca38a2667fa791d47c16bbd574b1a1e` | `docs/worksgood-concierge-trial.md` | `c6dcb28b5c31d6f102d27847a914e7b2d21fce0da1e692c221019e0b6ee3f085` | imported/renamed; effort and refined reconcile contract documented |
| 11 | `src/bin/worksg.rs` | `237bbc544a8b332709c0e39c0184c0e9bb4e4f4f5a8195c5b4a94d1e2040aaf6` | `src/bin/worksgood.rs` | `bf4a79664167be8ddaedf952a804aa6ddc25d872ca36ab5bcf20745ed0fbcd90` | imported and publicly renamed |
| 12 | `src/concierge.rs` | `18eb44c83979fdd1917e6f40f62a9d02bea0fc625fe9ff13cf1c0ea2242f6d9b` | same | `18722923af0e5fc233351edf3ae2f821e78c8f9b9da2d6c8ca0b0fc54b41ff29` | imported; naming/effort plus content-build reconcile/refusal refinement |
| 13 | `src/service_identity.rs` | `867b19a7cc45c266347c49ebe698c8475984d7766aa1aae47a5b616675397b56` | same | `bbce5498e6143409337f7576df9cbf3fd03e9ac870d38a67310300f71c80fa14` | imported; profile generation and foreign identity validation added |
| 14 | `tests/smoke/scenarios/worksg_concierge_trial.sh` | `25f579504cf8c0c122b6d7b6f27655ab9f8a4184fed487adb0c32a700b0ccb23` | `tests/smoke/scenarios/worksgood_concierge_trial.sh` | `ee1834f276619c2ade7e8b486540304f726d7f14a0cf09010c662bcf853eb126` | imported, renamed, and tightened to the refined human-flow contract |

## Reconciliation and bounded fixes

### Current-main reconciliation

Between donor HEAD and recovery base, only two donor-touched tracked paths changed on main:

- `src/commands/service/mod.rs`: the three-way import applied cleanly and retained both the donor service-identity fields and the later reviewed-main service work.
- `tests/smoke/manifest.toml`: both sides appended at the former file end. The sole conflict was resolved grow-only by retaining main's `isolation_worktree_collision_transaction` and `retained_worktree_slow_sweep_does_not_starve_dispatch` scenarios, then appending `worksgood_concierge_trial`.

No other donor-vs-main conflict existed.

### Required naming and effort closeout

- Renamed the candidate binary, source path, feature, docs, scenario, help, examples, messages, test sessions, and public test/receipt environment variables to `worksgood` / `WORKSGOOD_*`.
- Kept `wg` as the authenticated sibling/full expert CLI; no alias or compatibility fallback was added.
- Preserved separate explicit Worker/chat and Agency/FLIP/evaluation model selection. Selected core profiles now always materialize separate effort values (defaults `high` / `low`), the immutable plan shows them, and returning runs show both resolved routes and efforts.
- The selected Pi profile was exercised through a real fake-Pi process boundary: argv contained separate `--provider openai-codex --model gpt-5.6-sol --thinking xhigh`, with no reasoning encoded into the model string.

### Refined bare reconcile contract

A late maintainer refinement was implemented in this closeout rather than deferred:

- The service startup/IPC identity now carries canonical graph/digest, PID birth, socket, protocol, absolute executable, SHA-256 content build fingerprint/build ID, effective config fingerprint, and exact selected project-profile name/fingerprint.
- Exact content/protocol/profile/config identity reuses. A different absolute path with identical authenticated bytes is equivalent, preventing copy/hardlink spelling restart loops.
- Compatible-build profile/config/reasoning generation drift reloads and must converge to the expected handshake before TUI.
- Same semantic version with different content, protocol, or build fingerprint requires attended controlled restart; the replacement handshake must converge before TUI.
- Foreign graph/executable shape, state/socket disagreement, unresponsive handshake, or deleted/unverifiable running executable produces `SERVICE_IDENTITY_REFUSED`, signals nothing, and opens no TUI.
- A failed intended build start can restore only an absolute on-disk prior executable whose bytes still equal its startup fingerprint; the command still fails and explicitly reports that stale TUI was not opened.
- Strict dry-run serializes both action and exact reason while remaining byte-stable.

### Demonstrated validation blockers fixed

1. The new Pi-argv assertion initially spawned from an unborn scratch Git repository, so required worktree isolation correctly refused before launching the handler. The harness now creates one seed commit before the spawn probe.
2. Initial mismatch probes based on local config drift were unsuitable because profile overlay and then the live config watcher correctly resolved/reloaded them. The final flow separately proves profile-generation reload, same-byte alias reuse, same-version/different-content restart, and same-path binary replacement.
3. Clippy identified two candidate-only style warnings (`OpenOptions` truncate intent and a needless return). Both were fixed without changing behavior. Final clippy output contains no `src/concierge.rs`, `src/service_identity.rs`, or `src/bin/worksgood.rs` diagnostic.

## Validation matrix

All commands used the isolated target `/home/bot/wg/.wg-candidate-targets/closeout-worksg-concierge`; no `cargo install` was run.

| Check | Command / evidence | Result |
|---|---|---|
| Formatting | `cargo fmt --check` | PASS |
| Candidate build | `cargo build --locked --features worksgood-trial --bin wg --bin worksgood` with isolated `CARGO_TARGET_DIR` | PASS |
| Concierge/reconcile units (post-refinement) | `cargo test --locked --features worksgood-trial --lib concierge -- --test-threads=1` | PASS, 10/10 |
| Identity units (post-refinement) | same, filter `service_identity` | PASS, 2/2 |
| Service binary units (post-refinement) | `cargo test ... --bin wg commands::service::tests -- --test-threads=1` | PASS, 57/57 |
| Project-profile units | same, filter `profile::project::tests` | PASS, 17/17 |
| Dedicated live PTY/tmux flow (post-refinement) | `WG_SMOKE_CANDIDATE_DIR=<isolated-target> bash tests/smoke/scenarios/worksgood_concierge_trial.sh` | PASS |
| Clippy (post-refinement) | `cargo clippy --locked --features worksgood-trial --all-targets` | PASS; existing baseline warnings only, no candidate-file warning |
| Completed full serial suite | worker `WG_*` variables unset; `cargo test --locked --features worksgood-trial -- --test-threads=1` | PASS across 166 test binaries: 9,407 passed, 0 failed, 34 ignored; completed before the late reconcile refinement and retained per maintainer direction, then superseded for affected surfaces by the post-refinement focused/service/live matrix above |
| Public-name scan | forbidden `worksg`/`WORKSG_*` scan over imported/public paths | PASS; only historical task IDs remain in manifest owners |
| Donor preservation | before/after status and tracked-patch digests | PASS; byte/status identity unchanged |
| Global install guard | `command -v worksgood` | absent; candidate exists only at the path below |

The final dedicated live flow proves:

- first-run setup and returning-run exact PID reuse;
- explicit high/low effort persistence, returning display, and real Pi `--thinking` argv;
- exact-build reuse and same-byte absolute alias reuse without restart loops;
- profile/config/reasoning generation reload on a compatible build;
- same-version/different-content and same-path replaced-binary diff, attended confirmation, restart, and verified handshake;
- deleted running executable and foreign graph identity refuse without signal or TUI;
- failed replacement restores only the authenticated prior build and never opens stale TUI;
- **Continue without AI** with no service;
- byte-stable help, non-TTY refusal, cancel, and strict reconcile dry-run with exact reasons;
- no chat row created by opening either concierge/TUI path;
- service remains live after TUI exit;
- absolute repository, graph, executable, socket, PID-birth, content build, protocol, selected-profile generation, and config identity;
- stale repair, concurrent TUI clients/one daemon, explicit restart, and graceful stop.

Durable command/result hashes are listed in `docs/reports/worksgood-concierge-closeout-validation.log`.

## Candidate executable

- `worksgood`: `/home/bot/wg/.wg-candidate-targets/closeout-worksg-concierge/debug/worksgood`
- SHA-256: `de5cc02d941aa71c13008541187465af5f4f9161f69ac3d7913e13ee096163e5`
- authenticated sibling `wg`: `/home/bot/wg/.wg-candidate-targets/closeout-worksg-concierge/debug/wg`
- sibling SHA-256: `e57865b11c8587e8817f5c3b36296776040253c319ada095f0892a3bc761febd`

This is an uninstalled review artifact. The candidate feature remains non-default and is absent from installer/release surfaces.

## Remaining limitations / trial boundary

- Real provider credentials and remote endpoint availability were intentionally not exercised; the dedicated flow is credential-free and uses Pi-owned catalog-shaped data plus a fake Pi argv sink.
- Handler authentication, Pi's model registry, and plugin ownership remain with their existing owners; WorksGood does not infer, substitute, or fall back across systems.
- The candidate does not add WireGuard-specific behavior, package-manager changes, release artifacts, aliases, or a full CLI rename.
- Rollback remains deliberately bounded: it can reverse only the still-matching project association and a service started by the pending transaction; graph initialization and handler-owned auth/plugin state are preserved.
- The isolated candidate target is retained at the reported path for maintainer review and can be deleted without affecting the repository or globally installed `wg`.
