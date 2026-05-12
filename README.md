# wg

**The work OS for human/AI organizations.**

Agents can come and go. The graph remains.

![wg TUI showing tasks, agents, claims, logs, and dependencies](docs/assets/wg-tui.gif)

wg records what needs doing, who or what claimed it, what blocked it,
what evidence was produced, where judgment entered, what failed, what was
retried, and how the work changed over time.

Launch the operating surface:

```bash
wg tui
```

> **Most AI systems center the agent. wg centers the work.**

## The bottleneck is validation

AI can generate more work than humans can inspect.

wg exists because the hard problem is no longer only execution. It
is knowing what was done, what failed, what evidence exists, where judgment
entered, and how the organization should respond.

Generation, evidence, validation, repair, and human judgment stay in the
same durable structure — so judgment can catch up to generation instead of
being flattened by it.

## What wg gives you

- **Persistent task graph** — tasks, dependencies, status, and metadata
  stored as plain JSONL on disk. Git-friendly, human-readable, easy to
  inspect.
- **Claims and handoffs** — any agent (human or AI) can claim work; if it
  dies, another can pick up from where it left off.
- **Execution history** — every state transition, log line, and message is
  recorded. Nothing important is lost when a process exits.
- **Evidence and artifacts** — files produced by tasks are tracked alongside
  the tasks themselves, so downstream work can find the inputs it needs.
- **Human judgment points** — verification, approval, and rejection are
  first-class operations, not afterthoughts.
- **Agent continuity** — composable identities (role + tradeoff) outlive
  the individual processes that embody them, and improve via feedback over
  time.

## What wg is not

wg is not primarily a chatbot, an agent benchmark harness, a
project-management app, or an agent orchestration framework (LangGraph,
CrewAI, AutoGen).

Those categories center messages, scores, tickets, or agents.

wg centers **answerable work**: tasks with dependencies, claims,
evidence, validation, failures, handoffs, artifacts, and history.

## Theory-led design

wg was not designed by starting with agents and adding orchestration.

It started from a theory of organizations: work needs decomposition,
dependency, role, motivation, coordination, evaluation, memory, and
adaptation.

The implementation maps those organizational primitives into a working
system. Read [the theory](https://graphwork.github.io/theory/) — it is
foundational, not optional, reading.

## The proof surface

[Poietic PBC](https://poietic.life/) was formed, organized, and grant-funded
through wg. These are not demos. They are public traces of real
institutional work:

- **Company formation** — incorporation, structure, governance
- **Grant drafting and submission** — the grant referenced on poietic.life
  was drafted, edited, and submitted through the graph
- **Scientific analysis** — research coordination and findings
- **Website and theory development** — the Poietic mission site, the
  wg theory pages, even copy edits to this repo

> **The company is not a wrapper around the product. The company is an
> output of the product.**

## Start the OS

```bash
cargo install --git https://github.com/graphwork/wg
wg init
wg tui
```

### Pick your executor

```bash
wg init --route claude-cli                                  # Claude (default)
wg init --route codex-cli                                   # Codex
wg init -m nex:qwen3-coder -e https://your-endpoint:8080    # any OpenAI-compatible
```

### Then let agents work

```bash
wg service start
wg tui
```

The loop: declare work, let the service dispatch it, watch the graph evolve.

## Review this project in 10 minutes

1. Read the [Poietic mission](https://poietic.life/): why legible human/AI
   collaboration matters.
2. Inspect a public graph: incorporation, grant writing, research, or this
   website's own development.
3. Read [the theory](https://graphwork.github.io/theory/): how tasks, roles,
   evaluations, traces, and evolution form a cybernetic organization.
4. Install wg only after you understand the system it instantiates.

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

## Documentation

- **[docs/GUIDE.md](docs/GUIDE.md)** — operator manual: configuration, the
  service, agent management, models, TUI, troubleshooting, AI assistants
- **[docs/AGENT-GUIDE.md](docs/AGENT-GUIDE.md)** — how agents should use
  wg
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

---

> **Watch the organization think.**

## License

MIT
