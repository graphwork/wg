# Model and execution plane audit

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369` (inherited from the audit charter)  
**Evidence checked through:** 2026-08-08  
**Execution revision:** `98b319c36aa8a21fd4506fc7469fe6d58978cdda`  
**Freshness:** snapshot-current; `git diff --quiet b0892ea7..98b319c3 -- <audited source/test/docs>` returned 0, so the two intervening audit-charter commits did not change the evidence cited here  
**Scope:** model routing, configuration/profile/tier/reasoning precedence, Pi/Claude/Codex/native/OpenCode handlers, discovery, worker processes and wrappers, Pi streaming/watchdogs, usage/cost, credentials/fallbacks, deprecations, and documentation drift  
**Change boundary:** this new audit artifact only

## 1. Executive abstract

**`[FACT]`** Current unattended service admission is narrower than WorksGood's handler and executor catalog. Exact `pi:<provider>:<model>`, `claude:<model>`, and `codex:<model>` routes pass the execution-plane validator; nex/native, OpenCode, other external CLIs, leading-provider routes, and bare aliases do not (`src/config.rs:2395-2433,3590-3710` (`parse_supported_execution_route`, `resolve_execution_route_for_role`, `validate_execution_model_plane`); `src/commands/spawn/execution.rs:984-1046` (`execute_spawn_plan` preflight)). The broad resolver and live-chat layer still recognize those additional handlers (`src/dispatch/handler_for_model.rs:76-137`; `src/commands/spawn_task.rs:145-224,280-405`).

**`[VERIFIED]`** Focused tests executed on 2026-08-08 passed: 14 handler-routing unit tests; Pi stream deduplication and raw-stream accounting tests; 19 Pi-watchdog integration tests; 8 Pi-sole-model-plane tests; 6 two-tier-profile tests; 6 executor-taxonomy tests; two agency fallback tests; and one Pi process-argv test. Exact commands, environment, exit statuses, and bounded results are in section 7. No real provider or credential was invoked.

**`[FACT]`** The worker pipeline has strong positive controls: explicit route and reasoning propagation, a transactional launch gate, dedicated raw and canonical streams, provider-failure classification, completion/no-work gates, and a Pi watchdog whose continuation path is evidence-based and same-session (`src/dispatch/plan.rs:387-613,650-809`; `src/commands/spawn/execution.rs:1308-1438,3430-3707`; `src/pi_watchdog/mod.rs:1-15,1274-1730`). Pi usage translation counts only authoritative `turn_end.message.usage`, avoiding repeated update/end snapshots (`src/stream_event.rs:410-690,1125-1152`; `src/graph.rs:1459-1587`).

**`[CONTRADICTION]`** The ordinary unattended Pi task path does not use the documented hermetic `wg pi-handler` worker topology. Normal task execution constructs `pi --mode json`, then adds provider/model/thinking; it does not explicitly pass the embedded extension or plugin compatibility environment (`src/service/executor.rs:1729-1752`; `src/commands/spawn/execution.rs:1308-1403,3457-3483`). By contrast, `wg pi-handler` uses `pi --mode rpc -e <embedded plugin> -ne` and injects compatibility state (`src/commands/pi_handler.rs:504-537,855-902,1012-1040`). The operator quickstart calls the latter the path for “WG-spawned workers” (`docs/quickstart-pi-openrouter.md:209-217` [DOC-CLAIM; undated at snapshot]).

**`[INFERENCE]`** The highest bounded risk is an operator configuring or troubleshooting the wrong execution surface: a handler can be discoverable and usable for attended chat yet invalid for unattended work; a Pi task can run without the documented invocation-scoped plugin handshake; and old fallback/deprecation text can predict behavior that strict admission now refuses. Confidence is high for the source topology and medium for production impact because no live provider process was exercised. A falsifying check for the Pi concern is an end-to-end daemon-worker test proving that the actual `pi --mode json` child receives and validates the exact embedded extension despite the argv/environment construction identified here.

**`[RECOMMENDATION]`** The next decision is P0: publish and test one surface-specific capability matrix, then choose whether ordinary Pi task workers must explicitly load the embedded plugin or whether the hermetic-worker claim must be narrowed. See `MODEL-REC-001` and `MODEL-REC-002`.

## 2. Scope and map

### 2.1 Boundaries and execution surfaces

**`[FACT]`** This audit follows configuration from persisted inputs through `Config::load_merged`, role/tier/reasoning resolution, `plan_spawn`, strict service/worker admission, executor settings, process argv, wrapper files, raw/canonical events, and terminal accounting. It separately maps attended/live handlers and agency one-shots because they share route vocabulary but not admission or process topology.

**`[FACT]`** Remote-provider placement is only mapped at its model-plane seam. `ExecutorKind::RemoteRunner` is provider-plane driven and is not inferred from an ordinary handler prefix (`src/dispatch/handler_for_model.rs:45-74`; `src/dispatch/plan.rs:103-119,580-642`). Provider protocol and lease correctness belong to audit artifact 15.

**`[UNCERTAINTY]`** This audit excludes real provider responses, OS credential-store behavior, package installation on a clean host, Windows process execution, and long-duration watchdog behavior. Their absence is an evidence gap, not a failure result.

### 2.2 Route-to-surface matrix

**`[FACT]`** Handler-first lexical routing interprets the leading colon token as handler identity and leaves the remainder in that handler's dialect. Pi later splits the inner route into provider and model (`src/dispatch/handler_for_model.rs:98-137`; `src/commands/pi_handler.rs:82-158`). The effective capabilities are surface-specific:

**`[FACT]`** `config::HANDLER_PREFIXES` is not a complete handler catalog: it contains only Claude, Codex, nex, and native for provider-warning discrimination, while `handler_for_model` recognizes Pi and other `ExecutorKind` external CLIs (`src/config.rs:2523-2539`; `src/dispatch/handler_for_model.rs:87-108`). Treating that constant as discovery or admission authority would be incorrect.

| Evidence | Route | Lexical handler | Unattended service/worker | Other reachable surface | Qualification |
|---|---|---|---|---|---|
| `[FACT]` | `pi:<provider>:<model>` | Pi | **admitted** with effective reasoning | RPC handler and attended interactive Pi | Worker is `--mode json`; RPC handler is `--mode rpc`; TUI is interactive. |
| `[FACT]` | `claude:<model>` | Claude | **admitted** | Claude live handler/session | Strict worker admission requires the prefix; lenient bare aliases remain elsewhere. |
| `[FACT]` | `codex:<model>` | Codex | **admitted** | Codex live handler/session | CLI owns authentication and emits retained event output. |
| `[FACT]` | `nex:<model>` / `native:<model>` | native/nex | **rejected** by current validator | live native `wg nex --chat ...` is constructible | The shipped nex profile and registry survive as compatibility material. |
| `[FACT]` | `opencode:<model>` | OpenCode | **rejected** by current validator | attended/live OpenCode is supported | Handler requires a resolved model and replays prior turns. |
| `[FACT]` | `aider:`, `goose:`, `qwen:`, `gemini-cli:`, etc. | external CLI | **rejected** by current validator | discovery/registry and selected live surfaces | Enumeration is not dispatch authority. |
| `[FACT]` | `openrouter:<model>` or another leading provider | leniently maps toward nex/native | **rejected** by strict worker admission | migration can canonicalize it | Warning/migration phase and admission phase are not aligned. |
| `[FACT]` | bare alias or unqualified model | legacy resolver may infer | **rejected** by strict worker admission | attended Pi can intentionally omit a model | Attended Pi selection is independent of unattended automation. |
| `[FACT]` | task `execution.shell` | no model handler | explicit shell branch | shell process | Deliberate graph-only escape hatch. |
| `[FACT]` | provider placement | `RemoteRunner` | provider plane | remote grant/run/result | Not selected by normal model prefix. |

**`[FACT]`** `executor_discovery::discover()` enumerates many binaries and availability hints (`src/executor_discovery.rs:40-188`). Pi discovery separately reports binary, embedded-plugin, and optional dev-host status (`src/executor_discovery.rs:222-288`). Neither function is the current unattended admission authority; `validate_execution_model_plane()` is.

### 2.3 Configuration and precedence table

**`[FACT]`** The following precedence is encoded at the named boundary. “Highest first” does not mean every input is current product policy; executor fields remain compatibility inputs.

| Evidence | Decision | Highest-to-lowest precedence | Enforcement site / note |
|---|---|---|---|
| `[FACT]` | Base config merge | local `.workgraph/config.toml` → global `~/.wg/config.toml` → default structures | `src/config.rs:5990-6043,6060-6145` (`load_merged_toml_value`, `Config::load_merged`). |
| `[FACT]` | Project-scoped `profile select` | merge global+local, then selected profile replaces routing/model/reasoning; non-routing local settings remain | `src/config.rs:5990-6043,6060-6145`; selection is fingerprint-pinned at `src/commands/profile_cmd.rs:704-847`. |
| `[FACT]` | Legacy/global `profile use` | apply profile routing to global, optional model pin, clear local routing overrides, set active pointer, reload daemon | `src/commands/profile_cmd.rs:1046-1200`. This is distinct from project selection. |
| `[FACT]` | Role model | `[models.<role>].model` → explicit role tier → role default-tier route → `[models].default` → `dispatcher.model` → `agent.model` | `src/config.rs:3590-3669` (`configured_route_for_role`). |
| `[FACT]` | `plan_spawn` model | task model → supplied service/default-tier model → `coordinator.model` → `agent.model` | `src/dispatch/plan.rs:387-430`. Service normally supplies the resolved role model. |
| `[FACT]` | Executor compatibility input | task shell/command → task `exec_mode` → agency/live executor hint → dispatcher executor → default Pi floor; model compatibility and an explicit handler route then reconcile/override | `src/dispatch/plan.rs:387-493,650-809`. Model-first policy and compatibility mechanics should not be conflated. |
| `[FACT]` | Reasoning | task override → role explicit reasoning → role tier reasoning → role default-tier reasoning → default-model reasoning → omitted | `src/config.rs:3502-3553`; task override at `src/commands/spawn/execution.rs:1406-1438`. |
| `[FACT]` | Native endpoint | task URL/name → configured OpenRouter endpoint for an OpenRouter model → default endpoint → none | `src/dispatch/plan.rs:495-578`. Only native HTTP consumes it. |
| `[FACT]` | Native key | selected endpoint secret/env → endpoint-specific child environment | `src/config.rs:1217-1315,6151-6205`; `src/commands/spawn/execution.rs:1690-1751,2932-2978`. |
| `[FACT]` | Agency one-shot | explicit agency role → weak/fast tier; explicit fallback only within same execution system | `src/service/llm.rs:251-407,519-600`. |

**`[FACT]`** Task-agent role defaults to the standard tier; evaluator, assigner, FLIP, and reviewer roles default to fast (`src/config.rs:1749-1772`). Reasoning is an independent typed value and becomes Pi `--thinking`; it is not a model suffix (`src/config.rs:1508-1604,3502-3553`; `src/commands/spawn/execution.rs:1391-1403`).

**`[FACT]`** The default agent compatibility executor is Pi but its default model route is empty (`src/config.rs:5289-5303,5325-5340`). No-flag `wg init` deliberately follows its graph-only branch without writing a route (`src/commands/init.rs:87-123,247-266`). `wg service start` separately requires explicit selection and then validates all worker roles (`src/commands/service/mod.rs:1442-1475`).

### 2.4 Two config-to-process traces

#### Trace A — unattended Pi task

**`[FACT]`** A representative selected route `pi:openrouter:z-ai/glm-5.2` and strong reasoning `high` can originate in the Pi starter (`src/profile/templates/pi.toml:1-56`). Role resolution selects task-agent model/reasoning; `plan_spawn` selects `ExecutorKind::Pi` and strips the outer handler for executor use (`src/config.rs:3502-3710`; `src/dispatch/plan.rs:387-493,650-809`).

**`[FACT]`** The built-in worker base command is:

```text
pi --mode json -p "Complete the WG task prompt supplied on stdin."
```

**`[FACT]`** `build_inner_command_with_reasoning` then adds `--provider openrouter --model z-ai/glm-5.2 --thinking high` and pipes `prompt.txt` on stdin (`src/service/executor.rs:1729-1752`; `src/commands/spawn/execution.rs:1308-1403,3242-3360`). The child receives task/agent identity, `WG_EXECUTOR_TYPE`, inner `WG_MODEL`, `WG_REASONING`, and an opaque post-claim capability channel, while the raw graph path is removed (`src/commands/spawn/execution.rs:1650-1751,1830-1885`).

**`[FACT]`** The generated wrapper gates process start on durable claim/registry state, starts Pi and its observer, retains Pi stdout once in `raw_stream.jsonl`, invokes `wg pi-stream-bridge`, classifies provider failures, and reconciles task state (`src/commands/spawn/execution.rs:3457-3483,3550-3707`).

**`[VERIFIED]`** `cargo test --bin wg test_build_inner_command_pi_external_emits_model_and_thinking -- --nocapture` passed one test on 2026-08-08 and asserted the actual constructed command contains provider `openai-codex`, model `gpt-5.6-sol`, and thinking `high` (section 7.2).

#### Trace B — attended/live native and Pi

**`[VERIFIED]`** With a temporary native config, a paused task, and the live-surface hint `WG_EXECUTOR_TYPE=native`, snapshot-built `wg spawn-task native-live --dry-run` printed:

```text
[spawn_task] native-live: SpawnPlan executor=native (from agency.effective_executor), model=nex:audit-model (from local [dispatcher].model), endpoint=audit-local ([llm_endpoints] is_default)
wg nex --chat native-live -m nex:audit-model -e audit-local
```

**`[VERIFIED]`** With a separate exact Pi config, snapshot-built `wg spawn-task pi-chat --dry-run` printed:

```text
[spawn_task] pi-chat: SpawnPlan executor=pi (from model-route override: task.model requested executor=pi with inner model=openai-codex:gpt-audit), model=openai-codex:gpt-audit (from local [dispatcher].model (executor-qualified route pi:openai-codex:gpt-audit)), endpoint=none (none (executor=pi))
pi --provider openai-codex --model gpt-audit --session-id pi-chat --session-dir chat/pi-chat/pi-sessions
```

**`[FACT]`** These are live handler commands, not proof those routes pass unattended service admission. Attended Pi TUI execution additionally resolves an absolute Pi binary and appends `-e <exact embedded plugin>`, while intentionally leaving normal discovery enabled (`src/tui/viz_viewer/state.rs:21409-21491`). RPC `pi-handler` instead adds `--mode rpc -e <plugin> -ne` and compatibility environment (`src/commands/pi_handler.rs:504-537,855-902,1012-1040`).

**`[FACT]`** Claude and Codex have dedicated line/RPC-session handlers (`src/commands/claude_handler.rs:1-120`; `src/commands/codex_handler.rs:1-140`). OpenCode requires a resolved model, invokes `opencode run --format json --model ...`, and replays the conversation because it has no equivalent persistent server contract (`src/commands/opencode_handler.rs:1-23,142-194,236-317`).

### 2.5 Event, watchdog, usage, and credential flow

**`[FACT]`** Pi's worker event/accounting path is:

```text
Pi --mode json stdout
  └─ wrapper writes exact NDJSON once ───────────────► raw_stream.jsonl
                                                        ├─ live TUI/show parser
                                                        ├─ provider classifier/telemetry
                                                        ├─ graph token parser
                                                        └─ pi-stream-bridge
                                                             ├─ canonical stream.jsonl
                                                             └─ session-summary.md

