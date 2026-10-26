# First-user release rehearsal

Initial rehearsal: 2026-09-12 (UTC)

Final deployed-stable rerun: 2026-09-15 (UTC)

Task: `first-user-release-rehearsal`

Result: **the installed stable path completed one useful, reviewed, locally landed change; cancellation/restart cleanup passed; missing-auth admission remains misleading; three directly exercised fixtures and one startup summary were repaired candidate-locally**

> **Post-fix evidence reconciliation (2026-09-15):** The historical completion record includes a failed optional `explicit_execution_selection` receipt (`b3:9635c92af450ec594538fefaa7283a94163723a3dba693bb9e08a50c5ccb586b`, exit 101), despite later prose saying the repaired scenario passed; the subsequent FLIP rejection correctly caught that mismatch. The exact checked-in scenario has now passed against integrated source `5a4b9f98` through the candidate-bound Rust/subreaper harness, with the successful command captured on `close-pi-execution-release-candidate`. See [Integrated Pi execution release-candidate closure](pi-execution-release-candidate-closure.md). The old failed receipt and rejection remain historical evidence.

## Final deployed-stable rerun (2026-09-15)

This is the release decision evidence. The 2026-09-12 run below is retained as historical context, not presented as current-version evidence.

### Exact installed identity and boundary

| Item | Exact value |
|---|---|
| Operator-deployed source | `1f33c4e407feb29a0c05d0ca5ab0a1b4fa75844a` |
| Required CI | GitHub run `34914083289`, operator-reported successful required jobs |
| Installed binary | `/home/bot/.cargo/bin/wg` |
| `wg --version` | `wg 0.1.0` |
| Installed SHA-256 | `56c70833ba52c3899671d3ca28b6a8e96faa606c3c11260b96feea5f7417d750` |
| Installed size | 86,735,608 bytes |
| Pi | `/home/bot/.nvm/versions/node/v25.4.0/bin/pi`, `0.84.4` |
| Selected stable route | `pi:openai-codex:gpt-5.6-sol`, reasoning `high` |
| Experimental flags | `WG_EXPERIMENTAL_OPAQUE_ASSIGNMENT` absent; `WG_PI_PROCESS_WAKE_EXTENSION` absent |
| Disposable root | `/tmp/wg-first-user-release-rehearsal-agent108-final` |

The operator had already installed and restarted this exact build. This worker did **not** install globally or touch the production daemon. All commands used explicit `/home/bot/.cargo/bin/wg`, disposable `--dir` graphs, isolated `WG_GLOBAL_DIR`/XDG directories, and `--no-supervise`. `HOME` remained the existing user home only for the approved Pi-owned authentication lookup; no auth file or secret value was read, copied, exported, or logged.

### Terminal setup, isolation, and route inheritance

The real setup action was driven under `script -qec` from the disposable project:

```sh
/home/bot/.cargo/bin/wg --dir "$P/.wg" setup \
  --route pi --scope local --yes \
  --model pi:openai-codex:gpt-5.6-sol
```

It took 23 ms and wrote only `project-useful/worksgood.toml`. The setup output truthfully said that Pi auth/model were **not verified**, Pi owns `/login`, Console settings were unchanged, and worker spawn would use hermetic JIT. The now-deprecated `--scope local` spelling produced a warning but still selected project scope. `wg config --models` showed default, strong, weak, task agent, both FLIP lanes, evaluator, and every other role inheriting the same exact route. Unlike the historical run, no manual per-role override was needed.

The project setup scaffolding was inspected for credential assignments and committed before dispatch. The control project had no `worksgood.toml`; the isolated global directory had no `config.toml`; setup/execution in the useful project did not create either. The useful repository had no Git remote.

A fresh route-less project with a published task started a graph daemon but consumed no attempt. `wg status` exposed:

```text
Admission deferred: 1 task(s), no attempt or retry budget charged
WG-EXEC-ROUTE-MISSING ...
corrective_action=`wg setup --route pi --yes --model pi:<provider>:<model>`
```

That is actionable and correctly non-consuming. The immediate `wg service start` summary was not: it said `executor=claude, model=default` even for the selected Pi project. The candidate-local onboarding fix now renders the same project route snapshot admitted by the daemon; the updated real terminal scenario passes. This fix is not claimed deployed.

