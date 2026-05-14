# WG Operator Guide

This guide is the operator manual: configuration, the service daemon, agent
management, models, the TUI, and troubleshooting. The [README](../README.md)
explains *what* WG is; this document explains *how to drive it*.

For the agent-side contract (how agents themselves should behave), see
[AGENT-GUIDE.md](AGENT-GUIDE.md). For the agency system (roles, tradeoffs,
evaluation, evolution, federation), see [AGENCY.md](AGENCY.md). For the full
command reference, see [COMMANDS.md](COMMANDS.md).

---

## Setup

### Global config (once, after install)

A fresh install with no `~/.wg/config.toml` already runs `claude:opus` via the
`claude` CLI handler — built-in defaults cover the common case. The first time
you want to commit choices to disk you have three options:

```bash
wg setup                            # interactive wizard — pick one of 5 named routes
wg config init --global             # non-interactive: minimal canonical claude-cli config
wg config init --global --route openrouter   # non-interactive: openrouter route
```

Writes `~/.wg/config.toml`. Pick one of 5 smooth routes — each produces a
complete, working config (model + tiers + endpoint when applicable):

| Route | Default model spec | Use case |
|-------|--------------------|----------|
| `claude-cli` | `claude:opus` | Local `claude` CLI login (no API key in config) |
| `codex-cli` | `codex:gpt-5` | Local `codex` CLI login |
| `openrouter` | `openrouter:<model>` | One API key, every major provider |
| `local` | `nex:<model>` | Ollama / vLLM / llama.cpp on `localhost` (via nex) |
| `nex-custom` | `nex:<model>` | Bring your own OAI-compatible URL + key + model |

You don't pick an executor — wg derives the handler from the model spec's
provider prefix. The prefix matches the handler / subcommand name: `claude:*` →
claude CLI, `codex:*` → codex CLI, `nex:*` → in-process nex (`wg nex`).
`openrouter:*` also routes through nex but uses an implicit `api.openrouter.ai`
endpoint.

(`local:` and `oai-compat:` are deprecated aliases for `nex:` retained for one
release; `wg migrate config` rewrites them in existing config files.)

Non-interactive use:

```bash
wg setup --route claude-cli --yes
wg setup --route openrouter --api-key-env OPENROUTER_API_KEY --yes
wg setup --route local --url http://localhost:11434/v1 --model qwen3:4b --yes
wg setup --route nex-custom --url https://my.endpoint/v1 --api-key-env MY_KEY --model my-model --yes

# Preview without writing
wg setup --route claude-cli --dry-run
```

Switch routes later with `wg config reset --route <name>` (always backs up the
existing config first; `--keep-keys` preserves existing endpoint entries).

If you have an old config from a previous wg release with deprecated keys
(`agent.executor`, retired compactor knobs) or stale model strings
(`openrouter:anthropic/claude-sonnet-4` instead of `…-sonnet-4-6`), run:

```bash
wg migrate config --dry-run    # preview changes
wg migrate config --all        # rewrite global + local; backs up to .pre-migrate.<timestamp>
```

### Initialize a project

```bash
cd your-project
wg init
```

Creates `.wg/` with your task graph. Inherits global config; override
per-project with `wg config --local`.

---

## Adding tasks

```bash
# Simple task
wg add "Set up CI pipeline"

# Task with a blocker
wg add "Deploy to staging" --after set-up-ci-pipeline

# Task with metadata
wg add "Implement auth" \
  --hours 8 \
  --skill rust \
  --skill security \
  --deliverable src/auth.rs

# Per-task model override (use provider:model for non-default providers)
wg add "Quick formatting fix" --model haiku
wg add "Use GPT for this" --model openai:gpt-4o

# Execution weight controls what the agent can do
wg add "Quick lint fix" --exec-mode shell       # no LLM, just runs shell command
wg add "Research task" --exec-mode light        # read-only tools
wg add "Full implementation" --exec-mode full   # default: all tools

# Acceptance criteria — put criteria under `## Validation` in the description
wg add "Security audit" -d $'## Description\nReview surface for vulns.\n\n## Validation\n- [ ] All findings documented with severity ratings'

