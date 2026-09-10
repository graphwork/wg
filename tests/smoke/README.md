# Smoke gate (`wg done` regression contract)

This directory holds the **smoke manifest** — a structured TOML file plus a
set of shell scripts. The manifest is the regression contract for `wg done`:
**a task cannot be marked done while a smoke scenario it owns is failing.**

## How it works

`wg done <task-id>` does the following before mutating state:

1. Loads `tests/smoke/manifest.toml` (path overridable with
   `WG_SMOKE_MANIFEST=...`).
2. Selects scenarios whose `owners = [...]` list contains the task id, OR
   every scenario when `--full-smoke` is passed.
3. Runs each selected scenario with `bash <script>`.
4. Inspects the exit code:
   * **0** → PASS
   * **77** → loud SKIP (precondition missing — endpoint unreachable, no
     credentials, etc.); does not block `wg done`
   * **anything else** → FAIL; `wg done` exits non-zero with the broken
     scenario name(s)

Agents (`WG_AGENT_ID` set in env) cannot bypass the gate via `--skip-smoke`
unless `WG_SMOKE_AGENT_OVERRIDE=1` is also set in the same shell. This is
deliberate — agents claiming done is the failure mode the gate exists to
prevent.

Humans can pass `--skip-smoke` (a loud warning is printed) when they
understand why a particular scenario is not load-bearing for their change.

## Adding a scenario (this is grow-only)

Every regression that should have been caught by smoke gets a permanent
scenario here. Do not delete entries; extend.

1. Drop a script under `tests/smoke/scenarios/<name>.sh` that exits
   0/77/non-zero per the contract above.
2. Add a `[[scenario]]` block to `manifest.toml` with `name`, `script`
   (relative to the manifest), `owners` (the task id(s) this scenario
   protects), and `description`.
3. List `smoke-gate-is` in `owners` so the manifest's own ground-truth
   scenarios always run when modifying the gate.
4. Source `_helpers.sh` for the `loud_skip` / `loud_fail` / `require_wg` /
   `endpoint_reachable` helpers — the SKIP/FAIL banners are greppable.

## Fixture lifecycle (cleanup contract — REQUIRED)

Smoke scenarios spawn daemons, worker wrappers, observers, fake providers,
and real Pi processes. These can double-fork, start a new session, ignore TERM,
or outlive a failed assertion. Historically, Pi workers from the removed
`pi_threshold_compaction_same_process_kick` scenario survived with PPID 1 and
open deleted log/cwd file descriptors.

The Rust harness and `_helpers.sh` enforce one ownership contract:

* **Always use `make_scratch`** — never `mktemp -d` directly. Scratch dirs
  live under a single shared root (`${WG_SMOKE_ROOT:-${TMPDIR:-/tmp}/wgsmoke}`)
  so cleanup consumes explicit per-run scratch registries, not a glob hunt
  across `/tmp`. Registry files live inside the durable ownership directory.
* **Every manifest invocation receives an exact ownership identity.** The
  harness creates a random `WG_SMOKE_RUN_ID`, records the scenario root PID,
  immutable `/proc` start ticks, process group, and session, and launches the
  scenario in a new session. All ordinary descendants inherit the marker.
  `env -i` calls should preserve `WG_SMOKE_RUN_ID` and `WG_SMOKE_SCENARIO`;
  a process that intentionally sanitizes its environment must first be
  registered with a complete immutable PID/start identity.
* **Always use `start_wg_daemon`** — never `wg service start &; daemon_pid=$!`.
  The helper records both the start wrapper and the canonical daemon PID from
  `service/state.json`. For other background fixtures use
  `start_owned_process <role> <log> <command...>`; it creates a separate
  session whose leader stops before exec, publishes the exact registration,
  and resumes only after that durable authority exists.
* **Never install your own `EXIT`/`ERR`/`INT`/`TERM` trap.** `_helpers.sh` owns
  those paths and tears down the entire exact ownership set. If you need extra
  cleanup (for example a tmux session), register it with `add_cleanup_hook`.
* **Cleanup is ordered and bounded.** Harness entry probes readable Linux
  `/proc` stat/environ support before spawn, and a later `/proc` scan failure is
  an error that retains owner evidence rather than an empty-success result.
  Graceful service stop runs first, then the exact marker set is rescanned
  through TERM and KILL (catching respawns),
  direct/adopted children are reaped, and only then are scratch directories
  deleted by the Rust subreaper. If anything survives, scratch and its owner
  record are retained with bounded diagnostics
  naming scenario, PID, start ticks, PPID, PGID, and SID.
* **`wg_smoke_sweep` and the Rust pre/post sweep use owner records, not command
  names.** A process is signal-eligible only when `/proc/*/environ` contains
  both exact run markers immediately before signaling, or when the helper
  registered its PID/start tuple while those markers were still readable and
  cleanup revalidates that immutable tuple. An unrelated process named `pi` is
  therefore never a candidate. A dead PID+start supervisor is swept immediately; a live exact
  supervisor is never swept, and incomplete/legacy records remain evidence
  rather than becoming destructive-cleanup authority merely through age.

The regression tests are `smoke_cleanup_survives_panic.sh` (trap-defeating
SIGKILL backstop) and `smoke_process_ownership_cleanup.sh` (the historical Pi
leak replacement: TERM-ignoring double-forked Pi, observer, respawning daemon,
success/assertion/timeout/SIGINT/SIGTERM paths, repetition baseline, a
registered child that erases its environment, an unrelated concurrent `pi`
survivor, and a concurrently healthy owned scenario that a stale-record sweep
must not terminate).

## Live, not stubs

Scenarios MUST hit real endpoints / real binaries. The original wave-1 smoke
silently passed against a fake LLM and that's exactly how the wg-nex 404
shipped to users. If you need a stubbed scenario, write a unit test instead
— do not put it in this manifest.

## No eyeball gates

Every scenario MUST produce a programmatically-assertable text or data
stream — never "human looks at the terminal and judges." Each script states
the expected output (literal text, JSON shape, file content, log line) and
asserts on it. "Did not crash" is not enough. "Returned a non-error" is not
enough on its own — also assert the positive marker (role=coordinator, the
expected file appears, the expected substring is in the log, etc.).

If you cannot articulate the expected output as a grep/jq/diff, the scenario
is not ready to ship. Recent example: a Log view bug shipped because the fix
worked at the file layer but the rendering pipeline silently dropped lines —
no scenario asserted "after opening the Log view, output contains lines
{1..N} of the expected text." That's exactly the gap this manifest exists to
close.

## Initial scenarios

| Scenario | Protects | What it does |
|---|---|---|
| `nex_two_message_against_lambda01` | wg-nex-native* | Live two-message chat against lambda01 (qwen3-coder) |
| `dispatcher_boot_no_orphan_supervisor` | rename-dispatcher-daemon, bug-a-regression-test | Boots dispatcher, asserts no orphan / ghost coordinator entry |
| `claude_executor_with_global_openrouter_default` | model-is-not, wire-priority-field | Local claude + global openrouter is_default → no native-exec leak |
| `priority_int_and_string_deserialize` | wire-priority-field | graph.jsonl with int/string/map priority forms reads cleanly |
| `chat_create_via_ipc_works` | wg-nex-native, fix-tui-coordinator-2, fix-tui-new | Single `wg chat 'hi'` succeeds against a fresh claude-executor project |