An empty-`HOME`, credential-free probe remained `Open` for 120.892 s with no attempt and no owned process after stop. WG classified the absent Pi login as `WG-PI-PROVIDER-UNSUPPORTED` and suggested changing the model, which is misleading. Direct Pi 0.84.4 against the same provider/model failed immediately and correctly with:

```text
No API key found for openai-codex.
Use /login to log into a provider via OAuth or API key.
```

Thus login is demonstrably Pi-owned and requires no WG secret, while WG's new admission diagnostic still needs to preserve the login-vs-unsupported distinction. This probe is a **blocked onboarding case**, not a successful auth integration test. The successful case below is the separate real-auth evidence.

### First useful reviewed change through real Pi

The PTY-added task was neutral POSIX shell work: make `hello.sh` accept an optional name, document both invocations, and add `tests/test_hello.sh`. Its instructions explicitly rejected Cargo/WG-smoke overreach and asked the worker to use the supported optional evidence interface:

```sh
wg done useful-greeting --check './tests/test_hello.sh'
```

Actions and observations:

1. PTY: `wg add ... --id useful-greeting`, then `wg publish useful-greeting --only`.
2. Terminal: start the disposable service with one worker and no coordinator/supervisor.
3. Real tmux PTY (`180x44`): launch `wg tui`; observe `useful-greeting (in-progress)`.
4. Kill only that tmux session, restart the same TUI, and observe the same persisted in-progress row.
5. Stop/restart the disposable daemon while `agent-1` remained alive under its original process/session identity. The restarted daemon saw one live agent, the task retained generation 0 / attempt `attempt-0-1` / fence 1, and no second agent directory appeared.
6. Observe immutable baseline evidence `b3:02e8f034...` and worker-selected optional evidence `b3:591a3dda...` enter manifest `b3:21fc7624...` **before** review.
7. Observe two-phase FLIP pass `b3:b01eff33...`, Eval pass `b3:9f1799be...`, one local publication receipt `b3:e33e5d23...`, and `Done/Landed`.
8. TUI: observe `useful-greeting (done)`.

Service start to Done took 126 s (`01:02:04Z`–`01:04:10Z`). There was one source worker, no retry, no semantic repair round, and no landing rescue. The source task reported 32,229 input, 1,774 output, 34,003 total tokens, and `$0.3762`; the separate two-review lane reported 18,299 tokens and `$0.1155` (`$0.0626` FLIP, `$0.0528` Eval). The task's default review policy was advisory, but both reviewers actually passed; this run does not relabel advisory as strict.

The local integration checkout landed exactly `e1c1955d48f3265a12823e6848c4cc65399eaa10` with only:

```text
README.md           | 14 +++++++++++++-
hello.sh            |  3 ++-
tests/test_hello.sh | 18 ++++++++++++++++++
```

Post-landing checks produced `Hello, World!`, `Hello, Ada!`, `hello tests passed`, and a clean `sh -n`. The checkout was clean and `git remote -v` was empty. Therefore tests passed, semantic review passed, and local landing occurred; **remote publication did not occur**.

### Cancellation, restart, and direct-Pi control

A second real stable-path task asked Pi to run foreground `sleep 300`. When the tool was live, `agent-2` and owned sleep PID `2869184` were observed. `wg agents kill agent-2 --force` removed the owned sleep; reconciliation recorded the deliberate probe as `Failed/Lost` with retry count 1. After disposable daemon stop/start, it remained failed, no `agent-3` appeared, and the command was not revived. This is a cancellation test, not a useful-task success.

For a comparable current direct-Pi control, Pi ran `sleep 3; printf DIRECT_PI_WAIT_OK` through its bash tool and returned the marker once. Wall time was 10.229 s; Pi reported 1,186 tokens and `$0.00613`. This proves the provider/runtime could execute and wait for the neutral command; it does **not** prove WG's optional managed wake adapter.

The accepted `prove-pi-process-wakeup` evidence integrated into this deployed source remains the managed-wake evidence: one registered operation, automatic continuation in the same live RPC session/handle, deduplicated wake, and targeted cancellation that reaped its managed command while an unrelated process survived. The adapter is opt-in and was deliberately absent here. `verify-pi-execution-boundary` remains **NO-GO** for opaque Pi: selector dialect, incomplete pinned invocation, and managed-wake integration are unresolved. Neither experiment was enabled or treated as a stable-path requirement.

Final teardown stopped all disposable daemons, killed the disposable tmux server, and found no process whose command line contained the disposable root. The useful checkout remained clean. No global configuration, Pi Console setting, release ref, origin/main ref, or production service was changed.

