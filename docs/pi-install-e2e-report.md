# End-to-end test report: pi ↔ wg setup (install technique reliability)

**Task:** `test-end-to` — end-to-end test the whole pi↔wg setup produced by the
chained design (`docs/design-pi-plugin-install.md`) + impl (`implement-pi-plugin`)
tasks, proving the install technique is reliable from a clean state, in **both**
integration directions.

**Date:** 2026-06-23 · **wg compat:** `WG_PI_PLUGIN_COMPAT_VERSION = 0.1.0` ·
**pi:** 0.79.10 · **node:** v25.4.0

**Binary under test:** a debug `wg` built from this worktree
(`cargo build --bin wg`, copied to `/tmp/pi-e2e/wg`). The cache `dist/index.js`
exercised in every scenario is **byte-identical to the committed
`pi-plugin/embedded/dist/index.js`** (sha256 `720a80a3…c491b3`), so the tests
ran against the real shipped artifact, not a side build.

## Summary

| # | Scenario | Verdict | Mode |
|---|----------|---------|------|
| 1 | Clean hermetic handler spawn (`wg → pi`) | **PASS** | live OpenRouter + credential-free real-pi |
| 2 | Profile-driven install (`wg profile use pi`, declarative) | **PASS** | pure-wg + live OpenRouter (console auto-discovery) |
| 3 | Model bridge (`/wg-model` warm swap) | **PARTIAL** (swap + visibility PASS; `model_override` persistence is a known downstream gap) | live OpenRouter |
| 4 | Idempotency / self-heal | **PASS** | pure-wg |
| 5 | Compat mismatch fails loud | **PASS** | credential-free real-pi + faux-host |
| 6 | Node-less user path | **PASS** | pure-wg (no node/npm/pi on PATH) |

**Headline (primary reliability claim, scenario 1):** with **no `~/.pi` install
present**, `wg pi-handler` materializes the embedded, version-locked plugin into
the cache and spawns `pi --mode rpc … -e <cache>/0.1.0/dist/index.js -ne`,
without ever writing a global `~/.pi` plugin entry; a live pi at that identical
argv round-trips `wg_add` back into the isolated graph.

