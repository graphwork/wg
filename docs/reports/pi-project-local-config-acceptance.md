# Project-local Pi configuration acceptance

Task: `close-pi-config-acceptance`

Validated implementation base: `70ef064ffbb4ad4a5260e0409c78cd5bdeb569b0`
(current `main` when this audit began, including `9b994291` and `4890d7d6`).
Validated repair revision: `d1bb3721ddfe86836326d4fb08e0b15c70a1d8cb`.

## Review status of the historical implementation

The historical task `finish-pi-project-local-config` **did not receive semantic
approval**. Its first candidate was rejected by FLIP receipt
`b3:42d8ac5a189389224753d48047bae8895e7319419a8973cfd62a7b527dd570b8`.
Its final manifest
`b3:d651c4dcddb489e7bc74c06b2e96e5a1d2648a9e3bcb5d03b8c94e278ce1de95`
received only the unavailable-review receipt
`b3:4a0528aeeabfb41d5354274a6f627c81efef9613f7253eb36536042d4f6b70fd`:
`reviewer.invalid_response`, no JSON object. Eval is missing. The historical
landed receipt
`b3:34f2a3d4cc96f73fcbd75b571ab7d977c52fda437a74720a7c18c65809b54795`
is preserved as lifecycle history, not represented here as approval.

This audit used `wg show finish-pi-project-local-config`, the original contract
in `docs/design-project-local-pi-config.md`, and
`docs/reports/finish-pi-project-local-config-audit.md`. None of the prior
receipts or graph records was changed.

## Independent implementation findings

The landed architecture is substantially the requested one:

- `src/config.rs:5883-5947` selects exactly one project authority without
  reading machine-global `Config`. Root `worksgood.toml` wins; only when it is
  absent are `.wg/config.toml` and `.wg/profile-selection.json` read through
  the exclusive one-release compatibility path. `Config::load_merged` at
  `src/config.rs:6139` retains that rule. This matches the documented
  precedence: task/command override, then `worksgood.toml`, then structural
  default; there is no global layer.
- `src/project_config.rs:15,340-387` fixes the checked-in filename, validates
  schema/origin, and rejects credential/provider authority in the project
  document. `src/config.rs:6749-6771` labels effective leaves
  `project-file` or `project-profile-import`.
- `src/execution_selection.rs:14,179-235,284-310` refuses a missing project
  route with `WG-EXEC-UNSELECTED`, ignores `Default`/`Global` sources, and names
  stale global selectors and the inactive active-profile pointer.
- `src/migrate_project_local_pi.rs:153-181,511-659` uses an enumerated cleanup
  set, planned config/active-pointer preimages, an allowlisted write set,
  backups, preservation metadata, and CAS rollback. Profiles, secrets,
  keystore, identity, federation, and Pi settings are outside its mutation set.

The independent audit found and repaired three concrete gaps rather than
replacing this architecture:

1. **Global routing aliases were incomplete.** On the starting revision, an
   isolated command `wg config set profile pi --global --no-reload` exited 0
   and wrote `profile = "pi"` to the global file. The denylist also omitted the
   `execution` fallback namespace. The retired `wg config --install-global
   --force` and legacy `wg model ... --global` surfaces provided additional
   route-bearing global paths. `src/commands/config_cmd.rs:2105,2554,2825-2834,
   3042-3054,3261-3290` and `src/commands/model_cmd.rs:130-145` now funnel those
   routing surfaces through `WG-GLOBAL-CONFIG-WRITE-REFUSED`. Exact namespaces
   and both `dispatcher`/`coordinator` spellings are covered; the supported
   non-routing `config set --global` escape hatch still warns first with the
   exact path and `legacy-global-inactive` scope.
2. **Profile definition preimage was not bound to apply.** Profile selection
   parsed a definition in one read and separately read the bytes used for its
   fingerprint; the project materializer rechecked only `worksgood.toml`.
   `src/commands/profile_cmd.rs:694-738` now parses the exact captured bytes,
   and `src/project_config.rs:67-92,175-220` rechecks that exact file preimage
   (including “starter path absent”) immediately before the project document
   CAS. A changed definition fails with
   `WG-PROFILE-DEFINITION-CONCURRENT-WRITE` and writes no project bytes; the
   deterministic regression is at `src/project_config.rs:896-928`.
3. **The e2e proof had an evidence hole.** Its dry-run config check compared a
   hash with itself, and it did not exercise profile selection in the two-repo
   flow. `tests/smoke/scenarios/project_local_pi_e2e.sh:121-158,341-358` now
   performs setup, project config edits, profile selection, source checks, and
   manual route restoration while asserting repo B, global config/pointer,
   reusable definition, Pi console settings, and guardrails remain correct.
   Dry-run now compares saved before/after hashes for every protected fixture.
   `tests/smoke/manifest.toml` assigns this task as an owner, so completion
   cannot bypass that scenario.

## Requirement-by-requirement evidence