### Concrete final onboarding defects

1. **Missing login is mislabeled before Pi can explain it.** Empty Pi state becomes `WG-PI-PROVIDER-UNSUPPORTED`, stays Open indefinitely, and suggests changing the configured model. Setup and direct Pi correctly point to `/login`.
2. **Installed service-start summary lies about a selected route (fixed candidate-locally).** It printed `executor=claude, model=default` while the daemon and worker used `pi:openai-codex:gpt-5.6-sol`. The startup renderer now uses the authoritative project snapshot; no deployment is implied.
3. **Admission help is one command away from first output.** A missing route is visible in `wg status`, but `wg service start` says “started and ready” without naming the deferred task or `wg status` as the next action.
4. **Stopped-service launch settings can confuse rehearsal changes.** Reusing a graph first started with `--max-agents 0`, then starting with `--max-agents 1`, produced a start summary claiming 1 while the daemon retained 0. The clean control avoided relying on that graph, and this larger settings-authority issue is only recorded.
5. **Unrelated registry refresh noise remains.** Exact Pi-route daemons logged missing OpenRouter native-registry credentials even though the source and review route stayed on Pi/openai-codex and completed. It did not change routing, but it makes first-run diagnosis less credible.
6. **Explicit cancellation is projected as Lost/retry.** The requested kill correctly reaped the tree and did not revive, but user-facing lifecycle says `Failed/Lost` and increments retry count rather than naming deliberate cancellation.
7. **Optional execution experiments remain incomplete by design.** Opaque assignment is NO-GO, and managed wake remains an opt-in adapter. The flags were absent; no universal rollout is authorized.

The exact-route/login findings were sent to downstream `fix-first-user-exact-route`. Larger settings/cancellation projection work is recorded rather than silently expanded here.

### Release-level matrix

| Case | Final evidence and outcome |
|---|---|
| Ordinary useful task / neutral non-Rust work | **PASS, real Pi**: one shell change, optional evidence, FLIP+Eval pass, one local landing, no rescue |
| Same-worker deterministic repair | **PASS, controlled fixture**: `completion_repair_loop`; unchanged and changed candidates preserve owner/attempt/fence and exhaust at a visible decision |
| Semantic help / contract correction | **PASS, controlled fixture**: `completion_semantic_help`; receipt-bound rejection, idempotent attention across restart, explicit correction invalidates stale binding; not live semantic proof |
| Managed wait/wake | **REUSED accepted current-source evidence**, optional adapter only; direct Pi current control passed; adapter absent from stable rehearsal |
| Cancellation race | **PASS, real stable path** plus process-ownership harness: owned sleep reaped, no revival/duplicate after restart |
| Bounded provider outage | **PASS, controlled fixture** after adding the current offline-registry response to its fake Pi; initial+3 bound, exclusions, pause/cancel, route drift, and at-most-one publication |
| Route change | **PASS, controlled fixture**: `unified_project_route_authority`; inherited new attempts follow the project revision while active bindings are not rewritten |
| Restart | **PASS, real stable path**: TUI and daemon restarted around the same useful attempt; one worker and one publication |
| Cyclic/downstream blocker visibility | **PASS, real tmux controlled fixture** after correcting stale split-pane keystrokes; root blocker and one safe action are visible and traversal is deduplicated |
| Runtime upgrade | **PASS for installed identity and actual useful path**; CLI/source/operator daemon digest agreement is the deployment checkpoint, not re-proven by this worker |
| Opaque Pi rollout | **NO-GO / intentionally excluded**, not a failed stable requirement |

The controlled fixtures use the exact candidate binary but do not prove provider login or model semantics. The real useful and cancellation runs do.

### Current checks, failures, and directly observed repairs

The final passing results were:

- `completion_repair_loop`: 1.985 s through the Linux Rust subreaper.
- `completion_semantic_help`: 2.543 s through the same harness.
- `unified_project_route_authority`: 13.404 s.
- `source_provider_bounded_recovery`: 72.960 s after fixture repair.
- `smoke_process_ownership_cleanup_real_entry_point`: 80.110 s (104.861 s including build/harness overhead).
- `setup_route_activation_preflight`: 0.780 s.
- focused `service::tests::kill_process_force_tree_reaps_descendants`: 0.410 s (121.063 s including build), one pass under pinned Rust 1.96.0.

