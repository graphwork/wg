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

A normal install places three commands on `PATH`: `worksgood` (the attended human lifecycle concierge), `wg` (the complete expert task/tool CLI), and `nex` (the standalone native model client).

```bash
cargo install --git https://github.com/graphwork/wg --locked
# or, from npm (Node 20+): same three commands plus the Pi coding agent CLI,
# from prebuilt per-platform packages — no install scripts, no Rust toolchain:
npm install -g @worksgood/cli
mkdir -p ~/work/my-project && cd ~/work/my-project
worksgood
```

The npm route ships the same prebuilt release binaries via
[`@worksgood/cli`](https://www.npmjs.com/package/@worksgood/cli) (esbuild/biome-style
`optionalDependencies` — zero postinstall; fully functional under
`--ignore-scripts`; `WG_BINARY_PATH` overrides the packaged binary). The
metapackage declares `@earendil-works/pi-coding-agent ^0.85.1` as a
dependency, so one command brings the whole WG + Pi stack. On an
unsupported platform (or with `--no-optional`) the shim prints the `cargo
install` fallback instead of failing. See `scripts/npm/README.md`.

> **macOS limitation:** the macOS binaries currently ship **unsigned** and
> **un-notarized** (Apple Developer ID secrets are not configured), so
> Gatekeeper may block the first run. Allow it with
> `xattr -d com.apple.quarantine "$(which wg)"` (repeat for `worksgood` and
> `nex`) or right-click → Open. This is a known temporary limitation.

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

Bare `worksgood` takes the simple attended path: it verifies `pi`, ensures the compatible WorksGood plugin, initializes a route-free graph when needed, and opens the TUI. Choose **New chat → Pi**; Pi owns login, provider/model selection, and model switching. No profile, worker/evaluator route, reasoning tier, dispatcher, or service is required or changed. Use `worksgood --without-ai` (or `wg init --no-agency && wg tui`) to open a graph without checking Pi.

Repository-wide automation is a separate advanced choice. If you want unattended workers and evaluation and already know one exact Pi route, configure it with one paste and one confirmation:

```bash
worksgood setup --model pi:openrouter:deepseek/deepseek-v4-flash
# Or reconcile the same setup and then open the TUI:
worksgood --model pi:openrouter:deepseek/deepseek-v4-flash
```

Those exact routes and reasoning settings govern **unattended dispatch only**; they never select or rewrite the model a human chooses inside an attended Pi chat. The route is copied to every automation role without normalization or fallback. Worker roles default to reasoning `high`; eval/assign/FLIP roles default to `low`. Override those independently with `--strong-reasoning` and `--weak-reasoning`. `--profile` remains the advanced path for selecting and customizing an existing reusable automation base.

#### The two-tier model plane (strong vs weak)

Real deployments usually run **two** routes, not one. WG resolves every automation role through exactly two tiers:

- **STRONG** — workers and heavy generative roles (`task_agent`, `creator`, `merger`, `evolver`, `verification`), reasoning `high`. This is where implementation quality matters.
- **WEAK** — the cheap, recoverable one-shots (`evaluator`, `assigner`, `flip_inference`/`flip_comparison`, `triage`, `placer`, `compactor`, `chat_compactor`, `coordinator_eval`, `reviewer`), reasoning `low`. These run many times per task, and every verdict they produce is recoverable (re-run or escalate), so a fast model is usually enough.

Attended interactive `wg setup` is concierge-guided: right after you pick the route, three fail-clean **Pi-readiness gates** run before any tier prompt and before anything is written — (1) **Pi detected?** if the `pi` executable is missing, setup prints the exact install command (`npm install -g @earendil-works/pi-coding-agent`, Node 20+) and exits cleanly with “rerun `worksgood setup` after installing Pi”; (2) **provider authenticated?** setup runs Pi's own credential readiness check (`pi auth check --no-refresh`) across the providers Pi's offline registry exposes and, if none is authenticated, prints the guided login instruction (run `pi`, then `/login <provider>` — Pi owns the OAuth flow; WG never sees keys) and exits cleanly for you to return; (3) **model resolvable?** once authenticated, at least one model must resolve via Pi's offline registry (a bounded preflight — setup never live-calls a provider). Every gate stops with the exact next command and no partial state; WG never writes config, installs packages, or stores credentials on its own side. A user who explicitly declines execution (“Not now — keep this WG graph-only”) never reaches the gates.

Interactive `wg setup` (no `--yes`) then asks for the **strong** route first, then the **weak** route, shows the resulting role table (which roles resolve to which tier and the reasoning each gets) before writing anything, and reminds you that routes are re-changeable any time via `wg config -m <route>` / `wg config --set-model <role> <route>` / `wg profile select <name>` — Pi owns the model plane, WG stores exact routes only. Reusing the strong route at the weak prompt is a valid answer: single-model deployments keep working (the weak tier simply inherits the strong route, and no `[tiers]` keys are written).

Non-interactive setup keeps the single-model paste; an optional `--weak-model <pi:<provider>:<model>>` on `wg setup --route pi --yes` writes the distinct weak tier explicitly.

> **Naming note — two different "FLIP"s.** The *completion-review FLIP* (`flip_inference` / `flip_comparison` roles) runs by default on the weak tier as part of terminal-completion review. The separately-named `agency.flip_enabled` config flag is an opt-in agency rollout feature and has nothing to do with those FLIP roles' routing.

For explicit graph-only expert use, `wg init` followed by `wg tui` is **non-mutating** — those commands never select a model, authenticate, install packages, or start a service. The complete task/tool command set remains under `wg`; agent integrations continue to use the `wg_*` protocol.

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
# 1. install WorksGood (worksgood + wg + nex) and Pi (needs Node 20+).
#    Rust route (primary):    cargo install --git https://github.com/graphwork/wg --locked
#    npm route (prebuilt):    npm install -g @worksgood/cli        # brings WG + Pi in one install
cargo install --git https://github.com/graphwork/wg --locked
npm install -g --ignore-scripts @earendil-works/pi-coding-agent   # skip when using the npm route

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