| Requirement | Reproduced result |
|---|---|
| Fresh HOME, no WG global config/secrets | `integration_project_local_pi_cli::setup_defaults_to_authoritative_project_file_without_machine_state` passed. It removes credential env variables, writes only `worksgood.toml`, and asserts no `~/.wg/config.toml`, active pointer, or `.pi` directory. No Pi credential is inspected or copied. |
| Real setup/config/profile flow and two-repo isolation | The CLI suite passed its `script(1)` PTY setup test. The e2e scenario drives the built candidate's real `init`, `setup`, `config set/get`, `profile select`, `profile use`-covered integration path, service preflight, identity, peer, lint, and migrate entry points in one isolated HOME with repos A and B. Repo B and route-bearing global bytes remain unchanged by repo A operations. |
| Global warnings and restrictions | `integration_project_local_pi_cli::explicit_global_non_routing_write_warns_and_routing_rewrites_are_refused` passed. It checks the exact global path warning, inactive scope, coordinator alias, top-level profile, execution fallback namespace, OpenRouter namespace, model aliases, global/both setup, and whole-file install refusal without mutation. |
| Missing route despite stale global state | `integration_project_local_pi_config::stale_global_route_is_inactive_but_graph_config_loads` and the e2e service-start/service-tick paths passed with `WG-EXEC-UNSELECTED`; no daemon, agent, claim, or graph mutation was created. Global `max_agents=99` lost to `builtin-default`. |
| Guardrails and winning sources | The profile CLI test preserves `dispatcher.max_agents` and resource guardrails. The e2e profile flow preserves resource/archive values and reports route source `project-profile-import`; its direct route edit clears origin and reports `project-file`. `config get`, `config --models`, `profile show`, and `profile list` source assertions passed. |
| `worksgood.toml` / `.wg` precedence | `integration_project_local_pi_config::worksgood_document_disables_legacy_project_route_instead_of_merging` passed even with a conflicting legacy route and malformed legacy association. The legacy-only compatibility test also passed and is explicitly labeled `legacy-project-source`. |
| Migration dry-run/apply/repeat and preservation | Nine CLI/library integration tests plus ten migration unit tests passed. The e2e hashes the reusable profile, secret sentinel, keystore sentinel and full custody manifest, real identity record, federation registry, and project config across dry-run and apply. Apply removes only routing/pointer state; repeat creates no receipt and changes no mtime. Rollback exact-restore and changed-postimage refusal passed. |
| Changed planning preimages | `migrate_project_local_pi::tests::apply_refuses_when_global_preimage_changes_after_planning` and the new profile-definition preimage test passed. Both fail before candidate data is overwritten. Project materialization's document recheck remains at `src/project_config.rs:201-220`. |
| Explicit candidate smoke | `project_local_pi_e2e` passed with `PATH` prefixed by the Cargo target's exact `debug/wg`; no globally installed binary was used for the scenario. |
| Repository policy checks | `cargo fmt --check`, `cargo clippy`, and `git diff --check main...HEAD` all exited 0. Clippy emitted existing advisory warnings; there were no check failures to omit. |

## Exact reproducible validation

From the repository root at validated source revision
`d1bb3721ddfe86836326d4fb08e0b15c70a1d8cb`:

```bash
TARGET="$(cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
CANDIDATE="$TARGET/debug/wg"

cargo build --locked --bin wg
test -x "$CANDIDATE"
sha256sum "$CANDIDATE"

cargo test --locked \
  --test integration_project_local_pi_cli \
  --test integration_project_local_pi_config \
  --test integration_project_local_pi_migrate
cargo test --locked --lib migrate_project_local_pi::tests
cargo test --locked --lib \
  project_config::tests::profile_materialization_rejects_changed_definition_preimage
cargo test --locked --bin wg \
  commands::profile_cmd::tests::test_project_profile
cargo fmt --check
cargo clippy
PATH="$(dirname "$CANDIDATE"):$PATH" \
  WG_SMOKE_SCENARIO=project_local_pi_e2e \
  bash tests/smoke/scenarios/project_local_pi_e2e.sh
git diff --check main...HEAD
```

Results were respectively: build pass; integration `6 + 7 + 9` pass;
migration unit `10` pass; definition-preimage `1` pass; profile command `4`
pass; fmt pass; clippy pass; smoke pass; diff check pass. The candidate binary
SHA-256 was
`a1c8579e3f1b458b4b13f2e5108631e4f051551dcd4830cf0dde7b2aa2e1e35f`.

## Immutable evidence available before review

The exact combined 303,768-byte validation transcript was captured through the
supported `wg completion-object` API before this report was committed:

- content/locator digest:
  `b3:21bba0f1160355ff5070fba457b3bf1ff123406b3db2fddef79f6f703730e1e2`
- evidence kind: `candidate-validation/project-local-pi/v1`
- media type: `text/plain`
- transcript SHA-256:
  `4ef7befd53890321b1adb7087fd8b035674d9c777b83f4ea5b4bf54dc05ba21a`
- structured checked-in index:
  `docs/reports/pi-project-local-config-acceptance-evidence.json`

The historical manifest/review/land CIDs above are immutable earlier evidence.
This report deliberately does **not** name or require its own future candidate,
FLIP, Eval, Done, or publication receipt. Those cannot exist before review and
must not be projected backward into the evidence set.

## Remaining limits and operator gate

- This validation is credential-free. It proves route materialization and that
  Pi authentication stays external; it does not make a live provider call or
  attest an operator's Pi login.
- The focused migration tests deterministically inject changed preimages and
  rollback edits. They do not fault-inject every filesystem crash point or
  claim a distributed transaction across unrelated processes.
- The exclusive `.wg/config.toml`/association reader is intentionally
  one-release compatibility. This acceptance does not extend that lifetime.
- No global binary install, live daemon restart, real HOME cleanup, operator
  credential/configuration change, or `origin/main` push occurred.
- The operator must inspect this task's **new** candidate review chain after
  completion. Acceptance requires a fresh genuine FLIP-v2 semantic Pass and a
  subsequent Eval Pass bound to the selected candidate, with no invalid
  projection warnings. A malformed response, unavailable reviewer, missing
  Eval, lifecycle publication alone, or any historical receipt is not
  approval.
