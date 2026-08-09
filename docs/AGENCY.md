# Agency System

The agency system gives wg agents composable identities. Instead of every agent being a generic assistant, you define **roles** (what an agent does), **tradeoffs** (why it acts that way), and pair them into **agents** that are assigned to tasks, evaluated, and evolved over time.

Agents can be **human or AI**. The difference is the executor: AI agents use `claude` (or similar), human agents use `matrix`, `email`, or `shell`. Both share the same identity model — roles, tradeoffs, capabilities, trust levels, and performance tracking all work uniformly regardless of who (or what) is doing the work.

## Core Concepts

### Role

A role defines **what** an agent does.

| Field | Description | Identity-defining? |
|-------|-------------|--------------------|
| `name` | Human-readable label (e.g. "Programmer") | No |
| `description` | What this role is about | Yes |
| `skills` | List of skill references (see [Skill System](#skill-system)) | Yes |
| `desired_outcome` | What good output looks like | Yes |
| `performance` | Aggregated evaluation scores | No (mutable) |
| `lineage` | Evolutionary history | No (mutable) |
| `default_context_scope` | Default context scope for tasks dispatched with this role (`clean`, `task`, `graph`, `full`) | No (mutable) |
| `default_exec_mode` | Default execution mode for tasks with this role (`full`, `light`, `bare`, `shell`) | No (mutable) |

### Tradeoff

A tradeoff defines **why** an agent acts the way it does.

| Field | Description | Identity-defining? |
|-------|-------------|--------------------|
| `name` | Human-readable label (e.g. "Careful") | No |
| `description` | What this tradeoff prioritizes | Yes |
| `acceptable_tradeoffs` | Compromises the agent may make | Yes |
| `unacceptable_tradeoffs` | Hard constraints the agent must never violate | Yes |
| `performance` | Aggregated evaluation scores | No (mutable) |
| `lineage` | Evolutionary history | No (mutable) |

### Agent

An agent is the **unified identity** in wg — it can represent a human or an AI. For AI agents, it is a named pairing of a role and a tradeoff. For human agents, role and tradeoff are optional.

| Field | Description |
|-------|-------------|
| `name` | Human-readable label |
| `role_id` | Content-hash ID of the role (required for AI, optional for human) |
| `tradeoff_id` | Content-hash ID of the tradeoff (required for AI, optional for human) |
| `capabilities` | Skills/capabilities for task matching (e.g., `rust`, `testing`) |
| `rate` | Hourly rate for cost forecasting |
| `capacity` | Maximum concurrent task capacity |
| `trust_level` | `Verified`, `Provisional` (default), or `Unknown` |
| `contact` | Contact info — email, Matrix ID, etc. (primarily for human agents) |
| `executor` | How this agent receives work: `claude` (default), `matrix`, `email`, `shell` |
| `performance` | Agent-level aggregated evaluation scores |
| `lineage` | Evolutionary history |

The same role paired with different tradeoffs produces different agents. A "Programmer" role with a "Careful" tradeoff produces a different agent than with a "Fast" tradeoff.

Human agents are distinguished by their executor. Agents with a human executor (`matrix`, `email`, `shell`) don't need a role or tradeoff — they're real people who bring their own judgment. AI agents (executor `claude`) require both, because the role and tradeoff are injected into the prompt to shape behavior.

## Content-Hash IDs

Every role, tradeoff, and agent is identified by a **SHA-256 content hash** of its identity-defining fields.

- **Deterministic**: Same content → same ID
- **Deduplication**: Can't create two entities with identical content
- **Immutable identity**: Changing an identity-defining field produces a *new* entity. The old one stays.

| Entity | Hashed fields |
|--------|---------------|
| Role | `skills` + `desired_outcome` + `description` |
| Tradeoff | `acceptable_tradeoffs` + `unacceptable_tradeoffs` + `description` |
| Agent | `role_id` + `tradeoff_id` |

For display, IDs are shown as 8-character prefixes (e.g. `a3f7c21d`). All commands accept unique prefixes.

## The Current Agency Loop

The live loop is: explicitly bind an identity → execute the source task → record
source-bound completion/review evidence → optionally evolve. Assignment may be
selected explicitly with `wg assign --auto`, but the coordinator does not create
blocking assignment or evaluator graph satellites.

```bash
# 1. Bind identity directly (or rank the roster explicitly with --auto)
wg assign my-task a3f7c21d

# 2. Execute and produce receipt-bound completion evidence
wg service start

# 3. Optionally attach one receipt-bound score, inspect, then evolve
wg evaluate run my-task --dry-run
wg evaluate run my-task
wg evaluate show my-task
wg evolve run
```

`wg evaluate run` is a bounded observation-only Pi call over a verified `Done`
task. It re-verifies the terminal observation, generation/attempt/fence,
immutable completion candidate and current publication, then stores one
create-once Agency score. It never changes the task or creates evaluator graph
work. `wg evaluate record` remains the explicit external/manual score path.
`auto_assign`, pre-receipt soft statuses, and synthetic evaluator/FLIP lifecycle
fields remain compatibility-only.

Additional automation options:

```bash
# Auto-place new tasks in the graph (coordinator creates .place-* meta-tasks)
wg config --auto-place true

# Auto-create new primitives when the store needs expansion
wg config --auto-create true
```

When `auto_place` is enabled, the coordinator creates `.place-*` tasks for newly added tasks to determine optimal graph wiring. When `auto_create` is enabled, the coordinator invokes the creator agent to discover and add new primitives after a configurable number of completed tasks (`auto_create_threshold`, default: 20).

## Lifecycle

### 1. Create roles and tradeoffs

```bash
# Create a role
wg role add "Programmer" \
  --outcome "Working, tested code" \
  --skill code-writing \
  --skill testing \
  --description "Writes, tests, and debugs code"

# Create a tradeoff
wg tradeoff add "Careful" \
  --accept "Slow" \
  --accept "Verbose" \
  --reject "Unreliable" \
  --reject "Untested" \
  --description "Prioritizes reliability and correctness above speed"
```

Or seed the built-in starters:

```bash
wg agency init
```

This creates four starter roles (Programmer, Reviewer, Documenter, Architect) and four starter tradeoffs (Careful, Fast, Thorough, Balanced).

### 2. Pair into agents

```bash
# AI agent (role + tradeoff required)
wg agent create "Careful Programmer" --role <role-hash> --tradeoff <tradeoff-hash>

# AI agent with operational fields
wg agent create "Careful Programmer" \
  --role <role-hash> \
  --tradeoff <tradeoff-hash> \
  --capabilities rust,testing \
  --rate 50.0

# Human agent (role + tradeoff optional)
wg agent create "Erik" \
  --executor matrix \
  --contact "@erik:server" \
  --capabilities rust,python,architecture \
  --trust-level verified
```

### 3. Assign to tasks

```bash
wg assign <task-id> <agent-hash>
```

When the service spawns that task, the agent's role and tradeoff are rendered into the prompt as an identity section:

```markdown
# Task Assignment

## Agent Identity

### Role: Programmer
Writes, tests, and debugs code

#### Skills
- code-writing
- testing

#### Desired Outcome
Working, tested code

### Operational Parameters
#### Acceptable Trade-offs
- Slow
- Verbose

#### Non-negotiable Constraints
- Unreliable
- Untested
```

### 4. Terminal observations and scored evaluations

Completion review is recorded directly against the exact source attempt; it is
not an evaluator graph task and does not itself create an Agency score. After a
receipt-backed ordinary completion reaches `Done`, score it explicitly and view
the create-once result:

```bash
wg evaluate run <task-id> --dry-run       # verify route/reasoning/evidence; no mutation
wg evaluate run <task-id>                 # one bounded read-only Pi score
wg evaluate record --task <id> --score 0.8 --source manual
wg evaluate show                          # all evaluations
wg evaluate show --task <task-id>         # filter by task (prefix match)
wg evaluate show --agent <agent-id>       # filter by agent (prefix match)
wg evaluate show --source "outcome:*"     # filter by source (glob)
wg evaluate show --limit 10               # most recent N
```

The evaluator returns one finite overall score plus exactly seven finite 0..1
dimensions: `correctness`, `completeness`, `efficiency`, `style_adherence`,
`downstream_usability`, `coordination_overhead`, and `blocking_impact`. Notes,
response size, evidence previews, process time, tools, and call count are
bounded. The stored envelope also exposes exact evaluator route/reasoning,
Pi-reported usage/cost, evidence digest, completion receipt, and source terminal
observation. Provider/setup failure is loud and leaves both task and score store
neutral. The model response uses the following strict shape before WG adds its
receipt, route, reasoning, and usage metadata:

```json
{
  "overall_score": 0.85,
  "dimensions": {
    "correctness": 0.90,
    "completeness": 0.85,
    "efficiency": 0.80,
    "style_adherence": 0.75,
    "downstream_usability": 0.82,
    "coordination_overhead": 0.78,
    "blocking_impact": 0.91
  },
  "notes": "Bounded evidence-based assessment"
}
```

Scores propagate to three levels:
1. The **agent's** performance record
2. The **role's** performance record (with `tradeoff_id` as context)
3. The **tradeoff's** performance record (with `role_id` as context)

### 4b. Legacy FLIP records — Fidelity via Latent Intent Probing

The following describes the historical metric for interpreting retained
records. New `.flip-*` tasks are not created or dispatched.

FLIP is a **roundtrip intent fidelity** metric that complemented standard evaluation. While evaluation judges *quality* (was the approach good?), FLIP judges *fidelity* (did the output match what was asked?).

#### How it works

FLIP runs in two phases:

1. **Inference phase** (sonnet): An LLM reads *only* the agent's output (logs, artifacts, diffs) and reconstructs what the original task prompt must have been — without seeing the actual task description.
2. **Comparison phase** (haiku): A second LLM compares the inferred prompt to the actual task description, scoring similarity across four dimensions.

The resulting `flip_score` (0.0–1.0) measures whether the agent's output faithfully reflects the task spec. High FLIP = output clearly reflects the task. Low FLIP = agent went off-track.

#### FLIP dimensions

| Dimension | Description |
|-----------|-------------|
| `semantic_match` | How closely the inferred intent matches the actual task |
| `requirement_coverage` | What fraction of requirements are reflected in the output |
| `specificity_match` | Whether the output addresses task-specific details vs. generic work |
| `hallucination_rate` | How much of the output addresses things *not* in the task spec |

#### FLIP vs evaluation

FLIP and evaluation are independent — they measure different things and should not be merged into a single score:

- **High quality + low fidelity** = well-crafted code that doesn't match the spec
- **Low quality + high fidelity** = sloppy code that does what was asked
- **Both high** = ideal
- **Both low** = needs rework

#### Historical low-FLIP verification

Older graphs may contain verdict-bearing `.verify-flip-*` rows. They remain
loadable for exact evidence migration, but the coordinator never creates or
rearms one and never infers source success from its mere status.

#### Reading FLIP history

Use `wg evaluate show --source "flip"` to inspect retained records. The old
manual/automatic FLIP mutation commands are deliberately retired.

#### Per-role model routing

FLIP uses different models for each phase, configured via the model routing system:

| Role | Default model | Rationale |
|------|---------------|-----------|
| `flip_inference` | opus | Creative reconstruction uses the current standard tier |
| `flip_comparison` | haiku | Comparison/scoring is simpler, cost-effective |
| `verification` | opus | Independent verification needs highest capability |

Configure via `[models]` in config.toml:

```toml
[models.flip_inference]
model = "opus"

[models.flip_comparison]
model = "haiku"

[models.verification]
model = "opus"
```

#### Pipeline integration

FLIP fits into the coordinator tick as follows:

```
Phase 4:   Task completes -> .evaluate-* created
           Eval script runs standard eval, then FLIP (non-fatal)
Phase 4.5: If FLIP score < threshold -> .verify-flip-* created
Phase 4.6: Auto-evolve (if enabled)
```

### 5. Evolve

Use performance data to improve the agency:

```bash
wg evolve run                                     # full cycle, all strategies
wg evolve run --strategy mutation --budget 3      # targeted changes
wg evolve run --model opus                        # use specific model
wg evolve run --dry-run                           # preview without applying
wg evolve run --autopoietic                       # enable autopoietic cycle mode
wg evolve run --autopoietic --max-iterations 5    # cycle with custom iteration limit (default: 3)
wg evolve run --autopoietic --cycle-delay 1800    # custom delay between iterations (default: 3600s)
wg evolve run --force-fanout                      # force fan-out mode even with <50 evaluations
wg evolve run --single-shot                       # force legacy single-shot mode even with ≥50 evaluations
```

## CLI Reference

### `wg role`

| Command | Description |
|---------|-------------|
| `wg role add <name> --outcome <text> [--skill <spec>] [-d <text>]` | Create a new role |
| `wg role list` | List all roles |
| `wg role show <id>` | Show details |
| `wg role edit <id>` | Edit in `$EDITOR` (re-hashes on save) |
| `wg role rm <id>` | Delete a role |
| `wg role lineage <id>` | Show evolutionary ancestry |

### `wg tradeoff`

| Command | Description |
|---------|-------------|
| `wg tradeoff add <name> --accept <text> --reject <text> [-d <text>]` | Create a new tradeoff |
| `wg tradeoff list` | List all tradeoffs |
| `wg tradeoff show <id>` | Show details |
| `wg tradeoff edit <id>` | Edit in `$EDITOR` (re-hashes on save) |
| `wg tradeoff rm <id>` | Delete a tradeoff |
| `wg tradeoff lineage <id>` | Show evolutionary ancestry |

### `wg agent`

| Command | Description |
|---------|-------------|
| `wg agent create <name> [OPTIONS]` | Create an agent (see options below) |
| `wg agent list` | List all agents |
| `wg agent show <id>` | Show details with resolved role/tradeoff |
| `wg agent rm <id>` | Remove an agent |
| `wg agent lineage <id>` | Show agent + role + tradeoff ancestry |
| `wg agent performance <id>` | Show evaluation history |

**`wg agent create` options:**

| Option | Description |
|--------|-------------|
| `--role <ID>` | Role ID or prefix (required for AI agents) |
| `--tradeoff <ID>` | Tradeoff ID or prefix (required for AI agents) |
| `--capabilities <SKILLS>` | Comma-separated skills for task matching |
| `--rate <FLOAT>` | Hourly rate for cost tracking |
| `--capacity <FLOAT>` | Maximum concurrent task capacity |
| `--trust-level <LEVEL>` | `verified`, `provisional`, or `unknown` |
| `--contact <STRING>` | Contact info (email, Matrix ID, etc.) |
| `--executor <NAME>` | Executor backend: `claude` (default), `matrix`, `email`, `shell` |
| `--model <MODEL>` | Preferred model (e.g., `opus`, `sonnet`, `haiku`, or full model ID) |
| `--provider <PROVIDER>` | Preferred provider (e.g., `anthropic`, `openrouter`) |

### `wg agency`

| Command | Description |
|---------|-------------|
| `wg agency init` | Seed agency with starter roles and tradeoffs |
| `wg agency stats [--min-evals <N>] [--by-model] [--by-task-type]` | Show agency performance analytics |
| `wg agency create [--model <MODEL>] [--dry-run]` | Invoke the creator agent to discover and add new primitives |
| `wg agency import [CSV_PATH] [--url <URL>] [--upstream] [--dry-run] [--tag <TAG>] [--force] [--check]` | Import Agency starter.csv primitives into wg |
| `wg agency migrate [--dry-run]` | Migrate old-format agency store to primitive+cache format |
| `wg agency deferred` | List pending deferred evolver operations (alias for `wg evolve review list`) |
| `wg agency approve <id>` | Approve a deferred operation (alias for `wg evolve review approve`) |
| `wg agency reject <id>` | Reject a deferred operation (alias for `wg evolve review reject`) |
| `wg agency scan <root-dir> [--max-depth <N>]` | Scan filesystem for agency stores |
| `wg agency remote add\|list\|show\|remove` | Manage named references to other agency stores |
| `wg agency pull <source> [OPTIONS]` | Pull entities from another store |
| `wg agency push <target> [OPTIONS]` | Push local entities to another store |
| `wg agency merge <source1> <source2> ... [OPTIONS]` | Merge entities from multiple stores |

### `wg assign`

```bash
wg assign <task-id> <agent-hash>    # assign agent to task
wg assign <task-id> --auto          # automatically select an agent using LLM
wg assign <task-id> --clear         # remove assignment
```

### `wg evaluate`

```bash
wg evaluate run <done-task> [--dry-run]
wg evaluate record --task <id> --score <0..1> --source <source>
wg evaluate show [<task>] [--task <id>] [--agent <id>] [--source <glob>] [--limit <N>]
wg evaluate rollout status
```

| Subcommand | Description |
|------------|-------------|
| `run` | Re-verify one receipt-backed ordinary `Done`, call the exact configured Pi evaluator once, and create one immutable score |
| `record` | Preserve an explicit external/manual score in the canonical Agency store |
| `show` | View scores plus route/reasoning, usage/cost, receipt, and source terminal observation |
| `rollout status` | Read historical evaluation-rollout compatibility state |

### `wg evolve`

```bash
wg evolve run [--strategy <name>] [--budget <N>] [--model <model>] [--dry-run]
              [--autopoietic] [--max-iterations <N>] [--cycle-delay <secs>]
              [--force-fanout] [--single-shot]
wg evolve apply <synthesis-file> [-o <output>]   # apply a synthesis-result.json from a fan-out run
```

## Skill System

Skills define capabilities attached to a role. Four types of skill references:

### Name (tag-only)

Simple string label. No content, just matching and display.

```bash
wg role add "Coder" --skill rust --skill testing --outcome "Working code"
```

### File

Path to a file containing skill instructions. Supports absolute paths, relative paths, and `~` expansion.

```bash
wg role add "Coder" --skill "coding:file:///home/user/skills/rust-style.md" --outcome "Idiomatic Rust"
```

### Url

URL to fetch skill content from.

```bash
wg role add "Reviewer" --skill "review:https://example.com/checklist.md" --outcome "Review report"
```

### Inline

Skill content embedded directly.

```bash
wg role add "Writer" --skill "tone:inline:Write in a clear, technical style" --outcome "Documentation"
```

### Installing skills

The wg skill can be installed as a Claude Code skill:

```bash
wg skill install     # installs to ~/.claude/skills/wg/
```

Other skill management commands:

```bash
wg skill list                # list all skills used across tasks
wg skill task <task-id>      # show skills for a specific task
wg skill find <skill-name>   # find tasks requiring a specific skill
```

### Resolution

When a task is dispatched with an agent identity, all skill references on the role are resolved:
- `Name` → passes through as-is
- `File` → reads file content
- `Url` → fetches URL content
- `Inline` → uses content directly

Skills that fail to resolve produce a warning but don't block execution.

## Evolution

The evolution system improves agency performance by analyzing evaluation data and proposing changes. It spawns an LLM-powered "evolver agent" that reads performance summaries and proposes structured operations.

### Strategies

| Strategy | Description |
|----------|-------------|
| `mutation` | Modify a single existing role to improve weak dimensions |
| `crossover` | Combine traits from two high-performing roles into a new one |
| `gap-analysis` | Create entirely new roles/tradeoffs for unmet needs |
| `retirement` | Remove consistently poor-performing roles/tradeoffs |
| `tradeoff-tuning` | Adjust trade-offs and constraints on existing tradeoffs |
| `component-mutation` | Mutate individual components (skills, outcomes, tradeoffs) at the primitive level |
| `randomisation` | Randomly compose new roles or agents from existing primitives |
| `bizarre-ideation` | Generate novel primitives via creative/divergent prompting |
| `coordinator` | Evolve coordinator prompt files (`evolved-amendments.md`, `common-patterns.md`) |
| `all` | Use all strategies as appropriate (default) |

### Operations

The evolver outputs structured JSON operations. These span three levels of the agency hierarchy:

**Legacy (role/motivation level):**

| Operation | Effect |
|-----------|--------|
| `create_role` | Creates a new role (typically from gap-analysis) |
| `modify_role` | Mutates or crosses over an existing role into a new one |
| `create_motivation` | Creates a new tradeoff/motivation |
| `modify_motivation` | Tunes an existing tradeoff into a new variant |
| `retire_role` | Retires a poor-performing role (renamed to `.yaml.retired`) |
| `retire_motivation` | Retires a poor-performing tradeoff |

**Primitive-level mutations:**

| Operation | Effect |
|-----------|--------|
| `wording_mutation` | Rewrites description/content of a component, tradeoff, or outcome |
| `component_substitution` | Swaps one component for another in a role |
| `config_add_component` | Adds a component to an existing role |
| `config_remove_component` | Removes a component from an existing role |
| `config_swap_outcome` | Changes which outcome a role targets (deferred for approval) |
| `config_swap_tradeoff` | Changes which tradeoff an agent uses |
| `random_compose_role` | Randomly assembles a role from existing components |
| `random_compose_agent` | Randomly assembles an agent from existing role + tradeoff |
| `bizarre_ideation` | Generates a novel primitive via creative divergent prompting |

**Meta-agent operations (coordinator-level):**

| Operation | Effect |
|-----------|--------|
| `meta_swap_role` | Change which role a meta-agent (assigner/evaluator/evolver) uses |
| `meta_swap_tradeoff` | Change which tradeoff a meta-agent uses |
| `meta_compose_agent` | Compose a new agent for a meta-agent slot from scratch |
| `modify_coordinator_prompt` | Modify mutable coordinator prompt files (`evolved-amendments.md`, `common-patterns.md`) |

### Safety guardrails

- The last remaining role or tradeoff cannot be retired
- Retired entities are preserved as `.yaml.retired` files, not deleted
- `--dry-run` shows the full evolver prompt without making changes
- `--budget` limits the number of operations applied

### Deferred operations

Some operations are too impactful to apply immediately. The evolver automatically defers operations that require human approval:

- **Objective changes** (`config_swap_outcome`, or `wording_mutation` on outcomes with `requires_human_oversight`) — changing a role's target outcome changes what "success" means
- **Bizarre objectives** (`bizarre_ideation` targeting outcomes) — novel outcomes generated via divergent prompting need human review
- **Protected objectives** — outcomes marked with the `requires_human_oversight` flag in their YAML, or referenced by `random_compose_role`
- **Self-mutation** — operations targeting the evolver's own role or tradeoff (creates a review task in the graph rather than a deferred-ops file)

Deferred operations are saved to `.wg/agency/deferred/` and can be managed with:

```bash
wg evolve review list              # view pending deferred operations
wg evolve review approve <id>      # approve and apply a deferred operation
wg evolve review reject <id>       # reject and discard
```

### Coordinator prompt evolution

The evolver can modify the coordinator agent's behavior by writing to mutable prompt files in `.wg/agency/coordinator-prompt/`:

| File | Mutability | Purpose |
|------|-----------|---------|
| `base-system-prompt.md` | Immutable | Core coordinator instructions |
| `behavioral-rules.md` | Immutable | Safety and behavioral constraints |
| `evolved-amendments.md` | **Mutable** | Evolver-written rules and heuristics |
| `common-patterns.md` | **Mutable** | Evolver-written examples and patterns |

The `evolved-amendments.md` file is the primary output of coordinator prompt evolution — the evolver appends learned rules and heuristics based on evaluation data.

### Auto-evolve infrastructure

The coordinator can automatically trigger evolution cycles when sufficient evaluation data accumulates. This is opt-in:

```bash
wg config --auto-evolve true
```

When enabled, the coordinator's Phase 4.6 checks two triggers:

1. **Threshold trigger**: New evaluations since last evolution exceed `evolution_threshold` (default: 10)
2. **Reactive trigger**: Performance has dropped below `evolution_reactive_threshold`

The coordinator creates a `.evolve-*` meta-task that runs `wg evolve run` with the configured budget. A minimum interval (`evolution_interval`, default: 7200 seconds / 2 hours) prevents evolution from running too frequently.

Configuration:

```toml
[agency]
auto_evolve = false              # enable auto-evolution (default: false)
evolution_interval = 7200        # minimum seconds between cycles (default: 2h)
evolution_threshold = 10         # new evals needed to trigger (default: 10)
evolution_budget = 5             # max operations per cycle (default: 5)
```

### Evolver identity and meta-agent configuration

The evolver itself can have an agent identity. Configure meta-agents in config.toml:

```toml
[agency]
evolver_model = "opus"           # model for the evolver agent
evolver_agent = "abc123..."      # content-hash of evolver agent identity
assigner_model = "haiku"         # model for assigner agents
assigner_agent = "def456..."     # content-hash of assigner agent identity
evaluator_agent = "ghi789..."    # historical identity metadata only; no dispatch authority
retention_heuristics = "Retire roles scoring below 0.3 after 10 evaluations"
```

Or via CLI:

```bash
wg config --evolver-model opus --evolver-agent abc123
wg config --assigner-model haiku --assigner-agent def456
wg config --set-model evaluator pi:openrouter:anthropic/claude-sonnet-4.6 \
  --set-reasoning evaluator low
wg config --evaluator-agent ghi789       # compatibility metadata only
wg config --retention-heuristics "Retire roles scoring below 0.3 after 10 evaluations"
```

The evolver prompt includes:
- Performance summaries for all roles and tradeoffs
- Strategy-specific skill documents from `.wg/agency/evolver-skills/`
- The evolver's own identity (if configured)
- References to the assigner, evaluator, and evolver agent hashes
- Retention heuristics (if configured)

### Evolver skills

Strategy-specific guidance documents live in `.wg/agency/evolver-skills/`:

- `role-mutation.md` — procedures for improving a single role
- `role-crossover.md` — procedures for combining two roles
- `gap-analysis.md` — procedures for identifying missing capabilities
- `retirement.md` — procedures for removing underperformers
- `motivation-tuning.md` — procedures for adjusting trade-offs
- `component-mutation.md` — procedures for mutating individual primitives
- `randomisation.md` — procedures for random composition
- `bizarre-ideation.md` — procedures for divergent creative generation

## Performance Tracking

### Historical evaluation flow

Pre-receipt installations may already contain four-dimension evaluation YAML
and aggregated performance records. These remain readable for history and the
opt-in evolver. Current completion records source-bound review evidence instead;
it does not create a synthetic evaluation row or mutate these aggregates through
`wg evaluate run/record`.

### Performance records

Each entity maintains a `PerformanceRecord`:

```yaml
performance:
  task_count: 5
  avg_score: 0.82
  evaluations:
    - score: 0.85
      task_id: "implement-feature-x"
      timestamp: "2026-01-15T10:30:00Z"
      context_id: "<tradeoff_id>"  # on roles; role_id on tradeoffs
```

The `context_id` cross-references create a performance matrix: how a role performs with different tradeoffs, and vice versa. `wg agency stats` uses this to build a synergy matrix.

### Trend indicators

`wg agency stats` computes trends by comparing first and second halves of recent scores:

- **up** — second half averages >0.03 higher
- **down** — second half averages >0.03 lower
- **flat** — difference within 0.03

## Lineage

Every role, tradeoff, and agent tracks evolutionary history:

```yaml
lineage:
  parent_ids:
    - "a1b2c3d4..."   # single parent for mutation, two for crossover
  generation: 2
  created_by: "evolver-run-20260115-143022"
  created_at: "2026-01-15T14:30:22Z"
```

| Field | Description |
|-------|-------------|
| `parent_ids` | Empty for manual, single for mutation, multiple for crossover |
| `generation` | 0 for manual, incrementing for evolved |
| `created_by` | `"human"` for manual, `"evolver-{run_id}"` for evolved |
| `created_at` | Timestamp |

### Viewing lineage

```bash
wg role lineage <id>
wg tradeoff lineage <id>
wg agent lineage <id>        # shows agent + role + tradeoff ancestry
```

## Storage Layout

```
.wg/agency/
├── primitives/
│   ├── components/              # Skill components (atomic capabilities)
│   │   └── <sha256>.yaml
│   ├── outcomes/                # Desired outcomes
│   │   └── <sha256>.yaml
│   └── tradeoffs/               # Tradeoff definitions
│       ├── <sha256>.yaml
│       └── <sha256>.yaml.retired
├── cache/
│   ├── roles/                   # Composed roles (component_ids + outcome_id)
│   │   ├── <sha256>.yaml
│   │   └── <sha256>.yaml.retired
│   └── agents/                  # Agent definitions (role + tradeoff pairs)
│       └── <sha256>.yaml
├── assignments/                 # Task-to-agent assignment records
│   └── <task-id>.yaml
├── evaluations/
│   ├── eval-<task-id>-<timestamp>.json   # Standard evaluations (source: "llm")
│   └── flip-<task-id>-<timestamp>.json   # FLIP evaluations (source: "flip")
├── org-evaluations/             # Organization-level evaluations
│   └── org-eval-<task-id>-<timestamp>.json
├── evolution_runs/              # Evolution run history
│   └── evo-<date>.json
├── evolver-skills/              # Strategy-specific guidance documents
│   ├── role-mutation.md
│   ├── role-crossover.md
│   ├── gap-analysis.md
│   ├── retirement.md
│   ├── motivation-tuning.md
│   ├── component-mutation.md
│   ├── randomisation.md
│   └── bizarre-ideation.md
├── coordinator-prompt/          # Coordinator prompt files
│   ├── base-system-prompt.md    # (immutable)
│   ├── behavioral-rules.md      # (immutable)
│   ├── evolved-amendments.md    # (mutable — evolver-written rules)
│   └── common-patterns.md       # (mutable — evolver-written examples)
├── deferred/                    # Deferred evolution operations awaiting approval
│   └── <op-id>.json
└── creator_state.json           # Auto-create tracking (last count, timestamp)
```

Roles, tradeoffs, and agents are stored as YAML. Evaluations are stored as JSON. All filenames are based on the entity's content-hash ID.

## Federation

Federation lets you share agency entities (roles, tradeoffs, agents) and their performance data across wg projects. Because entities use content-hash IDs, the same role in two projects has the same ID — pull/push merges performance records automatically.

### Remotes

Named references to other agency stores:

```bash
wg agency remote add <name> <path>       # add a named remote
wg agency remote list                     # list all configured remotes
wg agency remote show <name>             # show remote details and entity counts
wg agency remote remove <name>           # remove a named remote
```

### Discovering stores

Scan a directory tree for wg agency stores:

```bash
wg agency scan <root-dir>                 # find all .wg/agency/ stores
wg agency scan <root-dir> --max-depth 5   # limit recursion depth (default: 10)
```

### Pull, push, and merge

```bash
# Pull entities from another store into local
wg agency pull <source>                              # pull all from path or named remote
wg agency pull <source> --type role                  # only roles
wg agency pull <source> --entity <id-prefix>         # specific entities
wg agency pull <source> --dry-run                    # preview without writing
wg agency pull <source> --no-performance             # definitions only, skip scores
wg agency pull <source> --no-evaluations             # skip evaluation JSON files
wg agency pull <source> --global                     # pull into ~/.wg/agency/

# Push local entities to another store
wg agency push <target>                              # push all to path or named remote
wg agency push <target> --type tradeoff            # only tradeoffs
wg agency push <target> --entity <id-prefix>         # specific entities
wg agency push <target> --dry-run                    # preview without writing
wg agency push <target> --global                     # push from ~/.wg/agency/

# Merge multiple stores
wg agency merge <source1> <source2> ...              # merge into local project
wg agency merge <source1> <source2> --into <path>    # merge into specific target
wg agency merge <source1> <source2> --dry-run        # preview
```

For the full federation design (conflict resolution, global store, trust propagation), see `docs/design/agency-federation.md`.

## Provider Profiles

Provider profiles (`wg profile`) define model tier presets that affect how agency meta-agents resolve their models. When a profile is active, tier names like `opus`, `sonnet`, and `haiku` in the agency configuration map to provider-specific model IDs.

```bash
wg profile list                   # list available profiles
wg profile show                   # show current profile and resolved model mappings
wg profile set <name>             # activate a profile
wg profile refresh                # refresh model data and recompute rankings
```

For example, with an OpenRouter profile active, `evaluator_model = "haiku"` resolves to the best-available haiku-tier model on OpenRouter rather than the default Anthropic endpoint.

## Configuration Reference

```toml
[agency]
auto_evaluate = false              # enable source-bound completion review/observation
auto_assign = false                # legacy inert key; synthetic assignment tasks are retired
auto_place = false                 # placement policy for explicit `wg assign --auto`
auto_create = false                # auto-invoke creator agent for new primitives
auto_create_threshold = 20         # completed tasks before triggering creator again
auto_triage = false                # auto-triage dead agents before respawning
assigner_model = "haiku"           # model for assigner agents
evaluator_model = "haiku"          # model for evaluator agents
evolver_model = "opus"             # model for evolver agents
creator_model = ""                 # model for agent-creator meta-tasks
triage_model = "haiku"             # model for triage (default: haiku)
assigner_agent = ""                # content-hash of assigner agent
evaluator_agent = ""               # content-hash of evaluator agent
evolver_agent = ""                 # content-hash of evolver agent
creator_agent = ""                 # content-hash of agent-creator agent
placer_agent = ""                  # content-hash of placer agent
retention_heuristics = ""          # prose policy for retirement decisions
triage_timeout = 30                # timeout in seconds for triage calls
triage_max_log_bytes = 50000       # max bytes of agent log to read for triage
# Evaluation gate settings
eval_gate_threshold = 0.7         # evaluations below this score reject the task (None = disabled)
eval_gate_all = false             # apply eval gate to ALL tasks, not just those tagged 'eval-gate'

# FLIP settings
flip_enabled = true                # enable FLIP fidelity evaluation (default: true)
flip_inference_model = "sonnet"   # model for FLIP inference phase (reconstructing prompt)
flip_comparison_model = "haiku"   # model for FLIP comparison phase (scoring similarity)
flip_verification_threshold = 0.7  # FLIP score below this triggers verification
flip_verification_model = "opus"  # model for FLIP-triggered verification agents

# Auto-evolve settings
auto_evolve = false                # enable automatic evolution cycles
evolution_interval = 7200          # minimum seconds between cycles (default: 2h)
evolution_threshold = 10           # new evals needed to trigger (default: 10)
evolution_budget = 5               # max operations per auto-evolve cycle
evolution_reactive_threshold = 0.4 # avg score below this triggers reactive evolution

# Learning/exploration settings
exploration_interval = 20         # force learning assignment every N tasks (0 = disabled)
cache_population_threshold = 0.8  # score threshold for populating composition cache
ucb_exploration_constant = 1.414  # UCB exploration constant C for primitive selection
novelty_bonus_multiplier = 1.5    # multiplier for low-attractor-weight primitives
bizarre_ideation_interval = 10    # force bizarre ideation every N learning assignments (0 = disabled)

# Agency server integration
agency_server_url = ""            # URL of Agency server for evaluation feedback (empty = disabled)
agency_token_path = ""            # path to file containing Agency API token
agency_project_id = ""            # project ID on the Agency server
assignment_source = ""            # default assignment source label (e.g. "native", "agency")
upstream_url = ""                 # URL for upstream agency bureau CSV (used by `wg agency import --upstream`)

# Per-role model routing (alternative to legacy model fields above)
[models.flip_inference]
model = "sonnet"                   # model for FLIP inference phase

[models.flip_comparison]
model = "haiku"                    # model for FLIP comparison phase

[models.verification]
model = "opus"                     # model for FLIP-triggered verification
```

```bash
# CLI equivalents for live opt-in operations
wg config --auto-place true
wg config --auto-create true
wg config --auto-triage true
wg config --assigner-model haiku
wg config --evolver-model opus
wg config --creator-model haiku
wg config --triage-model haiku
wg config --assigner-agent abc123
wg config --evaluator-agent def456
wg config --evolver-agent ghi789
wg config --creator-agent abc123
wg config --retention-heuristics "Retire roles scoring below 0.3 after 10 evaluations"
wg config --triage-timeout 30
wg config --triage-max-log-bytes 50000
wg config --eval-gate-threshold 0.7
wg config --eval-gate-all true
wg config --flip-enabled true
wg config --flip-inference-model sonnet
wg config --flip-comparison-model haiku
wg config --flip-verification-threshold 0.7
wg config --flip-verification-model opus
wg config --auto-evolve true
```