wg done / wg fail ── parse output/raw stream ───────► task.token_usage
wg show ─────────── stored usage, then live files
wg spend ───────── stored usage on Done/Failed only
```

**`[FACT]`** Wrapper capture differs by executor: Claude/Codex stdout is teed into `raw_stream.jsonl` and `output.log`; native writes its canonical stream directly; Pi writes authoritative stdout once to `raw_stream.jsonl` and delegates canonical output to the bridge; shell/custom paths receive synthetic init/result bookends (`src/commands/spawn/execution.rs:3430-3511`).

**`[FACT]`** Pi repeats cumulative usage on multiple event types. Translation and graph accounting count only `turn_end.message.usage` once per turn and map `{input, output, cacheRead, cacheWrite, totalTokens, cost.total}` to canonical fields (`src/stream_event.rs:410-690`; `src/graph.rs:1459-1587`). Cost prefers Pi's non-zero reported total, then exact-model registry estimation, then zero. The built-in pricing table contains Claude/Codex entries but no Pi entries (`src/graph.rs:1694-1785`).

**`[FACT]`** `wg show` falls back to live parsing if stored usage is absent (`src/commands/show.rs:692-720`). `wg spend` only includes stored usage on `Done|Failed` tasks and assigns selected tasks to `Utc::now().date_naive()` rather than a task terminal timestamp (`src/commands/spend.rs:18-56`). `wg show`'s human display subtracts cache-read input with saturation while its structured fields retain the full values (`src/commands/show.rs:1871-1900`).

**`[FACT]`** The Pi watchdog projects meaningful provider/session/tool/process observations and applies exact guards before continuation (`src/pi_watchdog/mod.rs:1274-1590`). Production soft silence is 300 seconds and free/low-QoS hard thresholds cannot be below 900 seconds (`src/pi_watchdog/mod.rs:14-15,123-181`). Continuation is persisted/fenced and appends to the same session (`src/pi_watchdog/mod.rs:1600-1730`); the module contract explicitly denies it direct task-status authority (`src/pi_watchdog/mod.rs:1-5`).

**`[FACT]`** Credential ownership is handler-specific. Pi, Codex, OpenCode, and other external CLIs principally own their login state. Claude can use its own login or a resolved `[auth]` OAuth token injected as `CLAUDE_CODE_OAUTH_TOKEN`; leaked host-bridge variables are removed (`src/commands/spawn/execution.rs:1728-1740,3570-3579`). Native endpoint keys are resolved from endpoint secret/env configuration and injected at spawn (`src/config.rs:1217-1315,6151-6205`; `src/commands/spawn/execution.rs:1690-1751,2932-2978`).

## 3. Findings

### `MODEL-001` — execution capability is surface-dependent but presented as one catalog

**`[FACT]`** **State:** shipped/current. **Severity:** S2 Medium. **Likelihood:** observed in source and focused tests. **Confidence:** high. **Boundary:** operators configuring unattended workers and developers extending handlers. **Owner:** model-plane/configuration. The handler resolver and discovery registry recognize nex/native and external CLIs, while unattended config validation and worker preflight admit only exact Pi/Claude/Codex (`src/dispatch/handler_for_model.rs:76-137`; `src/executor_discovery.rs:40-188`; `src/config.rs:2395-2433,3590-3710`; `src/commands/spawn/execution.rs:984-1046`).

**`[FACT]`** Counterevidence/positive scope: live native and OpenCode handlers remain intentionally reachable, and shell tasks deliberately bypass the model plane (`src/commands/spawn_task.rs:145-224,280-543`; `src/commands/opencode_handler.rs:142-194`; `src/dispatch/plan.rs:387-493,769-809`). The defect is not that all recognized handlers must be workers; it is that one vocabulary lacks a visible surface qualifier.

**`[RECOMMENDATION]`** Linked actions: `MODEL-REC-001` and `MODEL-REC-003`.

### `MODEL-002` — ordinary Pi task workers lack the documented invocation-scoped plugin boundary

**`[CONTRADICTION]`** **State:** partial. **Severity:** S2 Medium. **Likelihood:** possible operational skew; argv divergence is observed. **Confidence:** high for topology, medium for impact. **Boundary:** Pi worker tools, completion ergonomics, plugin compatibility. **Owner:** Pi handler/plugin and worker spawn. The ordinary task worker uses `pi --mode json` without explicit `-e`, `-ne`, or plugin compatibility injection (`src/service/executor.rs:1729-1752`; `src/commands/spawn/execution.rs:1308-1403,3457-3483`), while the documented “WG-spawned workers” path is `wg pi-handler` loading exactly the embedded build (`docs/quickstart-pi-openrouter.md:209-217` [DOC-CLAIM; undated]) and only `pi-handler` implements that argv/env (`src/commands/pi_handler.rs:504-537,855-902,1012-1040`).

**`[FACT]`** Counterevidence: `wg setup`, profile activation, and explicit plugin installation can wire the console extension (`src/commands/setup.rs:2688-2722`; `src/commands/profile_cmd.rs:1158-1194`; `src/commands/pi_plugin_install.rs:26-46`). The Pi worker also retains built-in file/bash tools, so absence of `wg_*` extension tools does not prove that all task execution or CLI-based completion fails.

**`[INFERENCE]`** A stale, missing, or differently discovered global extension can remove or skew `wg_*` tools without the expected-versus-found handshake at the normal worker edge. A negative end-to-end argv/environment test of the daemon-launched Pi child would confirm or falsify this risk.

**`[RECOMMENDATION]`** Linked action: `MODEL-REC-002`.

### `MODEL-003` — strict bare-provider behavior has advanced beyond the documented deprecation phase

**`[CONTRADICTION]`** **State:** partial migration. **Severity:** S2 Medium. **Likelihood:** likely for old configurations. **Confidence:** high. **Boundary:** upgrades, service startup, config CLI. **Owner:** configuration migration. Lenient parsing warns and rewrites leading provider forms while `HANDLER_FIRST_HARD_ERROR` is false (`src/config.rs:2660-2690,2786-2874`), but strict supported-route parsing accepts only Pi/Claude/Codex and rejects the route (`src/config.rs:2395-2433`). Service start emits the warning and then applies strict validation (`src/commands/service/mod.rs:1442-1475`).

**`[DOC-CLAIM]`** The worker guide says every strict entry point warns during the release window and then defaults to nex until one flag flips to hard error (`AGENTS.md:88-97` [agent-facing contract; undated at snapshot]). Both statements cannot describe current strict service/spawn behavior literally.

**`[RECOMMENDATION]`** Linked action: `MODEL-REC-004`.

### `MODEL-004` — agency fallback is explicit and same-system, contrary to older Claude fallback text

**`[VERIFIED]`** **State:** shipped/current. **Severity:** S2 Medium documentation/operability risk. **Likelihood:** possible when credentials/providers fail. **Confidence:** high. **Boundary:** agency evaluation/assignment authority and cost. **Owner:** agency/model plane. `test_production_agency_dispatch_has_no_hardcoded_claude_fallback` and `test_cross_system_fallback_is_rejected_before_any_call` each passed on 2026-08-08. Current source requires an explicitly configured fallback and rejects a different execution system before any call (`src/service/llm.rs:251-407,519-600,2578-2615,2652-2660`).

**`[DOC-CLAIM]`** The project guide and two-tier design still describe automatic/loud fallback to `claude:haiku` on missing native credentials or native call failure (`AGENTS.md:302-331`; `docs/design-two-tier-pi-profile.md:113,555-557` [design status “Proposed”]). This is stale for production resolution.

**`[FACT]`** Positive control: fail rather than silently crossing handler/provider authority is the safer current behavior; Claude stale-session retry is separately bounded to the same handler and only recognized missing-session errors (`src/commands/spawn/execution.rs:3523-3548`).

**`[RECOMMENDATION]`** Linked action: `MODEL-REC-005`.

### `MODEL-005` — explicit route/reasoning propagation and fail-closed worker admission are strong controls

**`[VERIFIED]`** **State:** shipped/current. **Severity:** S4 Informational positive control. **Likelihood:** observed in focused tests. **Confidence:** high. **Boundary:** unattended process identity. **Owner:** model-plane maintainers. The handler suite, sole-model-plane suite, two-tier profile suite, executor-taxonomy suite, and Pi argv test passed. Source independently validates role routes/reasoning at service startup and immediately before worker launch (`src/commands/service/mod.rs:1448-1475`; `src/commands/spawn/execution.rs:1033-1046,1406-1438`).

**`[FACT]`** Residual qualification: legacy executor fields still influence `plan_spawn`, and live surfaces intentionally have different defaults. Therefore this finding does not imply a single universal precedence function (`src/dispatch/plan.rs:387-493,650-809`).

**`[RECOMMENDATION]`** Preserve with `MODEL-REC-001` and `MODEL-REC-008`.

### `MODEL-006` — Pi stream accounting correctly avoids duplicate snapshots

**`[VERIFIED]`** **State:** shipped/current. **Severity:** S4 Informational positive control. **Likelihood:** observed for fixtures. **Confidence:** high. **Boundary:** task usage/cost accounting and event UI. **Owner:** streaming/accounting. The focused bridge and live-cache tests passed, and implementation counts authoritative `turn_end` events once (`src/stream_event.rs:497-690,1125-1152`; `src/graph.rs:1459-1587`).

**`[FACT]`** Counter-scope: no real provider stream was captured in this audit; schema evolution remains external. The inspected smoke `tests/smoke/scenarios/pi_stream_bridge_populates_usage.sh:1-93` asserts the end-to-end bridge contract but was not executed here.

**`[RECOMMENDATION]`** Preserve fixtures and add provider-schema compatibility monitoring under `MODEL-REC-007`.

### `MODEL-007` — Pi watchdog continuation is evidence-based and not completion authority

**`[VERIFIED]`** **State:** shipped/current. **Severity:** S4 Informational positive control. **Likelihood:** observed in 19 integration tests. **Confidence:** high for modeled short tests, medium for production timing. **Boundary:** long-running Pi worker liveness and duplicate continuation. **Owner:** watchdog/runtime. `cargo test --test integration_pi_watchdog` passed 19 tests; source requires guarded same-session/fenced continuation and leaves terminal state to normal task transitions (`src/pi_watchdog/mod.rs:1-5,1274-1730`).

**`[UNCERTAINTY]`** A real 300–900+ second silence, provider reconnect, or PID-reuse race was not run. `tests/smoke/scenarios/pi_session_watchdog_human_flow.sh:1-125` was inspected, not executed.

**`[RECOMMENDATION]`** Preserve current authority boundary; add periodic live canary coverage in `MODEL-REC-007`.

### `MODEL-008` — cost fallback and spend-date presentation can report misleading zero/current-day values

**`[FACT]`** **State:** shipped/current. **Severity:** S3 Low. **Likelihood:** likely when Pi reports zero cost or historical tasks are grouped. **Confidence:** high. **Boundary:** operator cost reporting, not token execution. **Owner:** accounting/UX. Pi zero-cost fallback uses the built-in model registry, but that registry has no Pi prices; `wg spend` groups every included task under the command's current UTC date (`src/graph.rs:1543-1587,1712-1785`; `src/commands/spend.rs:18-56`).

**`[FACT]`** Counterevidence: full stored token fields remain on the task, `wg show` can parse live streams, and a non-zero Pi-reported cost is preferred. The issue is fallback/presentation, not loss of every usage record (`src/commands/show.rs:692-720`; `src/stream_event.rs:425-448`).

**`[RECOMMENDATION]`** Linked action: `MODEL-REC-006`.

## 4. Contradictions and drift

| ID | Record |
|---|---|
| `MODEL-DRIFT-001` | **`[CONTRADICTION]`** `docs/README.md:85-99,157-181` presents executor-first concepts and Claude executor examples, while `README.md:93-113` separates attended Pi from exact unattended Pi routes and strict source admits Pi/Claude/Codex by model prefix (`src/config.rs:2395-2433,3590-3710`). **Authority:** current source for behavior; root README is closer but still documentation. **State:** open. **Severity/confidence:** S2/high. **Owner:** docs/model plane. |
| `MODEL-DRIFT-002` | **`[CONTRADICTION]`** handler-first and two-tier documents remain titled **“Proposed (design only)”** (`docs/design-handler-first-model-spec.md:1-39`; `docs/design-two-tier-pi-profile.md:1-5`), while corresponding route/profile logic and passing tests exist (`src/dispatch/handler_for_model.rs:76-137`; `src/config.rs:3502-3710`; `tests/integration_pi_two_tier_profile.rs:1-201` [executed]). **Authority:** implementation for shipped behavior; design text for history only until status is amended. **State:** open. **Severity/confidence:** S3/high. |
| `MODEL-DRIFT-003` | **`[CONTRADICTION]`** nex is a checked-in starter (`src/profile/templates/nex.toml:1-31`) and broad handler target, yet profile selection validates profiles as the strict Pi/Claude/Codex worker plane (`src/commands/profile_cmd.rs:729-737,1067-1075`) and service startup rejects nex. **Authority:** strict admission. **State:** open. **Severity/confidence:** S2/high. |
| `MODEL-DRIFT-004` | **`[CONTRADICTION]`** comments say task-agent routing uses `wg pi-handler` (`src/dispatch/plan.rs:91-100,145-153`; `src/service/executor.rs:1733-1738`), while normal task execution uses direct `pi --mode json` (`src/commands/spawn/execution.rs:3457-3483`). **Authority:** executed command construction and wrapper source. **State:** open. **Severity/confidence:** S2/high. |
| `MODEL-DRIFT-005` | **`[CONTRADICTION]`** guide fallback text promises `claude:haiku`, while current production tests assert no hard-coded or cross-system fallback. **Authority:** source + executed tests. **State:** open. **Severity/confidence:** S2/high. |
| `MODEL-DRIFT-006` | **`[CONTRADICTION]`** the deprecation flag says the hard-error release is off, but strict worker admission already hard-errors leading-provider routes. **Authority:** entry-point behavior should be decided, not inferred from the flag name. **State:** open. **Severity/confidence:** S2/high. |
| `MODEL-DRIFT-007` | **`[FACT]`** apparent contradiction resolved: the default compatibility executor is Pi while fresh expert initialization is described as graph-only. The default model is empty and no-flag `wg init` follows an explicit graph-only branch (`src/config.rs:5289-5303,5325-5340`; `src/commands/init.rs:87-123,247-266`). **State:** resolved/apparent non-issue. **Severity/confidence:** S4/high. |
| `MODEL-DRIFT-008` | **`[FACT]`** apparent contradiction qualified: discovery lists OpenCode/native while worker validation rejects them. This is coherent only when discovery means installed capability across all surfaces, not unattended eligibility. **State:** apparent/non-issue at implementation level, open terminology/UI debt. **Severity/confidence:** S3/high. |

## 5. Risks and gaps

### 5.1 Failure analysis

| ID | Label | Severity / likelihood | Failure behavior and residual gap |
|---|---|---|---|
| `MODEL-RISK-001` | `[INFERENCE]` | S2 / likely for legacy configs | A discoverable or shipped-template route can fail only at profile/service admission, creating setup churn or outage after upgrade. Supported by `MODEL-001`/`MODEL-003`; falsify by an executed service-start test proving nex is intentionally accepted at the snapshot. |
| `MODEL-RISK-002` | `[INFERENCE]` | S2 / possible | A normal Pi worker can see missing/stale ambient `wg_*` extension tools because its invocation does not load/check the embedded build. Built-in bash can still run `wg`, limiting blast radius. Falsify with captured real worker argv/env and startup handshake. |
| `MODEL-RISK-003` | `[FACT]` | S2 / possible | Unsupported or ambiguous worker routes fail strict validation before process execution (`src/config.rs:2395-2433,3590-3710`). This is a positive fail-closed behavior; documentation surprise is the residual risk. |
| `MODEL-RISK-004` | `[FACT]` | S2 / possible | Missing CLI/process launch errors enter transactional rollback before the launch permit, preserving dispatchability (`src/commands/spawn/execution.rs:2100-2160`). A CLI disappearing after preflight remains possible; rollback is the intended control. |
| `MODEL-RISK-005` | `[FACT]` | S2 / possible | Pi credential/provider errors are classified from raw stream/exit, recorded, and fail without implicit cross-handler fallback (`src/commands/spawn/execution.rs:3598-3640`; `src/service/llm.rs:251-407,519-600`). No universal credential preflight exists. |
| `MODEL-RISK-006` | `[FACT]` | S2 / possible | Pi error events can override a zero exit before generic no-op logic (`src/commands/spawn/execution.rs:3598-3616`). Coverage depends on provider event-schema classification. |
| `MODEL-RISK-007` | `[FACT]` | S2 / possible | A Pi child that exits without reviewed completion is failed unless watchdog policy authorizes continuation (`src/commands/spawn/execution.rs:3618-3640`). Completion is not inferred from prose. |
| `MODEL-RISK-008` | `[FACT]` | S3 / possible | Claude retries fresh only for recognized stale-session errors (`src/commands/spawn/execution.rs:3523-3548`). It remains same-handler but loses session continuity. |
| `MODEL-RISK-009` | `[FACT]` | S2 / possible | If GNU `timeout`/`gtimeout` is absent, the wrapper warns and runs without a hard timeout (`src/commands/spawn/execution.rs:3395-3420`). Availability wins over wall-clock enforcement. |
| `MODEL-RISK-010` | `[FACT]` | S3 / likely | Pi zero-cost fallback can remain zero and spend history is bucketed to today (`MODEL-008`). Token fields remain available, limiting accounting loss. |

### 5.2 Coverage and uncertainty gaps

**`[UNCERTAINTY]`** No real Pi/Claude/Codex/OpenCode/native provider, authentication failure, rate limit, context overflow, malformed live stream, or model registry refresh was exercised. Focused fixtures establish local behavior only. Next check: credential-isolated canaries using disposable accounts/endpoints and captured child argv/event artifacts.

**`[UNCERTAINTY]`** The watchdog passed short integration tests but not production-duration silence thresholds, process-group termination, PID reuse, or same-session continuation against a live Pi database. Next check: time-controlled integration plus one opt-in live provider scenario.

**`[UNCERTAINTY]`** The wrapper's Windows Bash path logic, macOS `gtimeout` fallback, profile daemon hot reload, and keyring backends were only inspected. Next check: CI/platform scenarios with exact binary provenance.

**`[FACT]`** Inspected smoke scenarios are executable specifications, not executed evidence in this artifact: `tests/smoke/scenarios/handler_first_bare_provider_model.sh`, `setup_routes_complete_configs.sh`, `pi_worker_one_shot_prompt_and_cred_error.sh`, `pi_stream_bridge_populates_usage.sh`, and `pi_session_watchdog_human_flow.sh` [inspected, not run].

## 6. Recommendations

### Documentation and factual synchronization

1. **`MODEL-REC-001` — `[RECOMMENDATION]` (P0, model-plane + docs; links `MODEL-001`, `MODEL-005`, `MODEL-DRIFT-001/003/008`):** publish one generated capability matrix with separate columns for unattended worker, attended TUI, live RPC/chat, agency one-shot, discovery-only, and deprecated. **Acceptance:** matrix rows are tested against `validate_execution_model_plane`, `handler_for_model`, discovery, and live command construction; UI never labels discovery as worker readiness.
2. **`MODEL-REC-005` — `[RECOMMENDATION]` (P0, agency/docs; links `MODEL-004`, `MODEL-DRIFT-005`):** replace automatic `claude:haiku` claims with “explicit fallback, same execution system only; otherwise fail.” **Acceptance:** AGENTS/CLAUDE/design/manual text agrees with the two passing production fallback tests.
3. **`MODEL-REC-003` — `[RECOMMENDATION]` (P1, profiles/docs; links `MODEL-001`, `MODEL-DRIFT-002/003`):** hide or label unusable worker starters and mark implemented design sections as shipped/superseded at paragraph granularity. **Acceptance:** `wg profile list/select`, templates, root README, and docs README describe the same admitted worker set.

### Implementation and tests

4. **`MODEL-REC-002` — `[RECOMMENDATION]` (P0, Pi/runtime; links `MODEL-002`, `MODEL-RISK-002`, `MODEL-DRIFT-004`):** decide and enforce the ordinary Pi worker plugin topology. Prefer explicit `-e <embedded>` plus compatibility env on the actual `--mode json` child unless Pi's worker contract forbids it; otherwise narrow the hermetic claim. **Acceptance:** daemon-worker integration captures argv/env, proves expected-versus-found mismatch fails loudly, and proves `wg_*` tools or the documented CLI completion path is available.
5. **`MODEL-REC-004` — `[RECOMMENDATION]` (P1, config/migration; links `MODEL-003`, `MODEL-DRIFT-006`):** make one release-phase policy control CLI config, config load, service start, and spawn behavior for leading-provider routes. **Acceptance:** a table-driven test asserts identical warn/canonicalize/reject semantics at all strict entry points.
6. **`MODEL-REC-006` — `[RECOMMENDATION]` (P1, accounting/UX; links `MODEL-008`, `MODEL-RISK-010`):** add Pi rates or label cost unavailable, and group spend by terminal timestamp. **Acceptance:** tests cover non-zero Pi cost, zero-cost/no-rate display, historical dates, cache fields, Done, and Failed.
7. **`MODEL-REC-007` — `[RECOMMENDATION]` (P1, streaming/watchdog; links `MODEL-006/007` and coverage gaps):** preserve fixture tests, add captured-schema regression fixtures, time-controlled long-silence/PID-reuse checks, and an opt-in live canary. **Acceptance:** no duplicate turn accounting; same-session continuation; no watchdog terminal write; explicit provider-schema version failure.
8. **`MODEL-REC-008` — `[RECOMMENDATION]` (P2, configuration/observability; links `MODEL-005`):** expose route provenance in `wg config --models`, spawn audit, and status: selected project/global profile, model source, reasoning source, handler, inner provider/model, and requested-surface admission. **Acceptance:** both traces in section 2 can be reconstructed from structured output without reading source.

### Product/design decisions

9. **`MODEL-REC-009` — `[RECOMMENDATION]` (P1, product owner; links `MODEL-001`):** decide whether nex/native and OpenCode are intentionally attended-only or planned unattended handlers. Do not let dormant registry/template code make that decision implicitly. **Acceptance:** an accepted decision names each surface and either removes dead worker cues or adds admission/security/accounting tests.
10. **`MODEL-REC-010` — `[RECOMMENDATION]` (P2, operations owner; links `MODEL-RISK-009`):** decide whether absence of GNU timeout is a startup blocker for unattended services or an accepted degraded mode. **Acceptance:** status exposes degraded timeout enforcement and the operator guide states the policy.

## 7. Evidence appendix

### 7.1 Snapshot, environment, and provenance

**`[VERIFIED]`** On 2026-08-08 UTC in `/home/bot/wg/.wg-worktrees/agent-5`, Linux `6.8.0-90-generic x86_64`, Rust/Cargo `1.96.0`, the following returned exit 0:

```bash
git diff --quiet \
  b0892ea7496fd2cc8f641417a3d8e33ca9add369..\
  98b319c36aa8a21fd4506fc7469fe6d58978cdda -- \
  src tests README.md docs/README.md \
  docs/design-handler-first-model-spec.md \
  docs/design-two-tier-pi-profile.md \
  docs/design-pi-plugin-install.md AGENTS.md
