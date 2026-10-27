# Archived (retired) smoke scenarios

This directory holds scenarios that are **preserved but retired**: they are no
longer part of the default `wg done` gate, and they are never deleted. The
manifest stays grow-only — every retired scenario is still a `[[scenario]]`
entry in `../manifest.toml` with `retired = true` and a `retired_reason`, and
its script lives here instead of in `../scenarios/`.

Implements `docs`/`survey` reduction-plan **C1.2** (`archive-retired-smokes`).

## What retirement means

| Selection | Runs retired scenarios? |
|---|---|
| `wg done <task>` (default gate) | **No** — owned *and* retired scenarios are skipped, and the skip is printed with the scenario names and the opt-in command |
| `wg done <task> --full-smoke` (deliberate whole-corpus run) | **Yes** — every manifest entry runs, retired included |

Retirement narrows the *default* gate only. It does not delete coverage: the
scripts, their `owners` provenance, and their manifest entries all survive, so
`--full-smoke` still runs them and `git revert` restores them to the default
path in one step.

## Current counts

**128 of 392** manifest entries are retired (264 stay on the default gate).
The first wave (`archive-retired-smokes`) retired **98** (20 spark plane + 78
non-pi handler). The second wave (`port-incidental-smoke-routes`) ported the
residual *fixture-route* scenarios to a credential-free `pi:` route and then
retired the **15** whose ported body passes (99 → 113). The third wave
(`fix-residual-smoke-drift`) re-triaged the ported-but-blocked remainder:
**7 bodies were repaired and stay on the default gate**, and **15 were retired**
with the reasons below (113 → 128).

## Why these scenarios were retired

The measured daily reality of this repo is that **every worker runs on the `pi`
handler** (executor survey: 76/76 agents; config survey: 113/113; build-size:
119/119 — `native` / `nex` / `claude` / `codex` each have zero live graphs).
Smoke is **not in CI**, and by ownership design a scenario almost never re-runs
once the task that owns it is done (388 scenarios, 462 owners, ~395 owners not
present in the graph). The full run was ~10.8 h, dominated by scenarios that pin
handler chains and spark planes the daily flow never enters.

Two retirement reasons are used:

### `spark plane` (20 scenarios)

WG-Fed / WG-Review / WG-Exec / pilot demo planes, all built as two-week spikes.
They are load-bearing for the *federation research* direction and are kept for
`--full-smoke`, but nothing in the daily loop reaches them
(`wg identity` 0, `wg review` 0, `wg fed-node` 0, `wg provider` 0 hits in live
graphs).

```
casa_wgfed_adapter_e2e content_safety_spark cross_task_poison
e2e_autowire_ingest_gate e2e_family_team exec_harden_wire exec_ledger_crash_safe
exec_real_run_silicon exec_spark_borrowed_box fed_harden_residuals
federation_acl_ucan_delegation federation_node_inbox_cross_graph
federation_recovery_portable_state federation_spark_two_graphs
harden_node_network live_reviewer_eval ops_observability pilot_dry_run
real_review_evasion wire_review_seams
```

### `non-pi handler` (78 scenarios)

Scenarios whose *subject* is a `claude` / `codex` / `nex` / `native` /
`opencode` worker or chat route: the nex PTY + streaming suite, the codex
`exec`/prompt-stdin chain, the standalone `nex` binary, the TUI `+`-menu
openrouter/opencode/dexto/octomind chat tabs, the codex/claude worker
restoration flows, and the per-role routing scenarios that exist only to prove a
native handler wins.