**No silent "it all worked":** the gaps and caveats are called out explicitly in
[§Gaps & caveats](#gaps--caveats). The most important is scenario 3's
`CoordinatorState.model_override` persistence, which does **not** function in
this build (the `wg chat model` verb is a separate downstream task).

## Isolation methodology

Every scenario ran against an isolated `HOME` + `XDG_CACHE_HOME` + `--dir`
project under `/tmp/pi-e2e/<scenario>/`, with inherited worker env stripped
(`env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID -u
WG_DIR -u WG_PROJECT_DIR -u WG_PI_PLUGIN_DIR`). This deliberately avoids the
global daemon in `/home/bot/wg` (project memory: *manual wg commands inside
`/home/bot/wg` hit the global daemon*). `WG_PI_PLUGIN_FORCE_CACHE=1` forces the
embedded→cache (user) source even though the tests run from inside the checkout
(where the Dev source would otherwise win, per Decision 4).

The OpenRouter live route used the key at `/home/bot/.openrouter.key`. Live model
turns used `openai/gpt-4o-mini` (tool-capable). Note `anthropic/claude-3.5-haiku`
returns `404 No endpoints found` on this OpenRouter account, so it is NOT usable
for live completions even though it appears as a placeholder model id elsewhere.

---

## Scenario 1 — Clean hermetic handler spawn (`wg → pi`) — **PASS**

**Claim:** with no `~/.pi` install, `wg pi-handler` runs end-to-end; the plugin
loads from the cache/embedded build and round-trips a wg verb back into the graph.

### 1a — Real `wg pi-handler` hermetic spawn (credential-free reliability core)

From a clean `HOME` (no `~/.pi`) + clean cache:

```
wg init -m openrouter:anthropic/claude-3.5-haiku --no-agency
wg pi-handler --chat chat-1 -m pi:openrouter/openai/gpt-4o-mini   # WG_PI_PLUGIN_FORCE_CACHE=1
```

`handler.log` evidence:

```
pi-handler: ensured plugin source=Cache compat=0.1.0 entry=/tmp/pi-e2e/s1a/cache/wg/pi-plugin/0.1.0/dist/index.js has_node_modules=false
pi-handler: spawning `/home/bot/.nvm/.../pi --mode rpc --provider openrouter --model openai/gpt-4o-mini --session-id chat-1 --session-dir …/pi-sessions --no-approve -e /tmp/pi-e2e/s1a/cache/wg/pi-plugin/0.1.0/dist/index.js -ne`
```

- `source=Cache` — embedded bundle materialized into the versioned cache.
- spawn argv carries `-e <cache>/0.1.0/dist/index.js -ne` (hermetic load).
- **Hermetic confirmed:** after the run, `~/.pi/agent/settings.json` is **absent**.
  The only thing under `~/.pi` is `agent/auth.json`, which **pi itself** creates
  at startup — `wg` never wrote a global plugin entry.

### 1b — Live wg-verb round-trip at the identical hermetic argv

Driving real `pi --mode rpc -e <cache>/0.1.0/dist/index.js -ne` (OpenRouter,
`openai/gpt-4o-mini`) and prompting for `wg_add`:

```
{"events": 38, "saw_wg_add_tool_event": true}
graph AFTER: [ ] pi-e2e-roundtrip - PI_E2E_ROUNDTRIP_OK   ← round-tripped into the isolated graph
```

### 1c — Full `wg pi-handler` inbox→pi→outbox→graph round-trip

Pre-seeding `chat-1/inbox.jsonl` and running the real handler, with `WG_DIR`
present in the handler env:

```
pi-handler: processing inbox id=1 request_id=r1
graph: [ ] handler-rt2-ok - HANDLER_RT2_OK   ← wg verb landed via the real handler loop
```

**Verdict:** PASS. Hermetic spawn (1a) + live verb round-trip (1b) + full handler
loop (1c) together prove the claim.

> **Robustness finding (not a prod defect):** the handler does **not** add `WG_DIR`
> to the explicit child `secret_env`; it relies on `WG_DIR` being inherited
> ambiently. The production dispatch path sets it (`spawn_task.rs:411`
> `cmd.env("WG_DIR", workgraph_dir)`), so the pi child receives it and the
> plugin's `wg --dir <project>` binds the right graph. When I simulated a **bare**
> `wg pi-handler` with `WG_DIR` unset (and pi does not preserve the project cwd at
> tool-exec time), the plugin's `wg add` (no `--dir`) landed in **no** graph and
> the round-trip silently missed. Belt-and-suspenders follow-up: have the handler
> inject `WG_DIR` into `secret_env` explicitly (filed as a follow-up task).

---

## Scenario 2 — Profile-driven install (declarative) — **PASS**

**Claim:** from clean `~/.pi` + cache, `wg profile use pi` leaves pi usable with
**no manual step** (plugin present + version-matched). Verify by launching a pi
console and calling a `/wg`/`wg_*` tool.

```
$ wg profile use pi
Active profile: pi. …
  Ensured wg-pi-plugin (compat 0.1.0): /tmp/pi-e2e/s2/cache/wg/pi-plugin/0.1.0/dist/index.js
```

```
$ wg pi-plugin status
  source:        Cache (embedded → versioned cache)
  build ready:   yes
  pi settings:   …/.pi/agent/settings.json
  console wired: yes
```

`~/.pi/agent/settings.json` → `{"extensions":["…/cache/wg/pi-plugin/0.1.0/dist/index.js"]}`,
version-matched (`version.json.compat == 0.1.0`).

**Console auto-discovery load (the `pi → wg` direction):** a real
`pi --mode rpc` launched with **no `-e/-ne`** (so it auto-discovers the plugin
from `settings.json`) loads the plugin and round-trips a tool:

```
tool_execution_start  toolName=wg_add args={"title":"CONSOLE_RAW_OK"}
tool_execution_end    result: "Added task: CONSOLE_RAW_OK (console-raw-ok)"
agent_end
graph: [ ] console-raw-ok - CONSOLE_RAW_OK
```

A no-tool turn additionally showed **~9970 input tokens**, reflecting the wg tool
schemas registered into the session — independent proof the plugin auto-loaded.

**Verdict:** PASS (pure-wg wiring + live console auto-discovery tool round-trip).

---

## Scenario 3 — Model bridge (`/wg-model` warm swap) — **PARTIAL**

**Claim:** `/wg-model <handler:native-model>` performs a warm in-session swap that
round-trips into `CoordinatorState.model_override`; the resolved handler is
surfaced (closes the invisibility gap behind today's 401).

**What works (live, via real pi RPC with the plugin loaded):**

- **Warm in-session swap — PASS.** `/wg-model openrouter:openai/gpt-4o`
  (sent as an extension command) changes the session model. `get_state` confirms:

  ```
  model_before: openrouter:openai/gpt-4o-mini
  model_after:  openrouter:openai/gpt-4o
  warm_swap_happened: true
  ```

- **Visibility — PASS (closes the 401 invisibility).** The command surfaces the
  resolved model:

  ```
  {"type":"extension_ui_request","method":"notify","message":"Model set to openrouter:openai/gpt-4o","notifyType":"info"}
  ```

  The failure branch (`pi.setModel` returns false) surfaces `No API key for
  <provider:id>` instead of a silent failure — this is the visibility fix.

- **Registration — PASS.** Both `/wg` and `/wg-model` are registered
  (host selftest: *"8 tools, 2 commands (/wg + /wg-model present)"*; the
  `pi_plugin_load_contract` smoke pins the same).

- **Bridge mapping — PASS (unit).** `model-bridge.test.ts` (8 tests) verifies
  `model_select → backend.setModelOverride("provider:id")`, the restore-skip, and
  the no-throw-on-backend-error contract.

**What does NOT work — `CoordinatorState.model_override` persistence (GAP):**

The plugin's `WgBackend.setModelOverride` shells **`wg chat model <chat> <spec>`**
— a verb that **does not exist** in this build:

```
$ wg chat model chat-1 claude:opus
error: the subcommand 'chat-1' cannot be used with '[MESSAGE]'
```

So the warm swap is **not** persisted: after `/wg-model`, no
`CoordinatorState.model_override` is written (no coordinator `state.json`
produced). Two compounding facts:

1. `wg chat model` is the downstream **`pi-plugin-impl-chat-model-verb`** task,
   not yet landed (the design and `wg-backend.ts:165` both flag it as such).
2. `setModelOverride` returns the non-zero `ExecResult` **without throwing**, so
   the model-bridge's `catch` never fires — the write-back is a **silent no-op**
   (no `model write-back failed` log was observed either).

**Verdict:** PARTIAL. The warm swap + visibility (the parts that close the 401
invisibility) work and are live-proven; the `model_override` round-trip is a
known, documented downstream gap. See [§Gaps & caveats](#gaps--caveats).

---

## Scenario 4 — Idempotency / self-heal — **PASS**

- **Idempotent:** a second `wg profile use pi` left `settings.json`
  **byte-identical** (sha256 unchanged) — see scenario 2.
- **Self-heal:** corrupting the cache (`printf '// CORRUPTED' > dist/index.js`;
  `rm .wg-ok`) is detected and repaired:

  ```
  post-corruption: wg pi-plugin status → build ready: NO — run `wg pi-plugin install` to repair
  after re-install:  index.js repaired (no longer corrupted); .wg-ok restored; build ready: yes
  ```

- **Rust unit cover:** `cargo test --lib pi_plugin` → **10 passed / 0 failed**,
  incl. `test_corrupted_cache_is_detected_and_repaired`,
  `test_console_mode_wires_settings_idempotently`,
  `test_stale_version_dir_is_gced`, `test_materialize_cache_extracts_then_is_noop`.

**Verdict:** PASS.

---

## Scenario 5 — Compat mismatch fails loud — **PASS** (critical validation item)

**Through real pi** (`-e <cache dist> -ne`, no credentials needed — the plugin
asserts at load before any model connection), a skewed
`WG_PI_PLUGIN_COMPAT_VERSION` fails **loudly and early**:

```
compat mismatch: plugin=0.1.0 wg=9.9.9-skew. The loaded pi plugin build does not
match the wg binary that spawned it; run `wg pi-plugin install` to repair …
```

- error contains `compat mismatch` ✓ and **names the skewed wg version**
  (`9.9.9-skew`) ✓.
- matching version → loads cleanly (no `Failed to load extension`) ✓.

**Through the Topology-B Node host** (`--force-compat-mismatch`, in-process,
auth-less faux registry) the same tripwire fires as an `extensionsResult.errors`
entry:

```
wg-pi-host compat-mismatch selftest OK: tripwire fired loudly —
  wg-pi-plugin compat mismatch: plugin=0.1.0 wg=9.9.9-deliberate-mismatch …
```

**Verdict:** PASS. This is the regression guard against the silent-skew class that
produced the glm-5.2 401 — the failure is non-silent, early (at extension load),
and names both versions.

---

## Scenario 6 — Node-less user path — **PASS**

Simulating a cargo-installed user with **no node/npm/pi** on PATH (a minimal
`PATH` of coreutils + `wg` only):

```
node on minimal PATH? NONE   npm? NONE   pi? NONE
$ wg pi-plugin install
Installed wg-pi-plugin (compat 0.1.0) from embedded → versioned cache.
  extension: …/cache/wg/pi-plugin/0.1.0/dist/index.js
  wired into pi settings: …/.pi/agent/settings.json
```

`dist/index.js` extracted ✓, `.wg-ok` stamp ✓, `settings.json` wired ✓,
`status` → `source: Cache / build ready: yes / console wired: yes`. The embed is
the source of truth; no toolchain is touched at install time (matches the design's
Decision 1: `cargo install --path .` stays node-free).

**Verdict:** PASS.

---

## Faux-provider host vs live route

| Evidence | Mode |
|---|---|
| Scenario 2 install/wiring/idempotency; scenario 4 self-heal; scenario 6 node-less | **pure `wg` binary**, no node/pi/creds |
| Scenario 1a hermetic spawn argv; scenario 5 real-pi mismatch | **real pi**, credential-free (plugin loads before model connection) |
| Scenario 1b/1c verb round-trips; scenario 2 console auto-load tool round-trip; scenario 3 warm swap | **live OpenRouter** (`openai/gpt-4o-mini`) |
| Scenario 5 host tripwire; `/wg`+`/wg-model` registration | **faux-provider host** (`pi-plugin/host/wg-pi-host.mjs --selftest` / `--force-compat-mismatch`), in-process, auth-less |

---

## Gaps & caveats (explicit — no silent "it all worked")

1. **Scenario 3 `model_override` persistence does NOT work (downstream gap).**
   `wg chat model <chat> <spec>` is absent (the `pi-plugin-impl-chat-model-verb`
   task). The warm swap + visibility work; the persistence into
   `CoordinatorState.model_override` is a **silent no-op** because
   `setModelOverride` returns the failing `ExecResult` without throwing. Until the
   verb lands, a `/wg-model` swap will **not** survive a WG-side respawn.
   *Recommended follow-ups:* (a) implement `wg chat model`; (b) make
   `setModelOverride` treat a non-zero exit as an error so the failure is at least
   logged rather than silent.

2. **Handler `WG_DIR` propagation is implicit.** `wg pi-handler` relies on ambient
   `WG_DIR` inheritance to reach the pi child (production sets it via
   `dispatch_pi`); it is not in the explicit `secret_env`. A bare `wg pi-handler`
   without `WG_DIR` + pi not preserving cwd ⇒ the plugin's verbs miss the graph.
   *Follow-up:* inject `WG_DIR` into `secret_env` explicitly.

3. **Live model tool-calls are nondeterministic.** `openai/gpt-4o-mini`
   sometimes replied without calling the requested tool (e.g. some scenario-1c
   handler turns answered "DONE"/"FINISHED" with no `wg_add`). This is model
   behaviour, not a plugin defect — the same model+plugin called `wg_add`
   reliably at the identical `-e/-ne` argv. The automated gate therefore does
   **not** assert on a model-initiated tool call; the live round-trips are
   documented here and in `pi_plugin_live_validation`.

4. **Test-harness learning — drain pi's stderr.** A Python driver that left
   `stderr=PIPE` unread **deadlocked pi** once the auto-discovery path's larger
   stderr exceeded the ~64KB pipe buffer (it masqueraded as a "stall": only the
   `response` ack, no turn). The `-ne` path emits less stderr so it slipped under
   the limit. All drivers must drain stderr (the committed scenario redirects pi
   output to files). This is a harness bug, not a product issue.

5. **`anthropic/claude-3.5-haiku` is not live-usable** on this OpenRouter account
   (`404 No endpoints found`). It only works for the credential-free *load* checks
   (the plugin loads before connecting), not for completions.

6. **Console auto-discovery + large cwd context.** Driving the "console" extension
   path through headless RPC from a cwd containing large `CLAUDE.md`/`AGENTS.md`
   context files was slow/contended in the harness; running from a clean cwd (and
   pinning the graph with `WG_DIR`) is the reliable shape. Attended-pi console use
   (the real Console target) is unaffected.

---

## Automated test added

`tests/smoke/scenarios/pi_install_e2e.sh` (owner `test-end-to` in
`tests/smoke/manifest.toml`, grow-only) gates the lifecycle entry points **not**
already pinned by `pi_plugin_install_hermetic`:

- **[2] profile-driven Console install** + version-match + idempotency (always-on, pure-wg).
- **[6] node-less embedded install** (always-on, pure-wg, asserts no node/npm/pi on PATH).
- **[1] hermetic `wg pi-handler` spawn** — `source=Cache`, `-e <cache>/<compat>/dist/index.js -ne`, no `~/.pi` wiring (live, credential-free; SKIP without a `pi` binary).
- **[5] loud compat mismatch** through real pi naming the skew (live, credential-free; SKIP without a `pi` binary).

The credentialed, model-nondeterministic wg-verb round-trips are intentionally
**not** gated here (they live in this report + `pi_plugin_live_validation`).

Local run (with `pi` present):

```
pi_install_e2e: [2] profile-driven Console install + version-match + idempotency PASS
pi_install_e2e: [6] node-less embedded install PASS
pi_install_e2e: [1] hermetic wg pi-handler spawn (source=Cache, -e<cache>-ne, no ~/.pi wiring) PASS
pi_install_e2e: [5] loud compat-mismatch through real pi PASS
pi_install_e2e: PASS (compat=0.1.0)
```

### Other automated cover exercised in this report

- `cargo test --lib pi_plugin` → **10 passed / 0 failed** (embed==const single
  source, hermetic-never-touches-`~/.pi`, atomic extract/no-op, GC, self-heal,
  idempotent console wiring).
- `npm --prefix pi-plugin test` → **3 files / 14 tests passed** (model-bridge
  write-back mapping, plugin registration, api-signature).
- `pi-plugin/host/wg-pi-host.mjs --selftest` and `--force-compat-mismatch` both
  exit 0 (registration + loud tripwire, auth-less).
- Existing scenarios `pi_plugin_install_hermetic`, `pi_plugin_load_contract`,
  `pi_plugin_live_validation` remain the companion gates.

## Reproduction

The driver scripts used for the live scenarios are under `/tmp/pi-e2e/`
(`pi_rpc_roundtrip.py`, `run_handler4.sh`, `s3_model_bridge.py`) with captured
output under `/tmp/pi-e2e/evidence/`. The durable, repeatable artifact is
`tests/smoke/scenarios/pi_install_e2e.sh` (run via the smoke gate or directly
with `WG_SMOKE_SCENARIO=pi_install_e2e bash …`).