The non-blocking nightly warning did not reproduce in this single focused run, and the real cancellation also left no process. One pass does not erase the recorded GitHub nightly failure; it remains a flake warning, not an invented hard gate.

Failures were preserved and repaired rather than called passes:

1. `completion_repair_loop` initially failed because its old `Enter End` sequence selected the last graph row under the current split-pane TUI. The fixture now opens each row and pages Detail; the real tmux assertion sees `ROOT BLOCKER` and `one safe action`.
2. `source_provider_bounded_recovery` initially never admitted its fake route after current Pi offline preflight was integrated. Its fake Pi now answers `--list-models` for both exact fixture routes. Intermediate reruns exposed the route-drift response mismatch; the final original fail-closed expectation passes.
3. The installed `explicit_execution_selection` exposed both its obsolete “service start must fail” expectation and the real `claude/default` startup-summary bug. The scenario now tests non-consuming admission via human `wg status`; the candidate-local startup display uses the authoritative project route. Candidate SHA `e0254430ff46a728540dbcb4f6e50de27d31dacebdac420aa439b57cb14c589d` passed the updated 2.000 s terminal scenario. This is fix evidence, not evidence that the already-installed binary changed.

Manual interventions in the useful path were limited to committing setup scaffolding before dispatch and deliberately driving TUI/service interruption. No contract edit, retry, worktree edit, forced dependency removal, or lifecycle-state edit was needed. The cancellation task was intentionally killed. Fixture diagnosis/repairs are test maintenance, not unattended product success.

## Historical 2026-09-12 scope and safety boundary

This was a candidate-scoped rehearsal, not a release and not a production-daemon exercise.

- Disposable root: `/tmp/wg-first-user-release-rehearsal-agent93`
- Useful-work project: `project-a`
- Graph-only control: `project-b`
- No-auth probe: `project-noauth`
- Candidate install root: `install/`
- Isolated WG state: `wg-global/`, `xdg-config/`, and `xdg-cache/`
- Existing authentication: the user-approved Pi context already owned by Pi under the existing `HOME`; its files and values were neither read, copied, printed, nor exported.
- No WG secret was created. The isolated `WG_GLOBAL_DIR` had no `config.toml` at teardown.
- No release was published, no Git remote existed in the disposable project, and nothing was pushed.
- The production WG daemon was not installed, stopped, or restarted.

The real-model section below used that existing Pi-owned authentication. The missing-auth and completion-repair sections used isolated/credential-free fixtures and are **not** evidence of model or authentication integration.

## Initial candidate identity (historical)

| Item | Exact value |
|---|---|
| Source commit | `e0a840eff10f8944cdc699fc62afa5afea43f20b` |
| Source describe | `v0-self-hosting-baseline-2871-ge0a840ef` |
| Candidate command | `CARGO_TARGET_DIR="$PWD/target" cargo install --path . --locked --root /tmp/wg-first-user-release-rehearsal-agent93/install` |
| Candidate binary | `/tmp/wg-first-user-release-rehearsal-agent93/install/bin/wg` |
| `wg --version` | `wg 0.1.0` |
| SHA-256 | `325e9e994b08a07fe760c192b600af2b2e73aa89a576ad69960f88014985b9b5` |
| Binary size | 86,074,584 bytes |
| Rust | `rustc 1.96.0 (ac68faa20 2026-05-25)` |
| Cargo | `cargo 1.96.0 (30a34c682 2026-05-25)` |
| Pi executable | `/home/bot/.nvm/versions/node/v25.4.0/bin/pi` |
| Pi version | `0.84.4` |
| Install elapsed | 219 s (17:36:10Z–17:39:49Z) |

## Initial commands and human-flow actions (historical)

The command pattern below was used throughout. Inherited worker/graph variables were removed; `WG_GLOBAL_DIR`, `XDG_CONFIG_HOME`, and `XDG_CACHE_HOME` pointed into the disposable root.

```sh
W=/tmp/wg-first-user-release-rehearsal-agent93/install/bin/wg
P=/tmp/wg-first-user-release-rehearsal-agent93/project-a

cd "$P"
env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_GRAPH_ID \
  HOME="$HOME" \
  WG_GLOBAL_DIR=/tmp/wg-first-user-release-rehearsal-agent93/wg-global \
  XDG_CONFIG_HOME=/tmp/wg-first-user-release-rehearsal-agent93/xdg-config \
  XDG_CACHE_HOME=/tmp/wg-first-user-release-rehearsal-agent93/xdg-cache \
  "$W" --dir "$P/.wg" ...
```

