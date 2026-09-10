# `fix-smoke-pi-process-leaks` validation evidence

This receipt preserves historical diagnostic and validation facts for the
smoke-process ownership repair. It is candidate input, not a new completion or
publication gate. The authoritative deterministic validation is host-captured
from the settled submitted candidate; this document does not attempt to name
its own containing commit.

## Repair boundary and pre-repair diagnostic

The process-ownership implementation was retained from commit `5cc76e392`. Before
changing the integration fixtures, the same four failing suites were reproduced
against both revisions below with the binary path pinned explicitly:

| revision | source tree | explicit `CARGO_BIN_EXE_wg` | result |
| --- | --- | --- | --- |
| integrated `main` `8b49ec1b` | `/tmp/wg-fix-smoke-baseline-agent89` | `/home/bot/.cache/wg/baseline-agent89-target/debug/wg` | same four suites failed |
| retained candidate `5cc76e392` | this worktree | the worktree's isolated Cargo target `debug/wg` | same four suites failed |

The command in each tree was:

```text
cargo test --locked \
  --test integration_e2e_smoke \
  --test integration_messaging \
  --test integration_smoke_gate \
  --test integration_triage_smoke \
  -- --test-threads=1
```

This established that the ownership-cleanup implementation introduced none of
the four suite failures. The failures had two pre-existing causes:

1. **Environment-dependent fixture leakage.** Disposable `wg --dir <temp>` CLI
   children inherited the validating worker's `WG_GRAPH_ID`, `WG_TASK_ID`,
   `WG_AGENT_ID`, `WG_WORKER_*`, and related control variables. Cross-graph
   rejection was the correct authority behavior. The repair boundary is a new
   test-only `tests/common/isolated_cli.rs` command constructor: it uses
   `env_clear()`, restores only a minimal host-tool/runtime allowlist, and
   positively asserts that no `WG_*` authority reaches the child.
2. **Obsolete lifecycle assumptions.** Synthetic fixtures still expected direct
   worker `done` and the retired `--skip-smoke` bypass. Completion is now
   publication-derived. Synthetic recovery therefore uses the current,
   reason-bearing operator-accept path. Tests continue to prove that worker
   attempts at operator authority are rejected and that legacy bypass flags are
   rejected.

No production authority check was relaxed. Negative coverage remains for failed
prerequisites, failed repair retry, the four-round triage budget, legacy bypass
flags, and worker/operator authority separation.

The complete pre-repair logs were registered on the WG task as artifacts. Their
SHA-256 digests are:

```text
main-four-suites-ambient.log      9cebe89a7a2b0d7e2bf2f6685432eb6366b7c071068da6a364077937c656f63c
candidate-four-suites-ambient.log cbbfc45ee97f43f8248ff1addbf8699c8f00c244634a0d7b7941118df8c28f16
```

## Historical implementation validation

Implementation revision `965e5be2413f5bab43d0aec2f000d5f1841964a0` was the
source boundary used for the validation facts recorded below. It is not asserted
to be the current submitted candidate. The exact current candidate revision and
its expanded authoritative gate are bound by the host completion manifest after
this report is committed, avoiding a self-referential commit claim.

At that historical implementation revision, the broad gate included the real,
credential-free `smoke_process_ownership_cleanup.sh` entry point under an
explicit candidate binary; its recorded test name was
`smoke_process_ownership_cleanup_real_entry_point`.

```text
$ cargo test --locked smoke --no-fail-fast
exit=0
log sha256=7172bf09daeb1ff94814ef8aef013a694c7db31e8795703e47d85622890069a1
...
test smoke_process_ownership_cleanup_real_entry_point ... ok
```

The focused cleanup/unit and required lint checks were then run together against
that exact source commit:

```text
$ cargo fmt
$ cargo fmt --check
$ cargo test --locked --lib smoke::tests::
test result: ok. 14 passed; 0 failed; 0 ignored
$ cargo clippy --locked --all-targets
$ git diff --check
exit=0
log sha256=4eef1f432f4b3c0a02e010264291da3efe57b5b0625d0e029d3fc93242019ca8
```

The clippy output contains repository-existing warnings but no clippy error; the
command exited zero. The complete log is registered as the WG task artifact
`/tmp/wg-fix-smoke-validation-agent89/exact-965e5be2-fmt-unit-clippy-diff.log`.
After adding the first version of this receipt, historical report revision
`048303994e3480ef4e77bbd8f7703fc51f231b39` was rechecked with
`cargo clippy --locked --all-targets`, `cargo fmt --check`, and
`git diff --check`; all exited zero (complete log SHA-256
`94adbf947b55d9a18e0e0698abe0cacbafccf9982f046c190943bbfa696f316c`).
This is retained as a historical check, not presented as the current candidate.

The ownership scenario and panic backstop were also executed directly with the
historical candidate binary. They passed while proving that TERM-ignoring/double-fork
owned descendants were removed and an unrelated `pi` process survived:

```text
PASS: exact smoke ownership reaped TERM-ignoring Pi/observer/supervised-daemon trees ...
PASS: cleanup survives mid-test SIGKILL ...
exit=0
log sha256=53958c51bdc5ecd55eb0dfea7fedd35eff4f1b400a69bf2fc1418d082e31211c
```
