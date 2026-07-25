# WG

**The work OS for human/AI organizations.**

WG stands for works good.

Agents can come and go. The graph remains.

![WG TUI showing tasks, agents, claims, logs, and dependencies](docs/assets/wg-tui.gif)

WG records what needs doing, who or what claimed it, what blocked it,
what evidence was produced, where judgment entered, what failed, what was
retried, and how the work changed over time.

Launch the operating surface:

```bash
wg tui
```

> **Most AI systems center the agent. WG centers the work.**

## The bottleneck is validation

AI can generate more work than humans can inspect.

WG exists because the hard problem is no longer only execution. It
is knowing what was done, what failed, what evidence exists, where judgment
entered, and how the organization should respond.

Generation, evidence, validation, repair, and human judgment stay in the
same durable structure — so judgment can catch up to generation instead of
being flattened by it.

## What WG gives you

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

## What WG is not

WG is not primarily a chatbot, an agent benchmark harness, a
project-management app, or an agent orchestration framework (LangGraph,
CrewAI, AutoGen).

Those categories center messages, scores, tickets, or agents.

WG centers **answerable work**: tasks with dependencies, claims,
evidence, validation, failures, handoffs, artifacts, and history.

## Theory-led design

WG was not designed by starting with agents and adding orchestration.

It started from a theory of organizations: work needs decomposition,
dependency, role, motivation, coordination, evaluation, memory, and
adaptation.

The implementation maps those organizational primitives into a working
system. Read [the theory](https://graphwork.github.io/theory/) — it is
foundational, not optional, reading.

## The proof surface

[Poietic PBC](https://poietic.life/) was formed, organized, and grant-funded
through WG. These are not demos. They are public traces of real
institutional work:

- **Company formation** — incorporation, structure, governance
- **Grant drafting and submission** — the grant referenced on poietic.life
  was drafted, edited, and submitted through the graph
- **Scientific analysis** — research coordination and findings
- **Website and theory development** — the Poietic mission site, the
  WG theory pages, even copy edits to this repo

> **The company is not a wrapper around the product. The company is an
> output of the product.**

## Start the OS

The fastest first look is graph-only and needs no credentials:

```bash
cargo install --git https://github.com/graphwork/wg
wg init
wg tui
```

`wg init` and `wg tui` are **non-mutating** — they never select a model,
authenticate, install packages, or start a service. To actually drive LLM
work you select the **Pi** model plane explicitly.

### Quickstart: drive a free OpenRouter model through Pi

Pi is WorksGood's sole model plane: Pi owns provider login, model discovery,
endpoints, availability, and cost; WG owns the task graph plus exact per-role
`pi:<provider>:<model>` routes. The full, verified, copy-paste path —
install WG and Pi, authenticate with OpenRouter, discover and validate a
current free model, install the `pi-worksgood` integration, optionally add
web plugins, select the route, and open the TUI — lives in
[**docs/quickstart-pi-openrouter.md**](docs/quickstart-pi-openrouter.md)
(and the same path, as a styled standalone page, ships in
[`website/quickstart-pi-openrouter.html`](website/quickstart-pi-openrouter.html)
for the graphwork.github.io site).
The spine:

```bash
# 1. install WG (needs Rust) and Pi (needs Node 20+)
cargo install --git https://github.com/graphwork/wg
npm install -g --ignore-scripts @earendil-works/pi-coding-agent

# 2. authenticate Pi with OpenRouter (once, in Pi — WG never sees the key)
pi
/login openrouter          # -> "Sign in with OpenRouter" (PKCE OAuth)

# 3. discover a CURRENT free model and validate it works in Pi
pi --list-models ":free"
pi --model "openrouter/nvidia/nemotron-3-ultra-550b-a55b:free" -p "Reply OK"

# 4. install the WorksGood Pi integration (pi-worksgood, embedded in wg)
wg pi-plugin install && wg pi-plugin status

# 5. initialize a project and select the Pi route (project-scoped; global untouched)
wg init
wg profile init-starters
wg profile pi --strong "pi:openrouter/nvidia/nemotron-3-ultra-550b-a55b:free" \
              --weak   "pi:openrouter/nvidia/nemotron-3-ultra-550b-a55b:free"
wg profile select pi
wg config --models         # every role shows handler=pi, exact route, reasoning

# 6. start the service and open the operating surface
wg service start
wg tui
```

Replace the model id with whatever `pi --list-models ":free"` currently
returns — free-model availability, limits, context, and tool support change
frequently. See the [full quickstart](docs/quickstart-pi-openrouter.md) for
macOS/Termux notes, optional `pi-web-access` / `pi-agent-browser-native`
plugins, the `pi-worksgood`/hermetic details, and troubleshooting
(PATH, `Failed to run wg`, missing Pi/plugin/model/auth). Legacy WG model
catalogs/endpoints remain migration-only and never authorize dispatch; see
[Pi model-plane configuration](docs/pi-model-plane.md).

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
4. Install WG only after you understand the system it instantiates.

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

- **[docs/quickstart-pi-openrouter.md](docs/quickstart-pi-openrouter.md)** — verified
  pushbutton path: install WG + Pi, authenticate with OpenRouter, find a free
  model, install `pi-worksgood`, select the Pi route, open `wg tui`
- **[docs/GUIDE.md](docs/GUIDE.md)** — operator manual: configuration, the
  service, agent management, models, TUI, troubleshooting, AI assistants
- **[docs/AGENT-GUIDE.md](docs/AGENT-GUIDE.md)** — how agents should use
  WG
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
