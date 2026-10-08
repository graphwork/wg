# WG

**The work OS for human/AI organizations.**

WG stands for works good.

Agents can come and go. The graph remains.

![WG inside a Pi session: the /wg-fleet cockpit — counts header, dependency tree, and task detail](docs/assets/pi-fleet.gif)

Pi sessions are the primary interface. The WorksGood plugin (`pi-worksgood`)
loads inside pi, so the task graph lives beside you — claim work, spawn agents,
watch the graph evolve, and open any task's detail without leaving the session.
The CLI is the plumbing; the TUI is the fallback console.

## Install once

```bash
npm install -g @worksgood/cli
```

One command installs WG (`worksgood`, `wg`, `nex`) **and** the Pi coding agent
CLI. It resolves prebuilt per-platform packages
(`@worksgood/linux-x64-gnu`, `@worksgood/darwin-arm64`) — zero postinstall
scripts, no Rust toolchain, fully functional under `--ignore-scripts`. Node
22.19+ (the floor Pi itself declares). The metapackage declares
`@earendil-works/pi-coding-agent ^0.85.1` as a dependency.

The `pi-worksgood` plugin **ships embedded in the `wg` binary**: the exact
compatible build is materialized into a versioned cache
(`${XDG_CACHE_HOME:-~/.cache}/wg/worksgood-pi/<compat>/`) and loaded by
absolute path, so there is no PATH/npm skew. It **self-heals** — after any
upgrade the next action that resolves the plugin recomputes the binary's embed
digest and re-materializes the cache, reporting the refresh loudly. Verify:

```bash
wg pi-plugin status   # embed digest == cache digest, cache state: current
```

`cargo install --git https://github.com/graphwork/wg --locked` (or
`cargo install --path . --locked` from a checkout) is the **from-source**
route — for unsupported platforms, Alpine/musl, Windows, locked-down
environments, and contributors. Full detail, channels, checksums, and
uninstall live in [docs/guides/install.md](docs/guides/install.md).

> **macOS limitation:** the macOS binaries currently ship **unsigned** and
> **un-notarized** (Apple Developer ID secrets are not configured), so
> Gatekeeper may block the first run. Allow it with
> `xattr -d com.apple.quarantine "$(which wg)"` (repeat for `worksgood` and
> `nex`) or right-click → Open. This is a known temporary limitation.

On an unsupported platform (or with `--no-optional`) the npm shim prints the
`cargo install` fallback instead of failing.

## Start in any project

```bash
mkdir -p ~/work/my-project && cd ~/work/my-project
wg init      # create this project's task graph (.wg/) — non-mutating, route-free
wg setup     # pick the routes unattended workers/evaluation use (optional for attended use)
pi           # open the Pi session; the WorksGood plugin loads automatically
```

Then, inside the session:

```text
/wg-fleet    # the cockpit: counts header + dependency tree; scroll and drill into any task
```

`/wg-fleet` is the in-session cockpit — a scrollable, read-only view of the
live graph: in-progress/ready/blocked/done counts, the dependency tree, and a
per-task detail pane (status, live agent activity, and a bounded stream tail).
It reads through the daemon's read-only `GetFleet` surface and never mutates
graph state. Enable it once with `{"fleetView": true}` in
`~/.pi/agent/extensions/pi-worksgood/config.json` (or `WG_PI_FLEET_VIEW=1`);
`/wg-viz` is the always-on lighter companion.

Claim work, spawn agents, and close tasks from the same session — no TUI
required.

## The agent loop

You don't drive the graph by hand. You talk to your pi agent, and it uses the
WG tools natively. The tools it has:

| Tool | What it does |
|---|---|
| `wg_capabilities` | the effective trusted/scoped/read-only policy before coordinating |
| `wg_ready` / `wg_show` | list ready work and inspect a task's detail, deps, artifacts, logs |
| `wg_add` / `wg_publish` | create a visible draft task, then release it for dispatch |
| `wg_done` / `wg_fail` | complete (with evidence) or fail after a genuine attempt |
| `wg_msg_send` / `wg_msg_read` | coordinate with other agents |
| `wg_run` | load a task into the agent's context and work it |

And the in-session commands: `/wg ready|graph|show|run|add|done|fail`,
`/wg-viz` and `/wg-fleet` (live graph views), `/wg-model <provider:id>` (warm
in-session model swap), and `/wg-wake [on|off]` (completion-event wakeups).
The graph is the shared medium — artifacts you write are read by other agents,
and tasks you publish are dispatched to other agents.

## The TUI is still there

`wg tui` remains the standalone console when you want the graph without pi —
watch any project without starting an agent session. It shows the same graph:
tasks, agents, claims, logs, and dependencies.

![WG TUI showing tasks, agents, claims, logs, and dependencies](docs/assets/wg-tui.gif)

```bash
wg tui
```

## The graph remains

WG records what needs doing, who or what claimed it, what blocked it,
what evidence was produced, where judgment entered, what failed, what was
retried, and how the work changed over time.

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

## Configure routes and tiers

