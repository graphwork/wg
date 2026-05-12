# Documentation Sync System

**Status:** Proposed
**Date:** 2026-02-19
**Author:** scout (analyst)

## Problem

Documentation drifts from implementation. Every feature addition requires manually updating multiple doc locations. Recent examples:

- **Federation** (4572d76): Full design doc exists, zero user-facing docs (not in README, COMMANDS.md, SKILL.md, or AGENCY.md)
- **Trace/Replay** (effbf5f): Test spec exists, zero user-facing docs anywhere
- **Global config** (db993fe): Mentioned in CLAUDE.md only, not in any user guide
- **Setup wizard** (dc0f0e4): No documentation at all
- **Amplifier executor** (a1fdaa1): Scattered mentions in README/CLAUDE.md/SKILL.md, not in COMMANDS.md

The pattern is clear: features land with design docs or test specs, but user-facing documentation never gets created.

## Documentation Inventory

### Primary User-Facing Docs

| File | Lines | Covers | Update Frequency |
|------|-------|--------|-----------------|
| `README.md` | ~755 | Entry point, quick start, feature overview, examples | Should track all features |
| `docs/COMMANDS.md` | ~1,265 | CLI command reference (~75% of commands) | Should track all commands |
| `CLAUDE.md` | ~26 | Agent instructions, critical warnings | Rarely changes |
| `.claude/skills/wg/SKILL.md` | ~365 | Claude Code skill, command cheat-sheet | Should track all commands |
| `docs/README.md` | ~271 | Doc index, core concepts, quick start | Should track doc structure |

### System-Specific Guides

| File | Lines | Covers |
|------|-------|--------|
| `docs/AGENCY.md` | ~572 | Agency system: roles, motivations, agents, evolution |
| `docs/AGENT-GUIDE.md` | ~353 | Operating AI agents with wg |
| `docs/AGENT-SERVICE.md` | ~327 | Service daemon architecture and lifecycle |
| `docs/LOGGING.md` | ~175 | Provenance and logging system |

### Reference Material (Conceptual, less update-sensitive)

| File | Lines | Covers |
|------|-------|--------|
| `docs/manual/*.typ` | ~2,499 | Typst manual (conceptual deep-dive, 6 files) |
| `docs/design/*.md` | ~3 files | Design docs for specific features |
| `docs/research/*.md` | ~18 files | Research and analysis documents |
| `docs/archive/` | ~30 files | Historical reviews, research |

### Embedded Documentation

| File | Lines | Covers |
|------|-------|--------|
| `src/commands/quickstart.rs` | ~217 | Interactive quickstart text + JSON output |
| CLI `--help` strings | scattered | Per-command help text in clap structs |

### Coverage Gaps (features with zero user-facing docs)

| Feature | Design/Spec Doc | README | COMMANDS.md | SKILL.md |
|---------|----------------|--------|-------------|----------|
| Federation (6 commands) | `docs/design/agency-federation.md` | - | - | - |
| Trace/Replay/Runs (3 families) | `docs/test-specs/trace-replay-test-spec.md` | - | - | - |
| Loop convergence (`--converged`) | `docs/design/loop-convergence.md` | - | - | - |
| Setup wizard (`wg setup`) | - | - | - | - |
| Global config (`--global`) | - | - | - | - |

## Overlap Analysis

**High overlap (intentional, each serves different audience):**
- Service operation → README (overview), AGENT-GUIDE (how-to), AGENT-SERVICE (architecture)
- Agency basics → README (overview), AGENCY.md (deep-dive), SKILL.md (cheat-sheet)
- Command reference → COMMANDS.md (comprehensive), SKILL.md (quick-lookup), README (highlights)

**Problematic overlap:**
- quickstart.rs and README both describe getting started, can diverge
- SKILL.md and COMMANDS.md both list commands but SKILL.md is actually more complete

The overlap is mostly healthy — different docs serve different audiences. The real problem is features missing from *all* locations, not inconsistency between locations.

## Proposed Solution: Docs Manifest + Post-Completion Reminder

After analyzing the options (see Alternatives Considered below), the simplest mechanism that works is a **docs manifest** combined with **automatic doc-update task creation**.

### Component 1: Docs Manifest (`docs/MANIFEST.md`)

A human-readable mapping from features to doc locations. Agents and humans consult this when implementing features to know what needs updating.

