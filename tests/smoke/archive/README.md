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

## What was deliberately KEPT

Retirement filters on the scenario's **subject**, not on a raw keyword scan.
~33 scenarios that merely *mention* or incidentally configure a non-pi handler
as a credential-free fixture — but assert graph / service / dispatch /
admission / worktree / TUI / secret / profile / html behavior — stay on the
default gate. Examples: `retry_kills_in_progress_agent`, `reset_clears_claim`,
`reaper_retry_in_place_protected`, `wg_show_retry_attempts`,
`isolation_worktree_collision_transaction`, `tui_log_pane_renders_raw_stream`,
`secret_keyring_round_trip`, `wg_html_*` (owned by `retire-html-renderer`).

The reduction plan's "**~155**" was a raw-mention upper bound; the reviewed
subject-based retire set is **98** (20 spark + 78 handler). The residual ~33
fixture-route scenarios are tracked by the follow-up task
`port-incidental-smoke-routes-to-pi`: port the assertion to a `pi:` route first,
then retire, so no behavior coverage is lost.

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
