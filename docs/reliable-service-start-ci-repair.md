# Service-start CI repair: route-less admission

## Scope

This note records the diagnosis and reproducible evidence for
`repair-service-start-ci`. It changes no routing or service policy.

## Intended contract

A missing execution route no longer prevents the graph service from running.
`src/commands/service/mod.rs` explicitly permits a route-less graph service and
requires work admission to resolve the current route on each tick. In
`src/commands/service/coordinator.rs`, failure to resolve the task-agent route is
recorded as an admission deferral before an attempt is reserved or an agent is
spawned. This matches the configuration guidance in
`docs/reliable-work-guiding-principles.md`: a pre-execution configuration problem
blocks admission without consuming a work attempt.

Therefore the old integration assertion—`wg service start` itself must fail—was
stale. The safety invariant remains: no model task may launch without explicit
route authority.

## Base reproduction

On the integrated base `f4ec14ac0bba6eec62d7e46ff2e7345fd55b26fa`, the real
candidate binary reproduced remote CI job `104083195624` with:

```console
$ cargo test --test integration_service \
    test_service_start_rejects_implicit_coordinator_config -- \
    --exact --test-threads=1 --nocapture
running 1 test
test test_service_start_rejects_implicit_coordinator_config ... FAILED
assertion failed: !output.status.success()
test result: FAILED. 0 passed; 1 failed
```

The failure was not leaked route authority. The old fixture called
`setup_workgraph`, whose real CLI setup runs `wg init --route pi`, then replaced
`.wg/config.toml` with agency-only settings. On that resulting fixture,
`wg config --models` exits nonzero with `WG-EXEC-ROUTE-MISSING`. The startup
success came from the intentional graph-service policy above.

## Corrected real-CLI distinction

The repaired integration test now proves both sides through CLI entry points:

1. `wg init --route pi` followed by `wg config --models` reports
   `project default = pi:...`.
2. The original route-initialized fixture is overwritten with its agency-only
   config; `wg config --models` then fails with `WG-EXEC-ROUTE-MISSING`.
3. A published model task is added and the route-less graph service is started.
4. The task reaches `admission-deferred` but remains `Open`, unassigned, with
   `attempt_sequence = 0`; the service registry contains zero agents.

A separate isolated `/tmp` CLI rehearsal against the test-built candidate
produced the same observable boundary:

```text
explicit: project default = pi:openrouter:z-ai/glm-5.2
unconfigured: service_start=success task_status=open attempt_sequence=0 assigned=null agents=0
admission_error=WG-EXEC-ROUTE-MISSING
```

## Validation

The unchanged repository-authorized command passes on the corrected candidate:

```text
cargo fmt --check && cargo clippy && cargo build --locked --bins && \
cargo test --test integration_service -- --test-threads=1 && git diff --check

integration_service: 9 passed; 0 failed; 3 ignored
```
