# Heroview Screencast Script v2

**Target length:** 40–50 seconds (compressed). Raw recording ~2–4 minutes.
**Terminal:** 65×38 (matches recording harness).
**Scenario:** "Write haiku about the seasons" — coordinator decomposes, agents produce readable output.

---

## Problems Fixed (from v1)

| # | Problem | Fix |
|---|---------|-----|
| 1 | Typing too slow | All input at 200+ WPM (≤1.5s per command). Total typing <5s. |
| 2 | Tasks appear instantly | Stagger `wg add` calls with 0.8s delays; viewer sees graph grow node by node. |
| 3 | What gets built is vague | Haiku — concrete, readable, fits in a log line. Output is *the point*. |
| 4 | Output flashes by too fast | Haiku output lingers 5s on screen. Final graph survey lingers 3s. |
| 5 | Unnecessary screen clear | No `clear` before `wg tui`. Just type it after previous output scrolls. |

---

## What Gets Built

The user asks: **"Write haiku about the four seasons"**

The coordinator decomposes into 6 tasks:

```
spring-haiku ───────┐
summer-haiku ───────┼──► compile-collection
autumn-haiku ───────┤
winter-haiku ───────┘       │
                        format-output
```

**Why this scenario works:**
- **Concrete output:** Each haiku task produces a readable 3-line poem. The viewer can *read* what was built.
- **Clean graph shape:** 4-way fan-out → convergence → sequential tail. Instantly legible.
- **Task names are self-documenting:** `spring-haiku`, `winter-haiku` — no explanation needed.
- **Short tasks:** Haiku agents finish fast, so real recording stays under 4 minutes.

### Task definitions

| ID | Title | After | Description |
|----|-------|-------|-------------|
| `spring-haiku` | Spring haiku | — | Write a haiku about spring |
| `summer-haiku` | Summer haiku | — | Write a haiku about summer |
| `autumn-haiku` | Autumn haiku | — | Write a haiku about autumn |
| `winter-haiku` | Winter haiku | — | Write a haiku about winter |
| `compile-collection` | Compile collection | spring-haiku, summer-haiku, autumn-haiku, winter-haiku | Gather all four haiku into a formatted collection |
| `format-output` | Format output | compile-collection | Final formatting and presentation |

### Sample haiku output (injected for deterministic recording)

**spring-haiku log:**
```
Cherry blossoms fall
Soft rain wakes the sleeping earth
New leaves reach for light
```

**summer-haiku log:**
```
Heat shimmers on stone
Cicadas drone through long days
Thunder breaks the calm
```

**autumn-haiku log:**
```
Crimson leaves descend
Cold wind strips the maple bare
Geese trace southern lines
```

**winter-haiku log:**
```
Snow blankets the field
Bare branches etch gray silence
Breath crystallizes
```

**compile-collection log:**
```
Four Seasons — A Haiku Collection

  Spring:
    Cherry blossoms fall
    Soft rain wakes the sleeping earth
    New leaves reach for light

  Summer:
    Heat shimmers on stone
    Cicadas drone through long days
    Thunder breaks the calm

  Autumn:
    Crimson leaves descend
    Cold wind strips the maple bare
    Geese trace southern lines

  Winter:
    Snow blankets the field
    Bare branches etch gray silence
    Breath crystallizes
```

---

## Pacing Priorities

Ranked most → least interesting (more screen time = more interesting):

1. **Graph growth** (tasks appearing one by one) — 6–8s compressed
2. **Task progression** (open → in-progress → done, parallel execution) — 8–10s compressed
3. **Haiku output reveal** (the payoff — readable poems on screen) — 6–8s compressed
4. **Setup commands** (`wg service start`, `wg status`) — 3–4s compressed
5. **User input** (typed fast, <5s total across entire screencast) — 1–2s compressed

---

## Detailed Storyboard

### Phase 0: CLI Orient (0s – 4s compressed)