### 1. Install, initialize, and refuse an unselected route

Both disposable projects were ordinary Git repositories. The candidate ran:

```sh
wg init --no-agency
wg service start --max-agents 0 --no-coordinator-agent --no-supervise
```

The graph-only control refused service startup with exit 1 and an actionable error:

```text
error[WG-EXEC-UNSELECTED]: No project Pi route is selected.
  wg profile select pi
  wg setup --route pi --yes --model pi:<provider>:<model>
```

No daemon socket/state was left for that failed start.

### 2. Configure one exact, project-local Pi route

Setup was driven inside a real PTY using `script -qec`:

```sh
wg setup --route pi --scope local --yes \
  --model pi:openai-codex:gpt-5.6-sol
```

Elapsed time was under one second. Its truthful bounded preflight said:

```text
Pi handler: AVAILABLE (.../bin/pi)
Profile: project-local route is effective; global active-profile intentionally unchanged
pi-worksgood: hermetic JIT at worker spawn; Console settings unchanged
Pi auth/model: NOT VERIFIED (Pi owns login and model discovery)
Next: run `pi`, use `/login` if needed ... and send a test prompt.
... no cross-provider fallback is selected.
```

`project-a/worksgood.toml` was written; `project-b/worksgood.toml` and the isolated global `config.toml` remained absent.

The setup projection unexpectedly assigned weak/reviewer roles to `pi:openrouter:deepseek/deepseek-chat`, outside the one approved authentication context. Before real execution, these project-local roles were explicitly changed to the approved route:

```sh
wg config --local --tier fast=pi:openai-codex:gpt-5.6-sol \
  --set-model evaluator pi:openai-codex:gpt-5.6-sol \
  --set-model flip_inference pi:openai-codex:gpt-5.6-sol \
  --set-model flip_comparison pi:openai-codex:gpt-5.6-sol \
  --set-model assigner pi:openai-codex:gpt-5.6-sol \
  --set-model triage pi:openai-codex:gpt-5.6-sol \
  --set-model compactor pi:openai-codex:gpt-5.6-sol \
  --set-model placer pi:openai-codex:gpt-5.6-sol \
  --set-model chat_compactor pi:openai-codex:gpt-5.6-sol \
  --set-model coordinator_eval pi:openai-codex:gpt-5.6-sol \
  --set-model reviewer pi:openai-codex:gpt-5.6-sol
```

`wg config --models` then showed all 17 roles on exactly `pi:openai-codex:gpt-5.6-sol`.

### 3. Missing Pi login is Pi-owned and actionable

A separate `env -i` probe used a new empty `HOME`, with no provider variables or copied files. After project-local setup, its first task failed before work with:

```text
No API key found for openai-codex.
Use /login to log into a provider via OAuth or API key.
```

WG reported `Status: failed` and `Pi worker exited with code 1 before reviewed completion`. Teardown reported no process whose command line was owned by that graph. This proves the missing-login error direction only; it is credential-free evidence, not a successful provider test.

### 4. Request useful work from a PTY and watch it in the TUI

The real PTY received these terminal actions:

```sh
wg add 'Make greeting useful' --id make-greeting-useful -d '...'
wg publish make-greeting-useful --only
wg service start --max-agents 1 --no-coordinator-agent --no-supervise
```

The request was:

```text
Update hello.sh to accept an optional name and print Hello, NAME!; default to
World. Add a README Usage section with both examples. Keep it POSIX-friendly
and add a focused shell test. Do not publish remotely.

## Validation
- ./hello.sh prints Hello, World!
- ./hello.sh Ada prints Hello, Ada!
- the focused shell test passes
- README documents both invocations
```

A real `tmux` PTY (`180x44`) ran `wg tui`. It showed:

```text
make-greeting-useful  (in-progress ...)
```

Actions driven through the PTY:

1. Launch TUI and observe `open` → `in-progress`, assigned to `agent-1`.
2. Send `Ctrl-C`. The TUI did not exit; it owns that key.
3. Interrupt the terminal with `tmux kill-session`.
4. Confirm the TUI process was gone while the disposable service/worker remained owned.
5. Restart the same TUI command in a fresh PTY.
6. Observe the persisted row: `make-greeting-useful (in-progress ...)`.
7. Later open the row with `Home`, `Enter`, `End`, and `PageDown` to inspect waiting and done states.

