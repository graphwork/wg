# Audit: recovery + outage workflows in agent-visible docs

**Date:** 2026-04-27
**Task:** `audit-recovery-outage`
**Scope:** Every text surface a fresh worker agent sees, examined for whether a worker hitting today's outage scenarios (credit exhaustion, mass-failure batch retry, openrouter→claude:opus migration, stale `coordinator-state-N.json` `model_override`) could self-recover.

**Outage TL;DR:** the chat agent (Claude with project memory) puzzled out the recovery path — `wg recover --filter "error~credit" --set-model claude:opus --set-endpoint <name> --keep-agency`, then manual edit of `.workgraph/service/coordinator-state-N.json` to clear the stale `model_override`, then `wg endpoints remove openrouter --global`. None of that workflow is written down where a worker (especially a non-Claude or non-memory-having worker) could find it.

---

## 1. Surfaces audited

| # | Surface | Bytes / lines | Found at |
|---|---------|---------------|----------|
| S1 | `wg quickstart` | ~30 KB / 587 lines | runtime CLI |
| S2 | `~/.claude/skills/wg/SKILL.md` (the prelude injected into every Claude Code worker) | 22 KB / 518 lines | global skill dir |
| S3 | `docs/AGENT-GUIDE.md` (referenced by the agent prompt prelude) | 1205 lines | repo |
| S4 | `docs/AGENT-LIFECYCLE.md` | 733 lines | repo |
| S5 | `docs/AGENT-SERVICE.md` | 677 lines | repo |
| S6 | `CLAUDE.md` (repo root) | 102 lines | repo |
| S7 | `wg recover --help` (short = long; no `long_about`) | 11-line block | CLI |
| S8 | `wg endpoints --help` + `wg endpoints remove --help` | combined ~25 lines | CLI |
| S9 | `wg service --help` | ~20 lines | CLI |
| S10 | `wg config --help` | ~120 flags | CLI |
| S11 | `wg agents --help` | ~15 lines | CLI |
| S12 | `.workgraph/` README / template files | **does not exist** (no README, no template, only `.gitignore`) | filesystem |
| S13 | `AGENT.md` / `AGENTS.md` at repo root | **do not exist** | filesystem |
| S14 | `RECOVERY.md` anywhere | **does not exist** | filesystem |

Note on `.wg/` vs `.workgraph/`: in the audited worktree only `.workgraph/` exists. The task description's recovery path of `.wg/service/coordinator-state-N.json` is itself stale-or-aspirational — the canonical location is `.workgraph/service/coordinator-state-N.json`. Today's "both exist with stale duplicates" symptom presumably lives at the user's main checkout from a prior layout. **The directory split is itself an undocumented gap (G6).**

---

## 2. Gap matrix

Legend: ✓ = explicitly documented in a way a worker can act on; ~ = partial / oblique mention; ✗ = not present.