# Scheduling: delay or absolute time gate
wg add "Follow-up check" --delay 1h
wg add "Deploy window" --not-before 2026-03-20T09:00:00Z   # ISO 8601

# Placement hints and paused creation
wg add "Related work" --place-near auth-task
wg add "Urgent fix" --place-before deploy-task
wg add "Standalone" --no-place
wg add "Draft idea" --paused

# Visibility for cross-org sharing
wg add "Public API design" --visibility public

# Control how much context the agent receives at dispatch
wg add "Quick lint fix" --context-scope clean
wg add "Complex refactor" --context-scope full
```

### Editing tasks after creation

```bash
wg edit my-task --title "Better title"
wg edit my-task --add-after other-task
wg edit my-task --remove-tag stale --add-tag urgent
wg edit my-task --model opus
wg edit my-task --exec-mode light
wg edit my-task --delay 30m --not-before 2026-03-20T09:00:00Z
wg edit my-task --add-skill security --remove-skill docs
```

### Verification workflow

Code tasks should include a `## Validation` section in their description
listing acceptance criteria. When an agent calls `wg done`, the agency
evaluator (auto_evaluate) reads the section and scores the agent's output
against it. If `auto_evaluate` is enabled, agents that exit without calling
`wg done` enter `failed-pending-eval` instead of `failed` until the evaluator
runs.

```bash
wg add "Security audit" -d $'## Description\nReview surface for vulns.\n\n## Validation\n- [ ] All findings documented with severity ratings\n- [ ] Each finding has reproduction steps'

wg done security-audit                                    # evaluator scores against Validation
wg approve security-audit                                  # operator transitions pending → Done
wg reject security-audit --reason "Missing CVE references" # reopens for rework
```

Rejected tasks reopen for the agent to address feedback. After too many
rejections (default: 3), the task is failed automatically. The legacy
`--verify <CRITERIA>` flag is no longer accepted; `wg add --verify` errors at
runtime.

### Registering agents

```bash
# Human
wg agent create "Erik" \
  --executor matrix \
  --contact "@erik:server" \
  --capabilities rust,python \
  --trust-level verified

# AI agent
wg agent create "Claude Coder" \
  --role <role-hash> \
  --tradeoff <tradeoff-hash> \
  --capabilities coding,testing,docs
```

---

## The service daemon

The service automates agent spawning and lifecycle. Start it once and it
continuously picks up ready tasks, spawns agents, and cleans up dead ones.

### Quick start

```bash
wg service start
wg service status    # daemon info, agent summary, dispatcher state
wg agents            # list all agents
wg tui               # interactive dashboard
wg service stop                  # stop daemon (agents keep running)
wg service stop --kill-agents    # stop daemon and all agents
```

### Configuration

`.wg/config.toml`:

```toml
[dispatcher]              # legacy alias [coordinator] still accepted
max_agents = 4            # max parallel agents (default: 4)
poll_interval = 5         # seconds between safety-net ticks
model = "claude:opus"     # provider:model — handler is implied

[agent]
model = "claude:opus"
heartbeat_timeout = 5     # minutes before agent is considered dead

[agency]
auto_evaluate = false
auto_assign = false
auto_triage = false
assigner_model = "haiku"
evaluator_model = "haiku"
evolver_model = "opus"
```

Set values with:

```bash
wg config --max-agents 8
wg config --model claude:sonnet
wg config --poll-interval 120

# Agency
wg config --auto-evaluate true
wg config --auto-assign true
wg config --auto-place true
wg config --auto-create true
wg config --assigner-model haiku
wg config --evaluator-model opus
wg config --evolver-model opus

# Creator tracking
wg config --creator-agent <agent-hash>
wg config --creator-model opus

# Triage
wg config --auto-triage true
wg config --triage-model haiku

# Eval gate and FLIP
wg config --eval-gate-threshold 0.7
wg config --flip-enabled true

# Model registry
wg config --registry
wg config --registry-add --id my-model --provider openrouter --reg-model my-model --reg-tier standard
wg config --set-model default sonnet
wg config --set-model evaluator opus

# Multi-chat
wg config --max-coordinators 3

# Inspect
wg config --list
wg config --global    # ~/.wg/config.toml
wg config --local     # .wg/config.toml
```

