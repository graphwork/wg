# FLIP route / PendingEval fix validation

Branch: `fix/pending-eval-flip-qualified-route`

Patch commit: `bef69590`

Review intake: <https://github.com/graphwork/wg/pull/new/fix/pending-eval-flip-qualified-route>

The GitHub CLI is not installed on the validation host, so the pushed branch's
canonical pull-request intake URL above is the handoff URL.

## Root cause and corrected path

`Config::resolve_model_for_role` correctly resolved `codex:gpt-5.4-mini` into
model `gpt-5.4-mini` and provider `codex`. `eval_scaffold` then persisted those
as separate task fields, but `coordinator::spawn_eval_inline` validated only
`task.model`. Strict `execution_system_key` therefore received the bare model
and rejected it before claim/spawn. The patch persists
`ResolvedModel::spawn_model_spec()` as the task model and retains provider,
endpoint, and reasoning fields. FLIP scaffolding now selects
`DispatchRole::FlipInference`, rather than the unrelated evaluator role.

`why-blocked` now calls the same dependent-aware blocker predicate used by
dispatcher readiness. A PendingEval parent is not presented as the root cause
for a system task that is allowed to bypass it; an incomplete FLIP task remains
the actionable root and its recorded route failure is displayed.

## Commands and results

The original disposable failure and installed-binary identity are linked from
`pending-eval-flip-inline-route-20260717.md` at report commit `80bcbfab`.

Validation on the patch used the host system linker explicitly because the
default Frontier Cray `cc` wrapper injects incompatible LTO plugin options into
Rust's `lld`:

```bash
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=/usr/bin/gcc
export CC=/usr/bin/gcc CXX=/usr/bin/g++
export LIBCLANG_PATH=/opt/cray/pe/cce/18.0.1/cce-clang/x86_64/lib
export BINDGEN_EXTRA_CLANG_ARGS='-I/opt/cray/pe/cce/18.0.1/cce-clang/x86_64/lib/clang/18/include -I/usr/include'

cargo fmt --all -- --check
cargo build -q
cargo test --bin wg -q
```

Result: all commands exited zero. The `wg` binary test target ran 3,789 tests.
Focused exact tests also exited zero:

```text
commands::eval_scaffold::tests::test_scaffold_flip_preserves_qualified_route_across_serialization
commands::why_blocked::tests::test_system_task_pending_eval_parent_is_not_reported_as_root_blocker
commands::why_blocked::tests::test_incomplete_flip_keeps_route_failure_as_actionable_root
```

The patch does not change native-provider retry policy. HTTP 429 remains
`FailureClass::ApiError429RateLimit`, HTTP 5xx remains
`FailureClass::ApiError5xxTransient`, and the existing retry-in-place/worktree
retention paths are unchanged.

## Existing graph repair

After installing a reviewed fixed build:

```bash
wg edit .flip-<task-id> --model codex:gpt-5.4-mini
wg retry .flip-<task-id>
wg why-blocked .evaluate-<task-id>
```

Do not manually complete lifecycle tasks. The dispatcher will advance the
existing chain without creating a duplicate evaluator.

## Containment

No emender source, live `.wg/config.toml`, installed `wg` binary, or Slurm job
was changed. FLIP remains disabled in the emender project pending review and a
separate upgrade/re-enable task.
