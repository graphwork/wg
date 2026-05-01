# Research: Locating the Full Upstream Primitive Pool

## Summary

The full upstream primitive pool **is already vendored** in the workgraph source tree at `agency/starter.csv` (committed in `64bcf83`). The `wg agency init` code path already auto-imports it. The gap between the hardcoded starter set and the full pool is documented below.

---

## 1. Where the full pool lives

**File:** `agency/starter.csv` (180 KB, 638 lines including header)

Git-tracked since commit `64bcf83` ("feat: bundle Agency starter.csv into workgraph repo").

### Entity counts in the full pool

| Type | Count |
|------|-------|
| `role_component` | 338 |
| `desired_outcome` | 98 |
| `trade_off_config` | 201 |
| **Total** | **637** |

### Scope breakdown (CSV `scope` column)

| Scope | Count | Purpose |
|-------|-------|---------|
| `task` | ~579 | General-purpose task primitives |
| `meta` | 26 | Meta-level agency primitives |
| `meta:assigner` | 14 | Assigner-specific metaprimitives |
| `meta:evaluator` | 5 | Evaluator-specific metaprimitives |
| `meta:evolver` | 4 | Evolver-specific metaprimitives |
| `meta:agent_creator` | 2 | Creator-specific metaprimitives |
| *(empty)* | 7 | Unscoped primitives |

---

## 2. How `wg init` seeds primitives

The code path is in `src/commands/agency_init.rs`, function `run()`. Seeding happens in three stages:

### Stage 1: Hardcoded starters (`agency::seed_starters()`)

**Source:** `src/agency/starters.rs`

Writes YAML files to `.wg/agency/` from Rust-embedded definitions:

| Category | Count | Examples |
|----------|-------|---------|
| Actor role components | 8 | code-writing, testing, debugging, code-review, security-audit, technical-writing, system-design, dependency-analysis |
| Actor desired outcomes | 4 | "Working, tested code", "Review report with findings", "Clear documentation", "Design document with rationale" |
| Actor tradeoff configs | 4 | Careful, Fast, Thorough, Balanced |
| Actor roles (compositions) | 4 | Programmer, Reviewer, Documenter, Architect |
| Special agent components | 27 | task-to-component-matching, cardinal-scale-grading, wording-mutation, etc. |
| Special agent outcomes | 4 | Optimal agent-task assignment, Calibrated evaluation grade, etc. |
| Special agent tradeoffs | 7 | Assigner Balanced, Evaluator Balanced, Evolver Balanced, Creator Unconstrained/Adjacent/Distant/Internal |
| Special agent roles | 4 | Assigner, Evaluator, Evolver, Agent Creator |
| **Total hardcoded** | **~62** | *(components + outcomes + tradeoffs + roles)* |

### Stage 2: Bundled CSV auto-import (`try_csv_import()`)

**Source:** `src/commands/agency_init.rs:302-333`

Conditions:
1. `agency/starter.csv` exists at `<project_root>/agency/starter.csv`
2. No import manifest exists yet at `.wg/agency/import-manifest.yaml`

When both conditions are met, calls `agency_import::run()` which parses the CSV and writes YAML primitives to `.wg/agency/primitives/`. Content-hash deduplication ensures no duplicates with the hardcoded starters.

After import, writes `import-manifest.yaml` with SHA-256 content hash to prevent re-import on subsequent `wg agency init` runs.

### Stage 3: Upstream pull (`try_upstream_pull()`)

**Source:** `src/commands/agency_init.rs:252-292`

If `agency.upstream_url` is configured in `config.toml` (global or local), fetches a remote CSV and imports it. This is non-blocking — failure to fetch prints a warning but does not fail init.

Currently no default upstream URL is configured, so this is a no-op for most installations.

---

## 3. The gap: starter seed vs. full pool

| Metric | Hardcoded starters | Full CSV pool | Gap |
|--------|-------------------|---------------|-----|
| Role components | 35 (8 actor + 27 special) | 338 | 303 |
| Desired outcomes | 8 (4 actor + 4 special) | 98 | 90 |
| Trade-off configs | 11 (4 actor + 7 special) | 201 | 190 |
| **Total primitives** | **54** | **637** | **583** |
| Roles (compositions) | 8 | 0 (CSV has primitives only, not compositions) | N/A |

Qualitative difference: The hardcoded starters are one-line labels (e.g., "Writes production-quality code."). The CSV primitives are instructional verb phrases averaging 50-150 characters (e.g., "Identify gaps, errors, or inconsistencies in provided content"). The CSV primitives provide actual behavioral instructions for LLM agents.

---

## 4. Is the full pool vendored? YES

**The full pool is vendored at `agency/starter.csv`** and is automatically imported on `wg agency init`.

The seeding pipeline is:
1. Hardcoded starters always write (fallback baseline)
2. CSV auto-import adds the full 637 primitives (if CSV exists and no prior import)
3. Upstream pull adds remote primitives (if configured)

### File paths containing primitive data

| Path | Format | Content |
|------|--------|---------|
| `agency/starter.csv` | CSV (9 columns) | Full upstream pool: 637 primitives |
| `src/agency/starters.rs` | Rust source | Hardcoded starters: ~62 entities (primitives + compositions) |
| `.wg/agency/primitives/components/*.yaml` | YAML | Imported component files (runtime) |
| `.wg/agency/primitives/outcomes/*.yaml` | YAML | Imported outcome files (runtime) |
| `.wg/agency/primitives/tradeoffs/*.yaml` | YAML | Imported tradeoff files (runtime) |
| `.wg/agency/cache/roles/*.yaml` | YAML | Composed role cache entries (runtime) |
| `.wg/agency/cache/agents/*.yaml` | YAML | Composed agent cache entries (runtime) |
| `.wg/agency/import-manifest.yaml` | YAML | Import provenance tracking (runtime) |

### Code paths for seeding

| Function | File:Line | Purpose |
|----------|-----------|---------|
| `run()` | `src/commands/agency_init.rs:11` | Main agency init orchestrator |
| `seed_starters()` | `src/agency/starters.rs:653` | Write hardcoded primitives to YAML |
| `try_csv_import()` | `src/commands/agency_init.rs:302` | Auto-import bundled CSV |
| `try_upstream_pull()` | `src/commands/agency_init.rs:252` | Pull from configured upstream URL |
| `run_import()` | `src/commands/agency_import.rs` | CSV parsing and YAML writing |

---

## 5. Existing research

The file `docs/research/primitive-pool-sync.md` contains a prior research document that proposed bundling the CSV. That proposal has since been **fully implemented** — the CSV is bundled, the import command handles Agency's 9-column format, and `wg agency init` auto-imports.
