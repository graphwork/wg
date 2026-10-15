# Survey synthesis — ONE ranked subtraction plan

**Task:** `survey-synthesis-produce` · **Status:** operator-facing decision document.
**Inputs read in full:** `.wg/survey/survey-build-size.md`, `survey-config-agency.md`,
`survey-executors.md`, `survey-federation.md`, `survey-tests-docs.md`,
`survey-tui-plugin.md`, `survey-cow-worktrees.md`; `docs/research/flip-eval-evolution.md`
(the `survey-how-flip` output); and the derived reports
`docs/survey-dependency-hardlink-baseline.md`, `docs/design-cow-worktrees.md`,
`docs/build-artifact-storage.md`.
**Covered-task check:** every item that already has a WG task was checked with `wg show`
(statuses as of this writing, §2 and the table's `Covered?` column).

> **Relation to the earlier synthesis.** `.wg/survey/reduction-plan.md` (task
> `survey-synthesis`, **done**) is an uncommitted working note in the shared graph dir.
> This file is the **committed** deliverable and supersedes it: it adds the
> operator-facing ranked table, the covered-task map (post-`wg show`), the
> top-3 dispatch packet, and the explicit non-goals/limits. Where they differ, the
> correction recorded here wins (notably §6 item 1, the dormant-baseline correction).

**Ranking rule:** (scope removed × confidence it is dead) ÷ (risk × reversibility cost).
"Attention" = always-loaded context or human confusion, counted separately from LOC.
**This is a subtraction plan. No fail-closed gate, provenance rule, or completion-review
guarantee is proposed for weakening (see §5).**

---

## 0. Verdict in one paragraph

The repo is measurably over-built for its real daily use. Every survey independently
found the same daily plane: **one `pi` worker handler, the plugin as the rising in-session
UI, and the TUI as the still-live console** — with the federation/review/exec/pilot spark
stack, the native worker path, the HTML renderer, the agency performance loop, and two
ungated test layers essentially never exercised by the operator. The three biggest
**safe** removals are (1) reclaiming ~53–66 GiB of dead disk/state, (2) the 16-role→2-tier
config collapse and the never-dispatched executor surface, and (3) feature-gating the
federation cluster so ~29k LOC and several heavy deps leave the default build without
deleting the option. The two biggest **gated** removals (the TUI chat/PTY pane and the
whole native `nex` stack) are product decisions blocked on open parity work and are
**frozen, not scheduled**.

---

## 1. The ranked subtraction table

`Action` ∈ **delete** (remove code/state), **retire** (delete a user surface after a
one-release deprecation), **demote** (keep the mechanism, remove the general interface),
**freeze** (do not grow; no removal yet), **finish** (deployment/rollout of existing code).
`Rev.` = reversibility. `Conf.` = confidence the thing is dead/unneeded.

| # | Candidate | Action | Evidence (survey + file/symbol; cost) | Expected saving | Risk if wrong | Rev. | Prereq | Conf. | Covered? |
|---|---|---|---|---|---|---|---|---|---|
| S1 | Dead disk / state mass | delete | `survey-build-size` §1.2,2b,2d,2e; `survey-cow-worktrees` §2.2 — `/home/bot/wg-quarantine/20260805T184151Z` 32 GiB (`DO_NOT_RUN`), `.wg/build-tmp/manual-service-merge` 8.8 GiB unkeyed/Sep 4, `wg.1` 112 worktrees ~13 GiB, `target/debug/incremental` 5 GiB, 380 MB stale `raw_stream.jsonl` (58 files >7 d uncompressed despite `stream_retention_days=7`) | **~53–66 GiB disk** now | Low (regenerable; archive ~5 MB metadata first) | High | Operator authority for deletes | High | `fix-disk-cleanup`, `fix-decay-the` (both **done**) fix the *recurrence*, not the bytes |
| S2 | Shared sccache build cache | **finish** | `survey-cow-worktrees` §1.2 (`command -v sccache` → none; no `.cargo/config.toml`); code already landed by `implement-shared-sccache` (**done**) at `src/commands/spawn/execution.rs:5566 configure_sccache_env`, resolver `:5512 resolve_sccache_binary` returns `None` when not on PATH, `sccache_enabled` + 20G budget `src/config.rs:5258` | Warm-dependency **compile time** (build latency); **does not shrink peak disk** | Low | High | Install `sccache` on PATH (operator) | High (code) / Med (time win unmeasured) | `implement-shared-sccache` **done** |
| S3 | Dependency-only hardlink baseline (Cow Option B) | **finish** (apply behind flag) | `docs/survey-dependency-hardlink-baseline.md` §0,§3 measured isolated instance: plain build **≈50%** per attempt; heavy `cargo test --no-run` **≈6%** (24.2 GB → ≈22.8 GB) because `deps/` is 18.88 GiB of 200 *workspace* test/bin executables, only 1.40 GiB registry deps; `src/target_cache.rs` + `src/disk_sentinel.rs` | Inner-loop **≈50%** per attempt; heavy **≈6%** | Med — hardlinks share inodes, a `chmod u+w` on a layer mutates the baseline (report §4.1 avenue 2) | High (config flag default `false`) | Manifest `(path,size,mtime,inode,mode)` verify; `layer_bytes` external-link charging; ideally distinct worker UID | Med | `implement-the-dependency` **done** (design+proof) |
| S4 | Dead TUI test/code blocks | delete | `survey-tui-plugin` §3.1 — `editor_tests.rs` 1,447 (always-false `cfg(all(test, any()))`), four dead `state.rs` modules ~1,290, `model_cmd.rs:213`, `file_browser_render.rs` 222 (no callers), `graph-widget.ts` 38 no-op | **≈3,000 LOC** | None (they never compile) | High | None | High | `cleanup-delete-always` **(PAUSED)** |
| S5 | Never-dispatched executor surface | delete | `survey-executors` C1/C3/C5/C6,§1.2 — worker-only arms `Aider/Goose/Qwen/Cline/Crush/Amplifier` + `Octomind/Dexto` (~30–60 LOC + ~200 test LOC, **no handler module**), `wg native-exec` 247 + native worker arms (~300–600), non-Pi `SetupRoute` builders (~200–400), `NativeExecutorConfig`/`NativeWebConfig`/etc. (~250–500); strict admission already rejects them (`parse_supported_execution_route`) | **≈0.8–1.7k LOC** + removal of confusing pickers/docs | Low (no compat const) | High | Do **not** touch `src/executor/native/` or `nex` | High | No |
| S6 | `rquest`+BoringSSL and `chromiumoxide` | delete (feature-gate) | `survey-build-size` §1.1, R5 — non-optional but used only by `src/executor/native/tools/{web_search,web_fetch,research}.rs`; `boring-sys2` six build dirs @86 MB is the heaviest compile item; `chromiumoxide*` ≈270 MB rlibs | **Minutes per cold build**, ~300–500 MB `target/` | Low functionally | High (feature off by default) | Keep native web tools behind `native-web` feature | Med (build time not measured) | No |
| S7 | Federation cluster (WG-Fed core) | retire (feature-gate) | `survey-federation` §0,§1,§4 P1 — "Nothing in the operator's daily workflow … reaches it"; graph string search `wg identity`/`wg review`/`wg fed-node`/`wgid:`/`wg peer`/`--review` all **0**; ~27,300 src + 1,218 adapter + ~730 CLI LOC; drops `ed25519-dalek`, `x25519-dalek`, `chacha20poly1305`, `hkdf`, `bs58`; `--help` −46 arms; 57% of `AGENTS.md` becomes opt-in | **~29k LOC** + deps + per-session attention | Low *if gated* | High (cargo feature) | **Extract** `canonical_json`/`content_cid`/`blake3_32`/`signing_digest` first (daily completion-evidence path) | High | `gate-federation-cluster` + `extract-content-addressing-helpers` **(both PAUSED)** |
| S8 | WG-Exec providers plane / `RemoteRunner` | delete | `survey-executors` C2,§6; `survey-federation` P3 — 0 tasks carry `remote_provider`; `spawn_task.rs:339-347` hard-errors; `src/providers/` 3,957 + `exec_fed_cmd.rs` 1,631 | **~5.9k LOC** + ~1k test/smoke LOC | Med — reuses `identity` UCANs; family-team/pilot depend on exec legs | High | Descope `e2e_family_team`/`pilot` together | High | Subsumed by `gate-federation-cluster` |
| S9 | WG-Review content-safety gate | delete | `survey-federation` P4 — opt-in `--review` never appears in the graph; only non-fed consumer `merge_resolution` is manual-only; `src/review/` 4,140 + `review_cmd.rs` 532 | **~4.8k LOC** + 5 smokes | Med — `merge-resolution` screening | High | Keep `review::detect` if merge screening stays; **do not** confuse with `src/completion_review.rs` | High | Subsumed by `gate-federation-cluster` |
| S10 | `wg pilot` | retire | `survey-federation` P2 §2 — "purely demo"; only `pilot_dry_run.sh`; `pilot_cmd.rs` 1,459 + `pilot.example.toml`; ships no substrate | **~1,459 LOC** + runbook §6 | Low | High | Archive smoke (manifest grow-only) | High | `retire-wg-pilot` **(PAUSED)** |
| S11 | Agency composition + federation CLI + legacy scored eval | retire | `survey-config-agency` C1/C4/C7,§2.3-2.4 (138 receipts all `"kind":"uncomposed"`, 0 outcome assessments/rewards/evolution runs, 1 lifetime evaluation); `audit-agency-federation-reachability` §0 verdict **RETIRE**; `src/agency/` 13,205, agency CLI 5,499, evaluation lane 5,546; `docs/research/flip-eval-evolution.md` §5 confirms FLIP-narrowing already de-duplicated FLIP/Eval | **~25–45k LOC** staged; ~8 MiB store (clarity, not disk) | Med–High on the `evaluation/` lane leg — it **is** wired into `done.rs`/`finalize.rs` | Med | Keep the observation ledger (828 candidate-ledger + 75 terminal episodes live); lane leg needs a completion-lifecycle ratification | High (fed CLI) / Med (lane) | `retire-agency-composition` **(PAUSED)** |
| S12 | 16-role model plane → strong/weak | demote | `survey-config-agency` C2,§2.1 — `worksgood.toml` has 16 `[models.*]` sections, every one `pi:lunaroute/deepseek-4.1-flash-background` differing only in `reasoning`; `setup_two_tier.rs:20-31` already presents 2 buckets; `DispatchRole` 15 roles + `Default` at `config.rs:1664` | **≈1.5–2.5k LOC** + a whole flag family | Med — per-role plane is a live *reasoning* table | Med (migratable) | `wg migrate config` must fold `[models.<role>].reasoning` into tier reasoning (strong→high/xhigh, weak→low) | High | No |
| S13 | 59 legacy `wg config` root flags | delete | `survey-config-agency` C5,§1.2 — 59 flags for 4 subcommands, many duplicating `set <key> <value>` | **≈1–2k LOC** + ~50 stale help lines | Low | Med (one-release warn) | Keep `init/set/get/lint` + `-m/--reasoning` | Med | Partly: `cut-config-deprecated-keys` **(done)** covers the 7 config keys only |
| S14 | Smoke corpus: spark + non-pi (~155) | retire (archive) | `survey-tests-docs` C1/C6,§2.2-2.3 — 376 scenarios / 62,778 lines; ownership: 462 owners, **395 not in the graph**; full run ≈10.8 h worst case; 13 spark + 142 non-pi-executor scenarios | **~26k shell LOC** + ~40% of full-smoke wall-clock | Low | High (archive) | Manifest `retired=true`; fold bare-alias routing invariants into unit tests | High | `archive-retired-smokes` **(PAUSED)** |
| S15 | 182 un-gated integration targets | triage, then delete the dead | `survey-tests-docs` C2,§2.4,§6 — `ci.yml` gates only 8 of 190 `tests/*.rs` (+`integration_service`); 119,861 LOC mostly never compiled/run | Up to **~100k LOC** + CI compile time (measure before culling) | Med — some hold the only daemon/IPC coverage | Med | Triage keep-in-CI / fold / retire **first** | Med | `triage-the-182-un` **(PAUSED)** |
| S16 | HTML renderer | retire | `survey-tui-plugin` C2,§2 — `stats.json` `html`: **1 lifetime call**; `src/html.rs` 3,375 + `html_assets/` 108 KB + 10 `wg_html_*` smokes; last real touch 2026-07-26 | **≈3,400 LOC** + 108 KB + 10 scenarios | Med — visible verb; `--format dot` feeds publish | Med (one-release deprecation) | Confirm `wg publish` rsync/cron has non-HTML use; operator sign-off | High (usage) | `retire-html-renderer` **(PAUSED)** |
| S17 | Divergent TUI status→colour mapping | demote (collapse) | `survey-tui-plugin` C3,§3.4 — `state.rs::flash_color_for_status` (canonical RGB) vs a second mapping at `render.rs:11042`; already caused `fleet-graph-view-2` | Small LOC; kills a whole parity-bug class | Low | High | Palette unit test | High | `collapse-tui-palette` **(PAUSED)** |
| S18 | Legacy right-panel tabs + `Files` file browser | freeze (audit first) | `survey-tui-plugin` C5,§3.1 — `Files`/`Firehose`/`Output` excluded from `RightPanelTab::ALL`, kept only for match arms | Low hundreds (761 if `file_browser` proven dead) | Med — `WorkspaceContext` still maps to `Firehose`; `Files` has live paths | Med | Runtime reachability audit | Low | No |
| S19 | Typst manual ground-truth fiction | retire (wire or archive) | `survey-tests-docs` C5,§3 — `docs/manual/*.typ` claims ground truth; `typst` wired into **no** CI/Makefile target; the derived `.md` is what is read | ~6 `.typ` files + drift | Low | High | CI "rebuild → `git diff --exit-code`" (mirrors embed-staleness) or archive | Med | No |
| S20 | AGENTS.md/CLAUDE.md spark narrative | retire (relocate) | `survey-tests-docs` C3,§1.4 — lines 343–764 (57% of the 764-line, 54 KB file) are WG-Fed/Review/Exec/e2e/Pilot spark narratives; loaded into **every** worker prompt | **~30 KB per session** | Low–Med (both files must change in lock-step) | High | Move verbatim to `docs/`, leave a stub index | Med | `doc-archive-policy` **(PAUSED)** |
| S21 | Stale docs mass (619 md; 91 orphans) | retire (archive) | `survey-tests-docs` C4,§1.3 — `docs/{research,reports,audit}` historical + 53 root-level reports; `KEY_DOCS.md` stale since 2026-04-29; docs churn 224 commits/60 d | ~10 MB / ~400 files out of the live surface (grep noise) | Low | High | Grep source-referenced ADRs before `git mv` | High | `doc-archive-policy` **(PAUSED)** |
| S22 | TUI chat/PTY pane | **freeze** | `survey-tui-plugin` C6,§6 — `pty_pane.rs` 5,055 + `chat_startup.rs` + `chat_tab_state.rs` + parts of `chat_cmd.rs` ≈8,900 LOC; plugin has **zero** chat surface; `full-mouse-parity` and `fleet-graph-view-2` **in-progress, not done** | (none now) | High if cut now — breaks the daily loop | n/a | A pi-session chat front-end + landed parity tasks | n/a | No (correctly blocked) |
| S23 | Native stack + `nex` binary | **freeze** (product decision) | `survey-executors` C4,§1.2 — `src/executor/native/` 37,419 + tools 13,351, ~17.8k native/nex test LOC, `nex` binary, MCP/resume/journal; `README.md:93` advertises 3 binaries; 35 nex/native smokes | ~50k LOC + one binary if retired | High / near-irreversible | Low if deleted | **Only** the operator can decide if `nex` is still a product | n/a | No |

---

## 2. Ordered execution plan (≤10 items)

Ordered by (safety × payoff), never breaking the daily loop (pi dispatch + plugin + TUI).
Items marked **[covered]** already have a WG task (status from `wg show`); do not re-file.

1. **Reclaim dead disk/state (S1)** — archive `DO_NOT_RUN.txt`, `git-worktrees.txt`,
   `repository-head.txt`, `graph.jsonl` (~5 MB), then delete the quarantine tree, the
   unkeyed `build-tmp/manual-service-merge`, `wg.1`'s 112 worktrees, `target/debug/incremental`,
   and run the (already-fixed) retention pass. **Operator-only deletes.** Zero code risk,
   ~53–66 GiB. Why first: recovery bytes are free and reduce admission pressure.
   **[covered]** `fix-disk-cleanup`, `fix-decay-the` (both **done** — code side).
2. **Deploy the shared sccache cache (S2)** — install `sccache` on PATH so the landed
   `configure_sccache_env` (`execution.rs:5566`) stops being a no-op. Why second: it makes
   every later rebuild/cull cheap. **[covered]** `implement-shared-sccache` (**done**);
   this is the operator install step the survey found missing.
3. **Delete always-dead code + fix doc drift (S4, and the drift half of the executor
   surface)** — unpause/publish `cleanup-delete-always` and record
   `survey-followup-executor-docs` (**done**). Zero behavioural risk. **[covered]**
   `cleanup-delete-always` **(PAUSED)**, `survey-followup-executor-docs` (**done**).
4. **Extract the content-addressing helpers, then feature-gate the federation cluster
   (S7–S10)** — `extract-content-addressing-helpers` → `gate-federation-cluster`; fold
   exec/review/pilot into the same gate. Why here: the single biggest safe LOC cut, fully
   reversible, no daily surface removed. **[covered]** both **PAUSED**;
   `retire-wg-pilot` **(PAUSED)** can ride the same wave.
5. **Archive the ceremonial smoke corpus (S14)** — unpause `archive-retired-smokes`;
   add manifest `retired=true`; move (do **not** delete) scripts. Why before integration
   triage: it is unambiguously low-risk and shrinks the suite the triage lane must reason
   about. **[covered]** `archive-retired-smokes` **(PAUSED)**.
6. **Triage the 182 un-gated integration targets (S15)** — unpause `triage-the-182-un`;
   wire keepers into CI, retire the dead. Why after smoke: measure before culling.
   **[covered]** `triage-the-182-un` **(PAUSED)**.
7. **Retire the agency performance/evolution half (S11)** — unpause
   `retire-agency-composition` after the reachability verdict (**done**); keep the
   observation ledger; defer only the `evaluation/{bounded,deep,rollout}.rs` lane leg to a
   completion-lifecycle ratification. Why here: high LOC, but the eval-lane leg is the one
   medium-high-risk cut. **[covered]** `retire-agency-composition` **(PAUSED)**.
8. **Collapse the config plane (S12, S13) + the dead executor surface (S5)** — new work:
   fold the 16 roles into strong/weak (preserving reasoning), prune the 59 legacy root
   flags, and delete the never-dispatched executor arms/config/setup routes + native worker
   path. Why: the largest *attention* reduction with no product decision. **[covered]**
   `cut-config-deprecated-keys` (**done**, keys only). Everything else is new.
9. **TUI parity cleanups + the operator-sign-off retire (S16, S17, S19, S20, S21)** —
   `collapse-tui-palette` (PAUSED), `retire-html-renderer` (PAUSED, needs sign-off),
   `doc-archive-policy` (PAUSED). Why last of the code waves: cosmetic/attention wins that
   do not touch the worker path.
10. **Apply Cow Option B behind a flag (S3)** — only after the safety work (manifest
    digest verify + `layer_bytes` external-link charging); enable for `build_capable`
    first (≈50%), measure `build_heavy` before enabling there (≈6%). **Freeze S22/S23**:
    do not touch TUI chat or `nex` until a pi chat front-end exists / the operator says
    `nex` is not a product.

---

## 3. Do-not-delete

Surfaces the surveys explicitly say are load-bearing; **subtraction only, never gate
relaxation.**

- **Fail-closed completion machinery — do NOT weaken.** `src/completion_review.rs` /
  `completion_review_model.rs` / `wg reviews`: semantic-vs-infrastructure separation,
  immutable binding/fences, manifest + requirements digest pinning, the typed
  `completion.missing_authoritative_runtime_evidence` marker with its exact/unmixed
  checks, fail-closed recoverability (`NeedsAttention`, no acceptance path)
  (`docs/research/flip-eval-evolution.md` §4). A "cut the review stack" proposal must
  name `src/review/` **only**; `src/completion_review*` is the daily flip/eval valve
  (`auto_evaluate=true`).
- **The completion-review observation ledger** — 828 candidate-ledger events + 75
  terminal episodes are live (`survey-config-agency` §2.3); `adaptive_agency.rs` is
  deliberately a WorkGraph-free observation package (a security boundary).
- **The worker runtime** — `src/commands/spawn/execution.rs` (9.2k) +
  `raw_stream_classifier.rs` + `pi_stream_bridge` + `pi_handler.rs` + `terminal_host/`
  + the plugin embed/cache path (`survey-executors` §3, "the daily worker runtime").
- **The single routing authority** — `src/dispatch/handler_for_model.rs` + `plan.rs`;
  simplify by deleting dead arms (S5), never the module. Keep **`Placement::Local`
  byte-for-byte** when removing `RemoteRunner`.
- **The single shared renderer + read model** — `src/commands/viz/ascii.rs` (5,670;
  feeds CLI `viz` 7,284 calls, daemon `GetFleet`, TUI, and plugin `/wg-fleet`) and
  `GetFleet` + `service/fleet_snapshot.rs` (448). This is the "WG exports, plugin renders"
  architecture already done.
- **The TUI coordination panes and its harness** — Agency/Dashboard/Config/Settings/
  Service-log/Messages/task forms/chat PTY (~107,500 LOC cluster, `survey-tui-plugin`
  §1.1) are the operator's **only** surface for those; keep `wg tui-dump` /
  `screen_dump.rs` (the harness behind 71 TUI smokes) and `snapshot_engine.rs` +
  the first-frame latency contract.
- **Config core and the project-local cutover** — `Config` load/merge + route
  resolution + `load_merged`; `worksgood.toml` as sole project authority;
  `profile select` projection writer; `wg setup` + `setup_pi_gates.rs` +
  `setup_two_tier.rs` (the target shape for S12, not a cut).
- **Test coverage that is genuinely load-bearing** — `cargo test --lib` +
  `--bin worksgood` (the only suite CI runs), the `integration_service` daemon/IPC
  target, `formal/` Lean proofs, `tests/install` + `tests/upgrade`, the ~260 TUI/PTY
  smokes, and the `smoke-gate-is` meta-scenarios. Keep `--full-smoke` opt-in after the
  archive.
- **The grow-only smoke manifest discipline** — do **not** delete entries; add
  `retired=true`/archive and keep `--full-smoke` able to re-enable.
- **`disk_sentinel.rs`** (4,016) — the guard that prevented the 2026-07-22 link-time
  disk exhaustion; trim constants (already `fix-decay-the`, done), never the mechanism.
- **The four content-addressing helpers** (`identity::{canonical_json, content_cid,
  blake3_32, signing_digest}`, ~50 LOC) — used by the daily completion-evidence path;
  **extract before gating `src/identity/`** (S7 prereq).
- **`wg msg` local send/list/read** and **`wg claude-handler`/`wg codex-handler`** — used
  daily / cheap and still selectable; only the cross-graph `--to wgid:` arm is federation.
- **The operational doc core** — README, `docs/ops/runbook.md` (minus pilot §6),
  `docs/guides/*`, `docs/manual/`, `docs/AGENT-*.md`, `docs/COMMANDS.md`,
  `docs/config-precedence.md`, AGENTS.md lines 1–342.

---

## 4. Highest-value 3 items — ready to dispatch

These are **new** (no existing task), ready, and high-value without an operator product
call. The two larger LOC levers (`gate-federation-cluster`, `retire-agency-composition`)
are already filed and PAUSED — unpause rather than re-file.

### 1. `collapse-config-to-two-tiers`
**Title:** *Collapse the 16-role model plane into the strong/weak two-tier plane*
**Scope:** Make `[tiers] strong/weak` the only supported model-routing surface and demote
per-role `[models.<role>].model` to an override-only escape hatch. Remove
`[models.<role>].tier`, the `coordinator_eval` pseudo-role (`config.rs:1664`/`:2273`), and
the legacy provider/endpoint cascades in `resolve_model_for_role` (`config.rs:4039`). The
migration must preserve per-role reasoning intent by folding each `[models.<role>].reasoning`
into the tier reasoning keys (strong→high/xhigh, weak→low) — the plane is a *reasoning*
table today, not a routing table (`survey-config-agency` §2.1: all 16 roles resolve to one
model). Wire `wg migrate config` to do the fold; make `wg config lint` flag residual
`[models.*]`; keep `--set-model <role>` working for one release. Update the pinned smokes
(`flip_role_model_routing.rs`, `integration_profile_tier_pinning.rs`,
`integration_pi_two_tier_profile.rs`). Validation: a migrate round-trip test that a
16-section doc becomes two tiers with equivalent resolved reasoning per role; `cargo test
--lib`; the tier-pinning smokes green; fmt/clippy clean.

### 2. `remove-never-dispatched-executor-surface`
**Title:** *Delete the never-dispatched executor surface (dead arms, native worker path,
non-Pi setup routes, native config blocks)*
**Scope:** Delete the `Aider/Goose/Qwen/Cline/Crush/Amplifier` and `Octomind/Dexto`
`ExecutorKind` variants and their error-returning `spawn_task` arms (`spawn_task.rs:323-347`),
discovery strings, `WORKER_ONLY_EXTERNALS`, and taxonomy tests (no handler module exists);
remove `wg native-exec` (`native_exec.rs` 247) and the `native` **worker** arm from the
spawn wrapper and `service/executor.rs`, keeping only a loud pointer to `wg nex`;
remove the non-Pi `SetupRoute::{Openrouter,ClaudeCli,CodexCli,Local,NexCustom}` builders
(`config_defaults.rs`) and the already-failing five-route smoke; drop
`NativeExecutorConfig`/`NativeWebConfig`/`NativeBackgroundConfig`/`NativeDelegateConfig`.
**Do not touch `src/executor/native/` or `wg nex`/standalone `nex`** (`survey-executors`
C3/C4) — that is the frozen product decision (S23). Validation: `cargo build` +
`cargo test --lib`; `wg add`/`wg edit` still reject removed routes with one generic
"unknown executor" error; the three native-routing smokes updated, not deleted blind.

### 3. `feature-gate-native-web-deps`
**Title:** *Feature-gate `rquest`/BoringSSL and `chromiumoxide` behind a non-default `native-web` feature*
**Scope:** Move `rquest` (→ `boring-sys2`, the heaviest compile-time item at six 86 MB
build dirs) and `chromiumoxide`/`chromiumoxide_cdp` (≈270 MB rlibs) behind a
**non-default** `native-web` cargo feature; they are used only by
`src/executor/native/tools/{web_search,web_fetch,research,deep_research}.rs`
(`survey-build-size` §1.1/R5), which are reached by no dispatch route. Pin a
default-features compile check so the default build cannot silently re-add them. This is
safe regardless of the `nex` decision and is the largest single cold-build reduction
available. Validation: `cargo build` default (no `boring-sys2`/`chromiumoxide` in the
dep graph, verified by `cargo tree`), `cargo build --features native-web` still compiles
the web tools, `cargo test --lib` green; report the measured cold-build delta if a build
is available.

---

## 5. Non-goals

1. **No gate relaxation.** This plan removes surfaces; it does not touch strict completion
   review, deterministic-validation authority, provenance/binding/fences, digest pinning,
   fail-closed recoverability, or the no-acceptance-path repair rule.
2. **No whole-TUI retirement.** The TUI is the incumbent console (`wg tui` up ~3 days at
   survey time; 89/837 commits in 90 d touch `src/tui/`). Only cosmetic/gated cuts here.
3. **No `nex` removal in this plan.** It is a shipped, documented, smoke-pinned attended
   client; only the operator decides (S23). The native *worker* path is removed; the
   attended native stack stays frozen.
4. **No deletion of grow-only manifest entries, and no removal of `--full-smoke`.**
   Archive only.
5. **No shared mutable `CARGO_TARGET_DIR`** (cargo serialises on the target lock —
   `survey-cow-worktrees` Option D rejected) and **no true CoW on ext4** (no `FICLONE`).
6. **No weakening of the TUI/plugin/trace/dependency sets** as a shortcut; cuts are
   enumerated, not bulk.
7. **No rewrite, dependency-version bump, or Rust toolchain change.**
8. **No claims about other WG deployments.** All usage evidence is scoped to
   `/home/bot/wg` (`survey-tui-plugin` §7).

---

## 6. Honest limits of the evidence

1. **The dormant-baseline correction (resolved argument).** `survey-build-size` §5 called
   the shared cargo layer cache "why 8 concurrent worktrees don't each materialize a
   15 GiB target"; `survey-cow-worktrees` §2.3 and the isolated proof
   (`docs/survey-dependency-hardlink-baseline.md`) show all **211** layer manifests are
   `baseline_reusable=false` with **0 READY baselines**, and that interactive `pi` workers
   (no attested command → `unknown-isolated:agent-N`) build **cold and private**. This
   document adopts the cow-worktrees account.
2. **Reasoning-not-measurement.** `survey-build-size` ran **no** build, so per-dependency
   wall-clock times (BoringSSL especially) are inferred from artifact sizes. S6/S2 time
   wins are therefore **unmeasured** and labelled as such.
3. **The Cow hardlink claim has a measured floor and a measured ceiling, but the shared
   rollout was NOT applied.** Option B was proven in an isolated instance only
   (`implement-the-dependency`, done): plain build ≈50%, heavy ≈6%. The heavy-class win is
   small because `target/debug/deps` is dominated by the workspace's own ~200 test/bin
   executables (18.88 GiB of a 21 GiB build), **not** dependencies (1.40 GiB) — so it does
   **not** by itself stop heavy admission deferrals; that needs the decay/cull (already
   done) and per-attempt lifecycle bounding.
4. **`implement-shared-sccache` is done in code but dormant in this environment.** The
   wiring exists (`execution.rs:5566`, `config.rs:5258`) yet the survey's `command -v
   sccache` found nothing on PATH and no cache dir exists, so `resolve_sccache_binary`
   (`:5512`) returns `None`. Context (f) is consistent: the mechanism is deployed, the
   binary is not. Treat S2 as a rollout, not a code task.
5. **TUI reachability is asserted, not proven.** `Files`/`Firehose`/`Output` paths exist
   though `RightPanelTab::ALL` excludes them; S18 is deliberately gated on a runtime audit.
6. **`wg html publish` non-browser uses were not fully traced** — S16 marks that as a
   check, not an assumption.
7. **No survey owned the completion/lifecycle plane.** Whether `src/evaluation/` is
   "enabled-but-unused" or "the intended v3 gate with a broken enqueue" is genuinely
   unanswered; the eval-lane leg of S11 is deferred to a completion-lifecycle decision, and
   the observation ledger is kept in the meantime.
8. **`survey-tests-docs` could not cross-reference the other surveys** (they were not yet
   written); this synthesis re-validated its C1/C6 against `survey-executors`/
   `survey-federation`, which agree.
9. **Usage counters are soft.** `stats.json` is cumulative and reset-prone (`"reset": 10`);
   `tui`=19 under-counts a long-lived process. Used for relative sign only (html ~0, viz
   huge), never as an absolute.
10. **Observer effect.** The `.wg/` graph inspected includes the survey tasks themselves;
    a search for federation verbs now matches these very task descriptions.

---

## 7. Source index (files quoted)

- `.wg/survey/survey-build-size.md` — disk/build mass, sentinel arithmetic, R1–R8.
- `.wg/survey/survey-config-agency.md` — 16 roles→1 model, adaptive loop never ran, C1–C7.
- `.wg/survey/survey-executors.md` — handler taxonomy, native-is-attended-only, C1–C6.
- `.wg/survey/survey-federation.md` — zero daily reachability of the spark stack, P1–P5.
- `.wg/survey/survey-tests-docs.md` — CI gating (8/190), smoke never-in-CI, C1–C8.
- `.wg/survey/survey-tui-plugin.md` — TUI incumbent (~107,500 LOC), duplication, C1–C7.
- `.wg/survey/survey-cow-worktrees.md` — ext4/no reflink, dormant baselines, Options A–E.
- `.wg/survey/audit-agency-federation-reachability.md` — RETIRE verdict for the agency CLI.
- `.wg/survey/reduction-plan.md` — predecessor synthesis (task `survey-synthesis`, done).
- `docs/research/flip-eval-evolution.md` — FLIP/Eval narrowing; invariants that must stay.
- `docs/survey-dependency-hardlink-baseline.md` — Cow Option B design + isolated proof.
- `docs/design-cow-worktrees.md`, `docs/build-artifact-storage.md` — target-layer model.
