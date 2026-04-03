# Terminal Bench Condition A Harness

This directory contains the Python adapter implementing Harbor's agent protocol for Terminal Bench evaluation. It provides a **bare agent** (Condition A) configuration to serve as the control group.

## Condition A Characteristics

| Aspect | Value |
|--------|-------|
| **Purpose** | Control group - what everyone has |
| **Tools** | bash, read_file, write_file, edit_file, glob, grep |
| **wg tools** | ❌ NONE |
| **Graph awareness** | ❌ NONE |
| **Journal/Resume** | ❌ DISABLED |
| **Task decomposition** | ❌ NONE |
| **System prompt** | Minimal (tool descriptions + task instruction) |

## Usage

### With Harbor Framework

```bash
# Install the adapter package
cd /home/erik/workgraph/terminal-bench
pip install -e .

# Run via Harbor
harbor run \
  --agent-import-path wg.adapter:WorkgraphAgent \
  -m minimax/minimax-m2.7 \
  --task-ids task-42 \
  -k 1
```

### Direct Python API

```python
from wg.adapter import Agent, run_task, TaskResult

# Simple one-liner
result = run_task(
    task_instruction="Fix the bug in module X",
    model="minimax/minimax-m2.7",
    max_turns=100,
)

print(f"Success: {result.success}")
print(f"Turns: {result.turns}")
print(f"Output: {result.output}")
```

### Using the Agent Class Directly

```python
from wg.adapter import Agent

agent = Agent(
    model="minimax/minimax-m2.7",
    max_turns=100,
    timeout_seconds=1800,
)

result = agent.run(
    task_instruction="Your task here",
    working_dir="/path/to/workspace",
)
```

## Architecture

The adapter:
1. Creates a temporary workgraph directory
2. Writes a Condition A bundle (`condition-a.toml`) with bash + file tools only
3. Writes a minimal system prompt with tool descriptions
4. Calls `wg native-exec` with `--exec-mode condition-a --no-resume`
5. Captures output from NDJSON logs
6. Returns standardized `TaskResult`

## Bundle Definition

The Condition A bundle is defined as:

```toml
name = "condition-a"
description = "Terminal Bench Condition A: Bare agent control group. No wg tools, no graph awareness."
tools = ["bash", "read_file", "write_file", "edit_file", "glob", "grep"]
context_scope = "clean"
```

## Comparison with Condition B

| Feature | Condition A | Condition B |
|---------|------------|------------|
| Tools | bash + file | bash + file + wg tools |
| Graph awareness | ❌ | ✅ |
| Journal/Resume | ❌ | ✅ |
| Task decomposition | ❌ | ✅ |
| External memory | ❌ | ✅ |
| Coordinator spawning | ❌ | ✅ |

## Files

- `wg/adapter.py` - Main adapter implementation
- `wg/__init__.py` - Package init
- `pyproject.toml` - Python package config
- `README.md` - This file