```

**`[VERIFIED]`** Bounded result: `scoped_diff_exit=0`; only the audit charter changed between snapshot and execution revision. Static line citations in this artifact are interpreted against that unchanged evidence.

### 7.2 Executed focused tests

**`[VERIFIED]`** The following command group ran in the same cwd/environment starting `2026-08-08T10:46:55Z`, revision `98b319c36aa8a21fd4506fc7469fe6d58978cdda`, Rust/Cargo 1.96.0, and returned aggregate exit 0:

```bash
cargo test --lib dispatch::handler_for_model::tests -- --nocapture
cargo test --lib \
  stream_event::tests::test_translate_pi_stream_sums_turn_end_once_no_double_count \
  -- --nocapture
cargo test --lib \
  graph::tests::test_pi_usage_reads_single_authoritative_raw_stream_and_live_cache_tracks_it \
  -- --nocapture
cargo test --test integration_pi_watchdog
cargo test --test integration_pi_sole_model_plane
cargo test --test integration_pi_two_tier_profile
env -u WG_TASK_ID -u WG_AGENT_ID -u WG_WORKER_CAPABILITY \
  -u WG_WORKER_CONTROL_PROTOCOL -u WG_WORKER_IPC -u WG_GRAPH_ID \
  -u WG_WORKTREE_ACTIVE -u WG_WORKTREE_PATH -u WG_BRANCH -u WG_PROJECT_ROOT \
  cargo test --test integration_simplify_executor_taxonomy
