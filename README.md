# WorkGraph

**WorkGraph is an operating surface for human/AI work.**

It records what needs doing, who or what claimed it, what blocked it, what
evidence was produced, where judgment entered, what failed, what was retried,
and how the work changed over time.

Agents can come and go. **The graph remains.**

WorkGraph is not an agent framework. It is the substrate beneath agents: a
persistent, inspectable graph where humans and machines coordinate through
tasks, dependencies, claims, evidence, artifacts, and history.

> **Most AI systems center the agent. WorkGraph centers the work.**

## Why this matters

AI agents are increasingly capable, but their work is often ephemeral: hidden in
chat logs, scattered across branches, trapped in prompt histories, or lost when
a process exits. Real work needs continuity. WorkGraph makes work durable —
something that can be inspected, resumed, audited, and shared independently of
whatever process happens to be running right now.

The agent that started a task may not be the one that finishes it. A human may
take over. A different model may retry. The original session may be gone. None
of that should matter to the work itself. The graph is the unit of memory; the
agent is one of many things that may touch it.

**Agents are transient. Work persists.**

## What WorkGraph gives you

- **Persistent task graph** — tasks, dependencies, status, and metadata stored
  as plain JSONL on disk. Git-friendly, human-readable, easy to inspect.
- **Claims and handoffs** — any agent (human or AI) can claim work; if it dies,
  another can pick up from where it left off.
- **Execution history** — every state transition, log line, and message is
  recorded. Nothing important is lost when a process exits.
- **Evidence and artifacts** — files produced by tasks are tracked alongside
  the tasks themselves, so downstream work can find the inputs it needs.
- **Human judgment points** — verification, approval, and rejection are
  first-class operations, not afterthoughts.
- **Agent continuity** — composable identities (role + tradeoff) outlive the
  individual processes that embody them, and improve via feedback over time.

## Five minutes from zero to running

```bash
cargo install --path .            # build the wg binary
cd your-project
wg init                           # set up .wg/ in your project
wg add 'Design API'
wg add 'Implement backend' --after design-api
wg service start                  # daemon spawns agents on ready tasks
wg tui                            # interactive dashboard
```

That's the whole loop: declare work, let the service dispatch it, watch the
graph evolve. Everything else is detail.

## Core concepts

**Tasks** are units of work. They have a status (`open`, `in-progress`, `done`,
`failed`, `blocked`, `pending-validation`, `waiting`, `abandoned`), a
description, optional acceptance criteria, and edges to other tasks they depend
on. Tasks may carry per-task overrides (model, execution mode, visibility,
context scope).

**Dependencies** (`after` edges) form the graph. A task is waiting until its
predecessors reach a terminal status. Cycles are allowed and represent
repeating workflows (write → review → revise) — they're configured with a
maximum iteration count and an optional convergence signal.

**Agents** are humans or AIs that do work. They claim tasks, log progress,
record artifacts, and either complete the work or hand it back. Agents are
identified, tracked, and can be killed and replaced without losing the work
itself.

**Claims** are how an agent says "I'm working on this." A claim is just a
record on the task. If the agent dies, the claim is released and another
agent can take over.

**Traces** record everything: state transitions, logs, messages, artifacts,
evaluations. The trace is the project's organizational memory and the basis
for sharing, replay, and learning.

**Verification** is built into the lifecycle. Tasks include a `## Validation`
section in their description listing acceptance criteria. When work is marked
done, an evaluator scores the output against those criteria. Low confidence
triggers verification by a stronger model.

**Agency** is the system of composable identities — a *role* (what an agent
does) paired with a *tradeoff* (why it acts that way). Agencies are evaluated
and evolve over time based on performance data, so the population of available
identities improves with use.

## How it's used

- **Solo with one AI**: declare tasks, start the service, let one agent at a
  time work through them. The graph survives sessions; you can return tomorrow
  and pick up where you left off.
- **Many AIs in parallel**: the service spawns up to `max_agents` workers, each
  in its own git worktree. They don't step on each other. Dependencies enforce
  ordering where it matters.
- **Mixed human + AI**: humans claim what they want to do; AIs claim what's
  left. Handoffs at any boundary work the same way.
- **Reflexive use**: a graph can describe its own evolution. WorkGraph itself
  is built using WorkGraph — agents extend it, evaluate the extensions, and
  evolve the substrate. The graph is the memory of its own construction.

## Storage

Everything lives in `.wg/`:

```
.wg/
  graph.jsonl         # task graph (one JSON object per line)
  config.toml         # configuration
  agency/             # roles, tradeoffs, agents, evaluations
  service/            # runtime state (daemon PID, registry, logs)
  functions/          # workflow templates
```

Plain text. Diffable. Inspectable without the tool. If `wg` disappeared
tomorrow, the work would still be there.

## Install

```bash
git clone https://github.com/graphwork/workgraph
cd workgraph
cargo install --path .
```

Or directly:

```bash
cargo install --git https://github.com/graphwork/workgraph
```

Then `wg --help` and `wg quickstart` to orient yourself.

## Documentation

- **[docs/GUIDE.md](docs/GUIDE.md)** — operator manual: configuration, the
  service, agent management, models, TUI, troubleshooting
- **[docs/AGENT-GUIDE.md](docs/AGENT-GUIDE.md)** — how agents should use
  WorkGraph
- **[docs/AGENT-SERVICE.md](docs/AGENT-SERVICE.md)** — service architecture
  and coordinator lifecycle
- **[docs/AGENCY.md](docs/AGENCY.md)** — agency system: roles, tradeoffs,
  evaluation, evolution, federation
- **[docs/COMMANDS.md](docs/COMMANDS.md)** — full command reference
- **[docs/LOGGING.md](docs/LOGGING.md)** — provenance and the operations log
- **[docs/WORKTREE-ISOLATION.md](docs/WORKTREE-ISOLATION.md)** — how parallel
  agents avoid file conflicts
- **[docs/DEV.md](docs/DEV.md)** — developer notes
- **[docs/KEY_DOCS.md](docs/KEY_DOCS.md)** — full documentation index

## Using with AI coding assistants

WorkGraph ships a skill that teaches AI assistants to use the service as a
coordinator rather than working ad-hoc. For Claude Code:

```bash
wg skill install        # ~/.claude/skills/wg/
```

Add to your `CLAUDE.md` (or `~/.claude/CLAUDE.md` for global use):

```markdown
Use workgraph for task management. Run `wg quickstart` at session start.
Use `wg service start` to dispatch work — do not manually claim tasks.
```

Other agent harnesses (Codex CLI, OpenCode, etc.) read `AGENTS.md` — the same
two lines work there. See [docs/GUIDE.md](docs/GUIDE.md#using-with-ai-coding-assistants)
for the longer form.

## License

MIT
