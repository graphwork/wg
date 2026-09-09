# Restore main CI completion canary — validation evidence

Task: `restore-main-ci-canary`

## Baseline reproduction

Baseline `4890d7d63681e9a863b6826e9242648c762feb9e` was checked out directly. The exact CI command

```sh
cargo test --locked --bin wg commands::completion_canary_tests
```

exited 101 during compilation with seven `E0425` diagnostics at `src/commands/profile_cmd.rs:2240-2285`, all for `parse_profile_use_target`. Commit `27e802d9` intentionally removed that global/model-qualified activation parser when `profile use` became an alias of the project-local, copy-by-value profile API, but five tests originating in `657a3d6c` still called the deleted helper (three of those tests made seven calls total).

Baseline reproduction log SHA-256: `cfd13d82697e73477e31426ae529d31393d6121497d9495667f98432b468b8fa`. The exact seven compiler diagnostics and capture identity are preserved in `docs/reports/restore-main-ci-canary-base-repro.log`.

## Focused repair

The obsolete parser tests now cover the supported contract instead:

- closed Pi projection materializes into project `worksgood.toml`;
- no global WG config, legacy active-profile, or Pi console mutation is reported;
- explicit non-Pi profile routes are rejected;
- profile names are literal, not the removed `<profile>:<model>` syntax;
- invalid/path-like profile names are rejected.

The CLI integration suite additionally drives these positive and negative cases through `CARGO_BIN_EXE_wg` in isolated `HOME` and project directories. Its existing explicit `--global` test continues to prove that non-routing legacy writes warn while all global routing rewrites fail without mutation.

After compilation was restored, the canary exposed a real stale-fixture failure: its synthetic tasks had no current attempt ID or positive fence, so current genuine-FLIP validation correctly downgraded their scripted semantic verdicts to `IncompleteEvidence`. Concurrent first use also raced graph-identity bootstrap. The canary now gives each candidate a real bound lifecycle attempt and establishes the graph identity before the ten concurrent submissions. Its outcome assertions are unchanged and diagnostics now include immutable findings on mismatch.

## Candidate validation

All commands used the pinned toolchain, the checkout's explicit Cargo candidate binary, and project-local temporary directories. No global install, daemon restart, or `origin/main` push occurred.

| Command | Result |
|---|---|
| `cargo test --locked --bin wg commands::completion_canary_tests` | PASS: 1 passed, 0 failed, 1 explicitly ignored opt-in live-provider test, 3835 filtered out |
| `cargo test --locked --bin wg commands::profile_cmd::tests::test_project_profile` | PASS: 4 passed, 0 failed |
| `cargo test --locked --test integration_project_local_pi_cli` | PASS: 6 passed, 0 failed; includes real CLI flows and the existing PTY setup flow |
| `cargo fmt` | PASS |
| `cargo fmt --check` | PASS |
| `cargo clippy` | PASS (exit 0; pre-existing warnings remain advisory) |
| `git diff --check` | PASS |

The exact canary wrote `worker-owned-completion-canary.json`; the checked-in byte-for-byte copy is `docs/reports/restore-main-ci-canary-evidence.json`, SHA-256 `048a27c5c6a7d99a885c962e244424c4a1a414345e81e3e7bb08ff190312107e`. It records all ten asserted outcomes: six accepted and Done, one FLIP rejection, one eval rejection, one unavailable review, one incomplete-evidence refusal, and no legacy finalization/save-transaction authority. Exact command identities, exit codes, capture hashes, and terminal result lines for every requested candidate check are preserved in `docs/reports/restore-main-ci-canary-validation.log`.

Actual GitHub CI for the integrated commit remains the attended operator's post-publication check.