CLI flags on `wg service start` override `config.toml`:

```bash
wg service start --max-agents 8 --interval 120 --model claude:haiku
```

### Service control

| Command | What it does |
|---------|-------------|
| `wg service start` | Start the background daemon |
| `wg service stop` | Stop daemon (agents continue independently) |
| `wg service stop --kill-agents` | Stop daemon and kill all running agents |
| `wg service stop --force` | Immediately SIGKILL the daemon |
| `wg service status` | Daemon PID, uptime, agent summary, dispatcher state |
| `wg service reload` | Re-read config.toml without restarting |
| `wg service restart` | Graceful stop then start |
| `wg service pause` | Pause dispatcher (running agents continue, no new spawns) |
| `wg service resume` | Resume coordinator (immediate tick) |
| `wg service freeze` | SIGSTOP all running agents and pause coordinator |
| `wg service thaw` | SIGCONT all frozen agents and resume coordinator |
| `wg service install` | Generate a systemd user service file |
| `wg service tick` | Run a single coordinator tick (debug) |
| `wg service create-coordinator` | Create a new coordinator session |
| `wg service stop-coordinator` | Stop a running coordinator session |
| `wg service archive-coordinator` | Archive a coordinator session |
| `wg service delete-coordinator` | Delete a coordinator session |
| `wg service interrupt-coordinator` | Interrupt a coordinator's current generation |

Reload changes settings at runtime:

```bash
wg service reload                              # re-read config.toml
wg service reload --max-agents 8 --model haiku # apply specific overrides
```

### Agent management

```bash
wg agents              # all agents
wg agents --alive      # running agents only
wg agents --dead
wg agents --working    # actively working on a task
wg agents --idle
wg agents --json
```

Kill agents:

```bash
wg kill agent-7          # graceful: SIGTERM → wait → SIGKILL
wg kill agent-7 --force  # immediate SIGKILL
wg kill --all
```

Killing an agent automatically unclaims its task.

**Dead agent detection.** Agents send heartbeats while working. If an agent's
process exits or the heartbeat goes stale (default 5 min), the coordinator
marks it dead and unclaims its task. Manual checks:

```bash
wg dead-agents                  # read-only check (default)
wg dead-agents --cleanup        # mark dead and unclaim their tasks
wg dead-agents --remove         # remove dead agents from registry
wg dead-agents --purge          # remove all dead agents and clean up
wg dead-agents --delete-dirs    # also delete agent working directories
wg dead-agents --threshold 10   # custom staleness threshold (minutes)
```

**Smart triage.** When a dead agent is detected, the coordinator can
automatically triage with an LLM that reads the agent's output log and decides
whether the task was actually completed (mark done), still running (leave
alone), or needs to be restarted (re-spawn):

```bash
wg config --auto-triage true
wg config --triage-model haiku       # cheap is usually enough
wg config --triage-timeout 30
wg config --triage-max-log-bytes 50000
```

### Service state files

`.wg/service/`:

| File | Purpose |
|------|---------|
| `state.json` | Daemon PID, socket path, start time |
| `daemon.log` | Persistent daemon logs |
| `coordinator-state.json` | Effective config and runtime metrics |
| `registry.json` | Agent registry (IDs, PIDs, tasks, status) |

---

## Models

### Selection priority