**What the viewer sees:** Shell prompt. Fast commands. Output appears.

| Compressed Time | Action | Duration |
|-----------------|--------|----------|
| 0.0s | Shell prompt visible | — |
| 0.3s | Type `wg service start` (200 WPM, ~0.8s) | 0.8s |
| 1.1s | Enter → output: "Service started. Coordinator active." | 1.0s |
| 2.1s | Type `wg status` (200 WPM, ~0.4s) | 0.4s |
| 2.5s | Enter → output: project info, agent count, service health | 1.5s |
| 4.0s | End of CLI phase | — |

**Typing budget:** ~1.2s total for 2 commands.

**Key decisions:**
- No `clear` between commands — output just scrolls.
- `wg service start` shown because it's the first thing any user would do.
- `wg status` shown briefly to establish: "this tool gives you project state."
- No `wg add` or `wg viz` in CLI phase — save the graph reveal for the TUI.

---

### Phase 1: Launch TUI (4s – 7s compressed)

**What the viewer sees:** `wg tui` typed fast, TUI fills the screen.

| Compressed Time | Action | Duration |
|-----------------|--------|----------|
| 4.0s | Type `wg tui` (200 WPM, ~0.3s) — **no screen clear first** | 0.3s |
| 4.3s | Enter → TUI renders | 0.7s |
| 5.0s | TUI visible: empty graph (top), chat panel (bottom), status bar "● LIVE" | 2.0s |
| 7.0s | End of launch phase | — |

**Typing budget:** ~0.3s.

**Key decisions:**
- Type `wg tui` right after `wg status` output — no clear. Previous output scrolls away naturally as TUI takes over.
- 2s pause lets the viewer orient to the TUI layout.
- Shrink inspector panel (Shift+I ×3) to make graph prominent — same as interaction screencast.

---

### Phase 2: Chat Prompt (7s – 9s compressed)

**What the viewer sees:** User types a short prompt, submits it.

| Compressed Time | Action | Duration |
|-----------------|--------|----------|
| 7.0s | Press `c` → chat input activates (cursor visible) | 0.5s |
| 7.5s | Type `Write haiku about the four seasons` (200 WPM, ~1.2s) | 1.2s |
| 8.7s | Press Enter → message appears in chat | 0.3s |
| 9.0s | End of prompt phase | — |

**Typing budget:** ~1.2s.

**Key decisions:**
- Prompt is 39 characters — short enough to type in ~1.2s at 200 WPM.
- The prompt is immediately understandable: "oh, it's going to write haiku."
- No pause after typing — submit immediately. The viewer's attention should shift to the graph.

---

### Phase 3: Graph Growth (9s – 17s compressed)

**This is the most important phase.** The viewer watches the coordinator decompose the request into tasks. Tasks appear one at a time in the graph panel.

| Compressed Time | Action | Duration |
|-----------------|--------|----------|
| 9.0s | Coordinator "thinking" indicator in chat | 1.0s |
| 10.0s | Chat response starts streaming: "I'll create a haiku for each season..." | 1.0s |
| 11.0s | `spring-haiku` appears in graph | 0.8s |
| 11.8s | `summer-haiku` appears | 0.8s |
| 12.6s | `autumn-haiku` appears | 0.8s |
| 13.4s | `winter-haiku` appears | 0.8s |
| 14.2s | `compile-collection` appears with edges from all 4 haiku tasks | 1.0s |
| 15.2s | `format-output` appears with edge from compile-collection | 0.8s |
| 16.0s | Full graph visible — 6 tasks, fan-out + convergence | 1.0s |
| 17.0s | End of graph growth phase | — |

**Implementation:**
- Background: inject tasks via `wg add` with 0.8s delays between each.
- The 0.8s stagger gives each task a clear "appear" moment in the TUI.
- Total stagger: 6 tasks × 0.8s = 4.8s real → compressed to ~6s.

