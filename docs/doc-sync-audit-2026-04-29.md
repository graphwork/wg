# Documentation Audit — 2026-04-29

Consolidated synthesis of 12 parallel per-zone audits run on 2026-04-29 (task graph: `audit-readme-md`, `audit-docs-commands`, `audit-docs-key`, `audit-claude-md`, `audit-skill-md`, `audit-wg-quickstart`, `audit-wg-agent`, `audit-docs-config`, `audit-every-wg`, `audit-terminology-consistency`, `audit-docs-designs`, `audit-docs-research`).

This run is a **candidate for `wg func extract`** — the 12-fan-out + 1-synthesis pattern is reusable as a doc-sync function. See "Reusable Pattern" at the end.

Last verified: 2026-04-29 (commits applied + deferred items listed).

---

## Summary

- **Audit fan-out**: 12 per-zone audit tasks, each producing a structured delta list via `wg log <id> --list`.
- **Synthesis**: 1 task (this one) — read 12 logs, resolve conflicts, apply edits, write report.
- **Files modified by this synthesis**: 11 (README.md, CLAUDE.md, AGENTS.md, KEY_DOCS.md, COMMANDS.md, AGENT-GUIDE.md, AGENT-SERVICE.md, agent_guide.md, quickstart.rs, lib.rs, main.rs).
- **Deferred items**: ~80 (mostly large bulk-edit sweeps and design-doc header rolls — listed below with reasons).

---

## Cross-Audit Conflicts Resolved

### 1. `coordinator` vs `dispatcher` vs `chat agent`

Canonical glossary (per `src/text/agent_guide.md` and CLAUDE.md): three distinct roles, all previously called "coordinator":

| Canonical | Replaces |
|---|---|
| **dispatcher** | The daemon launched by `wg service start` — polls the graph, spawns worker agents |
| **chat agent** | The persistent LLM session the user talks to (TUI / Claude Code / codex / nex) |
| **worker agent** | An LLM process spawned by the dispatcher to do a single wg task |

Resolution: in user-facing prose, replace role-noun "coordinator" / "orchestrator" with the appropriate canonical term. CLI command names (`wg service create-coordinator`, `[coordinator] model = ...`) and config-key names retain their legacy spellings as **back-compat aliases** of the new chat-named surface (`wg service create-chat`, `[dispatcher]`).

### 2. `local:` / `oai-compat:` / `openai:` model prefixes

Per `src/dispatch/handler_for_model.rs` and `wg config --help`, the canonical prefixes are: `claude:`, `codex:`, `nex:`, `openrouter:`, `ollama:`, `vllm:`, `llamacpp:`, `gemini:`, `native:`. The `local:` and `oai-compat:` (and `openai:`) prefixes are deprecated aliases for `nex:`; `wg migrate config` rewrites them.

Resolution: every example in user-facing docs uses `nex:` for the in-process nex handler. Deprecated-alias mention is kept in CLAUDE.md and AGENT-SERVICE.md so users who hit the deprecation warning can find the canonical name.

### 3. `wgnext` profile name

Per `docs/design-named-profiles.md` and `wg profile list`, the profile is named `nex` (renamed from `wgnext` in 3eee268de). The legacy `~/.wg/profiles/wgnext.toml` auto-migrates to `nex.toml` on next `wg profile init-starters` run; loading it warns.

Resolution: docs reference `nex` profile only. The migration mechanism is mentioned where users would discover stale `wgnext` references.

### 4. `--verify <CRITERIA>` flag

Per `src/commands/add.rs:390-393` and `src/commands/edit.rs:265`, `--verify` errors at runtime ("--verify is deprecated and no longer accepted"). The replacement is a `## Validation` section in the task description; the agency evaluator (auto_evaluate + FLIP) reads it.

Resolution: removed `--verify` rows from `wg add` / `wg edit` flag tables in COMMANDS.md, removed `wg add --verify "..."` examples from README.md / COMMANDS.md / SKILL.md / AGENT-GUIDE.md, replaced with `## Validation` examples. AGENTS.md template no longer recommends `--verify`. Added a one-line "legacy --verify is rejected" note where users would expect to find the flag.

### 5. `.compact-0` attribution

`AGENT-SERVICE.md:562` said "this is the coordinator's self-introspection loop" while `AGENT-GUIDE.md:793` said "this is the chat agent's self-introspection loop". The chat-agent attribution is correct (per CLAUDE.md memory and `wg migrate retire-compact-archive` description). Resolution: AGENT-SERVICE.md still uses `.compact-0` as illustration but doesn't conflict with the chat-agent attribution.

---

## Per-Document Delta Disposition

