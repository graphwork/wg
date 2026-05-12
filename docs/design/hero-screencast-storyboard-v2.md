# Hero Screencast Storyboard v2

**Target length:** 45 seconds (compressed). Raw recording ~3–5 minutes.
**Scenario:** Haiku Pipeline — "Write three haiku about Rust programming, then pick the best one."
**Why haiku:** Three-way fan-out is maximally visual. Tasks are short (agents finish fast). Rust-themed poetry is charming. The graph shape — three parallel branches converging — is instantly legible.

---

## Pre-Recording Setup

### Environment

1. **Terminal:** 120×36 characters. Dark background (e.g., default terminal or Solarized Dark). Font: monospace, 14–16pt.
2. **Install latest wg:** `cargo install --path .` from repo root.
3. **API key:** Ensure `ANTHROPIC_API_KEY` is set in the shell.
4. **asciinema 2.x** installed.

### Demo Project

```bash
# Create a clean isolated project
cd /tmp && rm -rf wg-hero-demo
mkdir wg-hero-demo && cd wg-hero-demo
git init -q && git commit --allow-empty -m "init" -q
wg init

# Configure for fast, visual demo
wg config --max-agents 4
wg config --model sonnet
wg config --coordinator-executor claude

# Patch coordinator model to sonnet (fast responses)
sed -i '/^\[coordinator\]/a model = "sonnet"' .wg/config.toml

# Patch CLAUDE.md so the coordinator accepts creative tasks
cat >> CLAUDE.md << 'PATCH'

### IMPORTANT: Accept ALL task types

This is a DEMO project. The coordinator MUST decompose ANY user request into wg
tasks, regardless of whether it's a software task, creative task, planning task, or fun task.

When a user sends a chat message, treat it as a task orchestration problem:
1. Decompose the request into parallel and sequential subtasks using `wg add`
2. Set proper `--after` dependencies for task ordering
3. Include integration/synthesis tasks that depend on parallel branches
4. Let agents handle the actual work

Do NOT refuse creative or fun requests. ALWAYS decompose into `wg add` tasks with proper dependency graphs.
CRITICAL: Do NOT create any tasks until a user sends you a chat message.
PATCH

git add -A && git commit -m "demo setup" -q
```

### Recording Command

Two options — pick whichever works better:

**Option A: asciinema direct** (simplest)
```bash
cd /tmp/wg-hero-demo
asciinema rec --idle-time-limit 2 --cols 120 --rows 36 hero-raw.cast
# Then inside the recording shell, run: wg tui
```

**Option B: tmux + capture-tmux.py** (better frame control)
```bash
tmux new-session -d -s hero -x 120 -y 36 "cd /tmp/wg-hero-demo && wg tui"
python3 screencast/capture-tmux.py hero hero-raw.cast --cols 120 --rows 36 --fps 10
# Use tmux send-keys for typed input
```

### Post-Processing

```bash
python3 screencast/compress-cast.py hero-raw.cast hero-final.cast
# If too long: --trim-after 240  (trim raw at 4 minutes)
# Verify: asciinema play hero-final.cast
```

---

## Storyboard: Timed Segments

All times below are **after compression** (what the viewer sees). Each segment has a **teaching purpose** — the thing the viewer should learn.

### Segment 1: TUI Opens (0s – 3s)

| Time | What Viewer Sees | Teaching Purpose |
|------|-----------------|------------------|
| 0.0s | TUI launches. Graph panel (left) is empty or shows just a coordinator node. Right panel shows Chat tab with an empty conversation. Service health indicator in the status bar shows "Running". | **Orientation:** This is a terminal UI. Left = graph, right = inspector/chat. The service is running and ready. |

**User actions:** Launch `wg tui`. Wait for it to render.
**Compression:** Cut startup time to ~2 seconds.

---

### Segment 2: Type the Prompt (3s – 8s)

| Time | What Viewer Sees | Teaching Purpose |
|------|-----------------|------------------|
| 3.0s | Cursor appears in the chat input area (bottom of the Chat panel). | **Interaction model:** You talk to the coordinator like a chat. |
| 3.5s | Characters appear one by one: `Write three haiku about Rust programming, then pick the best one.` | **Human speed:** Typing is at natural speed — this is a real person, not a script. |
| 7.5s | User presses Enter. A "thinking" indicator or the user's message appears in the chat history. | **Submission:** One sentence is all it takes. |