1. Task's `model` property (`wg add --model`, `wg edit --model`) — highest
2. Executor config model (in the executor's config file)
3. `coordinator.model` in config.toml (or `--model` on `wg spawn` / `wg service start`)
4. Handler default

```bash
wg add "Simple fix" --model claude:haiku
wg add "Complex design" --model claude:opus
wg edit my-task --model claude:sonnet
wg spawn my-task --model claude:haiku
wg config --model claude:sonnet
wg service reload
```

**Cost tips.** Use **haiku** for simple formatting/linting, **sonnet** for
typical coding, **opus** for complex reasoning and architecture.

**Alternative providers.** wg supports
[OpenRouter](https://openrouter.ai/) and any OpenAI-compatible API. Configure
an endpoint with `wg endpoints add` and use full model IDs like
`deepseek/deepseek-chat-v3`. See [guides/openrouter-setup.md](guides/openrouter-setup.md)
for details.

### Model registry

```bash
wg model list                                     # all models (built-in + user)
wg model add my-model --provider openrouter --model-id deepseek/deepseek-chat-v3
wg model remove my-model
wg model set-default sonnet                       # default dispatch model
wg model routing                                  # per-role routing
wg model set --role evaluator opus
```

### API keys

```bash
wg key set anthropic
wg key check
wg key list
```

---

## The TUI

```bash
wg tui [--refresh-rate 2000]   # default: 2000ms
```

Three main views plus an inspector panel:

- **Dashboard** — split-pane: tasks (left) and agents (right) with status bars.
- **Graph Explorer** — tree view of the dependency graph with task status and
  active-agent indicators. Touch drag-to-pan supported for mobile terminals
  (Termux).
- **Log Viewer** — real-time tailing of agent output with auto-scroll.
- **Inspector panel** — nine tabbed views via `Alt+Left`/`Alt+Right` (with
  slide animation): Chat, Detail, Log, Messages, Agency, Config, Files,
  Coordinator Log, and Firehose. Resize with `i` (1/3 → 1/2 → 2/3 → full)
  and `I` (shrink).

**Status bar.** Service health badge (green/yellow/red, tap-to-inspect), token
display (novel vs cached input per task), lifecycle indicators (⊳ assigning, ∴
evaluating, validating, verifying), and markdown rendering with syntax
highlighting.

### Keybindings

**Global:**

| Key | Action |
|-----|--------|
| `q` | Quit |
| `?` | Show help overlay |
| `Esc` | Back / close overlay |

**Dashboard:**

| Key | Action |
|-----|--------|
| `Tab` / `Shift+Tab` | Switch panel (Tasks ↔ Agents) |
| `j` / `k` or `↑` / `↓` | Scroll up / down |
| `Enter` | Drill into selected item |
| `g` | Open graph explorer |
| `r` | Refresh data |

**Graph Explorer:**

| Key | Action |
|-----|--------|
| `j` / `k` or `↑` / `↓` | Navigate up / down |
| `h` / `l` or `←` / `→` | Collapse / expand subtree |
| `d` | Toggle between tree and graph view |
| `Enter` | View task details or jump to agent log |
| `a` | Cycle to next task with active agents |
| `/` | Open search |
| `n` / `N` | Next / previous match |
| `Tab` / `Shift+Tab` | Next / previous match (in search mode) |
| `r` | Refresh graph |

**Log Viewer:**

| Key | Action |
|-----|--------|
| `j` / `k` or `↑` / `↓` | Scroll one line |
| `PageDown` / `PageUp` | Scroll half viewport |
| `g` | Jump to top (disable auto-scroll) |
| `G` | Jump to bottom (enable auto-scroll) |

---

## Cycles (repeating workflows)

Some workflows repeat: write → review → revise → write again. wg models
these as **structural cycles** — `after` back-edges with a `CycleConfig` that
controls iteration limits and behavior. Cycles are detected automatically using
Tarjan's SCC algorithm.

```bash
# write → review cycle, max 3 iterations
wg add "Write draft" --id write --after review --max-iterations 3
wg add "Review draft" --after write --id review

wg cycles                          # inspect detected cycles
```

The `--max-iterations` flag sets a `CycleConfig` on the task, making it the
**cycle header** — the entry point that controls iteration. Without
`--max-iterations`, a cycle is treated as an unconfigured deadlock (flagged by
`wg check`).

```bash
# Guard: only iterate if review failed
wg add "Write draft" --id write --after review \
  --max-iterations 5 --cycle-guard "task:review=failed"

wg edit write --cycle-delay "5m"                   # delay between iterations
wg edit write --no-converge                        # force all iterations
wg edit write --no-restart-on-failure
wg edit write --max-failure-restarts 5
```

When a cycle completes an iteration (all members reach `done`), the header and
all members are reset to `open` with `loop_iteration` incremented.

### Convergence

```bash
wg done <task-id> --converged   # stops the cycle even if iterations remain
```

Adds a `"converged"` tag to the **cycle header** (regardless of which member
you complete). Use `--converged` when no more iterations are needed.

### Inspection

```bash
wg cycles              # detected cycles, status, iteration counts
wg cycles --json
wg show <task-id>      # cycle membership + current iteration
wg viz                 # cycle edges appear as dashed lines
```

---

## Trace, replay, and functions

wg records every operation in a trace log — the project's organizational
memory.

### Watching events

```bash
wg watch                             # stream events
wg watch --event task_state          # only task state changes
wg watch --event evaluation
wg watch --task my-task
wg watch --replay 20                 # include 20 historical events
```

External adapters (CI integrations, Slack bots, monitoring) can observe the
event stream and react without polling.

### Exporting and importing traces

Tasks carry a `visibility` field (`internal`, `public`, `peer`) controlling
what crosses organizational boundaries:

```bash
wg trace export --visibility public   # sanitized for open sharing
wg trace export --visibility peer     # richer detail for trusted peers
wg trace import peer-export.json      # import as read-only context
```

### Functions (workflow templates)

Three layers of increasing sophistication:

- **Static** (v1): fixed task topology with `{{input.X}}` substitution
- **Generative** (v2): a planning node decides the task graph at apply time
- **Adaptive** (v3): generative + trace memory from past runs

```bash
wg func extract impl-auth --name impl-feature --subgraph
wg func extract impl-auth impl-caching impl-logging --generative --name impl-feature
wg func apply impl-feature --input feature_name=auth --input description="Add OAuth"
wg func make-adaptive impl-feature
wg func bootstrap                # the meta-function: extraction as a workflow

wg func list
wg func show impl-feature
```

### Trace visualization

```bash
wg trace show <task-id>
wg trace show <task-id> --animate
```

### Replay

```bash
wg replay --failed-only
wg replay --below-score 0.5
wg replay --subgraph task-id
wg replay --keep-done
```

---

## Communication

```bash
# Send a message to a task (any agent working on it sees it)
wg msg send my-task "The API schema changed — use v2 endpoints"

# Read messages as an agent
wg msg read my-task --agent $WG_AGENT_ID
```

Interactive coordinator chat:

```bash
wg chat "What's the status of the auth refactor?"
wg chat -i                                       # interactive REPL
wg chat "Here's the spec" --attachment spec.pdf
wg chat --coordinator 2 "Status?"
wg chat --history
wg chat --clear
```

---

## Agent isolation

When the service spawns multiple agents concurrently, each agent operates in
its own [git worktree](https://git-scm.com/docs/git-worktree). Each worktree
has an independent working tree and index while sharing the same repository,
so agents can build, test, and commit without interfering with each other.

See [WORKTREE-ISOLATION.md](WORKTREE-ISOLATION.md) for the full design.

wg also uses `flock`-based file locking on `.wg/graph.jsonl` to prevent
concurrent modifications. This is automatic.

---

## Agency system (summary)

The agency system gives agents composable identities — a *role* (what it does)
paired with a *tradeoff* (why it acts that way).

```bash
wg agency init                                          # seed starter roles/tradeoffs
wg agent create "Careful Coder" --role <hash> --tradeoff <hash>
wg assign my-task <agent-hash>
```

The agency loop: **eval → FLIP → verify → evolve**. Evaluation grades quality;
FLIP (Fidelity via Latent Intent Probing) grades fidelity by reconstructing
what the task must have been from only the agent's output; verification catches
low-confidence results; evolution uses performance data to improve identities.

```bash
wg evaluate run <task>                       # LLM evaluation
wg evaluate record --task <id> --score <n> --source <tag>
wg evaluate show

wg evolve run                                # full evolution cycle
wg evolve run --strategy mutation --budget 3
wg evolve run --dry-run
```

### Federation

Share agency entities across projects:

```bash
wg agency remote add partner /path/to/other/project/.wg/agency
wg agency scan partner
wg agency pull partner
wg agency push partner
```

Performance records merge during transfer; content-hash IDs make this natural.

### Peer WG instances

Cross-repo task coordination (separate from agency federation):

```bash
wg peer add partner /path/to/other/project
wg peer list
wg peer status
```

See [AGENCY.md](AGENCY.md) for the full system documentation.

---

## Query and analysis

```bash
wg ready              # what can be worked on now?
wg list               # all tasks (--status to filter)
wg show <id>          # full task details
wg status             # one-screen overview
wg viz                # ASCII dependency graph (--all to include done)
wg viz --graph        # 2D spatial layout with box-drawing characters
wg viz task-a task-b  # focus on subgraphs
wg viz --show-internal # include assign-*/evaluate-* meta-tasks
wg viz --no-tui       # force static output

wg why-blocked <id>   # trace the blocker chain
wg impact <id>        # what depends on this?
wg context <id>       # available context from completed dependencies
wg bottlenecks
wg critical-path
wg forecast
wg velocity
wg aging
wg workload
wg structure
wg analyze            # comprehensive health report
```

---

## Utilities

```bash
wg log <id> "message"                       # add progress notes
wg artifact <id> path                       # record a file produced by a task
wg compact                                  # distill graph state into context.md
wg sweep                                    # detect/recover orphaned in-progress tasks
wg checkpoint <id> -s "progress summary"
wg stats
wg exec <id>                                # claim + run shell command + done/fail
wg viz --mermaid                            # Mermaid flowchart
wg archive
wg screencast                               # render TUI traces to asciinema
wg server                                   # multi-user server setup
wg tui-dump                                 # dump current TUI screen
wg check                                    # cycles and graph issues
wg trajectory <id>                          # optimal claim order for agents
wg runs list
wg runs diff <snapshot>
wg runs restore <snapshot>
```

---

## Using with AI coding assistants

wg ships a skill that teaches AI assistants to use the service as a
coordinator rather than working ad-hoc.

### Claude Code

```bash
wg skill install           # ~/.claude/skills/wg/
wg skill list
wg skill find <query>
wg skill task <task-id>
```

The skill has YAML frontmatter so Claude auto-detects when to use it. You can
also invoke explicitly with `/wg`. Add to your `CLAUDE.md` (or
`~/.claude/CLAUDE.md` for global):

```markdown
Use wg for task management.

At the start of each session, run `wg quickstart` in your terminal to orient yourself.
Use `wg service start` to dispatch work — do not manually claim tasks.
```

### OpenCode / Codex / other agents

Add to the agent's system prompt or `AGENTS.md`:

```markdown
## Task Management

Use wg (`wg`) for task coordination. Run `wg quickstart` to orient yourself.

As a top-level agent, use service mode — do not manually claim tasks:
- `wg service start` to start the dispatcher
- `wg add "Task" --after dep` to define work
- `wg list` / `wg agents` to monitor progress

The service automatically spawns agents and claims tasks.
```

The skill teaches agents to:

- Run `wg quickstart` at session start
- Act as a dispatcher: start the service, define tasks, monitor progress
- Let the service handle claiming and spawning
- Use manual mode only as a fallback when working alone without the service

---

## Recommended workflow

1. **Plan first.** Sketch the major tasks and dependencies.

   ```bash
   wg add "Goal task"
   wg add "Step 1"
   wg add "Step 2" --after step-1
   wg add "Step 3" --after step-2
   ```

2. **Check the structure.**

   ```bash
   wg analyze        # health check
   wg critical-path  # longest chain
   wg bottlenecks    # priorities
   ```

3. **Execute.**

   ```bash
   wg service start --max-agents 4
   wg tui
   ```

4. **Adapt.** As you learn more, update the graph — the service picks up changes.

   ```bash
   wg add "New thing we discovered" --after whatever
   wg edit stuck-task --add-tag needs-rethink
   wg fail stuck-task --reason "Need to rethink this"
   wg retry stuck-task                                # retry-in-place: keep prior worktree
   wg retry stuck-task --fresh                        # discard prior worktree, start over
   wg retry stuck-task --reason "agent hung at 0% for 20min"
   ```

   `wg retry` also rescues hung in-progress tasks (SIGTERM → SIGKILL → reset).
   Default is retry-in-place; pass `--fresh` to discard the worktree, or
   `--preserve-session` to keep the stored Claude session ID across the retry.

5. **Ship.** When `wg ready` is empty and everything important is done, you're
   there.

---

## Storage layout

Everything lives in `.wg/`:

```
.wg/
  graph.jsonl              # task graph (operations log / trace)
  config.toml              # configuration
  federation.yaml          # named remotes for agency federation
  functions/               # workflow templates
    <name>.yaml
  agency/
    primitives/
      components/          # skill components (atomic capabilities)
      outcomes/            # desired outcomes
      tradeoffs/           # tradeoff definitions
    cache/
      roles/               # composed roles (component_ids + outcome_id)
      agents/              # agent definitions (role + tradeoff pairs)
    assignments/           # task-to-agent assignment records
    evaluations/           # evaluation records (JSON)
    org-evaluations/       # organization-level evaluation records
    evolution_runs/        # evolution run history
    evolver-skills/        # strategy-specific guidance documents
    coordinator-prompt/    # coordinator prompt files
    deferred/              # deferred evolution operations
    creator_state.json     # creator agent state
  service/
    state.json
    daemon.log
    coordinator-state.json
    registry.json
```

Graph format (`.wg/graph.jsonl`):

```jsonl
{"kind":"task","id":"design-api","title":"Design the API","status":"done"}
{"kind":"task","id":"build-backend","title":"Build the backend","status":"open","after":["design-api"],"model":"sonnet"}
```

One JSON object per line. Human-readable, git-friendly, easy to hack on.

---

## Testing

Run the wave-1 integration smoke test after any wave-1 task lands.

**This MUST be run live against real endpoints — no stubs, no mocks, no
special bypass.** The earlier version of this smoke silently passed because it
relied on a fake LLM and ran the daemon with `--no-coordinator-agent`, which
is exactly how the `wg nex` 404 reached the user on the first 'hi' in TUI
chat. Live scenarios cover the user's literal reproduction:

```bash
# Full suite — runs scenarios 1-7 (offline + live)
bash scripts/smoke/wave-1-smoke.sh

# Skip slow daemon/TUI scenarios (and the live ones)
bash scripts/smoke/wave-1-smoke.sh --quick

# Skip live scenarios (6, 7) but keep offline ones — for sandboxed CI
bash scripts/smoke/wave-1-smoke.sh --offline
```

If a live endpoint is unreachable, scenario 6/7 print a LOUD banner —
`*** NEX SMOKE SKIPPED — endpoint unreachable ***` — that is greppable in
output and impossible to miss. Set `WG_SMOKE_FAIL_ON_SKIP=1` to promote loud
skips to fail in CI. Set `WG_SMOKE_KEEP_SCRATCH=1` to preserve per-scenario
scratch dirs for post-mortem inspection.

Live scenarios point at `https://lambda01.tail334fe6.ts.net:30000` with model
`qwen3-coder` by default; override via `WG_LIVE_NEX_ENDPOINT` and
`WG_LIVE_NEX_MODEL`.

---

## Troubleshooting

**Daemon logs.** Check `.wg/service/daemon.log` for errors. The daemon logs
with timestamps and rotates at 10 MB (keeps one backup at `daemon.log.1`).
Recent errors are also surfaced in `wg service status`.

**Common issues:**

- **"Socket already exists"** — A previous daemon didn't clean up. Check if
  it's still running with `wg service status`, then `wg service stop` or
  manually remove the stale socket.
- **Agents not spawning** — Check `wg service status` for dispatcher state.
  Verify `max_agents` isn't already reached with `wg agents --alive`. Ensure
  there are tasks in `wg ready`.
- **Agent marked dead prematurely** — Increase `heartbeat_timeout` in
  `config.toml` if agents do long-running work without heartbeating.
- **Config changes not taking effect** — Run `wg service reload` after editing
  `config.toml`. CLI flag overrides on `wg service start` take precedence over
  the file.
- **Daemon won't start** — Check if another daemon is already running. Look at
  `.wg/service/state.json` for stale PID info.

---

## Reusable workflow functions

```bash
wg func list                                  # discover patterns
wg func apply <id> --input key=value          # instantiate
wg func show <id>                             # details and required inputs
```

See [Functions](#functions-workflow-templates) above for extraction and
adaptive learning.