### `README.md` (audit-readme-md)

**Applied:**
- Fixed wg skill install path comment: `~/.claude/skills/wg/` (was `~/.claude/skills/`).
- Replaced `wg add ... --verify ...` example with `## Validation` body example.
- Replaced section-7 "Verification workflow" — agency-evaluator path described, `--verify` flagged as no-longer-accepted.
- Removed `wg edit my-task --verify "cargo test passes"` example.
- Documented `wg retry --fresh` / `--preserve-session` / `--reason` flags in the Adapt section.

**Deferred:** Other audit-readme-md deltas (#1–#18, terminology touch-ups in long-form prose) — see "Deferred to follow-up" section.

### `docs/COMMANDS.md` (audit-docs-commands)

**Applied:**
- Removed `--verify <CRITERIA>` and `--verify-timeout <DUR>` from the `wg add` flag table.
- Removed `--verify <CRITERIA>` from `wg edit` flag table.
- Removed `wg edit my-task --verify "cargo test test_feature passes"` example.
- Replaced the `wg add ... --verify "All findings ..."` example with a `## Validation`-body example.
- Renamed Service section: added `wg service create-chat`, `delete-chat`, `archive-chat`, `interrupt-chat`, `stop-chat`, `set-executor`, `purge-chats` documentation; legacy `*-coordinator` names listed as aliases.
- Expanded `wg chat` section: added the full subcommand surface (`create / list / show / attach / send / stop / resume / archive / delete`) with positional `MESSAGE` mode preserved as the default form.
- Added new Utility commands sections: `wg which`, `wg executors`, `wg secret` (with backends + URI passthrough), `wg html`, `wg reprioritize`, `wg insert`, `wg rescue`, `wg reap`.

**Deferred:**
- Per-section reorganization (e.g., adding "Worktree Management" or "Native Executor / wg nex" sections — these need their own structure decisions, not in scope).
- `wg session` documentation — same reason.
- Trace Commands section narrative cleanup (the audit said the deprecation note is technically fine).

### `docs/KEY_DOCS.md` (audit-docs-key)

**Applied:**
- Updated "Last updated" timestamp to 2026-04-29 with this audit's task ID.
- Added `AGENTS.md` and `src/text/agent_guide.md` to Embedded Documentation.
- Added `docs/config-canonical.md` and `docs/config-ux-design.md` to User-Facing Documentation.
- Updated descriptions for `docs/AGENT-SERVICE.md`, `AGENT-GUIDE.md`, `AGENT-LIFECYCLE.md`, `models.md`, `COMMANDS.md` (per audit's "stale" findings, where supplemental docs are now cross-referenced).
- Added 17 design-document entries (sessions-as-identity, native-executor-run-loop, nex-as-coordinator, verify-deprecation-plan, llm-verification-gate, chat-agent-persistence, model-config-propagation, pdf-binary-failure-handling, design-actor-driven-cleanup, design-cow-worktrees, design-merge-task, design-agency-tasks-on-claude, design-named-profiles, archival-design, etc.)
- Added 13 research-document entries (agent-lifecycle-and-kill-mechanics, eval-wait-points, qwen3-nex-config, shell-verify-vs-llm-eval-gap, thin-wrapper-executors-2026-04, tui-detail-audit, verify-deprecation-survey, etc.)
- Added Plan documents entry (`docs/plan-of-attack-wg-nex.md`).
- Added Audit documents entries (this report; recovery / unmerged / triage / TUI / codex-handler-merge bug audits).
- Added rescued-archive directory (`docs/archive/2026-04-17-rescued/`).

**Deferred:**
- Some `docs/designs/README.md` and `docs/research/README.md` are now indexed; the bulk header-roll on individual research/design files is a separate task.
- Categorization tweaks (e.g., moving `config-canonical.md` between sections) — single-bucket assignment is good enough for now.

### `CLAUDE.md` (audit-claude-md)

**Applied:**
- Added a "Named profiles and secrets" subsection summarizing `wg profile use / show / list / create / edit / diff / init-starters` plus `wg secret set / get / list / rm / check / backend ...`.
- Added one-liner about `wg config -m/-e` auto-reload (with `--no-reload` opt-out).
- Mentioned `wg config init --route <ROUTE>` accepted values.
- Mentioned `wg config lint` as the canonical read-only audit path.
- Updated coverage list at top to include profiles + secrets.

**Deferred:**
- Bumping the openrouter example to a sonnet variant (cosmetic; the opus example in CLAUDE.md is fine).
- Removing the "one release" deprecation phrasing — kept because the deprecation aliases are still live in code.

### `.claude/skills/wg/SKILL.md` (audit-skill-md)

**Deferred (entirely):**
- This file is in a permission-restricted path (`.claude/skills/wg/`). Updates require explicit user permission to write. The audit findings (16+ "coordinator" → "chat agent" / "dispatcher" replacements, missing `wg service install / set-executor / purge-chats`, missing `wg chat` subcommand surface, missing `wg retry` flags, missing `wg done --ignore-unmerged-worktree`, missing `wg reap`, missing `failed-pending-eval`, missing quality-pass / paused-task / smoke-gate / worktree-isolation / prior-WIP behavioral content, stale `WG_EXECUTOR_TYPE` values, stale amplifier-only references, missing `wg config lint` / `--merged`) are **all valid and applicable**, but this synthesis cannot apply them without bypassing the permission gate. **Tracked as follow-up:** see "Deferred to follow-up" section. The file is the bootstrap skill injected into every new Claude Code session, so accuracy here matters; recommend a dedicated SKILL.md sync task.

### `wg quickstart` text (`src/commands/quickstart.rs`) (audit-wg-quickstart)

**Applied:**
- Replaced "Start the coordinator" → "Start the dispatcher daemon" (and similar role-noun fixes throughout the text and JSON sections).
- Replaced `--coordinator-executor amplifier` example with the canonical `wg config -m claude:opus` / `nex:qwen3-coder` / `openrouter:...` model-spec pattern.
- Replaced `--model anthropic:claude-sonnet-4-6` example with `--model claude:sonnet-4-6` (no `anthropic:` prefix in the handler map).
- Renamed JSON section `multi_coordinator` → `multi_chat` and updated example commands to `create-chat`/`stop-chat`/etc.
- Added `wg service install`, `wg service set-executor`, `wg service interrupt-chat`, `wg service purge-chats` to SERVICE MODE.
- Added `wg setup --route <ROUTE> --yes` example to GETTING STARTED.
- Added new commands to relevant sections: `wg secret` family + URI schemes, `wg executors`, `wg config lint`, `wg config --merged`, `wg config --no-reload`, `wg config init --route <ROUTE>`, `wg migrate secrets`, `wg reap`, `wg reprioritize`, `wg kill --tree`, `wg which`.
- Added `failed-pending-eval` task state explanation in TASK STATE COMMANDS.
- Added entire "API KEYS & SECRETS" section (`wg secret set/get/list/rm/check`, `backend show/set`).
- Added `named_profiles` and `secrets` sections to JSON output.
- Restructured the `chat` JSON section to expose subcommands.
- Updated test asserts (`test_quickstart_text_contains_dispatcher_reminder`, `test_quickstart_text_contains_executors_and_models`, `test_quickstart_text_contains_profiles`) to match.
- Added `#![recursion_limit = "256"]` to `src/lib.rs` and `src/main.rs` to accommodate the larger JSON tree.

**Deferred:**
- Other minor tips/wording cleanups not flagged by the audit.
- Mentioning tmux as a dependency for chat persistence (audit suggested; deferred — separate concern).

### `wg agent-guide` (`src/text/agent_guide.md`) (audit-wg-agent)

**Applied:**
- Added `wg incomplete` and `wg retry` (with `--fresh` / `--preserve-session`) to Worker Agent Workflow step 7.
- Added `wg done --ignore-unmerged-worktree` and `wg wait <task-id> --until <cond>` to step 7.
- Added `failed-pending-eval` state description.
- Added Exec Modes section (`full / light / bare / shell`) with `WG_EXEC_MODE` env var.
- Added remaining env vars (`WG_USER`, `WG_WORKTREE_PATH` / `WG_BRANCH` / `WG_PROJECT_ROOT` / `WG_WORKTREE_ACTIVE`).
- Replaced `--before` (which doesn't exist as a dependency flag) with the correct mechanism (`wg edit ... --add-after` or `--after .quality-pass-<batch-id>` at creation).
- Added cycle-config flags (`--no-converge`, `--no-restart-on-failure`, `--max-failure-restarts`).
- Expanded Three Roles → Chat Agent description with the `wg chat <subcommand>` surface and the `wg migrate chat-rename` migration path for legacy `.coordinator-N` IDs.

**Deferred:** None — all 10 audit deltas applied.

### `docs/AGENT-GUIDE.md` (also audit-readme-md / audit-wg-agent)

**Applied:**
- Added `FailedPendingEval` to the task-state table.
- Replaced "PendingValidation" description with the agency-evaluator-led explanation (no longer says "task has a `--verify` criterion").
- Replaced `--verify` reference in Validation Flow with `## Validation` description.
- Updated model-spec table: removed `local:` and `oai-compat:` rows, added a one-liner that those are deprecated aliases.
- Updated `wg config -m` examples to use `nex:`.
- Updated `openrouter:anthropic/claude-opus-4-6` → `openrouter:anthropic/claude-opus-4-7`.
- Updated `codex:gpt-5` → `codex:gpt-5.5`.

### `docs/AGENT-SERVICE.md` (audit-terminology-consistency)

**Applied:**
- Replaced "Coordinator loop" → "Dispatcher loop" in the architecture diagram.
- Renamed "## The Coordinator Tick" → "## The Dispatcher Tick" (with historical-name parenthetical).
- Replaced `local:qwen3-coder` → `nex:qwen3-coder` in `wg service reload --model` example and the model-spec table.
- Replaced `oai-compat:gpt-5` row in the model-spec table with deprecated-alias note.
- Updated `openrouter:anthropic/claude-opus-4-6` → `openrouter:anthropic/claude-opus-4-7`.
- Updated `codex:gpt-5` → `codex:gpt-5.5`.

**Deferred:**
- Bulk role-noun replacements throughout the rest of the doc (~38 remaining "coordinator" prose uses). The audit catalogued each instance; these are valid edits but each-line context needs care to avoid mistakes (some "coordinator" uses are config-key names that should remain). Tracked as a follow-up. The two highest-impact spots — the architecture diagram and the section heading — are already fixed.

### `docs/AGENCY.md` (audit-terminology-consistency)

**Deferred:** ~10 "coordinator" → "dispatcher" prose replacements (auto_place / auto_create / evolution-trigger sections, `.evolve-*` meta-task description). These are valid; tracked as a follow-up.

### `docs/AGENT-LIFECYCLE.md` (audit-terminology-consistency)

**Deferred:** 1 "Spawned by coordinator" → "Spawned by dispatcher" prose fix (line 30). Low impact alone; rolled into the AGENCY/SERVICE prose follow-up.

### `docs/config-ux-design.md` (audit-docs-config)

**Deferred:** Significant additions are needed (named profile system, secret backends, `wg migrate secrets`, OpenRouter-example freshness fixes, §2 status-update for shipped fixes, §5.2 byte-match claim that no longer holds). The audit's findings are detailed and applicable. CLAUDE.md now points users to this doc as the canonical config UX reference, so the gaps are real. Tracked as a follow-up — it's a long doc and the changes deserve their own focused commit.

### `AGENTS.md` (audit-docs-key)

**Applied:**
- Removed `--verify "..."` from the task-description template.
- Added explicit "legacy `--verify` is no longer accepted" note pointing readers to `## Validation`.

### `docs/designs/` and `docs/design/` (audit-docs-designs)

**Deferred (mostly):** The audit found 92 design files lacking a contributor-only header and ~17 with a stale "Status: Design / Proposed / Ready" header that should be flipped to "Implemented" / "Historical" / "Superseded". The required edits are programmatic but voluminous (one prepended block per file). KEY_DOCS.md has been updated to surface the most relevant new designs; the contributor-header roll is a separate sweep task. The duplicate `compaction-metrics-and-visibility.md` vs `compaction-metrics-visibility.md` should be deduped — also deferred. The singular-vs-plural dir question (`docs/design/` vs `docs/designs/`) is not in scope for a doc-sync run; flag for a structural sweep.

### `docs/research/` (audit-docs-research)

**Deferred:** The audit found 14+ research documents whose content has been resolved by shipped commits and could carry a "resolved by <SHA>" pointer; 10+ documents need a header noting their date and authoring task. These are valid and additive but voluminous. Tracked as a follow-up.

### Every-`wg`-command coverage (audit-every-wg)

**Applied:**
- Added `wg which`, `wg secret`, `wg html`, `wg executors`, `wg reprioritize`, `wg insert`, `wg rescue`, `wg reap` to COMMANDS.md (Utility section). All 8 of the audit's "fix recommended" zero-coverage commands now have at least one living-doc reference.
- The remaining 2 zero-coverage commands (`wg claude-handler`, `wg codex-handler`) remain undocumented intentionally — they are internal stream-bridge subprocesses, not user-facing surfaces. The audit agreed.
- `wg tui-nex` and `wg tui-pty` remain undocumented; per the audit, these are "possibly experimental/undocumented-on-purpose". Confirming-or-rejecting their public-facing status is out of scope.

---

## Deferred to Follow-Up

The following audit findings are valid but not applied in this synthesis run, with reasons. Each is a candidate for a separate task.

| Finding | Audit | Reason |
|---|---|---|
| SKILL.md (16+ terminology + 8 missing-command + 5 stale + 4 inconsistent items) | audit-skill-md | Permission-restricted path; needs explicit user-approved write |
| AGENT-SERVICE.md ~38 remaining "coordinator" prose uses | audit-terminology-consistency | Volume; per-line care needed (some uses are config-key names) |
| AGENCY.md ~10 "coordinator" → "dispatcher" prose fixes | audit-terminology-consistency | Volume; cleaner as a focused commit |
| AGENT-LIFECYCLE.md `Spawned by coordinator` line | audit-terminology-consistency | Roll into AGENCY/SERVICE follow-up |
| `docs/config-ux-design.md` profile + secret + status-update edits | audit-docs-config | Long doc; deserves a dedicated commit |
| `docs/design/*` 92 contributor-only headers + 17 status updates | audit-docs-designs | Bulk programmatic roll; separate task |
| `docs/research/*` 10+ headers + 14+ resolved-by pointers | audit-docs-research | Bulk programmatic roll; separate task |
| Singular-vs-plural directory (`docs/design/` vs `docs/designs/`) | audit-docs-designs | Structural decision; not in scope for doc-sync |
| Duplicate `compaction-metrics-and-visibility.md` vs `compaction-metrics-visibility.md` | audit-docs-research | Needs the originator's intent to dedupe correctly |
| `docs/manual/` Typst chapters update for new commands | audit-readme-md (implicit) | Out of scope; manual is a separate documentation surface |
| `wg session` / `wg nex` / Worktree Management sections in COMMANDS.md | audit-docs-commands | Need their own structure decisions |
| AGENT-SERVICE.md `--executor` deprecation phrasing tightening | audit-claude-md | Cosmetic; the code still ships the alias |

These deferrals are explicit and documented. None of them block this audit's primary deliverables.

---

## grep Verification

```
grep -rn '--verify' README.md AGENTS.md CLAUDE.md docs/COMMANDS.md docs/AGENT-GUIDE.md src/text/agent_guide.md src/commands/quickstart.rs
# (only legitimate references remain: --skip-verify on `wg done`, deprecation notes, archive/research files)

grep -rn '\blocal:\(qwen\|gpt\)\|\boai-compat:' README.md CLAUDE.md docs/AGENT-SERVICE.md docs/AGENT-GUIDE.md src/commands/quickstart.rs src/text/agent_guide.md
# (only deprecation-mention references remain)
```

Code-side `--verify` is rejected at runtime:
- `src/commands/add.rs:390-393` — error path
- `src/commands/edit.rs:265` — error path

`nex:` prefix routing:
- `src/dispatch/handler_for_model.rs` — single source of truth for handler routing

---

## Reusable Pattern (`wg func extract` candidate)

The pattern run today:

```
1 seed task: doc-sync-audit-2026-04-29
  ↓ fan-out (12 independent audits)
audit-readme-md          audit-docs-commands     audit-docs-key
audit-claude-md          audit-skill-md          audit-wg-quickstart
audit-wg-agent           audit-docs-config
audit-every-wg           audit-terminology-consistency
audit-docs-designs       audit-docs-research
  ↓ fan-in
1 synthesis task: doc-sync-audit (this task)
  → consolidated report at docs/doc-sync-audit-<date>.md
  → coherent commits applying findings
  → KEY_DOCS.md updated with new files
```

Each audit task description shared a common skeleton:
- "## Description" — what zone to audit (file or topic)
- "## Baseline" — last human-authored commit to that zone, or fallback to last-doc-sync date
- "## Synthesis steps" — for the integrator
- "## Validation" — `wg log <task-id> --list` produced structured deltas; no source/doc edits
- Validation gates: baseline cited, deltas posted via `wg log`, no edits.

The pattern is **fork-join** with a strong fan-in synthesizer. Reusable inputs: `<date>`, `<list-of-audit-zones>`. Reusable validation: each leaf audit must produce a structured delta list via `wg log`; the synthesis must apply (or explicitly defer) every delta and write a consolidated report. Recommend extracting this as a `doc-sync` function via `wg func extract` so the next quarterly doc-sync run can be instantiated with `wg func apply doc-sync --input date=2026-07-29`.

---

## Acknowledgements

All 12 leaf audits ran on claude:haiku via the agency pipeline; each produced structured `wg log --list` deltas with file:line citations. This synthesis read each via `wg log <id>`. No leaf audit modified source/doc files — all read-only. This synthesis applied the resolutions in coherent commits.

Baseline for this audit: 2026-04-12 (`docs/doc-sync-audit-2026-04-12.md`).

Next audit: recommended quarterly cadence; instantiate via `wg func apply doc-sync --input date=...` once the function is extracted.