cargo test --lib \
  service::llm::tests::test_production_agency_dispatch_has_no_hardcoded_claude_fallback \
  -- --nocapture
cargo test --lib \
  service::llm::tests::test_cross_system_fallback_is_rejected_before_any_call \
  -- --nocapture
```

**`[VERIFIED]`** Bounded results: handler resolver 14 passed; stream translation 1 passed; raw-stream/live-cache accounting 1 passed; watchdog 19 passed; Pi sole model plane 8 passed; two-tier Pi profile 6 passed; simplified executor taxonomy 6 passed; each fallback filter 1 passed; zero failures. Compiler warnings were emitted but did not affect exit status. The taxonomy subprocess tests required removal of inherited worker-control variables so child `wg init` represented a normal operator rather than this managed audit worker.

**`[VERIFIED]`** The process-argv test ran separately in the same cwd on 2026-08-08 and returned exit 0:

```bash
cargo test --bin wg \
  test_build_inner_command_pi_external_emits_model_and_thinking \
  -- --nocapture
```

**`[VERIFIED]`** Bounded result: 1 passed, 0 failed, 3820 filtered out. The test asserts explicit Pi provider, model, and `--thinking high` in the constructed process command (`src/commands/spawn/execution.rs:5544-5584`).

### 7.3 Executed dry-run process traces

**`[VERIFIED]`** A `target/debug/wg` built from the execution revision had SHA-256 `09ad159e9ed3a225ca05dc7823ef3c1950c5120ef672cca5ffec31fa4f011025`. On 2026-08-08, temporary isolated graphs under `/tmp/wg-model-audit-traces.DVI8hC` used only handwritten local configuration and paused tasks. The relevant exact commands were:

```bash
env -i PATH="$PATH" HOME="$TMP/home-native" USER="$(id -un)" \
  "$BIN" add "native live trace" --id native-live --description trace --paused