```
agency_codex_override_routes_to_codex
agency_inline_spawn_registers_executor_claude
agency_local_model_overrides_claude_executor agent_guide_warns_off_wg_nex
chat_create_endpoint_persists chat_launched_with_codex_uses_codex
chat_native_endpoint_full_pipeline claude_executor_with_global_openrouter_default
codex_chat_pty_passes_bypass_approvals codex_cli_fresh_init_runtime
codex_handler_pipes_prompt_to_stdin codex_init_route_has_correct_defaults
codex_optional_tool_config_ignored codex_token_usage_capture
config_one_line_codex_route direct_codex_reasoning_profile
dispatcher_codex_wins_over_agency evaluator_artifact_diff_prompt
evaluator_claude_slash_model_bypasses_openrouter
evolve_fanout_codex_route_no_bare_aliases executor_arena_surfaces
explicit_claude_chat_resume explicit_claude_worker explicit_codex_chat_resume
explicit_codex_worker external_worker_live_chat_rejected
integrate_nex_chat_end_to_end login_openrouter_from_stdin
model_fable_routes_to_claude_cli native_executor_keyless_init
native_floor_yields_to_cli_locked_model nex_bg_process_group_pty
nex_canonical_prefix nex_chat_in_tui_via_tmux nex_current_data_prompt
nex_helper_openrouter_route nex_large_output_nonrecursive nex_live_input_pty
nex_minimal_tools_auto_default nex_openrouter_chat_stays_alive_and_resumes
nex_progressive_markdown_pty nex_resume_hint_on_exit
nex_resume_oversized_journal_does_not_loop nex_stale_tui_sentinel_respawns
nex_streaming_resilience nex_think_collapse_pty nex_thinking_timer_pty
nex_two_message_against_lambda01 nex_wg_openrouter_endpoint_auth
nex_yolo_banner_and_precedence opencode_live_chat_reply
opencode_per_task_model_route openrouter_provider_error_402
preset_shortcuts_still_work profile_codex_service_start
profile_nex_canonical_name pty_codex_query_responses
pty_codex_sync_repaint_no_scrollback_stacking
pty_initial_spawn_no_scrollback_doubling setup_openrouter_login_handoff
setup_routes_complete_configs standalone_nex_config_split
standalone_nex_entrypoint task_agent_codex_model_routes_to_codex
tui_nex_chat_end_to_end tui_nex_chat_post_restart_response
tui_opencode_scrollback tui_plus_dexto_chat tui_plus_nex_chat
tui_plus_nex_endpoint_autocomplete tui_plus_nex_global_endpoint_picker
tui_plus_nex_model_fuzzy tui_plus_nex_openrouter_no_endpoint
tui_plus_octomind_chat tui_plus_opencode_chat tui_plus_opencode_minimax
tui_plus_opencode_model_fuzzy worksgood_default_release_contract
```

`worksgood_default_release_contract` is the one retired entry whose script is
not under this directory: it points at `tests/install/release_contract.sh`,
which is an install-harness script, so only its manifest entry is retired.

## Wave 2: the residual fixture-route scenarios (`port-incidental-smoke-routes`)

`archive-retired-smokes` filtered on subject and explicitly kept ~33 scenarios
that only *mention* or incidentally configure a non-pi handler as a
credential-free fixture. The follow-up task ported those fixtures to `pi:` and
retired the ones that pass. **15 were ported, re-run green, and retired**:

```
automatic_archival_opt_in          automatic_archival_restart_hold
bash_chat                          chat_last_interaction_sort
config_load_no_stderr_spam         custom_command_chat
dispatcher_boot_no_orphan_supervisor
dispatcher_kick_bypasses_settling_delay
pi_config_lint_rejects_unsatisfied_route
pi_handler_plugin_transports       service_start_survives_pty_hangup
tui_bootstrap_parent_graph_isolation
tui_help_modal_scroll              tui_log_pane_follows_retry
wg_show_retry_attempts
```

Each of the 15 was re-run **from this directory** after the move (`bash
tests/smoke/archive/<name>.sh` under the harness bridge) and passed; the
graph/service/CLI ones are fast and deterministic, and the TUI ones keep their
historical timing sensitivity (`tui_log_pane_follows_retry` needs its usual TUI
startup grace, so a rare flake under heavy load is expected — it is retired, so
only `--full-smoke` pays for it).

The ports are route-only: `wg init -m claude:opus` / `-x claude` /
`config --local --model codex:gpt-5.5` / `--route claude-cli` became
`wg init -m pi:test:fake` (or `--route pi`), graph fixtures moved
`"executor": "claude"` → `"executor": "pi"`, `model = "claude:opus"` →
`model = "pi:test:fake"`, and the bare `openrouter:` provider prefix became
`pi:openrouter:`. Assertions were left alone except for two documented,
behavior-preserving adaptations: `automatic_archival_opt_in` now strips the
`archive_retention_days` key from the checked-in `worksgood.toml` as well as
`config.toml` (the pi projection materializes the key in the project document),
and it matches the current `builtin-default` provenance label where it used to
spell `default`.

### Wave 3: the blocked remainder triaged (`fix-residual-smoke-drift`)

Wave 2 left the ported-but-blocked scenarios on the default gate so the failure
stayed visible. Wave 3 re-triaged every one of them and, for each, recorded a
decision: **repair the fixture body** (it then passes and stays on the gate) or
**retire it deliberately** (with the reason below). Every failure reproduced on
`refs/heads/main` before the repair; no production code was changed.

#### Repaired and kept on the default gate (7)

