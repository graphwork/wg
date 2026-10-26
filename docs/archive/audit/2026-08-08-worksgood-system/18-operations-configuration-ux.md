# Operations, configuration, installation, and UX audit

**Audit date:** 2026-08-08

**Audit snapshot:** `b0892ea7496fd2cc8f641417a3d8e33ca9add369`

**Inspection checkout:** `1899cdcf4fd414245a735e2e8f8c81d92b536ec5` (relative to the audit snapshot, `git diff --name-status b0892ea..1899cdc` contains only the audit charter and already-landed leaf audits 15–17)

**Artifact:** leaf audit required by the charter in this directory

**Change boundary:** this file only; production source, tests, workflows, and pre-existing documentation were not changed

## 1. Executive abstract

**`[FACT]`** WorksGood has substantial operator machinery: receipt-bound native installers, checksums and optional GitHub attestations, a separate attended `worksgood` entry point, explicit project-profile pinning, config source annotations, multiple secret backends, authenticated service identity, detailed service status, graph checks, task traces, spend/time views, static HTML, TUI, Telegram, user boards, conservative disk cleanup, and preserve-first worktree cleanup. The strongest operational defaults inspected are the profile fail-closed rule, secret redaction/plaintext opt-in, authenticated daemon control, and dry-run/dirty-work preservation in cleanup.

**`[VERIFIED]` `OPS-001` — S1 High, current, high confidence:** capability-scoped worker responses whose payload is a JSON array cannot be serialized. `IpcResponse.data` is `#[serde(flatten)]`, but worker message read/poll and artifact list return arrays (`src/commands/service/ipc.rs:251-274`, `716-758`). On this live audit graph, `wg msg read audit-operations-ux --agent agent-12` first returned `No response from service` after advancing the cursor to message 3 and marking it read; a later retry blocked for 30.010 seconds and returned `Worker control IPC timed out after 30s`. The daemon recorded `Worker IPC ... operation=MessageRead` followed by `Error handling connection: can only flatten structs and maps (got a sequence)` at 11:12:33 and 11:12:37. Worker mode is a hard switch with no filesystem fallback (`src/worker_cli.rs:1-5`, `111-125`, `154-180`), while the direct filesystem command works (`src/commands/msg.rs:150-205`). Thus a worker can lose the only response after a stateful read has already consumed its inbox.

**`[VERIFIED]` `OPS-002` — S1 High, current, high confidence:** the generic `wg config set` path destroys comments while its source documentation and `docs/config-precedence.md` promise comment preservation. It parses into `toml::Value` and serializes with `toml::to_string_pretty` (`src/commands/config_cmd.rs:3027-3102`). A clean-room reproduction changed a three-line commented config into only `[dispatcher]\nmax_agents = 4`.

**`[VERIFIED]` `OPS-003` — S1 High, current, high confidence:** the same config path accepts an unknown key, persists it, then reports the effective value as unset and incorrectly suggests a project-profile routing conflict. `wg config lint --local` subsequently called that file “clean — no stale keys found,” then said one issue was fixable by `wg migrate config`; that one issue was merely the absence of a Pi selection and migration cannot select one (`src/commands/config_cmd.rs:3476-3676`). Together, `OPS-002` and `OPS-003` make the advertised universal, safe config-editing surface lossy and diagnostically contradictory.

**`[FACT]` `OPS-007` — S1 High, current, high confidence:** observability has two materially misleading rollups. `wg metrics` reads process-local atomics, so the short-lived reporting process cannot see counters accumulated in the daemon or earlier cleanup processes (`src/metrics.rs:8-26`, `83-193`, `287-289`; `src/commands/metrics.rs:1-20`). A direct invocation returned all zeroes and the JSON sentinel `min_cleanup_duration_ms = 18446744073709551615`. `wg spend` assigns every completed task to the current UTC date rather than its completion date (`src/commands/spend.rs:27-67`), so its “Daily breakdown” and `--today` output are not historical/day-bounded accounting.

**`[FACT]` `OPS-004` — S1 High, current, high confidence:** onboarding and diagnosis have competing authorities. Current `wg setup --help`, `SetupArgs`, and `wg init` accept Pi as the sole supported LLM route (`src/commands/setup.rs:72-107`, `120-150`; `src/commands/init.rs:21-84`). Multiple smoke scenarios still assert the retired five-route surface, including `setup_routes_complete_configs.sh`, `setup_scope_flag.sh`, `setup_provider_login_onboarding.sh`, `codex_init_route_has_correct_defaults.sh`, and `wg_init_writes_lockstep_agent_guides.sh`. A direct `wg setup --route claude-cli --yes --dry-run` failed with “The supported route is: pi.” These scenarios are not continuously run, as audit 17 establishes. Separately, README/install/concierge prose says bare `worksgood` verifies Pi and ensures its plugin, but current executable help and `run_bare` say an existing graph opens the setup-neutral TUI without inspecting Pi, plugins, profiles, config, or services (`src/bin/worksgood.rs:6-16`; `src/concierge.rs:1620-1648`). The latter is runtime authority.