env -i PATH="$PATH" HOME="$TMP/home-native" USER="$(id -un)" \
  WG_EXECUTOR_TYPE=native \
  "$BIN" spawn-task native-live --dry-run

env -i PATH="$PATH" HOME="$TMP/home-pi" USER="$(id -un)" \
  "$BIN" add "pi chat trace" --id pi-chat --description trace --paused
env -i PATH="$PATH" HOME="$TMP/home-pi" USER="$(id -un)" \
  "$BIN" spawn-task pi-chat --dry-run
```

**`[VERIFIED]`** All four commands returned 0. Bounded process previews are reproduced in section 2.4. No provider process was started. The clean environment intentionally removed this worker's capability variables; native's `WG_EXECUTOR_TYPE=native` is the live-surface compatibility hint consumed by `spawn-task`, not a claim that model-first unattended admission selected native.

### 7.4 Primary static evidence

| Evidence | Observation | Class / freshness |
|---|---|---|
| `src/config.rs:1508-1772,2395-2433,2523-2669,2786-2985,3502-3710,5289-5340,5990-6205` | reasoning/tier definitions, strict routes, deprecation parsing, precedence, empty route default, config/profile merge, endpoint secrets | `[FACT]` E2, snapshot-current |
| `src/config_defaults.rs:20-139`; `src/commands/init.rs:87-123,247-266` | Pi-only setup parser, dormant builders, and graph-only initialization branch | `[FACT]` E2, snapshot-current |
| `src/dispatch/handler_for_model.rs:45-137,200-276` | broad handler-first resolution and tests | `[FACT]` E2/E3; unit tests executed |
| `src/dispatch/plan.rs:267-809` | model/executor/endpoint precedence and provenance | `[FACT]` E2, snapshot-current |
| `src/commands/service/mod.rs:1442-1475` | explicit selection and strict service validation | `[FACT]` E2, snapshot-current |
| `src/commands/spawn/execution.rs:1033-1046,1308-1438,1650-1788,2100-2160,3242-3707` | worker admission, argv, environment, rollback, wrapper/events/failures | `[FACT]` E2; one argv test executed |
| `src/service/executor.rs:1729-1752` | direct Pi JSON worker base command | `[FACT]` E2, snapshot-current |
| `src/commands/{pi_handler,spawn_task,opencode_handler}.rs` cited spans | RPC/attended/external handler process topology | `[FACT]` E2; dry-run traces executed |
| `src/executor_discovery.rs:40-188,222-288` | catalog/discovery and Pi availability | `[FACT]` E2, snapshot-current |
| `src/pi_plugin/mod.rs:479-574`; profile/setup plugin call sites | embedded plugin materialization and console/hermetic modes | `[FACT]` E2, snapshot-current |
| `src/pi_watchdog/mod.rs:1-15,123-181,1274-1730` | thresholds, evidence, continuation, authority boundary | `[FACT]` E2; 19 integration tests executed |
| `src/stream_event.rs:410-690,1125-1152`; `src/graph.rs:1459-1785` | Pi event/usage translation and cost fallback | `[FACT]` E2; focused tests executed |
| `src/commands/{show,spend}.rs` cited spans | live/stored usage and reporting aggregation | `[FACT]` E2, snapshot-current |
| `src/service/llm.rs:251-407,519-600,2578-2660` | same-system explicit fallback and negative tests | `[FACT]` E2/E3; two tests executed |
| `README.md`, `docs/README.md`, design docs, quickstart, `AGENTS.md` cited spans | conflicting current/design/operator claims | `[DOC-CLAIM]` E4/E5, snapshot-current text |

### 7.5 Inspected tests and limitations

**`[FACT]`** The five smoke scenarios named in section 5.2 were read as E3 executable specifications and were not run. Their presence does not establish pass status.

**`[UNCERTAINTY]`** No network request, model response, credential lookup, OS keyring operation, plugin clean-install, daemon hot reload, real TUI session, GNU-timeout absence, Windows/macOS execution, or production-length watchdog interval was executed. This artifact does not certify provider correctness, security, cost accuracy, or cross-platform readiness.