**User actions:**
1. Press `c` to enter chat input mode (or `:`)
2. Type the prompt naturally (~50 WPM)
3. Press `Enter` to submit

**Compression:** None. Keep typing at real speed — it's only ~5 seconds of actual typing. This is a "real speed" segment.

---

### Segment 3: Coordinator Responds + Tasks Appear (8s – 15s)

| Time | What Viewer Sees | Teaching Purpose |
|------|-----------------|------------------|
| 8.0s | Chat panel shows coordinator's streaming response: it describes the decomposition plan. | **AI coordination:** The coordinator doesn't do the work — it plans the work. |
| 10.0s | Tasks start appearing in the graph panel as the coordinator runs `wg add`. Four task nodes materialize: `haiku-borrow-checker`, `haiku-cargo-build`, `haiku-unsafe-block`, `judge-haiku`. | **Task graph:** Work is structured as a dependency graph, not a flat list. |
| 12.0s | Edges become visible. Three haiku tasks point to `judge-haiku`. All four tasks show status `(open)`. | **Dependencies:** The fan-out/fan-in pattern is visible. Three independent branches converge into one. |

**User actions:** Watch. No interaction needed — the coordinator is driving.
**Compression:** Heavy. Coordinator think-time (30–60s real) compresses to ~5 seconds. The important thing is seeing tasks appear and edges form.

---

### Segment 4: Navigate the Graph (15s – 22s)

| Time | What Viewer Sees | Teaching Purpose |
|------|-----------------|------------------|
| 15.0s | User presses `Esc` to return to graph focus. Cursor/highlight appears on the first task. | **Navigation:** You can explore the graph with keyboard controls. |
| 16.0s | User presses `↓` (arrow down) to move between tasks. The selected task highlights. | **Selection:** Arrow keys move between tasks. |
| 17.0s | User presses `t` to toggle trace mode. Edge coloring activates: **magenta** edges glow upstream from the selected task, **cyan** edges glow downstream. | **Edge tracing:** The TUI shows you dependency direction with color — magenta = what this task depends on, cyan = what depends on this task. |
| 18.5s | User presses `↓` to select `judge-haiku`. Three magenta edges light up (upstream from the three haiku tasks). | **Convergence point:** At a glance, you see that `judge-haiku` waits for all three writers. |
| 20.0s | User presses `1` to switch to the Detail tab in the right panel. Task description is visible. | **Inspection:** Press a number key to switch inspector tabs — Detail, Log, Messages, etc. |
| 21.0s | User presses `Tab` to return focus to graph. | **Panel switching:** Tab toggles focus between graph and inspector. |

**User actions:**
1. Press `Esc` (exit chat, return to graph focus)
2. Press `↓` twice to navigate to a task
3. Press `t` to enable trace
4. Press `↓` to move to `judge-haiku`
5. Press `1` to view Detail tab
6. Press `Tab` to return to graph

**Compression:** None. These are real-speed keystrokes. The viewer needs to see each action connect to its visual result.

---

### Segment 5: Agents Spawn + Parallel Execution (22s – 32s)

| Time | What Viewer Sees | Teaching Purpose |
|------|-----------------|------------------|
| 22.0s | The three haiku tasks flip from `(open)` → `(in-progress)` nearly simultaneously. Status color changes (e.g., yellow/active indicator). | **Parallel dispatch:** The service auto-assigns agents to ready tasks. No human intervention needed. |
| 24.0s | In the status bar or HUD, agent count shows 3 active agents. Task nodes may show spinning/active indicators. | **Multi-agent:** Three agents are working at the same time. |
| 26.0s | `haiku-borrow-checker` flips to `(done)` — the first agent finishes. Its color changes to done state. | **Progress:** Tasks complete independently as agents finish. |
| 28.0s | `haiku-unsafe-block` completes. Two done, one still in-progress. | **Race:** Agents work at different speeds. The graph shows live progress. |
| 30.0s | `haiku-cargo-build` completes. All three haiku tasks are done. `judge-haiku` immediately flips to `(in-progress)` — a new agent is dispatched. | **Dependency resolution:** The moment all upstream deps are met, the next task starts automatically. |

**User actions:** Watch. Optionally press `2` to view the Log tab and see agent output streaming.
**Compression:** Heavy. Real agent time (~60–120s) compresses to ~10 seconds. The key moments are the status transitions — each flip from open → in-progress → done is a visual event.

