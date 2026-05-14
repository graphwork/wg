# Hero Screencast Recording

Record short (~30-45s after compression) screencasts showing WG's TUI
chat agent handling real multi-agent workflows.

## Prerequisites

- `wg` installed (`cargo install --path .` from repo root)
- `asciinema` installed (2.x)
- Claude API key set in environment

## Quick Start

```bash
# 1. Set up a clean demo project
./setup-demo.sh

# 2. Record a scenario
./record.sh heist     # Plan a Heist Movie Night
./record.sh haiku     # Write a Haiku Pipeline
./record.sh pancakes  # Debug a Pancake Recipe
```

## Scenarios

### 1. Heist Movie Night

**Prompt:** `Plan a heist movie night for the team — snacks, movie picks, and a debate.`

**Graph shape:** Two parallel research tasks → pick final movie → send invitation

Shows: parallel fan-out, convergence, relatable task names.

### 2. Haiku Pipeline

**Prompt:** `Write three haiku about Rust programming, then pick the best one.`

**Graph shape:** Three parallel haiku tasks → judge picks the best

Shows: 3-way fan-out, gather pattern, fun creative task.

### 3. Debug Pancakes

**Prompt:** `My pancakes are flat. Diagnose the problem and fix my recipe.`

**Graph shape:** diagnose → parallel fixes (recipe + presentation) → taste test

Shows: pipeline + fan-out + convergence, "debugging" metaphor.

## Recording Workflow (Manual)

If you prefer manual control:

```bash
cd /tmp/wg-hero-demo

# Start recording with 2s idle time cap
asciinema rec --idle-time-limit 2 recording.cast

# Inside the recording session:
wg tui

# In the TUI:
# 1. Type your prompt in the chat input
# 2. Press Enter to submit
# 3. Watch agents spawn and tasks flow through the graph
# 4. When done, Ctrl-C to exit TUI
# Then Ctrl-D to stop recording
```

## Post-Processing Tips

- **Preview:** `asciinema play recordings/heist-*.cast`
- **Upload:** `asciinema upload recordings/heist-*.cast`
- **Convert to GIF:** Use [agg](https://github.com/asciinema/agg) or [svg-term-cli](https://github.com/marionebl/svg-term-cli)
- **Trim:** `asciinema cut --start 2.0 --end 45.0 input.cast -o trimmed.cast`
- **Target length:** 30-45 seconds after idle-time compression

## Recordings Directory

Recordings are saved to `screencast/recordings/` with timestamped filenames:
`{scenario}-{YYYYMMDD-HHMMSS}.cast`

## Troubleshooting

- **"Demo project not found"**: Run `./setup-demo.sh` first
- **Agents not spawning**: Check API key is set, check `wg service status`
- **TUI too wide/narrow**: Set terminal to 120 columns for best results
- **Recording too long**: Tasks are real LLM calls — timing depends on model speed.
  Use `--idle-time-limit 2` (default in record.sh) to compress wait time.