The task request, graph state, worker worktree, commits, review evidence, and messages survived both the TUI interruption and the later service stop/restart.

## Real execution, failure, repair, review, and landing

This section is real Pi/model/auth evidence. The worker and every semantic lane used `pi:openai-codex:gpt-5.6-sol` through Pi 0.84.4.

### First attempt: useful implementation, then evidence rejection

The worker changed the requested files, added a focused test, ran it, and committed locally:

```text
46a15de feat: add personalized greeting
60218df test: gate greeting CLI behavior
```

Observed local commands passed:

```sh
./hello.sh                 # Hello, World!
./hello.sh Ada             # Hello, Ada!
./tests/test_hello.sh      # hello.sh tests passed
sh -n hello.sh tests/test_hello.sh
git diff --check refs/heads/main..HEAD
```

The completion contract initially recorded only the built-in `git diff --check`. Two real FLIP reviews rejected the candidate—not because the code was wrong, but because worker log prose was not immutable runtime evidence:

```text
.flip-.../c1/r1  Semantic(Reject)  cost=$0.046660
.flip-.../c2/r1  Semantic(Reject)  cost=$0.052410
flip.validation: ... only validation evidence is `git diff --check`, which
cannot establish ... the focused test passes.
```

The same worker retained the same worktree/fence and attempted a bounded repair. It then gave a clear help explanation, but the installed CLI could not accept its semantic-rejection contract-correction intent, so it terminated `failed` rather than holding `NeedsAttention`:

```text
Operator action needed: add './tests/test_hello.sh' as an exact validation
command, then retry the retained commits. No remote push was performed.
```

First dispatch to this terminal state: 529 s (17:43:09Z–17:51:58Z).

### Stop/restart and successful reviewed completion

The disposable service was stopped. `wg service status` said `Service: not running`; the exact candidate worktree still existed at `60218df`, was clean, and its test still passed. No process owned by the project graph remained.

The operator then applied the requested, directly relevant correction and retried in place:

```sh
wg retry make-greeting-useful \
  --reason 'Approve focused runtime check ... reuse retained candidate'
wg contract make-greeting-useful \
  --add-validation-command './tests/test_hello.sh'
wg service start --max-agents 1 --no-coordinator-agent --no-supervise
```

`retry` explicitly reported that the next attempt would resume the existing worktree. After restart, `agent-2` reused the commits; it did not reimplement the feature. Exact host-bound checks were captured **before** semantic review:

1. `./tests/test_hello.sh` — exit 0, evidence `b3:f6fbcada...`
2. `git diff --check refs/heads/main..HEAD` — exit 0, evidence `b3:1be4731e...`

No future self-receipt was requested or treated as pre-review proof. The subsequent real review passed:

```text
.flip-make-greeting-useful@g1/.../c3/r1      Semantic(Pass) cost=$0.058945
.evaluate-make-greeting-useful@g1/.../c3/r1  Semantic(Pass) cost=$0.040400
manifest b3:9a1b9bcb... review outcome: Accepted
```

Restart to accepted review took 72 s (17:53:35Z–17:54:47Z).

### LandingPending and local publication

Landing then correctly failed closed because first-run scaffolding was still untracked in the integration checkout:

```text
Completion waiting/LandingPending: attached integration checkout has tracked,
index, or user-owned untracked changes; publication deferred without modifying
user bytes. Candidate and receipts preserved; source worker released.
Next: preserve user changes, clean the attached integration checkout, then run
`wg resume make-greeting-useful --only`.
```

The TUI showed the task as `waiting`, but its visible detail pane did not show that exact `wg resume` action; CLI `wg show` did. The generated `.gitignore`, `AGENTS.md`, `CLAUDE.md`, and `worksgood.toml` were checked for secret content, committed to the disposable project as `807ff2c`, and the checkout became clean. Then:

```sh
wg resume make-greeting-useful --only
```

returned:

```text
Landed 'make-greeting-useful' at 080a581946d017762e742f3d88bf03b305719080
Done ... advisory review evidence bound manifest b3:9a1b9bcb... and publication is verified
Resumed pending landing ... without rerunning source or review
```

The 16-minute accepted-to-resume interval was manual rehearsal/polling delay, not model or landing latency. Reconciliation and landing after `resume` took under one second. The TUI then showed `done` and `Some(Landed)`.

