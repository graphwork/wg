# Reliable main baseline diagnosis

This report is the completion artifact for `restore-reliable-main-baseline`. The
candidate is based on `5a0b28a6` plus the guiding-document commit `b025694e`.
It restores the existing reliability contract; it does not add a feature,
weaken a route/security check, install a binary, restart a service, or deploy.

## Reproduction evidence

### Stable serial library baseline

The retained CI transcript `/tmp/wg-5a0-stable-tests.log` recorded this result
before repair:

```text
running 3265 tests
test result: FAILED. 3198 passed; 30 failed; 37 ignored
```

The first independent failures were:

- both `concierge::tests::one_model_shorthand_*` checks: one-model generation
  still checked legacy agent/dispatcher aliases instead of the canonical
  project default;
- `eval_lifecycle::tests::legacy_explicit_codex_role_is_rejected_before_persistence`:
  the rejection remained fail-closed, but its diagnostic had become
  `WG-EXEC-ROUTE-UNSUPPORTED` rather than the obsolete
  `WG-EXEC-ROUTE-REQUIRED`;
- `model_scout::tests::apply_proposal_persists_strong_as_pi_and_weak_as_native_route`:
  the fixture expected a legacy agent alias, while the writer also still
  persisted an unsupported native OpenRouter weak route and rewrote explicit
  role routes;
- `profile::named::tests::test_apply_profile_preserves_openrouter_endpoint`:
  the fixture compared the unrelated legacy `agent.model` value instead of the
  effective canonical project route;
- `profile::named::tests::test_pi_starter_has_glm_workers_and_deepseek_agency_models`:
  the starter had intentionally become sparse, but the fixture still required
  duplicated standard/premium/task-agent values;
- all other profile failures in that run were cascades: the first assertion
  poisoned `HOME_MUTEX`, after which 24 otherwise independent tests failed at
  the mutex `unwrap()` with `PoisonError`.

A Nex runtime test had a separate fixture-boundary problem: under a test runner
whose temporary directory is below a live checkout, nearest-directory lookup
could discover the checkout's real `.nex` rather than the disposable fixture.

### Retained process-proof integrations

Both retained integrations were run directly before their related fixes:

```sh
cargo test --locked --test integration_pi_sole_model_plane -- --test-threads=1
cargo test --locked --test integration_service_control_permissions -- --test-threads=1
```

The Pi model-plane proof reproduced as **7 passed, 1 failed**. Its
`missing_non_pi_and_missing_reasoning_fail_closed` fixture unwrapped
`models.task_agent`, but unified project-route inheritance intentionally leaves
that duplicate role entry absent.

The service-control proof reproduced as **1 passed, 2 failed**. Both worker
cases were rejected early with
`worker_control.admin_operation_refused: command is outside trusted local graph
coordination`: subprocesses targeting a synthetic graph had inherited the
parent worker's exact capability, graph identity, and attempt fence. This was
fixture leakage across a graph boundary, not permission to weaken live worker
authority.

## Root causes, intended behavior, and coverage

| Cause | Repair and preserved contract | Regression coverage |
|---|---|---|
| One-model concierge generation did not replace the sparse starter's canonical `models.default.model`. | Replace that one authority with the requested route. Keep legacy `agent.model` and coordinator aliases absent, while every concrete dispatch slot remains pinned with its independent reasoning value. | Existing `one_model_shorthand_pins_every_effective_role_exactly` and `one_model_shorthand_content_addresses_independent_reasoning_overrides`. |
| Eval lifecycle expected an obsolete error code. | Update only the assertion to the current `WG-EXEC-ROUTE-UNSUPPORTED`; the explicit non-Pi role is still rejected before persistence. | Existing negative lifecycle test. |
| Model scout wrote legacy/duplicated route authority. | Persist only `tiers.standard` and `tiers.fast`; normalize historical bare/native OpenRouter selections to supported `pi:openrouter:` routes; preserve explicit role overrides instead of overwriting them. Premium and inherited agency roles are checked through effective resolution. | `apply_proposal_persists_both_tiers_as_supported_pi_routes`, `pi_weak_route_migrates_native_and_bare_openrouter_routes`, and the existing profile tier-edit tests. |
| Profile expectations predated sparse canonical inheritance. | Assert canonical/effective routes, inherited tiers, and retained explicit judgment-role pins. Preserve unrelated legacy config during copy-by-value profile application. | Updated starter, endpoint-preservation, and tier-edit tests retain both positive inheritance and explicit-override checks. |
| A profile assertion poisoned the test mutex and HOME leaked after a test. | Recover the mutex guard after a prior panic and restore the original `HOME` with an RAII guard. This removes cascades without hiding the original assertion failure. | The complete serial library suite now executes all profile tests independently. |
| Nex disposable project could search through a live ancestor. | Materialize the injected home's `.nex` boundary and place the project below it. Production nearest-directory behavior is unchanged. | Existing standalone project-vs-home precedence test. |
| Pi integration assumed a duplicated task-agent record. | Construct an explicit non-Pi task-agent override for the rejection case, and remove reasoning at the actual canonical default for the missing-reasoning case. Both negative checks remain fail-closed; positive inherited Pi routing remains covered. | All eight `integration_pi_sole_model_plane` tests. |
| Service-control integration inherited parent `WG_*` authority into a different graph. | Strip every inherited `WG_*` variable from the disposable child, then add only the test context each case intends. No production permission check changed. | Worker restart remains denied, worker status remains readable, and human status behavior is unchanged. |

The independent causes were route-fixture drift, model-scout persistence, the
concierge canonical-default omission, disposable environment leakage, and the
Nex fixture boundary. The 24 profile `PoisonError`s were cascades, not 24
additional product defects.

## Candidate validation

On the repaired implementation tree, the retained process-proof command was
run explicitly because those integration targets are required by this task but
are not listed in its deterministic completion command:

```sh
cargo test --locked \
  --test integration_service_control_permissions \
  --test integration_pi_sole_model_plane
```

Result: **11 passed, 0 failed** (Pi model plane 8/8; service control 3/3).
The service proof still includes the positive status path and the fail-closed
restart denial; the Pi proof still includes unsupported-route and
missing-reasoning rejection.

The exact requested combined validation was also run on the same candidate:

```sh
cargo fmt --check && cargo clippy && \
CARGO_TARGET_DIR=/tmp/wg-ci-target cargo test --locked --lib -- --test-threads=1 && \
CARGO_TARGET_DIR=/tmp/wg-ci-target cargo test --locked --bin worksgood -- --test-threads=1 && \
cargo test --locked --test integration_service_control_permissions --test integration_pi_sole_model_plane && \
cargo test --doc && \
cargo test --locked --bin wg commands::completion_canary_tests
```

Results:

- serial library: **3228 passed, 0 failed, 37 ignored**;
- retained integrations: **11 passed, 0 failed**;
- doctests: **8 passed, 0 failed, 2 ignored**;
- completion canary: **3 passed, 0 failed, 1 explicit real-Pi opt-in ignored**;
- formatting, Clippy, the configured bin target, and `git diff --check`: passed.

The ignored real-Pi canary is explicitly credentialed/opt-in and was not made a
future acceptance condition. There is no remaining failure in the requested
baseline. The worker branch was pushed for review, but `origin/main` was not
pushed and no install, daemon restart, credential change, or deployment was
performed.