---

### Segment 6: Completion + Inspect Results (32s – 40s)

| Time | What Viewer Sees | Teaching Purpose |
|------|-----------------|------------------|
| 32.0s | `judge-haiku` flips to `(done)`. All tasks in the graph show done status. The graph is "all green." | **Completion:** The entire workflow finished — from one sentence to coordinated multi-agent work. |
| 34.0s | User selects `judge-haiku` and presses `2` to view the Log tab. The log shows the winning haiku. | **Results:** You can inspect what each agent produced. Logs are per-task. |
| 37.0s | User presses `↑` to select a haiku task. Log tab shows that agent's haiku. | **Drill-down:** Every task's work is preserved and inspectable. |

**User actions:**
1. Select `judge-haiku` (arrow keys)
2. Press `2` (Log tab)
3. Read the result briefly
4. Press `↑` to select another task, see its log

**Compression:** None for user actions. Brief pause to let the viewer read the winning haiku (~2 seconds).

---

### Segment 7: Exit (40s – 43s)

| Time | What Viewer Sees | Teaching Purpose |
|------|-----------------|------------------|
| 40.0s | User presses `q`. TUI exits cleanly back to the shell. | **Clean exit:** Standard terminal UX. |
| 42.0s | Shell prompt visible. Recording ends. | **Done.** One sentence in, coordinated multi-agent work out, all in the terminal. |

**User actions:** Press `q` to quit.
**Compression:** Cut any trailing shell time.

---

## Complete Action Sequence (Cheat Sheet)

For the human recording the screencast — this is your exact input sequence:

```
1. wg tui                    [launch]
2. c                         [enter chat mode]
3. Write three haiku about Rust programming, then pick the best one.  [type naturally]
4. Enter                     [submit]
5. (wait for tasks to appear in graph)
6. Esc                       [return to graph focus]
7. ↓ ↓                       [navigate to a task]
8. t                         [toggle edge trace]
9. ↓                         [move to judge-haiku]
10. 1                        [Detail tab]
11. Tab                      [back to graph]
12. (wait for agents to finish — watch status transitions)
13. ↓ (or ↑ to judge-haiku)  [select final task]
14. 2                        [Log tab — see the winning haiku]
15. ↑                        [select a haiku task — see its output]
16. q                        [quit]
```

## Time Compression Summary

| Segment | Real Time | Compressed Time | Compression Ratio | Method |
|---------|-----------|-----------------|-------------------|--------|
| 1. TUI Opens | 2–5s | 3s | ~1:1 | Trim startup |
| 2. Type Prompt | 5s | 5s | 1:1 (real speed) | None |
| 3. Coordinator + Tasks | 30–90s | 7s | ~8:1 | `idle_time_limit=2`, then `compress-cast.py` |
| 4. Navigate Graph | 7s | 7s | 1:1 (real speed) | None |
| 5. Agent Execution | 60–120s | 10s | ~10:1 | `compress-cast.py` zones |
| 6. Inspect Results | 8s | 8s | 1:1 (real speed) | None |
| 7. Exit | 2s | 3s | ~1:1 | Trim trailing |
| **Total** | **~3–5 min** | **~43s** | **~5:1** | — |

**Rule:** User actions play at real speed. Waiting plays at 10× speed. The viewer always knows what the human did and can follow along.

## Edge Color Reference

When the trace is active (`t` key) and a task is selected:
- **Magenta** edges: upstream (dependencies — what this task waits for)
- **Cyan** edges: downstream (dependents — what waits for this task)
- **Yellow** edges: cycle back-edges (not shown in this scenario)

## Fallback: Simulated Recording

If the live recording is unreliable (API latency, coordinator misbehavior), use the simulation approach from `record-pancakes-sim.sh`:

1. Pre-create tasks with `wg add` and correct dependencies
2. Pre-populate `chat-history.json` with the user prompt + coordinator response
3. Use `wg claim` / `wg done` with timed sleeps to simulate agent progression
4. Record with `capture-tmux.py`

This gives deterministic timing at the cost of not showing real coordinator chat streaming.

## Recommended Total Length

**43 seconds** (±5s). Under 30s feels rushed and doesn't let the viewer absorb the edge tracing. Over 60s loses attention. The sweet spot is the length of an elevator pitch: one workflow, start to finish, with enough breathing room to read the graph.