```markdown
# Documentation Manifest

Maps features to the docs that cover them. When implementing or modifying
a feature, update all listed locations.

## Task Management
- README.md (§ Task Management)
- docs/COMMANDS.md (§ Task Management)
- .claude/skills/wg/SKILL.md (§ Commands)
- src/commands/quickstart.rs (QUICKSTART_TEXT)

## Agency System
- README.md (§ Agency)
- docs/AGENCY.md (full guide)
- docs/COMMANDS.md (§ Agency)
- .claude/skills/wg/SKILL.md (§ Agency)
- docs/manual/03-agency.typ

## Service / Coordinator
- README.md (§ Agentic Workflows)
- docs/AGENT-GUIDE.md (full guide)
- docs/AGENT-SERVICE.md (architecture)
- docs/COMMANDS.md (§ Service)
- .claude/skills/wg/SKILL.md (§ Service)
- docs/manual/04-coordination.typ

## Federation
- docs/AGENCY.md (§ Federation)
- docs/COMMANDS.md (§ Federation)
- .claude/skills/wg/SKILL.md (§ Federation)
- README.md (§ Agency — mention)

## Trace / Replay
- docs/COMMANDS.md (§ Trace & Replay)
- .claude/skills/wg/SKILL.md (§ Trace)
- README.md (§ Analysis — mention)

## Loop Edges
- README.md (§ Loop Edges)
- docs/COMMANDS.md (§ Loops)
- .claude/skills/wg/SKILL.md (§ Loops)
- docs/manual/02-task-graph.typ

## Configuration
- docs/COMMANDS.md (§ Configuration)
- .claude/skills/wg/SKILL.md (§ Config)
- README.md (§ Configuration — if global)

## Logging / Provenance
- docs/LOGGING.md (full guide)
- docs/COMMANDS.md (§ Logging)
```

**Why a manifest file instead of a database or code annotations:**
- Zero infrastructure cost — it's just a markdown file
- Agents can read it with standard file tools
- Humans can read and update it naturally
- It doubles as a documentation index
- No code changes required to adopt it

### Component 2: Auto-Create Doc-Update Tasks

Leverage the existing auto-evaluate pattern in the coordinator. When a task completes, the coordinator creates a `update-docs-{task-id}` meta-task if the completed task modified code (has a git diff with non-doc file changes).

**How it works:**

1. Add config flag:
   ```toml
   [coordinator]
   auto_doc_sync = false  # opt-in
   ```