## Inspection of the landed change

The integration branch ended at local commit `080a581`. It contained:

```text
README.md                             | 24 +++++++++++++++++++++++-
hello.sh                              |  6 ++++--
tests/smoke/manifest.toml             |  8 ++++++++
tests/smoke/scenarios/greeting_cli.sh |  5 +++++
tests/test_hello.sh                   | 20 ++++++++++++++++++++
```

Post-landing execution from `project-a/main` passed:

```text
$ ./hello.sh
Hello, World!
$ ./hello.sh Ada
Hello, Ada!
$ ./tests/test_hello.sh
hello.sh tests passed
```

`git remote -v` produced no output. Therefore:

- **Tests:** focused shell behavior and diff checks passed locally.
- **Semantic review:** real FLIP and completion evaluation passed against exact evidence on the successful candidate.
- **Local landing:** verified at `080a581...` in the disposable repository.
- **Remote publication:** did not occur and is not implied by “landed”.

The extra `tests/smoke/...` files were worker overreach for this tiny non-WG project; see defects.

## Usage and elapsed time

Pi-reported/accounting surfaces were available, but retry aggregation is inconsistent:

- First failed attempt, `wg show`: output about 11k, cache about 1.4M, `$1.35` (novel input displayed as 0 by WG's cache-subtraction convention).
- Successful retry, `wg show`: output 1,082, cache about 114k, `$0.23`.
- Final `wg spend`: source task `$0.2254`, 28,226 tokens (27,144 input, 1,082 output).
- Separate completion-review lane: `$0.1984`, 31,113 tokens; three FLIP attempts `$0.1580`, one completion eval `$0.0404`.

`wg spend` did not include the earlier `$1.35` task-attempt figure after retry, so no synthetic “total” is claimed.

Timing:

| Phase | Elapsed |
|---|---:|
| Candidate install | 219 s |
| Setup preflight | <1 s |
| Missing-auth failure | <2 s |
| First real dispatch → bounded terminal failure | 529 s |
| Service restart/retry → accepted review | 72 s |
| Manual accepted-review hold before cleanup/resume | ~975 s |
| `resume` reconciliation, renewed checks, and local landing | <1 s |
| Credential-free completion-repair smoke | 115 s |
| Process-ownership smoke under subreaper harness | 70 s |

## Controlled completion-loop and process evidence

The candidate binary passed `tests/smoke/scenarios/completion_repair_loop.sh` under the Rust smoke/subreaper harness (115 s). That human-flow scenario remains owned by its prerequisite implementation task, `simplify-completion-repair-loop`; this report does not expand the checked-in ownership manifest merely to attach a second owner. This credential-free scenario exercises the required queued cases through real CLI/TUI entry points:

- a required-check failure returns to the same worker/attempt/fence;
- repeating the unchanged failure becomes `NeedsAttention` without rerunning validation;
- `--intent request-help` preserves work and exposes one bounded contract action;
- three distinct failures exhaust the two-opportunity budget;
- `wg status` shows one root blocker with both affected downstream tasks;
- a real tmux-driven TUI inspector shows `ROOT BLOCKER` and `one safe action`.

This is controlled-fixture evidence only; it does not prove Pi login or model behavior.

`smoke_process_ownership_cleanup.sh` initially refused direct execution, correctly requiring the Rust Linux subreaper harness. Rerun under that harness passed in 70 s. Final candidate teardown additionally reported both disposable services stopped, no owned TUI session, and no `/proc` command line tied to either graph.

Two other relevant checked-in scenarios initially failed and were repaired within the rehearsal's bounded onboarding-test scope:

1. `setup_route_activation_preflight` still expected `~/.wg/active-profile`, which the current project-local design intentionally no longer writes. It also invoked some `--dir` commands from the source CWD, reproducing the route-target mismatch. The scenario now asserts `worksgood.toml`, unchanged global state, hermetic JIT messaging, and runs config/service commands from the disposable project.
2. `explicit_execution_selection` reached its final selected-route service fixture without the `--no-supervise` isolation used by current service smokes. The fixture now supplies it.

Both were rerun under the Rust subreaper harness with the supported short `WG_SMOKE_ROOT=/tmp/wgsmoke-a93` (the worker's inherited deep `TMPDIR` exceeded Linux `sockaddr_un.sun_path`). Final results: `setup_route_activation_preflight` **PASS** in 0.72 s and `explicit_execution_selection` **PASS** in 0.91 s. The initial failures remain reported here rather than retroactively called passes.

## Isolation and teardown

At completion:

- `project-a/worksgood.toml`: present and committed, exact approved route only.
- `project-b/worksgood.toml`: absent. Its only untracked files were the expected files produced by the deliberate `wg init`; project A setup/execution did not create its route.
- Isolated `wg-global/config.toml`: absent.
- Source task worktree: clean after restoring one accidental route-targeting probe described below.
- Disposable service status: `Service: not running`.
- Owned `/proc` scan: `none`.
- No Git remote and no push.

## Manual interventions

1. Override all weak/review roles from the setup-projected OpenRouter model to the one approved Pi route.
2. Restore the source worktree's `worksgood.toml` after discovering that `wg setup --dir OTHER/.wg` still targets configuration by current working directory; the probe was rerun from the correct disposable project directory.
3. Stop/restart the disposable service and TUI.
4. Add the focused test as an authoritative completion check after two truthful semantic evidence rejections.
5. Commit generated first-run scaffolding so the integration checkout was clean.
6. Run `wg resume ... --only` to finalize the preserved accepted candidate.
7. Rerun the process test through the required subreaper harness after its direct-execution refusal.
8. Repair and rerun two stale project-local setup/service smoke fixtures; use the harness's supported short smoke root to avoid the Linux Unix-socket path limit.

## Initial-run onboarding defects (historical)

1. **Exact setup model does not mean one usable auth route.** `wg setup --model pi:openai-codex:...` still projected weak/reviewer roles onto DeepSeek/OpenRouter. A new user with only the selected Pi provider login can execute the worker but fail review/agency. Setup should either keep all first-run roles on the exact selected route or explicitly obtain approval for a second provider.
2. **`--dir` and project-config target disagree.** `wg --dir /tmp/project/.wg setup ...` wrote `worksgood.toml` in the process CWD, not beside `--dir`. The command should resolve one project target or refuse this mismatch loudly.
3. **First run creates a dirty integration checkout.** `wg init`/`setup` create `.gitignore`, guides, and `worksgood.toml` without a prominent “commit these before dispatch” step. The first accepted change therefore parks at `LandingPending`.
4. **Semantic evidence repair falls out of the bounded help loop.** The worker correctly diagnosed a missing exact check, but the available correction intent could not hold a semantic rejection in `NeedsAttention`; it terminally failed, requiring operator retry before contract update.
5. **TUI omits the useful landing action.** The waiting detail showed watchdog text and lifecycle state but not the CLI's exact `wg resume <task> --only` instruction.
6. **Generated agent guidance over-scopes ordinary projects.** For a tiny greeting script, the worker created WG-style `tests/smoke/manifest.toml` and an owned smoke scenario. Guidance should distinguish work on WG itself from a user's application.
7. **Retry usage is not episode-cumulative.** Final `wg spend` retained the successful retry and all review-lane costs but omitted the earlier `$1.35` source-attempt figure shown before retry.
8. **Setup smoke coverage was stale (fixed here).** One scenario required the removed global active-profile write and ran `--dir` operations from the wrong CWD; another selected-route service fixture lacked current isolation flags. The focused scenario-only repairs now pass.
9. **Deep worker temp roots can exceed the Unix-socket limit.** The smoke harness inherited this worker's long `TMPDIR`, making an otherwise-correct service fixture fail `sockaddr_un.sun_path`. The supported short `WG_SMOKE_ROOT` was required; harness defaults should stay socket-safe automatically.
10. **`Ctrl-C` is not an obvious TUI exit.** It was consumed; interrupting required killing the PTY session. The TUI should show its exit key or make the behavior explicit.
11. **Rehearsal evidence and completion evidence are disconnected.** The first completion review saw only the built-in `git diff --check` receipt, not the already executed real-model/PTTY and credential-free smoke evidence documented here. Adding this rehearsal as another smoke owner would be out of documentation scope and, because semantic review runs before that new ownership can land, would not supply pre-review evidence anyway. A report task needs a supported way to bind redacted, already-run human-flow evidence without inventing a future self-receipt.

These are recorded as follow-up work rather than expanded into architecture changes during this rehearsal:

- `fix-first-user-exact-route`
- `fix-first-user-landing-ux`
- `fix-semantic-completion-help`
- `fix-retry-episode-usage`