| Surface | G1 `wg recover` mentioned outside its own help? | G2 `--keep-agency` / `--set-model` / `--set-endpoint` / `--filter` example invocations? | G3 Model-precedence chain (incl. `coordinator-state.model_override` rung) | G4 Stale `coordinator-state-N.json` `model_override` trap (existence + path + clear procedure) | G5 `wg endpoints remove` framed as a recovery step + global-vs-local `is_default` merge semantics | G6 `.wg/` vs `.workgraph/` canonical-directory + migration |
|---|---|---|---|---|---|---|
| S1 `wg quickstart` | ✗ | ✗ | ~ partial (line 427: `task --model > executor model > coordinator model > default` — does NOT mention `coordinator-state.model_override` rung) | ✗ | ~ partial (endpoints CRUD shown 449–461; no recovery framing, no `is_default` merge note) | ✗ |
| S2 `~/.claude/skills/wg/SKILL.md` (worker bootstrap) | ✗ | ✗ | ✗ | ✗ | ✗ | ✗ |
| S3 `docs/AGENT-GUIDE.md` | ✗ (only `wg sweep` for orphaned tasks) | ✗ | ~ partial (line 689: 4-rung priority list, but rung 2 says "Executor config model" — does NOT name `coordinator-state.model_override`) | ✗ | ✗ | ✗ |
| S4 `docs/AGENT-LIFECYCLE.md` | ✗ (only "recovery branches" — git-commit recovery, **different meaning**) | ✗ | ✗ | ✗ | ✗ | ✗ |
| S5 `docs/AGENT-SERVICE.md` | ✗ | ✗ | ✓ (line 329: `task.model > executor.model > coordinator.model/CLI --model`; §"Model hierarchy" line 568 — still no `model_override` rung) | ✗ (file `coordinator-state.json` named at line 619 but as a metrics-store; nothing about the override trap) | ✓ for merge semantics (lines 455–481 fully cover `inherit_global` + local-replaces-global), ~ for recovery framing (no "after credit exhaustion, run `wg endpoints remove`") | ✗ |
| S6 `CLAUDE.md` (repo root) | ✗ | ✗ | ~ partial (model+endpoint pairs listed but no precedence chain) | ✗ | ✗ | ✗ |
| S7 `wg recover --help` | ✓ (this *is* the surface) | ~ partial (one filter example: `error~credit`; no full credit-outage runbook, no `--keep-agency` rationale) | ✗ | ✗ | ✗ | ✗ |
| S8 `wg endpoints [remove] --help` | ✗ | ✗ | ✗ | ✗ | ✗ (no merge semantics, no recovery hint) | ✗ |
| S9 `wg service --help` | ✗ | n/a | n/a | ✗ | n/a | ✗ |
| S10 `wg config --help` | ✗ | n/a | n/a | ~ partial (the `--merged` flag's help string says "why is openrouter still in my routing when I removed it locally?" — closest thing to a recovery breadcrumb in the entire CLI) | ~ partial (`--reset --route <name>` and `--merged` documented; merge semantics not) | ✗ |
| S11 `wg agents --help` | ✗ | n/a | n/a | ✗ | n/a | ✗ |

**Summary of coverage:**
- **G1** (`wg recover` cross-reference): **✗ across every surface except its own help.** A worker that doesn't already know `wg recover` exists will not discover it.
- **G2** (recipe-grade examples): **✗ everywhere.** Even `wg recover --help` only gives one filter example.
- **G3** (model precedence chain): **partially documented in three places**, but **none of the existing chains include the `coordinator-state.model_override` rung that bit us today.** That rung is invisible — coordinator-state lives in `service/` state files and is set by IPC, but it silently overrides config and CLI choice at chat-agent spawn time (`src/commands/service/coordinator_agent.rs:727`).
- **G4** (stale `model_override` trap): **✗ everywhere.** No surface mentions the file path, the trap, or how to clear it. Today's recovery required reading source.
- **G5** (`wg endpoints remove` as recovery + `is_default` merge): merge semantics covered well in AGENT-SERVICE.md §"Endpoint inheritance"; **the recovery framing — "if a global `is_default = true` openrouter endpoint is poisoning new chats, run `wg endpoints remove openrouter --global`" — is not written down anywhere a worker would find at task time.**
- **G6** (`.wg/` vs `.workgraph/`): **✗ everywhere.** No surface acknowledges that two directories may coexist or which is canonical. The agent guide says "worktrees are created under `.wg-worktrees/`" (a third name), which compounds the confusion.

---

## 3. Prioritized fixes

Ranked by leverage (= likelihood that a future worker would have hit this gap × severity of resulting failure mode).

### Priority 1 — Stop the bleed: cross-reference `wg recover` from the worker's bootstrap path

**Why first:** A worker that doesn't know the command exists cannot run it — no number of internal `--help` improvements helps. The cheapest fix with the largest blast-radius is one paragraph in two places: `wg quickstart` and `~/.claude/skills/wg/SKILL.md`.

**Surfaces to update:**
1. `wg quickstart` — add a `RECOVERY` section near the existing `COMPACT, SWEEP & CHECKPOINT` section.
2. `~/.claude/skills/wg/SKILL.md` (and the equivalent amplifier-bundle prompt) — add a short "If many tasks failed at once" sub-section.
3. `docs/AGENT-GUIDE.md` §7b "Operational Commands" — add a `### Recover` subsection alongside Sweep.

**Proposed wording (drop-in for `wg quickstart`):**

```
RECOVERY (after credit exhaustion, mass-failure, or wrong-model routing)
─────────────────────────────────────────
  When many tasks fail with the same root cause (provider out of credits,
  endpoint dead, wrong model selected) — DO NOT mass-retry by hand.
  Use `wg recover` to plan-then-execute a batch reset:

    wg recover                                      # dry-run: preview the plan
    wg recover --filter "error~credit" --yes        # only credit-exhaustion failures
    wg recover --filter "tag=eval-scheduled" \
               --set-model claude:opus --yes        # also rewrite model on retry
    wg recover --filter "id-prefix=tui-" \
               --set-endpoint local-claude --yes    # rewrite endpoint on retry
    wg recover --keep-agency --yes                  # keep .evaluate-*/.flip-* followups

  Filter clauses (repeatable, comma-separated):
    status=failed   tag=<name>   id-prefix=<str>   attempts<=<N>   error~<substr>

  Stale model selection in chat agents:
    Each running chat agent has a state file at:
      .workgraph/service/coordinator-state-<N>.json
    Field `model_override` (set by `wg service set-executor`) overrides
    config and CLI. If a chat keeps spawning with the wrong model after
    you reset config, edit that file (set `"model_override": null`) and
    `wg service restart` — or rerun `wg service set-executor` with the
    intended model.

  Stale endpoint poisoning all new chats:
    `wg config --merged` shows the effective endpoint list. If a global
    `is_default = true` endpoint is leaking into local projects, remove
    it from global with:  wg endpoints remove <name> --global
    Local entries fully replace global unless local has no
    [llm_endpoints] section (see docs/AGENT-SERVICE.md "Endpoint
    inheritance").

  Model precedence (highest wins):
    1. task.model (per-task, set by `wg add --model` / `wg edit --model`)
    2. coordinator-state model_override (chat agents only)
    3. dispatcher / executor config model
    4. global config model
    5. handler default
```

This single block closes G1, G2, G3 (adding rung 2), G4, and G5 in one place. Estimated work: edit `src/commands/quickstart.rs`, mirror to SKILL.md, mirror to AGENT-GUIDE.md §7b.

### Priority 2 — Make `wg recover --help` self-sufficient

**Why second:** Even after P1, a worker may land on `wg recover --help` first. Right now the short help is the only help (`--help-long` is rejected). Add a `long_about` with the same examples block from P1.

**Surface:** `src/commands/recover.rs` and `src/cli.rs` `Recover` command (line 590).

**Proposed:** add a `long_about` derive that includes:
- The full filter-clause grammar with one example each
- An end-to-end runbook for credit exhaustion ("you ran out of OpenRouter credits — run `wg recover --filter error~credit --set-model claude:opus --keep-agency --yes`")
- The two adjacent traps (stale `model_override`, stale `is_default = true` endpoint)
- A pointer to `docs/AGENT-SERVICE.md` §"Endpoint inheritance" for merge semantics

### Priority 3 — Fill the `model_override` rung in every existing precedence chain

**Why third:** Three documents (AGENT-GUIDE.md:689, AGENT-SERVICE.md:329 + §"Model hierarchy", quickstart line 427) already document a precedence chain. Each is wrong-by-omission today: none names the `coordinator-state.model_override` rung that overrides them all for chat agents. Edit the existing lists rather than adding new ones — three parallel partials are how today's gap was created.

**Single canonical wording:**
```
Model resolution (highest wins):
  1. task.model               (per-task: `wg add --model`, `wg edit --model`)
  2. chat coordinator-state.model_override
                              (chat agents only — set by `wg service set-executor`,
                               persisted in .workgraph/service/coordinator-state-<N>.json)
  3. agent.preferred_model    (when an agent identity is assigned)
  4. dispatcher.model         ([dispatcher] / legacy [coordinator] in config.toml)
  5. agent.model              ([agent] in config.toml)
  6. handler default          (no model flag passed; handler uses its own default)
```

### Priority 4 — Document `.wg/` vs `.workgraph/` canonical-directory rule

**Why fourth (lower than recovery itself):** This is a layout cleanup, not a runbook. But every recovery procedure references a path, and pointing a worker at the wrong path wastes the recovery budget. Add a one-liner to `CLAUDE.md` and `wg quickstart`:

> Canonical project state lives in `.workgraph/`. Agent worktrees live in `.wg-worktrees/`. There is no `.wg/` directory; if you find one, it is leftover from a prior layout and can be removed (verify with `wg config --merged`).

If `.wg/` is in fact a planned new layout, the answer is the inverse — but either way, write it down. **This is the one item that needs a human decision before doc updates ship; everything else above is uncontroversial.**

### Priority 5 — `wg endpoints remove` recovery hint in `--help`

Add a one-line `long_about` to `wg endpoints remove`:

> Use during recovery when a stale global endpoint (e.g., `is_default = true` openrouter after credit exhaustion) is poisoning all local projects. Run with `--global` to scrub from `~/.workgraph/config.toml`. See `docs/AGENT-SERVICE.md` §"Endpoint inheritance" for merge semantics.

---

## 4. Concrete recommendation

**Recovery should live in all three of: quickstart, agent guide, and a new `RECOVERY.md` — but with strict role separation. Do NOT triple-write the same content; that's how today's three-partial-precedence-chains gap was created.**

| Surface | Role | Content |
|---------|------|---------|
| `wg quickstart` | **Discoverability** — the one place every fresh agent reads | A ~30-line `RECOVERY` section with the wording in §3 P1 above. Cross-references `RECOVERY.md` for depth. Lists the four trap commands (`wg recover`, edit `coordinator-state-N.json`, `wg endpoints remove --global`, `wg config --merged` to verify). |
| `docs/AGENT-GUIDE.md` §7b | **Operational** — sits next to Compact/Sweep/Checkpoint | A 5–10 line `### Recover` subsection that points at `RECOVERY.md` and `wg recover --help`. Do not duplicate the runbook. |
| **NEW** `docs/RECOVERY.md` | **Depth** — the single source of truth | The full runbook: every failure mode, every command, every state-file path, every config-precedence rung. Quickstart and AGENT-GUIDE.md link here. `wg recover --help`'s `long_about` ends with "see docs/RECOVERY.md for the full runbook." |
| `~/.claude/skills/wg/SKILL.md` | **Bootstrap** — injected into every Claude Code worker | One paragraph + link to `RECOVERY.md`. Mirrors the quickstart section but tighter. |
| `CLAUDE.md` (repo root) | **Project-specific** — the directory canonicalization note (P4) only | Single line on `.workgraph/` canonicality. |

**Why this split:**
- **A new `RECOVERY.md` is justified** because (a) the topic crosses ≥3 commands (`recover`, `service`, `endpoints`, `config`) and ≥2 state surfaces (config files + `coordinator-state-N.json`), and (b) recovery procedures are read under stress and need to be findable by name (`grep RECOVERY` works; trying to remember which doc had the credit-exhaustion runbook does not).
- **Quickstart must mention it** because that is what `CLAUDE.md` tells every chat agent to run at session start, and it's the worker's first stop. Anything not in quickstart effectively does not exist for non-Claude workers.
- **AGENT-GUIDE.md §7b should not duplicate** — three parallel precedence-chain docs is exactly the failure pattern that produced gap G3. Use a pointer.

**Single follow-up implementation task** (filable from this audit without re-research):

```
wg add "Implement audit-recovery-outage P1+P2+P3+P4" \
  --after audit-recovery-outage \
  -d "## Description
Implement Priority 1, 2, 3, 4 fixes from docs/audit-recovery-docs-2026-04-27.md.
Files to edit (file-scoped; no overlap):
  - src/commands/quickstart.rs (new RECOVERY section, exact wording in P1)
  - src/cli.rs + src/commands/recover.rs (long_about per P2)
  - src/commands/endpoints.rs (long_about per P5)
  - docs/AGENT-GUIDE.md §7b (### Recover subsection per P3+P4)
  - docs/AGENT-SERVICE.md §Model hierarchy (rewrite per P3 canonical chain)
  - docs/RECOVERY.md (new file — full runbook)
  - ~/.claude/skills/wg/SKILL.md (one paragraph + link)
  - CLAUDE.md (one-liner on .workgraph/ canonicality)
P4 — confirm with user whether .wg/ is leftover or planned new layout BEFORE editing.

## Validation
- [ ] cargo build && cargo test pass
- [ ] \`wg quickstart\` output contains the literal string 'RECOVERY'
- [ ] \`wg recover --help\` output contains 'coordinator-state' and 'is_default'
- [ ] \`wg endpoints remove --help\` output contains 'recovery' or 'stale'
- [ ] grep -l 'model_override' docs/AGENT-GUIDE.md docs/AGENT-SERVICE.md returns both files
- [ ] docs/RECOVERY.md exists and references all four trap state files
- [ ] No surface duplicates the precedence chain (single canonical version in RECOVERY.md, others link)
"
```
