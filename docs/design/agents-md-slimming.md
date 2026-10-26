# Proposal: slim `AGENTS.md` / `CLAUDE.md` to the operational core

**Status:** DRAFT — **the cut needs operator approval** (task `doc-archive-policy`
suggested slice; survey item S20). The patch is written, verified to apply
cleanly to both files, and left **unapplied**. Nothing in this commit changes
`AGENTS.md` or `CLAUDE.md`.

**Patch (generated, never stored):** `scripts/agents-md-slimming.py` —
`--check` proves it applies, `--stats` prints the counts, `-o FILE` writes it.
*A stored unified diff cannot be committed here: its blank context lines trip
`git diff --check`, which is a hard completion gate in this repo.*
**Extraction target (already committed):** [`docs/spark/`](../spark/README.md)

---

## 1. The problem, measured

`AGENTS.md` and `CLAUDE.md` are byte-identical (enforced by
`src/commands/agent_guide.rs::agents_md_and_claude_md_are_layer2_parity`) and are
injected into **every** agent prompt — `wg agent-guide` for the bundled
universal contract plus the repo file as the layer-2 project guide.

| Measure | Value |
|---|---|
| `AGENTS.md` / `CLAUDE.md` | 791 lines, 57,065 bytes each, byte-identical |
| Spark-stage narrative (this proposal's target) | lines **370–791** = 422 lines (53.4% of the file, 55.2% of the bytes) |
| Bytes of that block | 31,506 B per file, i.e. **~31 KB loaded into every worker prompt** |
| Daily-plane content (kept) | lines 1–369: task management, development loop, `cargo fmt`/clippy policy, service configuration, profiles/secrets, Pi plugin, pi accounting, project profile selection, agency weak tier |

The 422-line block documents WG-Fed Waves 3–6, WG-Review, WG-Exec, the
family-team e2e, and WG-Pilot. `docs/survey/survey-synthesis-subtraction-plan.md`
§1 S7 records that nothing in the operator's daily workflow reaches those
surfaces (graph string search for `wg identity` / `wg review` / `wg fed-node` /
`wgid:` / `wg peer` / `--review` → **0**).

The survey measured the boundary at line 343 (file was 764 lines then). The file
has since grown by exactly 27 lines **above** the boundary, so the boundary
moved with it to line 370 — the split point is the first spark `##` heading,
`## WG-Fed identity (...) — federation Wave 3 spark`.

## 2. The rule used to pick the line

> A section stays inline in `AGENTS.md`/`CLAUDE.md` only if a **daily worker**
> needs it to not break something. A section moves out if it documents a
> subsystem that is (a) not on the daily dispatch path, (b) already documented
> under `docs/`, and (c) discoverable from a one-line pointer.

Applied mechanically, that yields exactly the five `##` sections from line 370
on. Everything above them — including the agency weak-tier section, which names
`WG_AGENCY_COMPAT_VERSION` — is retained: it decides where `.evaluate-*` /
`.flip-*` / `.assign-*` tasks run, which every dispatch touches.

## 3. What moves where (all text verbatim, heading levels shifted one step)

| New file | Source (AGENTS.md today) | Lines |
|---|---|---|
| `docs/spark/wg-fed.md` | 370–548 `## WG-Fed identity … (Waves 3–6)` | 185 |
| `docs/spark/wg-review.md` | 549–630 `## WG-Review …` | 88 |
| `docs/spark/wg-exec.md` | 631–700 `## WG-Exec …` | 76 |
| `docs/spark/e2e-family-team.md` | 701–749 `## WG end-to-end …` | 55 |
| `docs/spark/wg-pilot.md` | 750–791 `## WG-Pilot …` | 48 |
| `docs/spark/README.md` | new index: subsystem ↔ verbs ↔ compat const ↔ smoke owner, plus reading rules | 36 |

Each extracted file carries a provenance comment naming the source and this
task, so the move is auditable without digging through git history.

## 4. The replacement stub (exactly this, in both files)

```markdown
## Spark-stage features (documented under `docs/spark/`, not inlined)

The federation / content-safety / remote-execution / pilot stack is
**spark-stage**: complete, smoke-pinned, and off the operator's daily path. It is
documented in `docs/spark/` and deliberately **not inlined in this file**, because
this file is loaded into every agent prompt (rationale + patch:
`docs/design/agents-md-slimming.md`).

| Subsystem | Verbs | Compat const | Doc |
|---|---|---|---|
| WG-Fed (Waves 3–6) | `wg identity`, `wg fed-node`, `wg peer`, `wg msg --to wgid:` | `WG_FED_COMPAT_VERSION` | [`docs/spark/wg-fed.md`](docs/spark/wg-fed.md) |
| WG-Review | `wg review` | — (rides WG-Fed envelopes) | [`docs/spark/wg-review.md`](docs/spark/wg-review.md) |
| WG-Exec | `wg provider` | `WG_EXEC_COMPAT_VERSION` | [`docs/spark/wg-exec.md`](docs/spark/wg-exec.md) |
| Family-team e2e | `tests/smoke/scenarios/e2e_family_team.sh` | — | [`docs/spark/e2e-family-team.md`](docs/spark/e2e-family-team.md) |
| WG-Pilot | `wg pilot` | — | [`docs/spark/wg-pilot.md`](docs/spark/wg-pilot.md) |

Read the matching spark doc **before** changing that code (each carries its
compat-const rule and its smoke owner). A spark doc is never the authority for
the daily worker contract, and never a licence to widen a compat const.
```

## 5. Line counts before → after

| File | Before | After | Δ |
|---|---:|---:|---:|
| `AGENTS.md` | 791 lines / 57,065 B | **388 lines / 25,559 B** | −403 lines, −31,506 B (−55.2%) |
| `CLAUDE.md` | 791 lines / 57,065 B | **388 lines / 25,559 B** | −403 lines, −31,506 B (−55.2%) |
| combined prompt cost (both files) | 114,130 B | **51,118 B** | −63,012 B (−55.2%) |
| `docs/spark/**` (loaded only on demand) | 0 | 488 lines / 36,873 B | +488 lines |

The one-line summary: **the same words, moved from "always in the prompt" to
"loaded when you touch that code"**.

## 6. Lock-step and guard-test proof (verified against the patched bytes)

The two files must stay byte-for-byte identical. The generated patch carries the
identical hunk for both, so the invariant holds by construction, and
`scripts/agents-md-slimming.py --check` runs `git apply --check` on the combined
patch (currently: *"patch applies cleanly to AGENTS.md and CLAUDE.md"*).

The Rust guard tests are **string-containment checks that survive the cut** —
verified by running each assertion against the proposed new content:

| Guard (in `src/commands/agent_guide.rs` / `tests/terminology_lint.rs`) | Assertion | After cut |
|---|---|---|
| `project_guide_scopes_cargo_install_to_the_operator` | contains `WORKERS MUST NOT RUN `cargo install`` | PASS |
| " | contains `Operator install (from the repository root, as the operator)` | PASS |
| " | contains `cargo build` and `cargo test` | PASS |
| " | contains `build/wg-binary-from-worktree` | PASS |
| `agents_md_and_claude_md_are_layer2_parity` | contains `wg agent-guide` and `layer-2` | PASS |
| " | does **not** contain `### Chat agent role` / `### Smoke gate` | PASS |
| " | cross-references `CLAUDE.md` / `AGENTS.md` | PASS |
| " | bytes identical | PASS (identical hunks) |
| `terminology_lint::active_agent_facing_files_do_not_use_retired_branding` | no `workgraph` (case-insensitive) | PASS (0 occurrences) |

So **no test, embed, or smoke change is required** for the cut itself.

## 7. Apply + verify (operator, one shot)

```bash
# 1. preconditions — derives the patch from the CURRENT files and proves it applies
scripts/agents-md-slimming.py --check
scripts/agents-md-slimming.py --stats                      # before/after counts

# 2. apply (both files in ONE patch — never one of them)
scripts/agents-md-slimming.py -o /tmp/agents-md-slim.patch
git apply /tmp/agents-md-slim.patch

# 3. lock-step + guard tests
md5sum AGENTS.md CLAUDE.md                                  # identical
cargo test --lib agent_guide                                # guard tests
cargo test --test terminology_lint

# 4. sanity: the stub still resolves
ls docs/spark/                                              # 5 docs + README
```

Rollback is `git revert` of the apply commit; nothing else depends on the block
being inline.

## 8. Why it is ordered *after* the archive policy

The extracted docs inherit the archive policy's keep rule: `docs/spark/*` is
**live documentation for a spark-stage subsystem**, so it stays (referenced by
`AGENTS.md` and `docs/KEY_DOCS.md`, with a smoke owner each). If the federation
cluster is later feature-gated (S7) or retired, the `docs/spark/` tree is the
single thing that moves to `docs/archive/superseded-designs/` — the stub row is
the only place in the always-loaded file that has to change.

## 9. Residual risks

- **Discoverability.** A worker touching `src/identity/**` no longer sees the
  Wave-4/5/6 narrative inline. Mitigated by: the stub table with a direct link,
  each `src/identity/*` module's own module docs, and `docs/KEY_DOCS.md`.
- **Link rot if `docs/spark/` is later archived.** The stub row is the single
  redirect point; the archive policy requires repointing inbound markdown links
  in the same commit (§5 of `docs/archive/POLICY.md`).
- **No compat-const change.** The consts themselves live in source
  (`src/identity/mod.rs`, `src/providers/mod.rs`); this proposal only moves prose,
  so no handshake is touched.
