# Agency Research: Vaughn's Agency Repo and Integrations

**Repo:** https://github.com/agentbureau/agency
**Date:** 2026-03-19
**Task:** agency-research

---

## 1. What is Agency?

**Agency** is a self-hosted engine for composing, evaluating, and evolving AI agents. It is written in Python (v3.13+), published as `agency-engine` on PyPI, and licensed under the **Elastic License 2.0** (code) and **CC BY 4.0** (docs).

### Core thesis

Agency treats AI agents not as monolithic system prompts but as **composable, evaluable, evolvable entities**. It decomposes agents into independent primitives and composes them into agents matched to specific tasks, using structured performance data to improve future compositions over time.

> "Agency does not execute tasks. It composes agent descriptions and returns them to a task manager (Claude Code, Superpowers, or any MCP-compatible system) which executes the work. Agency handles composition, evaluation, and evolution; the task manager handles execution."
> — README.md

### Three problems it solves

1. **No way to specify subjective trade-offs** — Agency makes trade-off preferences explicit through structured trade-off configurations, based on the Boris methodology.
2. **No inter-deployment memory** — Agency records structured performance data across deployments, building a track record for each primitive and composition.
3. **No controlled evolution** — Agency provides mechanisms for mutation, recombination, and selection of agent components under human-defined fitness criteria.

### Architecture

- **FastAPI server** (`agency serve`) — exposes REST API + MCP stdio server
- **SQLite database** with WAL mode, `sqlite-vec` for vector similarity search
- **Ed25519 JWT auth** — asymmetric signing for all tokens
- **Embedding model** — `sentence-transformers` for semantic primitive matching
- **CLI** (`agency` command) — init, serve, project management, token management, task commands, MCP server

### Key abstractions

| Concept | Description |
|---------|-------------|
| **Role components** | Individual capabilities an agent brings to a task |
| **Desired outcomes** | What success looks like, specific enough for an evaluator to grade against |
| **Trade-off configurations** | Acceptable and unacceptable trade-offs governing how work is done |
| **Agents** | Compositions of role components + desired outcomes + trade-off configs |
| **Actor-agents** | Agents assigned to and actively performing a specific task |
| **Evaluators** | Specialised agents that grade task output against desired outcomes |
| **Primitives** | The three types above (role components, desired outcomes, trade-off configs) — each with quality scores, domain tags, content hashes, embeddings |

### Self-similar system

All special-type agents (assigner, evaluator, evolver, agent creator) are first-class agents governed by the same primitive structure. None are privileged system components.

---

## 2. Integration Documents (docs/integrations/)

There are **4 files** in `docs/integrations/`:

### 2.1 `caller-protocol.md` (6,144 bytes)

Defines the **assign → execute → evaluate** contract between Agency and any requester:

1. **Assign** — send task descriptions, receive `rendered_prompt` + `agency_task_id`
2. **Execute** — adopt the rendered prompt as operating instructions and do the work
3. **Get evaluator** — fetch evaluation criteria + single-use callback JWT
4. **Evaluate** — follow evaluator prompt to assess output
5. **Submit evaluation** — record structured evaluation against the primitive composition

Four requester types are defined:

| Type | Interface | Token file |
|------|-----------|------------|
| MCP (Claude Code) | MCP tools via stdio | `~/.agency-mcp-token` |
| CLI | `agency task` commands | `~/.agency-cli-token` |
| Superpowers | MCP tools + skill orchestration | `~/.agency-superpowers-token` |
| **wg** | **HTTP API via shell scripts** | `~/.agency-wg-token` |

### 2.2 `using agency as an MCP with claude code.md` (9,573 bytes)

The primary reference doc. Covers:
- 6 MCP tools: `agency_assign`, `agency_evaluator`, `agency_submit_evaluation`, `agency_list_projects`, `agency_create_project`, `agency_status`
- MCP server registration in `~/.claude.json`
- Full parameter/response schemas for each tool
- Token management, project ID resolution, troubleshooting

### 2.3 `using agency with superpowers.md` (5,597 bytes)

Explains how Agency fits into the Superpowers workflow:
- Superpowers handles orchestration (brainstorming, planning, dispatch)
- Agency handles composition (selecting primitives, composing agents, recording evaluations)
- Workflow: brainstorm → assign → dispatch subagents with Agency prompts → evaluate → review