2. In the coordinator tick (same phase as `build_auto_evaluate_tasks()`), add `build_auto_doc_sync_tasks()`:
   - For each task that just reached `Done` status
   - Check `.wg/output/{task-id}/changes.patch` for non-doc file changes
   - If changes exist and no `update-docs-{task-id}` task already exists:
     - Create task `update-docs-{task-id}` with description:
       ```
       Review changes from task {task-id} ("{title}") and update documentation.

       Consult docs/MANIFEST.md for the mapping of features → doc locations.
       Read .wg/output/{task-id}/changes.patch to understand what changed.
       Update all relevant doc locations listed in the manifest.
       If no doc updates are needed, mark this task done with a note explaining why.
       ```
     - Tag it `doc-sync`, `meta`
     - Block it on the source task (already done, so it's immediately ready)

3. The coordinator spawns an agent on the doc-sync task like any other task. The agent:
   - Reads the manifest to find relevant doc locations
   - Reads the changes.patch to understand what changed
   - Updates docs as needed
   - Runs `wg done update-docs-{task-id}`

**Why mirror auto-evaluate:** This is a proven, battle-tested pattern. Auto-evaluate already creates meta-tasks, blocks them on source tasks, and lets the coordinator handle spawning. Doc-sync is structurally identical.

### Component 3: Prompt Injection for Feature Agents

Regardless of whether auto doc-sync is enabled, all agents should know about the manifest. Add a line to the default executor prompt template:

```
If you modify or add features, consult docs/MANIFEST.md to identify
which documentation files need updating.
```

This is a lightweight nudge that costs nothing and helps even when auto doc-sync is off. Agents doing feature work will at least be aware that docs exist and where to find the mapping.

## How Agents Know Which Docs to Update

The flow for an agent implementing a feature:

1. Agent reads task description, starts implementing
2. Agent modifies code in `src/commands/foo.rs` (for example)
3. Before running `wg done`, agent reads `docs/MANIFEST.md`
4. Manifest maps "Foo Feature" → README.md (§ Foo), COMMANDS.md (§ Foo), SKILL.md (§ Foo)
5. Agent updates those locations
6. Agent runs `wg done`

If the agent forgets step 3-5 and auto_doc_sync is enabled:

7. Coordinator sees task completed with code changes
8. Creates `update-docs-{task-id}` meta-task
9. Spawns a fresh agent that reads the manifest + diff
10. Fresh agent updates the docs

This is defense-in-depth: prompt injection catches most cases, auto-created tasks catch the rest.

## Integration with the Coordinator

```
Coordinator Tick
├── Phase 1: Dead agent detection
├── Phase 2: Blocker resolution
├── Phase 3: Auto-assign agents
├── Phase 4: Auto-evaluate tasks        ← existing
├── Phase 4b: Auto doc-sync tasks       ← NEW (same pattern)
├── Phase 5: Query ready tasks
└── Phase 6: Spawn agents
```

The new phase slots in alongside auto-evaluate. Both create meta-tasks blocked on completed source tasks. Both are config-gated. Both use the same spawning pipeline.

## Implementation Plan

**Phase 1: Manifest (zero code changes)**
1. Create `docs/MANIFEST.md` with the feature → doc-location mapping
2. Add the prompt nudge to executor template in `src/service/executor.rs`
3. Update CLAUDE.md to mention the manifest

**Phase 2: Auto doc-sync (code changes)**
1. Add `auto_doc_sync: bool` to coordinator config in `src/config.rs`
2. Add `build_auto_doc_sync_tasks()` to `src/commands/service.rs` (mirror auto-evaluate)
3. Wire it into the coordinator tick between phases 4 and 5
4. Add `doc-sync` and `meta` tags to auto-created tasks

**Phase 3: Refinement (based on experience)**
1. Add `wg docs check` command that reads the manifest and flags stale docs
2. Optionally: track doc-sync task success rate and adjust prompts
3. Optionally: add manifest validation (check that listed files exist)

## Alternatives Considered

### `wg docs sync` staleness-check command
**Pros:** Explicit, can run in CI.
**Cons:** How do you define "stale"? Comparing timestamps is meaningless for docs. Comparing content requires understanding *what* should be documented — that's an AI task, not a script. This is really just the auto doc-sync task with extra steps.
**Verdict:** Deferred to Phase 3 as `wg docs check` (manifest validation only, not content staleness).

### Post-completion hook in `wg done`
**Pros:** Immediate, synchronous.
**Cons:** Blocks the completing agent. Doc updates are a separate concern from task completion. Would make `wg done` slower and more complex. Hooks that create tasks create ordering problems.
**Verdict:** Rejected. Async meta-tasks are cleaner.

### Trace-based change detection
**Pros:** Could automatically detect which features changed by analyzing diffs.
**Cons:** Requires AI to map code changes to features to docs. This is exactly what the doc-sync agent does when reading the manifest + diff. Building it into the trace system adds complexity without clear benefit over "create a task and let an agent figure it out."
**Verdict:** Rejected as separate system. The doc-sync agent already uses trace output (changes.patch).

### Code annotations (e.g., `// @docs: README.md#task-management`)
**Pros:** Precise mapping at the code level.
**Cons:** High maintenance burden. Annotations drift just like docs do. Developers forget to add them. Requires tooling to extract and validate. The manifest file achieves the same mapping with less friction.
**Verdict:** Rejected. Manifest is simpler and equally effective.

### Doc generation from code
**Pros:** Single source of truth.
**Cons:** Only works for reference docs (command help). Conceptual guides, tutorials, and architectural docs can't be generated. Most of wg's documentation is explanatory, not reference.
**Verdict:** Out of scope. Could complement this system but doesn't replace it.

## Cost Analysis

- **Manifest (Phase 1):** Zero runtime cost. One file to maintain.
- **Prompt nudge (Phase 1):** ~30 tokens added to each agent prompt. Negligible.
- **Auto doc-sync tasks (Phase 2):** One additional agent invocation per completed feature task. At haiku-tier model cost, this is ~$0.01-0.05 per doc-sync task. Gated by config flag.
- **Human overhead:** Keeping the manifest current requires updating it when new features are added. This is a small, focused task compared to remembering to update 5+ doc files.

## Success Criteria

1. New features land with their docs updated in the same PR (prompt nudge working)
2. When docs are missed, a doc-sync task catches them within one coordinator tick
3. The manifest accurately reflects the feature → doc mapping (validated by `wg docs check`)
4. No feature goes more than one release without user-facing documentation