**What makes this visually interesting:**
- The graph *grows*. Nodes materialize one at a time. Edges form.
- The 4-way fan-out is immediately legible — four tasks branching from nothing.
- The convergence into `compile-collection` is visually dramatic — four edges joining one node.
- The viewer thinks: "one sentence, and it built a structured plan."

---

### Phase 4: Task Progression (17s – 27s compressed)

**What the viewer sees:** Tasks go through status transitions. Parallel execution is visible.

| Compressed Time | Action | Duration |
|-----------------|--------|----------|
| 17.0s | Press `Esc` → exit chat, graph focus | 0.5s |
| 17.5s | `spring-haiku` → in-progress (color change) | 0.5s |
| 18.0s | `summer-haiku` → in-progress | 0.3s |
| 18.3s | `autumn-haiku` → in-progress | 0.3s |
| 18.6s | `winter-haiku` → in-progress | 0.3s |
| 18.9s | All 4 haiku tasks active — status bar shows "● 4 agents" | 1.5s |
| 20.4s | Navigate down with ↓↓ — highlight moves through tasks | 1.5s |
| 21.9s | `spring-haiku` → done | 0.5s |
| 22.4s | `summer-haiku` → done | 0.8s |
| 23.2s | `autumn-haiku` → done | 0.8s |
| 24.0s | `winter-haiku` → done — all 4 done | 0.5s |
| 24.5s | `compile-collection` → in-progress (auto-dispatched) | 1.0s |
| 25.5s | `compile-collection` → done | 0.8s |
| 26.3s | `format-output` → in-progress → done | 0.7s |
| 27.0s | All tasks done — graph is "all green" | — |