**`[FACT]` `OPS-005` — S1 High, current, high confidence:** `wg doctor` is not route-aware. It unconditionally makes missing/failing `claude` a hard error, then separately treats Pi as optional (`src/commands/doctor.rs:166-226`, `267-412`), even though setup/init declare Pi the sole supported LLM handler. A correctly configured Pi-only installation without Claude therefore exits 2 (“WorksGood probably won't function correctly”). `wg check` is a separate graph-integrity check only (`src/commands/check.rs:1-65`). There is no one trustworthy command answering “is this selected project ready for its chosen route, config, secrets, plugin, daemon, graph, and disk policy?”

**`[VERIFIED]` `OPS-006` — S1 High operational contention, current, high confidence:** exact reviewed publication is globally blocked whenever the attached integration-root checkout has unrelated tracked/index changes. The safety refusal is deliberate (`src/commands/completion_land.rs:82-122`, `214-249`), but its error names no files and offers no accepted-candidate parking/publication-lane remedy. The audit-charter task's durable graph log independently records an accepted candidate, refusal on `.gitignore`, `AGENTS.md`, and `CLAUDE.md`, failed worker attempt after its deferral paths were refused, operator cleanup/stash, retry, re-review, and later successful land. One unrelated dirty root therefore caused a completed isolated worker to fail and be respawned. Safety was preserved; liveness and operator ergonomics were not.

**`[INFERENCE]`** Operational readiness is therefore **mixed, not production-green**. The persistence and destructive-operation defaults are generally conservative, but the surfaces used to understand and control the system can lose replies, erase config commentary, report false daily/daemon-wide aggregates, or diagnose the wrong model plane. Confidence is high because direct reproductions, live daemon evidence, source dispatch, help, docs, and stale tests converge. A falsifying check for the central conclusion would require current release-path tests proving array-valued worker IPC round trips, comment-preserving config edits with unknown-key policy, route-aware doctor success, and persisted/correctly dated metrics; none was found.

**Next decision:** block release on `OPS-REC-001` (worker IPC), then fix the config mutation/diagnostic contract and the route-aware readiness/accounting surfaces before adding more operator commands.

## 2. Scope and operational map

### 2.1 What was inspected and what was executed

**`[FACT]`** Inspection covered `Cargo.toml`, both install scripts, install tests, CI/release workflows, install/setup/config/profile/secret docs and source, service lifecycle/IPC/status, doctor/check, logs/trace/stats/spend/metrics, disk/worktree cleanup, TUI/HTML/server/Telegram/user-board entry points, the general/federation runbooks, and representative unit/integration/smoke tests.

**`[VERIFIED]`** The checkout built with:

```text
cargo build --locked --bin wg --bin worksgood
```

The built binary reported `wg 0.1.0`. This audit also executed three isolated config/setup reproductions, one live worker message-read reproduction, and these bounded tests:

```text
cargo test --locked --lib secret::tests -- --test-threads=1
  17 passed
cargo test --locked --lib project_profile_overlay -- --test-threads=1
  3 passed
cargo test --locked --bin wg worktree_gc_ -- --test-threads=1
  14 passed
```

**`[UNCERTAINTY]`** The full test suite, smoke manifest, installers, released archives, live Pi/provider login, TUI flows, Telegram network calls, HTML rsync, systemd, mosh, Windows/macOS runtime behavior, and destructive cleanup were not executed here. Test files inspected are evidence of intended contracts, not proof they run in CI; audit 17 gives the CI-selection analysis.

### 2.2 Install and first-use journeys

#### Journey A — native archive, attended human use

1. **`[FACT]`** The shell/PowerShell installers select one of five release targets, verify SHA-256, optionally verify GitHub attestations when `gh` is available, check all three payload files, refuse foreign/symlink collisions, replace files via temporary destination + rename, and write a mode-0600 receipt (`scripts/install-wg.sh:179-287`, `290-352`, `355-493`, `609-654`; PowerShell counterpart inspected). Provenance is explicitly skipped when `gh` is absent or a mirror is used.
2. **`[FACT]`** The installed archive set is `worksgood`, `wg`, and `nex`; uninstall is receipt-bound and preserves `.wg` project/global data (`docs/guides/install.md:1-31`, `142-198`; `scripts/install-wg.sh:390-493`).
3. **`[FACT]`** In a new repository, bare `worksgood` requires stdin and stdout TTYs, runs exact sibling `wg --dir <graph> init --no-agency`, then opens the TUI. In an existing graph it opens the same TUI immediately (`src/concierge.rs:1620-1648`). The credential-free `worksgood_attended_pi_simple.sh` smoke scenario is a strong intended human-flow contract for this thin-launcher behavior (`tests/smoke/scenarios/worksgood_attended_pi_simple.sh:1-25`, `117-248`), but was not run here or by normal CI.
4. **`[FACT]`** Pi availability is deferred until the user chooses Pi inside the TUI on an existing graph. This is safer and less intrusive than the README/install-guide claim that bare entry verifies Pi and prepares the plugin, but the prose needs to match.

#### Journey B — unattended workers/evaluation

1. **`[FACT]`** `worksgood setup --model pi:<provider>:<model>` is the attended one-model automation path. It requires a TTY unless dry-running, refuses unavailable Pi before transaction writes, prints a redacted immutable plan, asks for confirmation, pins a project-only generated profile, and reconciles authenticated service state (`src/bin/worksgood.rs:19-95`, `119-158`; `src/concierge.rs:1650-1735`; `docs/worksgood-concierge.md:38-91`).
2. **`[FACT]`** Expert/headless setup is `wg setup --route pi --yes --model ...`; noninteractive scope defaults global, while `--scope local|both` is explicit (`src/commands/setup.rs:72-150`; checkout-built `wg setup --help`). Graph-only `wg init` creates no route (`src/commands/init.rs:77-116`).
3. **`[FACT]`** Project profile selection is separate from legacy machine-global `wg profile use`: it writes `<graph>/profile-selection.json`, pins a semantic BLAKE3 fingerprint, and never rewrites global config or `active-profile` (`src/profile/project.rs:1-13`, `31-75`; `src/commands/profile_cmd.rs:703-863`). Missing, moved, malformed, or changed definitions fail closed without global route fallback (`src/profile/project.rs:330-460`).
4. **`[VERIFIED]`** Three profile overlay unit tests passed: the profile owns routing; explicit local `max_agents` wins; the profile supplies that value only when local/global leave it unset.

#### Journey C — source install and upgrade

1. **`[FACT]`** `cargo install --path . --locked` installs every Cargo binary, which currently includes `casa-adapter` as well as the three documented commands (`Cargo.toml:20-41`). `cargo metadata` exposed all four, and this machine's prior source install had all four on `PATH`.
2. **`[FACT]`** Native release archives intentionally package only three (`.github/workflows/release.yml:470-523`, `646-667`). Thus source and archive package layouts differ.
3. **`[FACT]`** The installer rerun is the native upgrade path; `wg upgrade` currently exists but its help describes a managed source checkout and rollback. `docs/guides/install.md:240-309` still says “When `wg upgrade` is available,” despite the current command being present.
4. **`[INFERENCE]`** Upgrade replacement is file-atomic but not set-atomic: each binary is renamed independently and the receipt is written last (`scripts/install-wg.sh:373-381`, `641-654`). A disk/permission failure after one rename can leave a mixed-version bundle with the old receipt. The probability is low; no failure-injection test for this boundary was found.

### 2.3 Day-2 operator journey

| Need | Best current surface | What it actually establishes | Gap / next diagnostic |
|---|---|---|---|
| “What is happening?” | `wg status [--json]` | one-screen daemon, effective route/reasoning, agents, task/eval/FLIP state, dangling dependencies, verify failures, recent work, cached disk sentinel (`src/commands/status.rs:1-15`, `144-193`, `246-286`) | Not daemon errors or full identity; continue to `wg service status` |
| “Is the daemon healthy?” | `wg service status [--json]` | PID/socket/uptime, graph/build/config/profile identity, worker isolation, lanes, admission deferral, observers, retained cleanup, log path and five recent errors (`src/commands/service/mod.rs:4383-4795`) | Status verifies process liveness more deeply than `doctor`; no log-follow command under `service` |
| “Why is no work spawning?” | `wg service status`; `wg ready`; `wg agents --alive`; `wg config --models`; `wg profile show` | status distinguishes admission deferral from “ready but spawned zero” and suggests unclaim/reset (`service/mod.rs:4527-4577`, `4625-4647`) | Multiple commands; doctor does not resolve selected routing readiness |
| “Is config canonical/effective?” | `wg config get`, `--list`, `--show`, `--models`; `wg profile show`; `wg config lint` | winning source labels and profile fingerprint are visible (`src/config.rs:6733-6846`; `docs/config-precedence.md:79-106`) | Generic setter is lossy; lint has unknown-key and summary gaps |
| “Are credentials reachable?” | `wg secret backend show`; `wg secret check <ref>` | backend reachability and a specific ref without revealing it (`src/secret.rs:641-696`; `src/commands/secret_cmd.rs:234-250`) | Pi auth remains Pi-owned; doctor focuses on Claude auth; Telegram tokens use separate plaintext config |
| “Is graph data coherent?” | `wg check [--json]` | cycles, orphan refs, stale assignments, stuck blocked tasks, abandoned-dependency violations (`src/commands/check.rs:1-65`) | Does not check service/config/secrets/logs/disk |
| “Why did one task fail?” | `wg show <id>`; `wg trace <id>`; agent runtime files | task diagnostics, observer/watchdog/worktree state, usage; provenance and archived prompt/output (`src/commands/show.rs:690-707`, `1380-1475`, `1666-1908`; `src/commands/trace.rs:127-246`) | Runtime and archive naming differs; full trace deliberately exposes prompt/output content |
| “What did this cost/take?” | `wg show`; `wg spend`; `wg stats` | task usage, graph totals, service/agent wall time (`src/commands/stats.rs:29-120`) | Daily spend is misdated; stats are wall-time, not CPU/accounting; metrics are process-local |
| “Is disk pressure blocking work?” | `wg status`; `wg disk doctor`; `wg disk cleanup` | cached sentinel for fast status; explicit owned-cache report/reap (`src/disk_sentinel.rs:670-773`, `1657-1764`; `src/commands/disk.rs:1-92`) | Predictive admission is opt-in; automated cleanup can run inside daemon and is reported only through logs/status snapshot |
| “How do I recover space/worktrees?” | `wg worktree list/archive/gc`; `wg cleanup ...` | GC requires a filter, defaults dry-run, blocks dirty work, points to archive-first (`src/commands/worktree_cmd.rs:267-455`) | `cleanup nightly` is a second overlapping cleanup taxonomy |
| “How do I stop/restart safely?” | `worksgood stop/restart`; `wg service stop/restart` | daemon stop defaults to leaving detached agents alive; `--kill-agents` explicit; concierge authenticates graph/executable/PID identity (`docs/worksgood-concierge.md:62-108`; checkout help) | `service start --force` is suggested for several IPC failures and can be disruptive; user must understand detached agents |
| “How do I publish/share?” | `wg html`; `wg html publish`; `wg server`; Telegram/user boards | local static viewer, rsync deploys, Unix terminal-server scaffolding, chat integration | Remote HTML defaults include all tasks; server surface is Unix-specific; Telegram secrets are outside `wg secret` |

### 2.4 Configuration and profile precedence map

**`[FACT]`** Effective file/profile resolution is:

```text
$WG_GLOBAL_DIR/config.toml (when explicitly set), otherwise ~/.wg/config.toml
    legacy read only: ~/.workgraph/config.toml when canonical is absent
                         │
                         ├── deep merge ──> <graph>/config.toml (local wins)
                         │
                         └── if <graph>/profile-selection.json is valid:
                              fingerprint-verified ~/.wg/profiles/<name>.toml
                              overlays routing after global+local
```

Evidence: canonical/legacy global path handling (`src/config.rs:5807-5907`), global/local merge and legacy normalization (`src/config.rs:6060-6137`), project profile overlay (`src/profile/named.rs:448-540`), and fingerprint refusal (`src/profile/project.rs:330-460`).

**`[FACT]`** Authority is key-class dependent:

| Setting class | Higher to lower effective authority | Important exception |
|---|---|---|
| One-shot launch values | explicit command flag → loaded config | Service launch `--max-agents` can seed a runtime pin unless `--no-pin`; not every flag is persisted |
| Routing (`agent.model`, dispatcher model/provider/executor, tiers, role models) | valid project profile → local → global → built-in/default cascade | Under a selected profile, a later local routing write is stripped and the profile remains authoritative (`src/profile/named.rs:462-525`) |
| Non-routing tuning | local → global → profile-provided default → built-in default | Profile overlay preserves explicit existing values; verified by three passing unit tests |
| Legacy global profile | `wg profile use` materializes a global snapshot and writes `~/.wg/active-profile` | Project selection is separate and wins for the project (`src/config.rs:6138-6144`; `profile_cmd.rs:492-535`) |
| Legacy section aliases | canonical and legacy normalized before merge; canonical conflicts win | `[coordinator]` becomes `[dispatcher]` with structured diagnostic (`src/config.rs:5480-5547`) |

**`[FACT]`** Endpoint arrays do not follow ordinary deep-merge intuition. Global `[[llm_endpoints.endpoints]]` entries are suppressed unless local sets `inherit_global = true`; however, an active named/project profile implicitly inherits global endpoints unless local explicitly sets false or declares its own endpoints (`src/config.rs:5549-5590`). A profile that declares `llm_endpoints` replaces the table (`src/profile/named.rs:527-535`). `wg config --show` explicitly renders `inherit_global` (`src/commands/config_cmd.rs:78-96`).

**`[UNCERTAINTY]`** Model-role resolution has additional per-role/tier/default fallbacks not fully duplicated here. This map answers file/profile authority, not every role's final model. `wg config --models` and `wg profile show` are the safer runtime views.

### 2.5 Secret and endpoint precedence map

**`[FACT]`** For one selected endpoint, `EndpointConfig::resolve_api_key` checks:

```text
1 inline api_key
2 api_key_file (absolute, ~-expanded, or relative to graph dir)
3 api_key_ref
4 explicitly named api_key_env
5 provider fallback env (e.g. OPENROUTER_API_KEY, then OPENAI_API_KEY)
```

The native strict variant omits provider-ambient fallback but still honors the four explicitly configured sources (`src/config.rs:1187-1335`). The higher-level provider resolver checks a matching configured endpoint first, provider env next, then legacy native-executor config (`src/config.rs:6147-6198`). Callers therefore matter; “the” secret precedence is not globally singular.

**`[FACT]`** `api_key_ref` schemes are `keyring:`, `keystore:`, `plain:`, `env:`, `op://`, `pass:`, and warning-only `literal:`. Default backend is keyring; plaintext requires `secrets.allow_plaintext = true`; list/get redact by default; `--value` warns about shell history; non-TTY set requires `--from-stdin`; non-TTY deletion requires `--yes` (`src/secret.rs:1-18`, `29-89`, `586-696`; `src/commands/secret_cmd.rs:13-74`, `89-184`). Seventeen secret unit tests passed.

**`[FACT]`** On an unreachable OS keyring, `keyring:` transparently falls back to `~/.wg/keystore/<name>` with a once-per-process warning. Unix writes set directory 0700/file 0600; the non-Unix branch creates/writes files but does not set an explicit ACL (`src/secret.rs:123-165`, `330-418`). `wg secret backend show` reports actual reachability and whether WG-Fed custody material is encrypted at rest (`src/secret.rs:641-696`).

**`[FACT]`** Secret configuration and storage do not honor the same global override chokepoint as `Config`: `SecretsConfig::load_global`, `keystore_dir`, and `secrets_dir` derive `HOME/.wg` directly (`src/secret.rs:72-129`), while `Config::global_dir` honors `WG_GLOBAL_DIR` (`src/config.rs:5807-5838`). This can break isolation/testing expectations and makes “all machine-global state” an overstatement outside the `Config`/named-profile subsystem.

**`[FACT]`** Telegram uses separate `~/.config/worksgood/notify.toml` or project `.wg/notify.toml`, with bot tokens as literal TOML strings (`src/notify/config.rs:1-4`, `99-133`, `230-236`; `src/commands/telegram.rs:310-407`). Status masks a prefix and list-bots emits a preview, but this path does not use `wg secret` refs or enforce file permissions in the inspected loader.

### 2.6 Observability surface inventory

| Surface | Backing data / output | Retention and machine use | Audit assessment |
|---|---|---|---|
| `wg status [--json]` | graph, service state, registry, coordinator config, cached disk sentinel | current snapshot; JSON | Good first screen; intentionally bounded and does not walk disk (`src/commands/status.rs:246-286`) |
| `wg service status [--json]` | authenticated state, runtime registry, coordinator/eval/FLIP/cleanup/observer state, daemon log tail | current + five ERROR/FATAL lines since start | Best operator diagnostic; explicit admission-deferred and “ready but no spawn” wording (`service/mod.rs:4383-4795`) |
| daemon log | `.wg/service/daemon.log`, rotated backup | 10 MB rotation per docs/source; text | Necessary for IPC root cause; no direct `wg service logs --follow` command was found (`docs/AGENT-SERVICE.md:627-668`) |
| live agent runtime | `.wg/agents/<agent>/metadata.json`, `prompt.txt`, `output.log`, `raw_stream.jsonl`, `stream.jsonl`, `session-summary.md`, watchdog/observer files depending executor | retained while runtime/worktree cleanup policy permits | Rich but fragmented; `wg show` bridges selected fields |
| operation provenance | `.wg/log/operations.jsonl`, rotated `.zst` | append/rotate; `wg log --operations`; JSON option | Docs claim every mutation; not revalidated exhaustively (`docs/LOGGING.md:1-69`, `86-143`) |
| archived attempts | `.wg/log/agents/<task>/<timestamp>/{prompt,output}.txt` | docs say indefinite/manual pruning | `wg trace` reads this layout; potentially sensitive (`src/commands/trace.rs:127-203`) |
| `wg show <id>` | graph task + runtime/observer/watchdog/worktree + usage | human/JSON | Strong task-level diagnosis; points to recovery action |
| `wg trace <id>` | provenance + archived prompt/output | summary, full, ops, JSON | JSON/full includes conversation content; use as sensitive export (`src/commands/trace.rs:205-279`) |
| `wg stats` | service state + registry timestamps | current computation; JSON via root `--json` | Honest wall-time summary, not utilization |
| `wg spend` | task `token_usage` in graph | aggregate + alleged daily JSON | Totals useful; daily/today wrong (`OPS-007`) |
| `wg metrics` | process-local cleanup atomics | no persistence/export | Not daemon-wide observability (`OPS-007`) |
| federation node `/metrics` | process/node counters in Prometheus text | external scrape | Runbook covers this well, but it is federation-node-only (`docs/ops/runbook.md:43-76`) |
| TUI | graph/status/events/session panes | interactive | Broad intended tmux smoke evidence; normal CI does not run manifest scenarios (audit 17) |
| HTML | static graph/detail pages, optional sanitized transcripts | local output or rsync target | Sanitizes markdown; transcript redaction explicitly best-effort (`src/html.rs:704-732`, `2533-2565`) |

### 2.7 Human and remote UX surfaces

- **`[FACT]` TUI:** the primary interactive graph/chat surface is route-neutral on open. The smoke inventory contains extensive tmux/PTY flows, including the thin `worksgood` journey, model picker, large graph, Termux width, sessions, and Pi recovery. Those are valuable specifications but not normal-CI evidence (audit 17).
- **`[FACT]` HTML:** local `wg html` defaults to all tasks and no transcripts. `--public-only` is opt-in; `--chat` adds public chat transcripts; `--chat --all` adds every transcript. Help clearly warns that sanitizer output must be manually reviewed. `wg html publish add` also makes `--public-only` opt-in and `run` rsyncs the generated tree (`src/html.rs:105-138`; checkout help).
- **`[FACT]` Server:** `wg server init` is dry-run by default and generates users, profiles, tmux commands and optional ttyd/Caddy configuration. Implementation shells to `which`, `sh`, and tmux (`src/commands/server.rs:1-6`, `46-137`, `227-338`). This is a Unix deployment helper, not a portable server runtime.
- **`[FACT]` Telegram:** single/multi-bot long polling, same-bot replies, status/list/send/poll/ask, and human-binding confirmation are implemented (`src/commands/telegram.rs:17-215`, `290-577`). Tokens live in separate config, as above.
- **`[FACT]` User boards:** `wg user init/list/archive` manages `.user-<handle>-N`; sending to a missing user-board alias lazily creates the board (`src/commands/user.rs:15-136`; `src/commands/msg.rs:20-86`). This convenience is a graph mutation hidden inside “send,” but it is printed to stderr.

### 2.8 Platform and packaging matrix

| Platform | Release/install evidence | Runtime caveats |
|---|---|---|
| Linux x86_64/aarch64 | native tarballs; shell installer; Ubuntu CI builds/tests/install smoke | server/runbook/systemd path best supported; file permissions enforced |
| macOS x86_64/arm64 | native tarballs and release build/signing path; shell installer | no regular macOS Rust test job; GNU `timeout` not standard, while some smoke runner behavior depends on it (audit 17) |
| Windows x86_64 | zip, PowerShell installer smoke, release build/signing path; local sockets map to namespaced IPC | main Rust integration suite not run on Windows; wrappers still require Git-for-Windows bash (`doctor.rs:166-214`); `service install` exposes systemd help; Unix-server commands are not appropriate |
| Windows ARM64 | no native artifact | PowerShell explicitly refuses and recommends emulation/source (`scripts/install-wg.ps1:145-146`; `docs/guides/install.md:310-324`) |
| Other Unix/Termux | source install possible; Termux docs/smokes exist | no release target/CI promise; server assumptions vary |

**`[FACT]`** Release construction covers five target triples (`.github/workflows/release.yml:29`, `122-151`) but normal Rust CI is Ubuntu-centric; Windows CI covers the synthetic installer only (`.github/workflows/ci.yml:1-201`). Native archives contain three binaries while source installs expose four. The shell installer supports Linux/macOS x86_64/aarch64, while PowerShell explicitly handles Windows (`scripts/install-wg.sh:179-229`).

**`[UNCERTAINTY]`** Successful cross-compilation/release build is not runtime qualification. No claim is made here that Windows daemon, TUI, Pi, worktree, secret fallback ACL, or server behavior works end to end.

## 3. Findings

### `OPS-001` — worker array responses fail after possible mutation

- **Label/state:** **`[VERIFIED]`**, current and reproduced.
- **Severity/likelihood/confidence:** **S1 High; deterministic for nonempty or empty array payloads reaching serialization; high confidence.**
- **Affected journeys:** every capability-scoped worker `wg msg read`, `msg poll`, and likely `artifact <task>` list response; inbox reliability; agent coordination.
- **Evidence:** `IpcResponse.data` is flattened (`src/commands/service/ipc.rs:251-274`); serde flatten accepts maps/structs, not sequences. Worker operations return serialized vectors (`ipc.rs:738-758`). The worker path hard-switches to IPC and renders arrays only after a successful response (`src/worker_cli.rs:1-5`, `35-89`, `111-180`). `read_unread` writes cursor and marks messages read before returning (`src/messages.rs:631-696`). Live audit evidence showed cursor `agent-12.audit-operations-ux = 3`, message 3 marked read, no CLI response, exact daemon serialization error, then a 30-second retry timeout.
- **Additional diagnosis defect:** ordinary operation request IDs are fresh UUIDs (`worker_cli.rs:17-34`), but timeout text says “retry with the same request id” (`service/mod.rs:5947-5970`). The CLI does not automatically do so; only `DoneHandoff` derives a stable ID.
- **Counterevidence:** object-valued worker `show` responses often work, and the daemon stores audit/idempotency records. That does not make array responses serializable.
- **Recommendation:** `OPS-REC-001`.

### `OPS-002` — `config set` erases comments

- **Label/state:** **`[VERIFIED]` and `[CONTRADICTION]`**, current.
- **Severity/likelihood/confidence:** **S1 High; occurs on every generic edit of a commented file; high confidence.**
- **Evidence:** source comments and `docs/config-precedence.md:13-21` claim comments and unknown sections are preserved. Implementation uses semantic `toml::Value` plus pretty serialization (`src/commands/config_cmd.rs:3027-3102`), which cannot preserve formatting/comments. Clean-room reproduction removed both leading and inline comments.
- **Counterevidence:** named-profile-specific setters contain a deliberate line patcher and tests that preserve comments (`src/profile/named.rs:935-1015`, `2027-2048`, `2106-2113`). This proves a viable preservation pattern exists, but the generic config setter does not use it.
- **Recommendation:** `OPS-REC-002`.

### `OPS-003` — unknown config is accepted, ineffective, and lint-clean

- **Label/state:** **`[VERIFIED]` and `[CONTRADICTION]`**, current.
- **Severity/likelihood/confidence:** **S1 High; easy typo path; high confidence.**
- **Evidence:** generic set deliberately writes unknown paths (`config_cmd.rs:3027-3061`); typed deserialization ignores unknown fields. A reproduction persisted `[totally.unknown] key = "x"`, reported effective value unset, and blamed project-profile routing although no profile existed. `config lint` called it clean because lint reuses only known migration predicates (`config_cmd.rs:3476-3549`, `3644-3676`). The final lint summary counts “missing execution selection” as something migration “would fix,” although it prints that nothing was selected automatically (`3549-3642`).
- **Counterevidence:** known typed keys receive some validation, and the command truthfully says the effective value is unset. The cause/remedy is wrong and the stale key remains.
- **Recommendation:** `OPS-REC-002`.

### `OPS-004` — onboarding authority is internally inconsistent

- **Label/state:** **`[FACT]`, `[VERIFIED]`, and `[CONTRADICTION]`**, current.
- **Severity/likelihood/confidence:** **S1 High; observed help/source/test drift; high confidence.**
- **Evidence:** current setup help/source supports Pi only (`setup.rs:72-150`; `init.rs:21-84`). The direct retired-route invocation failed. At least five smoke scripts still execute retired routes, especially `tests/smoke/scenarios/setup_routes_complete_configs.sh:1-111` and `setup_scope_flag.sh:1-105`. README/install/concierge docs claim bare `worksgood` verifies Pi/plugin (`README.md:87-113`; `docs/guides/install.md:30-52`; `docs/worksgood-concierge.md:23-36`), while executable help and runtime expressly do not on existing graphs (`src/bin/worksgood.rs:6-16`; `src/concierge.rs:1620-1648`).
- **Counterevidence:** current runtime separation is coherent and has a strong thin-launcher smoke script. The problem is authority synchronization and inactive stale evidence, not necessarily the selected design.
- **Recommendation:** `OPS-REC-003`.

### `OPS-005` — `doctor` diagnoses Claude regardless of selected Pi route

- **Label/state:** **`[FACT]` and `[CONTRADICTION]`**, current.
- **Severity/likelihood/confidence:** **S1 High; normal on Pi-only installations; high confidence.**
- **Evidence:** doctor exit policy is 0/1/2 (`src/commands/doctor.rs:1-17`, `94-139`). Missing Claude always creates `Err`; Pi is `Info` if absent and guarded only if present (`166-226`, `267-412`). Setup's supported route is Pi. Doctor checks local `.wg`, selected Claude auth environment, and PID state, but does not resolve project profile fingerprint, effective route, Pi plugin compatibility/auth readiness, endpoint secret refs, config lint, socket responsiveness, or graph integrity.
- **Counterevidence:** its Claude OAuth-header trap, bash/git, Pi output-guard byte inspection, stale PID, JSON, hints, and stable exit codes are useful diagnostics for their specific conditions.
- **Recommendation:** `OPS-REC-004`.

### `OPS-006` — root-checkout cleanliness serializes independent publication

- **Label/state:** **`[VERIFIED]`**, current safety/liveness tradeoff.
- **Severity/likelihood/confidence:** **S1 High in self-hosted parallel operation; observed once in this audit; high confidence.**
- **Evidence:** landing holds a repository lock, compares the integration ref, verifies exact clean worker commit, then refuses if the attached root has any tracked/index status (`src/commands/completion_land.rs:82-122`, `201-249`, `310-350`). Audit-charter durable task log records the full incident independently of the original user message. The only error text is “integration root has tracked or index changes; refusing publication.”
- **Positive control:** untracked `.wg` does not block (`--untracked-files=no`), and refusing `reset --hard` over operator edits is correct. Atomic compare-and-fast-forward and exact reviewed evidence are strong.
- **Inference:** publication authority is unnecessarily coupled to synchronization of one attached checkout. A bare integration ref can advance safely without resetting a dirty checkout, leaving a loud “root not synchronized” state, or publication can run in a dedicated clean integration worktree.
- **Recommendation:** `OPS-REC-006`.

### `OPS-007` — three observability commands overstate their semantics

- **Label/state:** **`[FACT]` plus `[VERIFIED]` for metrics output**, current.
- **Severity/likelihood/confidence:** **S1 High for cost/operational decisions; certain; high confidence.**
- **Evidence:** process-local static atomics and no persistence/IPC make `wg metrics` a new empty collector (`src/metrics.rs:8-26`, `83-193`, `287-289`). Direct JSON was all-zero with max-u64 minimum. `wg spend` groups each usage record under `Utc::now()` (`src/commands/spend.rs:27-67`), making every run's historical breakdown “today.” `wg stats` is correctly named time statistics and derives durations from service/agent timestamps (`src/commands/stats.rs:29-120`); it should not be conflated with utilization.
- **Counterevidence:** overall spend totals still sum persisted task usage, and service status persists cleanup-lane snapshots. The defect is rollup semantics, not absence of all raw data.
- **Recommendation:** `OPS-REC-005`.

### `OPS-008` — HTML remote publishing is broad by default

- **Label/state:** **`[FACT]` and `[INFERENCE]`**, current.
- **Severity/likelihood/confidence:** **S1 High impact, user-dependent likelihood, high confidence.**
- **Evidence:** `wg html` defaults all tasks; `--public-only` is opt-in (`src/html.rs:105-138`; checkout-built help lines 320-344). `wg html publish add` likewise defaults without `--public-only`, then `run` builds and rsyncs. Transcript inclusion is off and sanitization/manual-review warnings are good, but task descriptions, messages, metadata, and topology are still published regardless of visibility.
- **Inference:** local TUI parity is a defensible generation default; a remote deployment default should invert to public-only because `visibility` otherwise creates a false expectation of an access boundary.
- **Recommendation:** `OPS-REC-007`.

### `OPS-009` — source and release package layouts diverge

- **Label/state:** **`[FACT]`**, current.
- **Severity/likelihood/confidence:** **S2 Medium; observed; high confidence.**
- **Evidence:** Cargo declares four binaries (`Cargo.toml:20-41`); cargo metadata and installed PATH confirm four. Installer/docs/release manifest package three (`docs/guides/install.md:1-14`; `scripts/install-wg.sh:403-493`, `645-654`; `release.yml:470-523`, `646-667`). CI source-install asserts only that three exist, not that the source/release sets match (`.github/workflows/ci.yml:127-162`). The guide also says “native binary pair” immediately before listing three (`docs/guides/install.md:240-258`).
- **Uncertainty:** `casa-adapter` may intentionally be source-only, but no user-facing packaging-status declaration was found. If deliberate, it needs explicit support/install policy rather than accidental Cargo behavior.
- **Recommendation:** `OPS-REC-008`.

### `OPS-010` — installer safety is good, but bundle replacement is not transactional

- **Label/state:** **`[FACT]` and `[INFERENCE]`**, partial.
- **Severity/likelihood/confidence:** **S2 Medium; low-probability partial failure; high confidence in implementation.**
- **Positive evidence:** checksum failure blocks; GitHub attestation is attempted when available; all payload files are checked before writes; collisions/symlinks are refused; receipt and destination must agree; uninstall preserves data (`scripts/install-wg.sh:290-493`, `609-654`). Shell and PowerShell synthetic installer tests are selected by CI (`ci.yml:10-42`, `151-165`).
- **Gap:** three independent temp-copy/rename operations precede receipt write. There is no rollback journal or directory-level atomic switch. Receipt ownership is name/path based, not a per-installed-file digest check; replacing a receipted binary manually does not remove it from receipt ownership.
- **Recommendation:** `OPS-REC-008`.

### `OPS-011` — secrets are mostly safe, but “keyring” can mean unencrypted file

- **Label/state:** **`[FACT]`**, current mixed/positive control.
- **Severity/likelihood/confidence:** **S2 Medium; common on headless Linux; high confidence.**
- **Positive evidence:** redaction by default, no values in list, stdin-safe script path, plaintext opt-in, path traversal rejection, OS keyring support, 0600/0700 on Unix, backend status, and 17 passing tests (`src/commands/secret_cmd.rs:13-184`; `src/secret.rs:123-165`, `349-418`, `520-696`).
- **Risk:** transparent fallback stores API keys as permission-protected but unencrypted file content. The warning is loud once per process, yet a config continues to say `keyring:<name>`. On non-Unix, fallback has no explicit ACL hardening in source. Secret paths and secrets config ignore `WG_GLOBAL_DIR`. Telegram tokens use another plaintext config plane.
- **Recommendation:** preserve fallback availability but expose resolved storage in every status/check result, unify global-dir semantics, add Windows ACL tests, and allow notification `api_key_ref`-style token references.

### `OPS-012` — service status and cleanup defaults are strong positive controls

- **Label/state:** **`[VERIFIED]` and `[FACT]`**, current positive control.
- **Severity/confidence:** **S4 Informational; high confidence.**
- **Evidence:** service status distinguishes missing, orphaned, stale and live state, cleans dead stale state, prints identity/config/profile, admission deferral, observers, cleanup, log path and recent errors (`service/mod.rs:4383-4795`). Worktree GC requires a filter, defaults dry-run, blocks dirty work, and recommends snapshot archive; destructive discard is double-explicit (`worktree_cmd.rs:267-455`). Fourteen bounded worktree-GC tests passed. Disk cleanup is dry-run by default and limits deletion to explicit ownership while preserving source (`disk_sentinel.rs:734-773`, `998-1166`, `1657-1764`).
- **Limitation:** positive unit/source evidence is not a cross-platform destructive-flow execution in this audit.

### `OPS-013` — help is curated but brittle for exploration and pipes

- **Label/state:** **`[VERIFIED]`**, current.
- **Severity/likelihood/confidence:** **S3 Low; routine discoverability/scripting issue; high confidence.**
- **Evidence:** built `wg --help` displayed 15 command rows and “118 more (`--help-all`)”; `--help-all` displayed 133 rows. The small primary list is a reasonable progressive-disclosure choice, but users must know a nonstandard global flag to discover setup, doctor, secret, disk, server, Telegram, user boards, metrics, spend, and trace. `target/debug/wg --help-all | head -45` panicked with exit 101 on broken pipe rather than exiting cleanly.
- **Counterevidence:** root help explicitly advertises `--help-all`, groups common commands, and subcommand help is generally rich.
- **Recommendation:** `OPS-REC-009`.

### `OPS-014` — platform support claims exceed runtime qualification

- **Label/state:** **`[FACT]` and `[UNCERTAINTY]`**, partial.
- **Severity/likelihood/confidence:** **S2 Medium; possible platform-specific failures; high confidence for missing qualification.**
- **Evidence:** releases build five targets, but normal Rust tests are Ubuntu-only; Windows gets installer testing, not service/TUI/worktree/secret behavior (`ci.yml:1-201`; `release.yml:122-151`). Doctor requires bash on all hosts and specifically Git-for-Windows bash (`doctor.rs:166-214`). `wg server` shells to Unix tools, while `wg service install` unconditionally emits systemd-user setup and its help carries no platform restriction (`src/commands/server.rs:227-338`; `src/commands/service/mod.rs:1185-1247`). Service start help calls its IPC path a “Unix socket” although Windows uses namespaced IPC (`service/mod.rs:80-105`).
- **Recommendation:** `OPS-REC-010`.

## 4. Contradictions and drift

### `OPS-DRIFT-001` — bare `worksgood` prerequisite behavior (**open, S1**)

**`[CONTRADICTION]`** README, install guide, and concierge guide say bare `worksgood` verifies Pi and ensures the plugin (`README.md:87-113`; `docs/guides/install.md:30-52`; `docs/worksgood-concierge.md:23-36`). Executable help and source say an existing graph directly opens the setup-neutral TUI and does not inspect those surfaces (`src/bin/worksgood.rs:6-16`; `src/concierge.rs:1620-1648`). Runtime source is authority. Resolve by regenerating all first-use prose from one journey contract.

### `OPS-DRIFT-002` — setup route set versus smoke suite (**open, S1**)

**`[CONTRADICTION]`** Current help/source supports only `pi`; multiple smoke scripts still require `claude-cli`, `codex-cli`, `openrouter`, local, and custom routes. A direct retired-route invocation failed. Retire/rewrite the scenarios or restore a declared compatibility surface; do not leave failing historical scripts in the current grow-only manifest.

### `OPS-DRIFT-003` — comment-preserving config edits (**open, S1**)

**`[CONTRADICTION]`** `docs/config-precedence.md:13-21` and `src/commands/config_cmd.rs:3027-3033` promise comment preservation. The implementation and reproduction show comments are erased. Named-profile code already uses a tested line patcher, demonstrating the mismatch is localized.

### `OPS-DRIFT-004` — config lint result and remedy (**open, S1**)

**`[CONTRADICTION]`** An unknown persisted key is called clean. Missing execution selection increments the finding count, after which lint says all findings are fixed by `wg migrate config`; migration cannot choose a Pi route (`config_cmd.rs:3476-3676`). Separate schema, migration, selection, secret and runtime findings with exact remedies.

### `OPS-DRIFT-005` — “daily” spend (**open, S1**)

**`[CONTRADICTION]`** CLI/help names `--today` and “Daily breakdown,” but every usage record is grouped under invocation date (`src/commands/spend.rs:27-67`). Use `completed_at` or rename the output to an undated aggregate until timestamps are reliable.

### `OPS-DRIFT-006` — cleanup metrics scope (**open, S1**)

**`[CONTRADICTION]`** `wg metrics` presents global cleanup/monitoring totals, but storage is in-process only. A separate CLI process starts at zero (`src/metrics.rs:8-26`; `src/commands/metrics.rs:1-20`).

### `OPS-DRIFT-007` — doctor versus sole model plane (**open, S1**)

**`[CONTRADICTION]`** setup says Pi is the sole supported LLM route; doctor calls missing Claude a hard failure and absent Pi informational (`setup.rs:72-107`; `doctor.rs:166-226`, `267-412`).

### `OPS-DRIFT-008` — package membership (**open, S2**)

**`[CONTRADICTION]`** source install exposes four Cargo binaries; docs/native archive/receipt expose three (`Cargo.toml:20-41`; `release.yml:470-523`; install guide). Declare `casa-adapter` source-only or distribute it consistently.

### `OPS-DRIFT-009` — upgrade availability wording (**open, S3**)

**`[CONTRADICTION]`** install guide says “When `wg upgrade` is available” (`docs/guides/install.md:285-299`); current built help exposes it. Its scope is managed source checkouts, so the guide should explain installer-managed versus source-managed upgrades rather than future tense.

### `OPS-DRIFT-010` — general runbook coverage (**open, S2**)

**`[FACT]`** `docs/ops/runbook.md` is titled and structured as a federation operator runbook; it covers node deploy/monitor/backup/rotation, dual-main footguns, and Pilot exceptionally well. General install, config/profile drift, Pi-only readiness, daemon IPC, logs, disk, landing contention, and upgrade rollback remain distributed across install guide, concierge guide, `AGENT-SERVICE.md`, `LOGGING.md`, and help. No single release-scoped general operator runbook was found.

## 5. Risk register

| ID | Risk event | Impact | Likelihood | Existing control | Residual risk / confidence |
|---|---|---:|---:|---|---|
| `OPS-R001` | worker inbox read mutates cursor then response serialization fails | High coordination loss/confusion | Certain on array response path | message JSONL retained; daemon log; cursor; no fs fallback by design | **High / high confidence** |
| `OPS-R002` | operator uses `config set`; comments/context disappear | High config-maintenance damage | Likely | atomic file write; effective source print | **High / high confidence** |
| `OPS-R003` | typo/extension key persists but has no effect | High silent misconfiguration | Likely | effective value shown unset | remedy is falsely profile-specific; lint says clean; **high** |
| `OPS-R004` | Pi-only host fails doctor due missing Claude | High false outage/automation failure | Likely | rich individual hints/JSON | overall exit is wrong; **high** |
| `OPS-R005` | operator makes budget/cleanup decision from false rollup | High cost/capacity error | Likely if commands used | raw task usage and service cleanup snapshot exist | daily/metrics semantics false; **high** |
| `OPS-R006` | unrelated root edit blocks accepted publication | High throughput loss/retry cost | Possible in parallel/self-hosted use | fail-safe refusal, land lock, reviewed commit retained | no publication queue/parking remedy; observed; **high** |
| `OPS-R007` | remote HTML deploy leaks non-public task data | High confidentiality impact | User-dependent | transcripts off; warnings; public-only flag; sanitizer | publish default remains all tasks; **medium-high** |
| `OPS-R008` | headless keyring fallback stores unencrypted API key file | Medium confidentiality impact | Common on headless Linux | 0600/0700 and warning; backend show | name still says keyring; Windows ACL unproven; **high/medium** |
| `OPS-R009` | source/native package or mid-upgrade version skew | Medium supportability/runtime impact | Low–possible | receipt, checksum, payload precheck, per-file rename | no set transaction; 3-vs-4 layout; **high** |
| `OPS-R010` | Windows/macOS-specific runtime fails after successful install | Medium–high | Possible | five release builds; Windows installer test | little non-Linux runtime qualification; **high for gap** |
| `OPS-R011` | stale setup smoke files create false confidence | High regression blind spot | Current | current help and newer thin-launcher smoke | manifest not normal-CI run; **high** |
| `OPS-R012` | cleanup destroys uncommitted worker work | Critical | Low under intended CLI | filter required; dry-run; dirty block; archive-first; explicit discard | strong positive control; **high confidence** |

## 6. Recommendations and acceptance checks

### `OPS-REC-001` — P0: repair worker IPC response semantics

**`[RECOMMENDATION]`** Replace flattened arbitrary `data` with a named JSON field (or flatten only map payloads); preserve one stable request ID across automatic retries; make read delivery transactional/reconcilable; and ensure the daemon returns an error response rather than dropping the connection if response serialization fails.

**Acceptance checks:**

1. Real-socket tests round-trip empty/nonempty arrays and objects for `MessageRead`, `MessagePoll`, `ArtifactList`, `Show`, and `Context`.
2. Inject response loss after `read_unread`: retry returns the same delivery or a precise “already committed through message N” receipt, never an empty success that hides consumed data.
3. Client-generated request ID is printed on failure and automatically reused; timeout text matches actual behavior.
4. A worker-mode end-to-end test receives a message and artifact list with no direct graph access.
5. Daemon log has no `can only flatten structs and maps` and a serialization failure cannot terminate the one-response connection silently.

### `OPS-REC-002` — P0/P1: make config editing lossless and schema-aware

**`[RECOMMENDATION]`** Use `toml_edit` or the existing tested profile line-patcher for generic edits. Establish explicit unknown-key policy: reject unknown typed paths by default, or require an extension namespace/`--raw`. Add schema lint independent of migration lint, and make each finding name the command that can actually fix it.

**Acceptance checks:**

1. Golden test preserves leading/inline comments, blank lines, order, arrays, and unrelated extension tables byte-for-byte except the edited scalar.
2. A typo such as `dispatcher.max_agent` fails before write and suggests `max_agents`.
3. If raw unknown keys are supported, output says “stored as extension; ignored by core Config,” not “profile-owned routing.”
4. `config lint` separately reports `schema`, `migration`, `execution-selection`, `secret-ref`, and `runtime-readiness`; only migration findings recommend `wg migrate config`.
5. Human and JSON modes agree on counts/severity and return a documented nonzero code for actionable errors.

### `OPS-REC-003` — P1: elect and generate one onboarding contract

**`[RECOMMENDATION]`** Make executable journey tests the authority for: existing bare entry, new route-free bootstrap, attended Pi chat, explicit automation setup, and graph-only mode. Generate README/install/concierge snippets and retire or rewrite every legacy-route smoke scenario.

**Acceptance checks:**

1. Search finds no current scenario invoking unsupported setup/init routes unless explicitly labeled migration-negative.
2. `worksgood --help`, README, install guide, concierge guide, and smoke expected argv agree on whether Pi/plugin are touched.
3. One clean-home Linux test and one Windows-equivalent test cover install → existing/new entry → graph-only → explicit automation dry-run.
4. Manifest/CI rejects a scenario that names a retired route as a success path.

### `OPS-REC-004` — P1: create route-aware `wg doctor --all`

**`[RECOMMENDATION]`** Preserve current targeted checks but drive severity from the effective selected route. Compose config/profile fingerprint, exact handler/Pi/plugin/auth readiness, secret refs, daemon handshake, graph check, cached/fresh disk sentinel, installer/source identity, and platform prerequisites. Explain the narrower scope of `wg check`.

**Acceptance checks:**

1. Pi-only healthy project without Claude exits 0.
2. Claude is warning/info when unused and error only when an effective route requires it.
3. Drifted project profile fails closed and doctor prints exact reselect command.
4. JSON has stable check codes and remediation commands; exit 0/1/2 policy is tested.
5. Doctor never reveals secret values and can validate refs/permissions.

### `OPS-REC-005` — P1: make accounting persisted and time-correct

**`[RECOMMENDATION]`** Persist cleanup metrics in daemon/service state or query daemon IPC; encode “since process start” if that is the intended scope. Group spend by task `completed_at` (and explicitly classify missing timestamps). Avoid max-u64 sentinel in JSON.

**Acceptance checks:**

1. Cleanup in one process is visible to a later `wg metrics` invocation and after daemon restart according to documented retention.
2. Two tasks completed on different dates appear on those dates; `--today` excludes the old task.
3. Missing/malformed completion times are surfaced as `unknown_date`, not silently moved to today.
4. Cost totals reconcile exactly to `wg show --json` task usage.

### `OPS-REC-006` — P1: decouple publication from a human's dirty root checkout

**`[RECOMMENDATION]`** Publish the reviewed ref under the land lock using a dedicated integration worktree or allow ref advancement without resetting a dirty attached checkout. Treat root synchronization as a separate, loud state. Until then, list exact blocking paths and support a nonterminal “accepted, awaiting integration-root release” state that workers can enter without failing.

**Acceptance checks:**

1. Unrelated tracked root edits survive byte-for-byte while accepted commit atomically advances the integration ref or parks visibly.
2. Worker receives exact paths and operator commands; no worker is instructed to stash/reset another actor's files.
3. No second review is required when candidate/ref inputs are unchanged.
4. Concurrent lands remain compare-and-fast-forward and never overwrite dirty state.

### `OPS-REC-007` — P1: make remote publication public-only by default

**`[RECOMMENDATION]`** Keep local `wg html` TUI parity if desired, but default `html publish add/run` to public-only. Require a conspicuous `--include-non-public` acknowledgement for remote rsync. Render a manifest of included visibility classes and sanitizer limitations.

**Acceptance checks:** private task title/body/message never reaches a default deployment fixture; all-task publish requires explicit opt-in; transcripts remain separate explicit opt-in; generated manifest can be reviewed before rsync.

### `OPS-REC-008` — P2: unify package manifest and transactional upgrade

**`[RECOMMENDATION]`** Declare the supported binary set once and generate Cargo/release/installer/receipt/docs checks from it, including an explicit `casa-adapter` policy. Stage and verify the complete replacement, preserve prior binaries, and rollback the set on any write failure.

**Acceptance checks:** source/native membership test is intentionally equal or records an explicit source-only allowlist; failure after each replacement step restores prior hashes/receipt; uninstall verifies installed hashes or asks before deleting a changed receipted binary; archive execution tests run every shipped binary.

### `OPS-REC-009` — P2: improve command discovery and pipe behavior

**`[RECOMMENDATION]`** Keep curated help but add category search/listing (`wg help config`, `wg help observe`, etc.), surface `doctor`, `setup`, `secret`, and cleanup in the main recovery path, and treat EPIPE as clean termination.

**Acceptance checks:** `wg --help-all | head -1` exits 0 without panic; JSON/help completion lists all commands; novice usability test can find setup, diagnosis, logs, secrets and cleanup from root help only.

### `OPS-REC-010` — P2: publish a truthful platform/support and general-ops runbook

**`[RECOMMENDATION]`** Separate core CLI support, service support, TUI support, server tooling, and release-artifact availability by OS/arch. Gate Unix-only commands/help. Add a release-scoped general runbook linking the federation runbook rather than making federation procedures stand in for core operations.

**Acceptance checks:** Windows runtime CI covers init/config/secrets/service named-pipe start-status-stop/worktree negative paths; macOS covers install/TUI/service basics; `wg server`/`service install` fail early with platform-specific guidance; runbook has install/upgrade/rollback, config/profile/secret backup, daemon/log/task triage, disk/worktree recovery, landing contention, and verification commands.

## 7. Evidence ledger, commands, limitations, and uncertainty

### 7.1 Primary evidence index

| Topic | Direct evidence |
|---|---|
| Package/install | `Cargo.toml:1-41`; `scripts/install-wg.sh:1-55`, `179-229`, `290-493`, `609-654`; `scripts/install-wg.ps1:131-166`, `369-650`; `docs/guides/install.md:1-324`; `.github/workflows/{ci,release}.yml`; `tests/install/*` |
| Attended/setup | `src/bin/worksgood.rs:1-158`; `src/concierge.rs:1620-1735`; `src/commands/{init,setup}.rs`; `docs/worksgood-concierge.md:1-121`; `tests/smoke/scenarios/worksgood_{attended_pi_simple,one_model_setup}.sh` |
| Config/profile | `src/config.rs:5480-5648`, `5807-6144`, `6733-6846`; `src/profile/{named,project}.rs`; `src/commands/{config_cmd,profile_cmd}.rs`; `docs/config-precedence.md:1-117` |
| Secrets/endpoints | `src/config.rs:1120-1335`, `6147-6198`; `src/secret.rs:1-180`, `330-418`, `460-696`; `src/commands/secret_cmd.rs:1-390`; `src/notify/config.rs`; `src/notify/telegram.rs` |
| Service/IPC | `src/commands/service/mod.rs:80-128`, `4383-4795`, `5920-6090`; `src/commands/service/ipc.rs:245-408`, `716-1110`; `src/worker_cli.rs:1-180`; `src/messages.rs:120-150`, `631-710` |
| Diagnosis/observability | `src/commands/{status,doctor,check,show,trace,stats,spend,metrics}.rs`; `src/metrics.rs`; `docs/{AGENT-SERVICE,LOGGING}.md` |
| Cleanup | `src/disk_sentinel.rs`; `src/commands/{disk,cleanup,worktree_cmd}.rs`; relevant inline tests |
| Human/remote surfaces | `src/tui/`; `src/html.rs`; `src/commands/{server,telegram,user}.rs`; `docs/guides/server-setup.md`; HTML/TUI/Telegram smoke/integration files |
| Publication incident | `src/commands/completion_land.rs:30-249`; live graph task `audit-charter` log; live daemon log; message queue/cursor files |
| General operations docs | `docs/ops/runbook.md`; `docs/guides/install.md`; `docs/AGENT-SERVICE.md`; `docs/LOGGING.md`; built CLI help |

### 7.2 Reproducible command record

Commands ran from `/home/bot/wg/.wg-worktrees/agent-12` unless noted. Worker-control variables were removed only for isolated, read-only or temporary-directory operator reproductions.

```text
git rev-parse HEAD
  1899cdcf4fd414245a735e2e8f8c81d92b536ec5

git diff --name-status b0892ea..HEAD
  only audit README and leaf files 15–17

cargo build --locked --bin wg --bin worksgood
  exit 0

target/debug/wg --version
  wg 0.1.0

target/debug/wg --help > /tmp/wg-help.txt
  15 command rows; “... and 118 more (--help-all)”

target/debug/wg --help-all > /tmp/wg-help-all.txt
  133 parsed command rows

target/debug/wg --help-all | head -45
  wg panicked on Broken pipe; upstream exit 101

wg msg read audit-operations-ux --agent agent-12
  initial audit-start invocation: “No response from service” after cursor/read mutation
  repeat: 30.010s, exit 1, “Worker control IPC timed out after 30s”

rg 'Worker IPC|can only flatten|late response' /home/bot/wg/.wg/service/daemon.log
  MessageRead followed by “can only flatten structs and maps (got a sequence)”
  many late-response cancellation records

cursor + message inspection
  .wg/messages/.cursors/agent-12.audit-operations-ux contained 3
  message 3 had status=read/read_at at the first failed read timestamp

isolated commented-config reproduction
  wg config set dispatcher.max_agents 4 --no-reload
  leading and inline comments disappeared

isolated unknown-key reproduction
  wg config set totally.unknown.key x --no-reload
  key persisted; effective unset; incorrect profile-owned note
  wg config lint --local called file clean, then claimed one migratable issue

isolated retired-route reproduction
  wg setup --route claude-cli --yes --dry-run
  exit 1: supported route is pi

env -u worker-control ... target/debug/wg --dir .wg metrics --json
  all counters zero; min_cleanup_duration_ms = 18446744073709551615

cargo metadata --no-deps --format-version 1
  bin targets: casa-adapter, nex, wg, worksgood

command -v worksgood wg nex casa-adapter
  all four present from source installation on this machine

cargo test --locked --lib secret::tests -- --test-threads=1
  17 passed
cargo test --locked --lib project_profile_overlay -- --test-threads=1
  3 passed
cargo test --locked --bin wg worktree_gc_ -- --test-threads=1
  14 passed
```

**`[UNCERTAINTY]`** Main-help row counts are conservative grep counts, not a public/private command taxonomy. Dynamic daemon/graph evidence is point-in-time operational evidence outside the immutable audit snapshot; it corroborates source but is not part of the committed repository. The original user report was treated only as a lead; the finding relies on independently inspected source, queue/cursor state, durable task log, daemon log, and reproduction.

### 7.3 What would change confidence

- A current release-path, real-socket worker test demonstrating array response serialization and replay would falsify or narrow `OPS-001`; current source and live behavior say the opposite.
- A hidden comment-preserving layer after `toml::to_string_pretty` would falsify `OPS-002`; the byte reproduction rules that out for the executed path.
- Persisted metrics loaded before `get_metrics_snapshot` or per-task historical dates elsewhere would narrow `OPS-007`; no such read appears in the invoked commands.
- Current CI logs running and passing the retired setup smoke scenarios against the same commit would require explanation; workflow selection and direct help/source disagree.
- Windows/macOS runtime logs could materially improve the platform assessment; build matrices alone cannot.
- A declared packaging policy marking `casa-adapter` source-only would convert `OPS-009` from contradiction to documented product boundary.

**`[FACT]`** Final artifact validation is limited to this audit deliverable: nonempty file, required journey/maps/findings sections, and repository diff hygiene. No production code, tests, or pre-existing docs were modified.