A normal install places three commands on `PATH`: `worksgood` (the attended
human lifecycle concierge), `wg` (the complete expert task/tool CLI), and `nex`
(the standalone native model client).

For explicit graph-only expert use, `wg init` followed by `wg tui` is
**non-mutating** — those commands never select a model, authenticate, install
packages, or start a service. The complete task/tool command set remains under
`wg`; agent integrations continue to use the `wg_*` protocol.

### This system is yours

Once installed, this is your tool, not a research artifact:

- The **graph** in your project is your durable record of the work — every
  task, decision, and result stays there, readable at any time.
- **Routes are choices, not commitments.** Change them whenever you like with
  `wg config` or `wg profile select`; Pi owns authentication and model
  selection, so WG only ever remembers the exact routes you picked.
- The **strong tier** drives your workers and heavy generative roles. The
  **weak tier** drives the cheap, recoverable one-shots (evaluation,
  assignment, the FLIP completion reviewer, triage, compaction). Pointing
  both tiers at the same model is a perfectly valid choice.
- The **concierge** is the front door: install → Pi login → tiers → TUI.
  Run `worksgood` and follow the prompts.

Bare `worksgood` takes the simple attended path: it verifies `pi`, ensures the
compatible WorksGood plugin, initializes a route-free graph when needed, and
opens the TUI. Choose **New chat → Pi**; Pi owns login, provider/model
selection, and model switching. No profile, worker/evaluator route, reasoning
tier, dispatcher, or service is required or changed. Use
`worksgood --without-ai` (or `wg init --no-agency && wg tui`) to open a graph
without checking Pi.

Repository-wide automation is a separate advanced choice. If you want
unattended workers and evaluation and already know one exact Pi route,
configure it with one paste and one confirmation:

```bash
worksgood setup --model pi:openrouter:deepseek/deepseek-v4-flash
# Or reconcile the same setup and then open the TUI:
worksgood --model pi:openrouter:deepseek/deepseek-v4-flash
```

Those exact routes and reasoning settings govern **unattended dispatch only**;
they never select or rewrite the model a human chooses inside an attended Pi
chat. The route is copied to every automation role without normalization or
fallback. Worker roles default to reasoning `high`; eval/assign/FLIP roles
default to `low`. Override those independently with `--strong-reasoning` and
`--weak-reasoning`. `--profile` remains the advanced path for selecting and
customizing an existing reusable automation base.

#### The two-tier model plane (strong vs weak)

Real deployments usually run **two** routes, not one. WG resolves every automation role through exactly two tiers:

- **STRONG** — workers and heavy generative roles (`task_agent`, `creator`, `merger`, `evolver`, `verification`), reasoning `high`. This is where implementation quality matters.
- **WEAK** — the cheap, recoverable one-shots (`evaluator`, `assigner`, `flip_inference`/`flip_comparison`, `triage`, `placer`, `compactor`, `chat_compactor`, `coordinator_eval`, `reviewer`), reasoning `low`. These run many times per task, and every verdict they produce is recoverable (re-run or escalate), so a fast model is usually enough.

Attended interactive `wg setup` is concierge-guided: right after you pick the route, three fail-clean **Pi-readiness gates** run before any tier prompt and before anything is written — (1) **Pi detected?** if the `pi` executable is missing, setup prints the exact install command (`npm install -g @earendil-works/pi-coding-agent`, Node 22.19+) and exits cleanly with “rerun `worksgood setup` after installing Pi”; (2) **provider authenticated?** setup runs Pi's own credential readiness check (`pi auth check --no-refresh`) across the providers Pi's offline registry exposes and, if none is authenticated, prints the guided login instruction (run `pi`, then `/login <provider>` — Pi owns the OAuth flow; WG never sees keys) and exits cleanly for you to return; (3) **model resolvable?** once authenticated, at least one model must resolve via Pi's offline registry (a bounded preflight — setup never live-calls a provider). Every gate stops with the exact next command and no partial state; WG never writes config, installs packages, or stores credentials on its own side. A user who explicitly declines execution (“Not now — keep this WG graph-only”) never reaches the gates.

Interactive `wg setup` (no `--yes`) then asks for the **strong** route first, then the **weak** route, shows the resulting role table (which roles resolve to which tier and the reasoning each gets) before writing anything, and reminds you that routes are re-changeable any time via `wg config -m <route>` / `wg config --set-model <role> <route>` / `wg profile select <name>` — Pi owns the model plane, WG stores exact routes only. Reusing the strong route at the weak prompt is a valid answer: single-model deployments keep working (the weak tier simply inherits the strong route, and no `[tiers]` keys are written).

Non-interactive setup keeps the single-model paste; an optional `--weak-model <pi:<provider>:<model>>` on `wg setup --route pi --yes` writes the distinct weak tier explicitly.

> **Naming note — two different "FLIP"s.** The *completion-review FLIP* (`flip_inference` / `flip_comparison` roles) runs by default on the weak tier as part of terminal-completion review. The separately-named `agency.flip_enabled` config flag is an opt-in agency rollout feature and has nothing to do with those FLIP roles' routing.