**Implementation:**
- Background: `wg claim` then `wg done` with timed delays.
- Claim all 4 haiku tasks simultaneously (they're parallel), then complete them staggered.
- Inject haiku log entries before marking done: `wg log spring-haiku "Cherry blossoms fall..."`.

**What makes this visually interesting:**
- Four tasks flip to in-progress nearly simultaneously — visible parallelism.
- Status bar shows 4 agents working at once.
- The staggered completion creates a "race" feel.
- The instant `compile-collection` starts after all 4 finish — automatic dependency resolution.

---

### Phase 5: Results Reveal (27s – 35s compressed)

**This is the payoff.** The viewer reads actual haiku. Output lingers on screen.

| Compressed Time | Action | Duration |
|-----------------|--------|----------|
| 27.0s | Navigate to `compile-collection` with ↓ keys | 1.0s |
| 28.0s | Press `2` → Log tab | 0.5s |
| 28.5s | **Log shows the full haiku collection — all four poems visible** | **5.0s** |
| 33.5s | Navigate to `spring-haiku` with ↑ keys | 0.8s |
| 34.3s | Log tab updates — shows the individual spring haiku | 0.7s |
| 35.0s | End of results reveal | — |

**Implementation:**
- The compile-collection task's log contains the full formatted collection (see sample above).
- **5 full seconds** on the collection output — long enough for the viewer to read at least 2 haiku.
- Then a quick peek at an individual task's log to show per-task output preservation.

**What makes this visually interesting:**
- The viewer can *read the poems*. This is concrete, tangible output — not vague "API endpoint" text.
- The haiku are charming and memorable. They give the demo personality.
- The linger time lets the viewer absorb: "agents actually produced this."

---

### Phase 6: Final Survey + Exit (35s – 40s compressed)

| Compressed Time | Action | Duration |
|-----------------|--------|----------|
| 35.0s | Navigate back to top of graph (Home or ↑↑↑) | 1.0s |
| 36.0s | Slow scroll through completed graph — all green | 2.0s |
| 38.0s | Press `q` → TUI exits, shell prompt returns | 1.0s |
| 39.0s | Hold on shell prompt (clean ending) | 1.0s |
| 40.0s | End of recording | — |

---

## Timing Summary

| Phase | Compressed Time | Content |
|-------|-----------------|---------|
| 0: CLI Orient | 0–4s (4s) | `wg service start`, `wg status` |
| 1: Launch TUI | 4–7s (3s) | `wg tui`, TUI renders |
| 2: Chat Prompt | 7–9s (2s) | Type + submit prompt |
| 3: Graph Growth | 9–17s (8s) | 6 tasks appear one by one |
| 4: Task Progression | 17–27s (10s) | Parallel execution, all tasks complete |
| 5: Results Reveal | 27–35s (8s) | Haiku output on screen, **5s linger** |
| 6: Survey + Exit | 35–40s (5s) | Final graph review, clean exit |
| **Total** | **40s** | — |

### Typing Budget

| Command | Characters | Time at 200 WPM |
|---------|-----------|-----------------|
| `wg service start` | 16 | 0.8s |
| `wg status` | 9 | 0.4s |
| `wg tui` | 6 | 0.3s |
| `Write haiku about the four seasons` | 39 | 1.2s |
| **Total** | **70** | **2.7s** |

All typing segments total **2.7 seconds** — well under the 5s budget.

---

## Implementation Notes for `record-heroview-v2.py`

### Opening sequence (Phase 0)

```python
# Phase 0: CLI Orient
h.wait_for("$", timeout=5)
# NO screen clear — start from natural shell prompt

h.type_naturally("wg service start", wpm=200)
h.send_keys("Enter")
h.sleep(2)  # Let output render

h.type_naturally("wg status", wpm=200)
h.send_keys("Enter")
h.sleep(2)  # Let output render

# Phase 1: Launch TUI — NO clear, just type after previous output
h.type_naturally("wg tui", wpm=200)
h.send_keys("Enter")
```

### Graph growth (Phase 3)

```python
# Inject tasks with stagger — viewer sees graph grow
TASKS = [
    ("Spring haiku", "spring-haiku", None, "Write a haiku about spring"),
    ("Summer haiku", "summer-haiku", None, "Write a haiku about summer"),
    ("Autumn haiku", "autumn-haiku", None, "Write a haiku about autumn"),
    ("Winter haiku", "winter-haiku", None, "Write a haiku about winter"),
    ("Compile collection", "compile-collection",
     "spring-haiku,summer-haiku,autumn-haiku,winter-haiku",
     "Gather all four haiku into a formatted collection"),
    ("Format output", "format-output", "compile-collection",
     "Final formatting and presentation"),
]

for title, tid, after, desc in TASKS:
    cmd = ["add", title, "--id", tid, "-d", desc]
    if after:
        cmd.extend(["--after", after])
    wg(*cmd)
    time.sleep(0.8)  # Stagger for visible graph growth
```

### Haiku injection (before Phase 5)

```python
HAIKU = {
    "spring-haiku": "Cherry blossoms fall\nSoft rain wakes the sleeping earth\nNew leaves reach for light",
    "summer-haiku": "Heat shimmers on stone\nCicadas drone through long days\nThunder breaks the calm",
    "autumn-haiku": "Crimson leaves descend\nCold wind strips the maple bare\nGeese trace southern lines",
    "winter-haiku": "Snow blankets the field\nBare branches etch gray silence\nBreath crystallizes",
}

COLLECTION = """Four Seasons — A Haiku Collection

  Spring:
    Cherry blossoms fall
    Soft rain wakes the sleeping earth
    New leaves reach for light

  Summer:
    Heat shimmers on stone
    Cicadas drone through long days
    Thunder breaks the calm

  Autumn:
    Crimson leaves descend
    Cold wind strips the maple bare
    Geese trace southern lines

  Winter:
    Snow blankets the field
    Bare branches etch gray silence
    Breath crystallizes"""

# Inject haiku into task logs before marking done
for tid, haiku in HAIKU.items():
    for line in haiku.split("\n"):
        wg("log", tid, line)
    wg("done", tid)
    time.sleep(0.3)

# Compile collection
for line in COLLECTION.split("\n"):
    if line.strip():
        wg("log", "compile-collection", line)
wg("done", "compile-collection")

wg("log", "format-output", "Collection formatted and ready")
wg("done", "format-output")
```

### Compression parameters

```python
# compress-heroview-v2.py scene params
SCENE_PARAMS = {
    "cli":          (3.0, 2.0, 0.15, 0.10),  # Fast typing, short waits
    "launch":       (3.0, 2.0, 0.20, 0.15),  # Fast typing
    "chat":         (3.0, 1.5, 0.30, 0.20),  # Fast typing, moderate coordinator
    "graph_growth": (1.0, 1.0, 0.60, 0.50),  # SLOW — viewer must see each task appear
    "progression":  (1.5, 1.5, 0.25, 0.20),  # Moderate — keep status transitions
    "results":      (1.0, 1.0, 0.80, 0.60),  # SLOW — let viewer read the haiku
    "survey":       (1.5, 2.0, 0.20, 0.15),  # Moderate navigation
}
```

**Critical compression rule for Phase 5 (results):** The linger on `compile-collection`'s log tab must survive compression. Set a minimum dwell of 5s for the results scene. Do NOT compress the pause between "log tab opens" and "navigate away" — the haiku must be readable.

---

## CLAUDE.md for Demo Project

```markdown
# Haiku Seasons Demo

When the user asks to write haiku about the seasons, decompose into these tasks:

1. spring-haiku — Write a haiku about spring (no dependencies)
2. summer-haiku — Write a haiku about summer (no dependencies)
3. autumn-haiku — Write a haiku about autumn (no dependencies)
4. winter-haiku — Write a haiku about winter (no dependencies)
5. compile-collection — Gather all four haiku (after spring-haiku, summer-haiku, autumn-haiku, winter-haiku)
6. format-output — Final formatting (after compile-collection)

Use exactly these task IDs. Create all 6 tasks using wg add with --after dependencies.
Tasks 1-4 MUST be parallel (no dependencies). Keep your response brief.
Do NOT create any other tasks or subtasks.
```

---

## Differences from Previous Designs

| Aspect | Hero v2 (storyboard-v2.md) | Interaction (interaction-flow.md) | **This design (heroview v2)** |
|--------|---------------------------|----------------------------------|-------------------------------|
| Opening | TUI direct | CLI (wg status, wg viz) → TUI | **CLI (service start, status) → TUI** |
| Typing speed | ~50 WPM (natural) | 150 WPM | **200 WPM (≤1.5s per command)** |
| What gets built | Rust haiku (3 tasks) | Haiku news pipeline (8+3 tasks) | **Season haiku (6 tasks)** |
| Graph complexity | Simple fan-out (3→1) | Complex pipeline + 2nd round | **Clean fan-out (4→1→1)** |
| Output visibility | Log tab, brief | Results reveal scene, ~4s | **5s linger on collection, readable poems** |
| Screen clear | Yes, before TUI | Yes (C-l) | **No — natural scroll** |
| Total typing time | ~5s | ~3s (2 prompts) | **<3s (4 commands)** |
| Second chat round | No | Yes (roast mode) | **No — keep it simple, one arc** |
| Target length | 43s | 50-60s | **40s** |

**Key simplification:** One clean arc. No second chat round. The story is: type one sentence → watch graph grow → watch agents work → read the output. Done.

---

## Validation Checklist

- [x] Concrete storyboard with timing for each phase (6 phases, all timed)
- [x] The 'what gets built' is specific (haiku) and produces visible, readable output (poems)
- [x] Typing segments total <5 seconds (2.7s total)
- [x] Graph growth is scripted to be visible (0.8s stagger between each task)
- [x] Final output lingers on screen for at least 3-5 seconds (5s on collection)
- [x] Total screencast length estimated (40s compressed, aim 30-60s range)
