# Manual Audit Inventory — 2026-04-12

Initial inventory and decomposition of docs/manual/ directory audit against current CLI help and runtime behavior.

**Scope:** docs/manual/ directory only (per task specification)

---

## Manual Documentation Inventory

### Core Manual Structure
The manual is organized as a conceptual guide for humans to understand wg, structured in 6 main components:

#### Source Files Inventoried
- **README.md** — Manual overview, compilation instructions, structure guide
- **wg-manual.md/.typ** — Unified manual with comprehensive glossary  
- **01-overview.md/.typ** — System overview, core concepts
- **02-task-graph.md/.typ** — Tasks, dependencies, cycles, readiness
- **03-agency.md/.typ** — Roles, tradeoffs, agents, federation  
- **04-coordination.md/.typ** — Service daemon, coordinator, dispatch
- **05-evolution.md/.typ** — Evaluation, evolution, improvement
- **PLAN.md** — Planning document for manual structure
- **UPDATE-SPEC.md** — Update specifications and processes

#### Format Distribution
- **Markdown (.md):** 9 files — source format for CLI-friendly reading
- **Typst (.typ):** 6 files — source format for PDF compilation  
- **PDF (.pdf):** 6 files — compiled output for distribution

### CLI Command Reference Density

**Total CLI references found:** 579 occurrences of "wg [command]" pattern across all manual files

**Command categories observed:**
- Task management: `wg add`, `wg edit`, `wg done`, `wg fail`, `wg list`, `wg show`
- Dependencies: `--after`, `--before`, cycle configurations
- Service: `wg service start`, `wg agents`, coordination features
- Agency: `wg role`, `wg tradeoff`, `wg agent`, `wg assign`, `wg evolve`
- Evaluation: `wg evaluate`, performance tracking, scoring
- Federation: `wg agency remote`, sharing capabilities
- Advanced: `wg trace`, `wg func`, `wg replay`, integration features

---

## Audit Decomposition Strategy

Given the substantial scope (579 CLI references across 9 core files), this audit has been decomposed into parallel verification tasks with a synthesis phase:

### Parallel Audit Tasks Created
1. **audit-manual-section-1** — Overview section CLI verification
2. **audit-manual-section-2** — Task graph commands and workflows  
3. **audit-manual-section-3** — Agency system commands and features
4. **audit-manual-section-4** — Coordination and service management
5. **audit-manual-section-5** — Evolution and evaluation systems
6. **audit-unified-manual** — Unified manual and glossary verification
7. **audit-manual-supporting** — Supporting files (README, PLAN, UPDATE-SPEC)

### Integration Task
- **synthesize-manual-audit** — Depends on all 7 parallel tasks, produces comprehensive gap analysis

---

## Validation Reference

Each subtask will verify manual content against:
- Current CLI help output (`cli-commands-help-all.txt` from prior doc sync)
- Individual command help (`wg [command] --help`)  
- Actual runtime behavior testing
- Current feature availability and syntax

### Gap Analysis Criteria
- **Accuracy:** Commands/flags that exist in manual but differ in current CLI
- **Completeness:** CLI features missing from manual documentation
- **Staleness:** Deprecated features still documented or current features missing

---

## Next Steps

The manual audit infrastructure is now established. The 7 parallel subtasks will conduct detailed verification of specific manual sections, with findings integrated into a comprehensive gap analysis by the synthesis task.

**File scope confirmed:** All work constrained to docs/manual/ directory as specified.