### 2.4 `using agency with wg.md` (5,874 bytes)

**wg is explicitly supported as a first-class integration.** This doc covers:
- Prerequisites: Agency v1.2.2+, `agency serve` running, wg token
- Two translator scripts in `translators/wg/`
- Environment variables (`AGENCY_PROJECT_ID`, `WG_TASK_ID`, `WG_AGENT_ID`, `WG_MODEL`)
- Batch assignment workflow
- CLI-based executor alternative (v1.2.2)
- Comparison table: MCP vs CLI vs wg integration

---

## 3. Recent Activity (Last 40 Commits)

### Version timeline
- **v1.0.0** — initial implementation (PR #1)
- **v1.1.0** — persistent projects/tasks, batch assignment, Ed25519 auth groundwork (PR #2)
- **v1.2.0** — MCP integration, Ed25519 auth, token management, two-phase init wizard, config hierarchy, attribution (PR #3)
- **v1.2.1** — documentation rewrites, README overhaul, 6 MCP tools documented
- **v1.2.2** — CLI task commands, shared client module, GET /tasks endpoint, `error_type`, `task_ids` summary block, caller protocol docs, integration guides (current)

### Recent trajectory (most recent commits first)

1. **Primitives expansion** — Adding domain-specific primitives (strategy/consulting, trading, meta-evaluation, iterative comparative testing). The `starter.csv` has grown from 113 → 189 → 272 primitives across recent sessions.

2. **CLI task commands** (v1.2.2) — `agency task {assign,evaluator,submit,get}` for shell/script-based workflows, reducing dependency on MCP for automation.

3. **Documentation overhaul** — Caller protocol doc, integration guide rewrites, specification updates for v1.2.2.

4. **Agent reuse primitives** — New primitives for composition caching, task-agent similarity matching, identity preservation across sessions (preparing for v1.2.3).

5. **Dependency wave planning** — New primitives for structured implementation planning (decompose-plan-into-dependency-waves, lock-interface-schemas-before-implementation).

### Key observation: Vaughn is actively growing the primitive store

The `primitives/` directory contains 11 CSV files — `starter.csv` (current) plus 10 dated backups. This shows a pattern of rapid primitive accumulation, with new primitives being added from real project sessions (trading, PRD workflows, reportage/synthesis).

---

## 4. wg Relationship

### Does Agency already use wg?

**No .wg/ directory exists** in the Agency repo. Agency does not use wg internally for its own task management.

However, **Agency explicitly supports wg as an integration target**:

1. **Dedicated integration doc**: `docs/integrations/using agency with wg.md`
2. **Translator scripts**: `translators/wg/agency-assign-wg` (batch assignment) and `translators/wg/agency-wg-executor.sh` (per-task executor)
3. **Dedicated token type**: `~/.agency-wg-token` with `client_id: wg`
4. **README routing table**: wg listed alongside Claude Code and Superpowers as a first-class integration

### How the integration works today

The current integration is **shell-script-based HTTP**, operating outside of wg's coordinator:

1. `agency-assign-wg` script:
   - Lists all open wg tasks (`wg list --status open --json`)
   - Sends them as a batch to Agency's `POST /projects/{project_id}/assign`
   - Stores rendered prompts in `.wg/agency-prompts/{task_id}.prompt`
   - Stores `agency_task_id` mappings in `.wg/agency-prompts/{task_id}.task_id`
   - Sets the Agency executor on each task (`wg exec <task> --set "agency-wg-executor.sh"`)

2. `agency-wg-executor.sh` script:
   - Reads stored prompt and agency task ID
   - Runs `claude --print` with the rendered prompt
   - On success: marks task done, fetches evaluator, runs evaluation, submits
   - On failure: marks task failed
   - Includes heartbeat loop (90s interval)

### How the agent/task model relates to wg's

| Concept | Agency | wg |
|---------|--------|-----------|
| **Agent composition** | role components + desired outcomes + trade-off configs, composed via embedding similarity | role + tradeoff pair, bound to tasks via `wg assign` |
| **Primitives** | Stored in SQLite with embeddings, quality scores, content hashes | Stored in `.wg/agency/` as YAML/JSON files |
| **Assignment** | Semantic similarity matching (embedding vector search) | LLM-based or forced assignment |
| **Evaluation** | Structured: evaluator prompt → evaluation text → score submission via JWT-protected API | Four-dimensional scoring + FLIP (fidelity via latent intent probing) |
| **Evolution** | Random perturbation mutation + LLM variation + selection of best variant | Uses performance data to create/retire roles and tradeoffs |
| **Task identity** | UUID v7 (`agency_task_id`) + optional `external_id` | Kebab-case string IDs |
| **Task lifecycle** | Simple: assigned → evaluated | Rich: open → in-progress → done/failed/abandoned/blocked/waiting/pending-validation |
| **Execution** | Does NOT execute — returns prompts to the requester | Full execution: coordinator spawns agents, git worktree isolation |

---

## 5. Integration Surface Areas

### 5.1 Shared concepts with direct mapping

| Concept | Agency location | wg location |
|---------|----------------|-------------------|
| Role components | `src/agency/db/primitives.py`, `primitives/starter.csv` | `src/agency/types.rs` (Role struct) |
| Desired outcomes | `src/agency/db/primitives.py` | `src/agency/types.rs` (DesiredOutcome) |
| Trade-off configs | `src/agency/db/primitives.py` | `src/agency/types.rs` (Tradeoff struct) |
| Agent composition | `src/agency/engine/assigner.py` | `src/agency/mod.rs` (agent creation) |
| Agent rendering | `src/agency/engine/renderer.py` | `src/agency/prompt.rs` |
| Evaluation | `src/agency/engine/evaluator.py`, `src/agency/db/evaluations.py` | `src/commands/evaluate.rs` |
| Evolution | `src/agency/engine/evolver.py` | `src/agency/evolver.rs` |
| Content hashing | `src/agency/utils/hashing.py` | `src/agency/hash.rs` |
| Federation/lineage | Permission blocks, `origin_instance_id`, `parent_content_hash` | `src/agency/lineage.rs` |

### 5.2 Natural integration points

**A. Primitive synchronization**
- Agency's `primitives/starter.csv` contains 272 primitives with quality scores, domain tags, and content hashes
- wg's agency system stores primitives locally in `.wg/agency/`
- Content hashes are used by both systems — potential for deduplication/sync

**B. Assignment enrichment**
- Agency's assignment uses embedding-based semantic similarity search
- wg's assignment is LLM-based or forced
- Integration: wg could call Agency's assign API to get richer, primitive-backed compositions

**C. Evaluation feedback loop**
- Agency records evaluations against specific primitive compositions, building performance data
- wg has its own four-dimensional evaluation + FLIP scoring
- Integration: evaluation results could flow from wg → Agency to improve future compositions

**D. Batch pre-flight**
- Agency's batch assignment endpoint (`POST /projects/{project_id}/assign`) is designed for exactly wg's pattern: assign all open tasks at once before execution begins
- The existing `agency-assign-wg` translator already implements this

**E. Evolution coordination**
- Agency's evolver does random perturbation + LLM variation on compositions
- wg's evolver creates/retires roles and tradeoffs based on performance data
- These could run in concert: Agency evolves primitives, wg evolves task-role assignments

### 5.3 Gaps / friction points

1. **Duplicated agency systems**: Both repos implement role/tradeoff/agent/evaluation/evolution independently. wg's is Rust-native; Agency's is Python with SQLite+embeddings.

2. **Different primitive models**: Agency primitives are text descriptions with embeddings and quality scores. wg primitives are structured types with fields like `skills`, `desired_outcome`, `acceptable_tradeoffs`, `non_negotiable_constraints`.

3. **Different evaluation models**: Agency uses a simple score + text evaluation. wg uses four-dimensional scoring + FLIP.

4. **No live integration**: The current translator scripts are pre-flight (run before execution). There's no runtime integration where Agency influences task execution while it's happening.

---

## 6. API/Protocol/CLI Surface

### REST API endpoints

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/health` | Health check |
| GET | `/status` | Instance status, task progress, primitive counts |
| POST | `/projects/{id}/assign` | Batch assignment (the key endpoint for wg) |
| GET | `/tasks/{task_id}` | Get task with re-rendered prompt (v1.2.2) |
| GET | `/tasks/{task_id}/evaluator` | Get evaluator prompt + callback JWT |
| POST | `/tasks/{task_id}/evaluation` | Submit evaluation |
| GET | `/projects` | List projects |
| POST | `/projects` | Create project |

### MCP tools (6 total)

`agency_assign`, `agency_evaluator`, `agency_submit_evaluation`, `agency_list_projects`, `agency_create_project`, `agency_status`

### CLI commands

```
agency init                          # Two-phase setup wizard
agency serve                         # Start FastAPI server
agency mcp                           # Start MCP stdio server
agency token create/list/revoke      # Token management
agency project list/create/pin       # Project management
agency client setup                  # Instance settings
agency skills install                # Install Claude Code skills
agency primitives install/update     # Primitive management
agency task assign/evaluator/submit/get  # Task lifecycle (v1.2.2)
```

### Authentication

- Ed25519 asymmetric signing (EdDSA algorithm)
- Per-client tokens: `~/.agency-{client_id}-token`
- Single-use callback JWTs for evaluation submission (24h expiry)
- Token revocation via `issued_tokens` table

### Data format

All API responses include:
- `status: "ok"` on success
- `next_step` field with plain-language instructions
- Standard error envelope with `error_type`, `cause`, `fix` fields

---

## 7. Repo Structure Summary

```
agentbureau/agency/
├── README.md                           # Overview, quick start, integration routing
├── specification.md                    # Full spec (815 lines, v1.2.2)
├── agency-status.json                  # Version/status metadata
├── pyproject.toml                      # agency-engine v1.2.2, Python >=3.13
├── LICENSE                             # Elastic License 2.0
├── docs/integrations/
│   ├── caller-protocol.md              # Requester contract
│   ├── using agency as an MCP with claude code.md
│   ├── using agency with superpowers.md
│   └── using agency with wg.md  # ← First-class WG integration
├── primitives/
│   ├── starter.csv                     # 272 primitives (current)
│   └── starter_*.csv                   # 10 dated backups
├── src/agency/
│   ├── api/                            # FastAPI app, middleware, routes
│   │   └── routes/                     # evolution, primitives, projects, status, tasks
│   ├── auth/                           # Ed25519 keypair + JWT
│   ├── cli/                            # Click CLI commands
│   ├── client.py                       # Shared HTTP client (v1.2.2)
│   ├── config/                         # TOML config + hierarchy resolution
│   ├── db/                             # SQLite: compositions, evaluations, migrations, primitives, projects, schema, tasks, templates, tokens
│   ├── engine/                         # Core logic: assigner, evaluator, evolver, permissions, renderer
│   ├── models/                         # Pydantic models
│   ├── skills/                         # Claude Code skills (primitive extraction)
│   ├── status/                         # Status poller
│   └── utils/                          # Email, embedding, errors, hashing, IDs
├── tests/
│   ├── conftest.py
│   ├── integration/                    # 7 test files
│   └── unit/                           # 23 test files
└── translators/
    ├── superpowers/agency-dispatch/    # Superpowers skill
    └── wg/                      # ← wg translators
        ├── agency-assign-wg     # Batch assignment script
        └── agency-wg-executor.sh       # Per-task executor
```

---

## 8. Key Findings Summary

1. **Agency is a composition/evaluation engine, not an execution engine.** It produces agent prompts; task managers execute them. This is a clean separation of concerns.

2. **wg is already a first-class integration target.** Dedicated docs, translator scripts, and token type exist. The `translators/wg/` directory contains working shell scripts.

3. **Agency does NOT use wg internally.** No `.wg/` directory. Agency manages its own tasks through its SQLite database.

4. **The two repos share deeply overlapping concepts** (roles, tradeoffs, desired outcomes, agents, evaluation, evolution) but implement them independently in different languages with different data models.

5. **Active development trajectory**: Vaughn is rapidly expanding the primitive store (272 primitives), adding CLI task commands for automation, and preparing agent reuse primitives (v1.2.3). The project is moving toward richer composition caching and cross-deployment memory.

6. **The existing integration is pre-flight, not runtime**: The translator scripts batch-assign before execution starts. There's no feedback loop during execution, and evaluations currently flow one way (wg executor → Agency API).

7. **Potential for deeper integration**: The shared conceptual surface (primitives, composition, evaluation, evolution) suggests opportunities for bidirectional synchronization, unified evaluation pipelines, and shared primitive stores.