#### Selecting the project profile (per-repo, no global round-trip)

Project route selection is per-repo and writes only `worksgood.toml`; there is
no machine-global active-profile to flip. To run a batch of tasks on a given
provider's credits in one repo, select that profile for that repo; other repos
are unaffected:

```bash
wg profile select claude   # this repo's workers run the claude profile (opus worker)
# ... dispatch / run a batch on Anthropic credits in THIS repo ...
wg profile select nex      # switch this repo back to the in-process localhost endpoint
```

`wg profile select codex` is the third target. Every `profile select` writes
the closed Pi projection into `worksgood.toml` and reloads this project's
daemon — already-spawned workers keep their model; the *next* worker the
daemon spawns picks up the new projection (no daemon restart). Pass
`--no-reload` to stage the switch without poking the daemon. Two repos sharing
one `$HOME` are fully isolated.

To run through OpenRouter, use the `openrouter:` provider prefix inside a
`pi:` route, e.g. `wg setup --route pi --model pi:openrouter:anthropic/claude-opus-4-7`.
API keys live in a credential store managed by `wg secret`. See
[docs/config-precedence.md](docs/config-precedence.md) and
[docs/pi-model-plane.md](docs/pi-model-plane.md).

### Quickstart: drive a free OpenRouter model through Pi

Pi is WorksGood's sole model plane: Pi owns provider login, model discovery,
endpoints, availability, and cost; WG owns the task graph plus exact per-role
`pi:<provider>:<model>` routes. The full, verified, copy-paste path —
install WG and Pi, authenticate with OpenRouter, discover and validate a
current free model, install the `pi-worksgood` integration, optionally add
web plugins, select the route, and open the session — lives in
[**docs/quickstart-pi-openrouter.md**](docs/quickstart-pi-openrouter.md)
(and the same path, as a styled standalone page, ships in
[`website/quickstart-pi-openrouter.html`](website/quickstart-pi-openrouter.html)
for the graphwork.github.io site).
The spine:

```bash
# 1. install WorksGood (worksgood + wg + nex) and Pi (needs Node 22.19+).
npm install -g @worksgood/cli        # brings WG + Pi in one install
# from-source alternative:
#   cargo install --git https://github.com/graphwork/wg --locked
#   npm install -g --ignore-scripts @earendil-works/pi-coding-agent

# 2. authenticate Pi with OpenRouter (once, in Pi — WG never sees the key)
pi
/login openrouter          # -> "Sign in with OpenRouter" (PKCE OAuth)

# 3. discover a CURRENT free model and validate it works in Pi
pi --list-models ":free"
pi --model "openrouter/nvidia/nemotron-3-ultra-550b-a55b:free" -p "Reply OK"

# 4. verify the embedded Pi integration (self-healing, no separate install)
wg pi-plugin status

# 5. initialize a project and select the Pi route (project-scoped; global untouched)
wg init
wg profile init-starters
wg profile pi --strong "pi:openrouter/nvidia/nemotron-3-ultra-550b-a55b:free" \
              --weak   "pi:openrouter/nvidia/nemotron-3-ultra-550b-a55b:free"
wg profile select pi
wg config --models         # every role shows handler=pi, exact route, reasoning

# 6. start the service and open the session
wg service start
pi                         # /wg-fleet is the cockpit
```

Replace the model id with whatever `pi --list-models ":free"` currently
returns — free-model availability, limits, context, and tool support change
frequently. See the [full quickstart](docs/quickstart-pi-openrouter.md) for
macOS/Termux notes, optional `pi-web-access` / `pi-agent-browser-native`
plugins, the `pi-worksgood`/hermetic details, and troubleshooting
(PATH, `Failed to run wg`, missing Pi/plugin/model/auth). Legacy WG model
catalogs/endpoints remain migration-only and never authorize dispatch.

### Then let agents work

```bash
wg service start
pi                         # talk to your agent; /wg-fleet watches the graph
```

The loop: declare work, let the service dispatch it, watch the graph evolve.

If a readiness-confirmed daemon restart meets a reviewed candidate waiting to land, use
the supported [`wg show` → `wg merge-resolution status` → `wg resume --only` recovery
flow](docs/ops/maze-free-recovery.md). It retains immutable candidate/receipt/fence
evidence and never requires source-worker resubmission or manual Git history surgery.

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

- **[docs/worksgood-concierge.md](docs/worksgood-concierge.md)** — attended
  setup/status/stop/restart/TUI lifecycle; `wg` remains the expert CLI
- **[docs/guides/install.md](docs/guides/install.md)** — install paths (npm
  primary, cargo from source), channels, verification, uninstall
- **[docs/quickstart-pi-openrouter.md](docs/quickstart-pi-openrouter.md)** — verified
  pushbutton path: install WG + Pi, authenticate with OpenRouter, find a free
  model, verify `pi-worksgood`, select the Pi route, open the session
- **[worksgood-pi/README.md](worksgood-pi/README.md)** — the Pi plugin: tools,
  `/wg` commands, `/wg-fleet`, `/wg-viz`, model bridge, completion wakeups
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
