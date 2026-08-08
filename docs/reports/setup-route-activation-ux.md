# Setup route activation and readiness investigation

Task: `setup-route-activation-ux`

## Isolated pre-change reproduction

From the source immediately before this change, with a clean temporary `HOME`,
`WG_GLOBAL_DIR`, project directory, and a `PATH` containing no Pi executable:

```bash
env -i HOME="$tmp/home" WG_GLOBAL_DIR="$tmp/home/.wg" \
  PATH="$tmp/empty:/usr/bin:/bin" \
  wg setup --route pi --yes --model pi:openrouter:test/model
```

The command exited successfully and wrote `~/.wg/config.toml`. `wg config
--models` resolved both `default` and `task_agent` to the requested exact route,
but `~/.wg/active-profile` was absent. Output only said that Pi owned login and
model availability; it did not say whether Pi was installed, distinguish
configuration from authenticated/model-probed readiness, or give a bounded
login/model test action. A later `wg profile use pi` created the missing pointer,
which made that undocumented second command appear necessary.

The setup path writes a complete config from `config_for_route`, while profile
activation separately writes `~/.wg/active-profile`, reapplies profile routing,
prepares Pi integration, and asks a live daemon to reload. Pi and its provider
own credentials and model discovery; WorksGood has no credential-safe,
noninteractive live-auth probe that can truthfully prove access without entering
Pi's login flow or making a provider request.

## Implemented behavior

- Global/default and `both` setup persist the selected `pi` pointer in the same
  rollback-protected transaction as the exact config. Config-write or pointer
  activation failure restores prior config/profile state. Configured custom
  default/task-agent routes are not replaced by starter routes.
- `--scope local` keeps its project config authoritative and deliberately does
  not mutate the machine-global active pointer.
- Setup idempotently ensures the version-locked `pi-worksgood` console plugin
  before committing route/profile state, then requires a running project daemon
  to reload. Plugin preparation is an intentional additive/idempotent side
  effect and is not rolled back. Plugin failure is pre-activation. A reload
  failure exits nonzero and says on-disk activation succeeded but runtime state
  is unknown, with the exact `wg service reload` recovery action; it does not
  risk a compensating rollback after a lost-but-applied reload response.
- The completion report separates: Pi executable `AVAILABLE`/`UNAVAILABLE`,
  verified pi-worksgood ready/not-ready status, profile active/local/dry-run, and Pi auth/model
  `NOT VERIFIED`. It states that no provider request occurred and directs the
  operator to run Pi, use `/login` if necessary, select the configured model,
  and send a test prompt. No fallback route is selected.
- Dry-run reports the same bounded readiness and intended activation without
  writing config, profile, cache, plugin, or daemon state.

## Product boundary left for adjudication

A custom local config cannot truthfully select the shipped `pi` reusable
profile: that profile's fingerprint may name different model IDs. This change
does not invent a generated project association or mutate global profile state
for `--scope local`. Product owners can later decide whether expert `wg setup
--scope local` should adopt the content-addressed generated-profile transaction
already used by `worksgood setup --model ...`.

## Deterministic validation

- `cargo fmt --check` — pass
- `git diff --check` — pass
- `cargo build` — pass
- `cargo clippy` — pass (existing warnings)
- `cargo test --test integration_setup_routes` — 13 pass, 11 retired tests ignored
- setup scope/rollback unit filters — 5 pass, including injected global
  active-profile failure and restoration of both prior config/profile state
- `integration_pi_two_tier_profile` + `integration_profile_tier_pinning` — 12 pass
- `tests/smoke/scenarios/setup_route_activation_preflight.sh` — pass using a real
  PTY, isolated homes/projects, fake Pi, absent-Pi fixtures, a trap provider key,
  deliberately unreachable HTTP/HTTPS/ALL proxy settings, and `strace` connect
  tracing; setup preflight never invokes Pi and produces no IPv4/IPv6 provider
  connection. The same scenario then starts a real daemon, reruns setup against
  it with a checked reload acknowledgement, and invokes the first real
  `spawn-task` LLM command through fake Pi with the exact provider/model argv

A full repository `cargo test` run reached more than 3,100 passing library tests
but remains non-green because unrelated tests share and race process-global
`HOME`/`WG_GLOBAL_DIR`; running inside a WG worker also leaks worker-control
authority into completion tests. The implicated profile tests pass individually.
No failures pointed to the changed setup/profile paths.