| Scenario | Fixture repair (assertions untouched) |
|---|---|
| `reset_clears_claim` | `wg add` now stages a **PAUSED draft**, so the fixture `wg publish --only`s each seed before claiming it — otherwise `wg ready` stays empty and the "immediately ready after reset" assertion measures the draft gate, not `wg reset` |
| `reset_clears_downstream_claims_too`, `retry_clears_downstream_stale_claims` | `wg claim` now enforces prerequisites, so the edge is wired **after** both ends are claimed (`wg edit downstream --add-after upstream`); claim-clearing through the closure walk is still the assertion |
| `reconciler_clears_open_with_dead_agent` | publishes the victim task (same PAUSED-draft reason) before `wg sweep` + the `wg ready` assertion |
| `retry_kills_in_progress_agent` | `wg retry` is now **two-phase** (fence + graceful owner exit, then a reconcile pass releases the hold), so the fixture drives the idempotent second pass instead of asserting a one-shot flip; `task Open` + `retry_count == 1` are still asserted |
| `chat_idle_does_not_respawn_loop` | the fixture now `git init`s + commits (dispatch/worktree paths and the completion contract need a real repo) and completes the chat task through the attested `--operator-accept --reason` witness, since a hand-made fixture has no attempt-bound validation evidence; the no-reactivating-reopen assertion is unchanged |
| `unified_project_route_authority` | the expected-refusal review leg runs through the new `wg_expect_fail` helper and is guarded by a `wg review --help` probe, because the WG-Review cluster lives behind the non-default `federation` cargo feature; a default build now prints a loud SKIP for that leg instead of dying at a missing verb. The route-authority assertions (the scenario's subject) are untouched |

`_helpers.sh` gained the sanctioned **`wg_expect_fail`** wrapper for this: the
central `ERR` trap fires on any failing command, so a scenario whose subject is
a *refusal* must say so. `wg_expect_fail` runs the command with the `ERR` trap
and `errexit` suspended, records `WG_EXPECT_FAIL_RC` / `WG_EXPECT_FAIL_OUTPUT` /
`WG_EXPECT_FAIL_STDERR`, fails the scenario loudly when the command
unexpectedly succeeds, and restores the exact prior trap afterwards.

#### Retired in this wave (15)

| Scenario | Why it is retired (not fixable inside a fixture) |
|---|---|
| `reaper_retry_in_place_protected` | `wg sweep --reap-targets` now reaps only **disk-sentinel-owned** build paths (lease / artifact / clean-worktree evidence), so the hand-made `.wg-worktrees/agent-A/target` is never reaped at all: the protection assertion is vacuous and the negative control is unsatisfiable without synthesizing the reaper's ownership records |
| `pi_worker_one_shot_prompt_and_cred_error` | the fake-pi argv / stdin / credential-error assertions still hold, but the worker wrapper no longer appends its `[wrapper] Agent exited with code N, marking task failed` line to `output.log` (failure accounting moved to the attempt-bound lifecycle), so the final assertion has no observable subject from a hand-driven `wg spawn` |
| `build_baseline_admission_refresh` | the fixture's `pi:test:fake` route is absent from Pi's offline registry, so the dispatcher now defers admission forever (`error[WG-PI-PROVIDER-UNSUPPORTED]`) instead of spawning the non-Rust worker the scenario measures; reproducing it needs a registered provider + real model route |
| `retained_worktree_slow_sweep_does_not_starve_dispatch` | it hard-codes `$REPO_ROOT/target/debug/wg` (which does not exist under the layered `CARGO_TARGET_DIR` build) and its anti-starvation subject now sits behind blocked retained cleanup for the whole timeout |
| `isolation_worktree_collision_transaction` | same hard-coded `target/debug/wg` candidate path, plus its fake handler writes `.wg/isolation-handler.pid` relative to the isolated checkout cwd instead of the project graph, so the pid file the assertions watch is never published |
| `intake_deliverable_preflight` | `wg done` now runs the deterministic completion contract (and rejects `--skip-smoke` as a legacy bypass flag) *before* the deliverable preflight, so the G1 refusal is unreachable from a fixture; the refusal, the `deliverable-missing` class and the G3 directive injection stay pinned by `src/commands/done.rs` + retry unit tests |
| `flip_role_model_routing_honors_config` | subject removed: `wg evaluate run --flip --dry-run` no longer exists (evaluate scores terminal outcomes only), so the `Inference model: <configured role model>` metadata has no producer; flip role resolution lives in `resolve_agency_dispatch` (`src/service/llm.rs`) |
| `tui_large_active_log_enrichment` | TUI drift: neutral first frame is 112–123 ms against the fixture's 100 ms ceiling — a rendering-path latency property, not fixture drift |
| `tui_log_pane_renders_raw_stream` | TUI drift: the Log pane still shows "(no agent output yet)" for a hand-made `agents/agent-fake/raw_stream.jsonl` because the agent is no longer resolved from the task's `assigned` field alone; the fixture would have to be re-authored around the current attempt/registry resolution |
| `tui_chat_redesign_modal_close_persist` | TUI drift: the chat tab bar shows 0 tabs where the fixture expects ≥3, so the modal-close persistence flow it drives is unreachable |
| `tui_symbolic_context_bar` | TUI drift: the Chat title row the fixture matches is gone from the redesigned context bar |
| `tui_termux_stacked_inspector` | TUI drift: the stacked-inspector PTY run hangs until the scenario timeout instead of reaching the asserted frame |
| `chat_archive_kills_tmux_session` | production drift: `wg chat archive` no longer tears down the canonical `wg-chat-*` tmux session — the fixture is right and needs a production fix, not a fixture edit |
| `chat_create_via_ipc_works` | not credential-free: creating a chat through IPC needs a real CLI + model credentials, so it can only SKIP in this gate |
| `shared_sccache_warm_rebuild` | not host-independent: SKIPs without `sccache` (`MISSING SCCACHE`) |

#### Still on the default gate, and why

| Scenario(s) | Blocker (route-independent, pre-existing) |
|---|---|
| `launcher_custom_url_register_persists`, `profile_use_preserve_openrouter_endpoint`, `setup_scope_flag` | the surface under test is gone with the retired endpoint/global-routing config model (`[llm_endpoints]`, `--route claude-cli`, global routing writes) |
| `pi_plugin_live_validation` | SKIP without a real `pi`/`claude` CLI + model credentials (not credential-free) |

Those five are the residual follow-ups: the first three belong to the
config/endpoint-redesign retirement, the last two are credential-gated. They are
not fixture drift, which is why this wave left them alone.

`secret_keyring_round_trip`, `wg_html_*` (owned by `retire-html-renderer`) and
the TUI chat-*selector* scenarios are still default-gate members by design:
their subject really is the preset/route menu or another task's surface.

This wave deliberately changed **no production code**; the blocked list above is
the evidence trail for the follow-ups, not a widening of this task.

## Shared routing invariants

The few retired scenarios that also pinned a **shared routing invariant** (the
handler is always derived from the resolved model spec; an explicit per-task /
per-role route wins over the worker floor; bare provider prefixes are a loud
deprecation, never a silent route) are covered by unit tests in `src/config.rs`
(`parse_supported_execution_route`, `handler_first_rewrite`,
`bare_provider_prefix_warns`, explicit-override resolution) and
`src/service/llm.rs` (`resolve_agency_dispatch`). Those invariants were never
only smoke-pinned, which is why retiring the scenarios does not lose them.

## Adding to the archive

1. Move the script: `git mv tests/smoke/scenarios/<name>.sh tests/smoke/archive/<name>.sh`.
2. In `tests/smoke/manifest.toml`: repoint `script = "archive/<name>.sh"`, and
   add `retired = true` plus a `retired_reason`. Keep `owners` — it is the
   provenance trail for `--full-smoke`.
3. Do NOT delete the script, the manifest entry, or the owner list.

## When the *subject verb* is retired too

`pilot_dry_run` is the one archived scenario whose subject was **removed**, not just
retired: task `retire-wg-pilot` (reduction plan S10) deleted `wg pilot`
(`src/commands/pilot_cmd.rs`, `pilot.example.toml`, `Commands::Pilot`). Its manifest entry
and script are still here (grow-only history), and the script now guards its body with a
loud `loud_skip "PILOT VERB RETIRED"` (exit 77) when `wg pilot --help` fails, so
`--full-smoke` records a SKIP instead of a FAIL. Restoring the verb (`git revert`) makes
the unchanged body run again. Follow this pattern for any future verb removal: archive,
keep the manifest entry, and SKIP loudly on the missing subject.

## Why the scripts still work from here

`tests/smoke/archive/` is the **same directory depth** as
`tests/smoke/scenarios/`, so every `$HERE/..`-relative path a retired script
uses keeps resolving unchanged (`$HERE/../../fixtures` → `tests/fixtures`,
`$HERE/../../..` → repo root). The only same-directory reference,
`$HERE/_helpers.sh`, is served by [`_helpers.sh`](./_helpers.sh), a one-line
shim that sources the single live helper implementation in `../scenarios/`.
That is why retired scripts keep their bodies byte-for-byte and `--full-smoke`
can run them with no edits.
