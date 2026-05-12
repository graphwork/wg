# How to Submit to Terminal Bench 2.0 Leaderboard

Adapted from the Harbor HOWTO for wg's multi-condition experiment.

## Submission Format

Each condition is submitted as an independent entry to the
[Terminal-Bench 2.0 leaderboard](https://huggingface.co/datasets/harborframework/terminal-bench-2-leaderboard).

### Directory Structure (per condition)

```
submissions/terminal-bench/2.0/<agent-name>__<model>/
  metadata.yaml          # Hand-written (see template below)
  <job-name>/            # Harbor's raw output directory
    config.json
    <task>__<hash>/      # One directory per trial
      result.json        # Harbor-generated result
      config.json        # Trial-level config
```

### metadata.yaml Template

```yaml
agent_url: https://github.com/graphwork/wg
agent_display_name: "wg Condition X (description)"
agent_org_display_name: "wg"

models:
  - model_name: minimax-m2.7
    model_provider: openrouter
    model_display_name: "MiniMax-M2.7"
    model_org_display_name: "MiniMax"
```

## Our Conditions

| Condition | Agent | Description |
|-----------|-------|-------------|
| A | `ConditionAAgent` | Bare agent, no wg context (control) |
| B | `ConditionBAgent` | wg stigmergic context |
| C | `ConditionCAgent` | Enhanced planning + snapshots |
| F | `ConditionFAgent` | Full wg context + surveillance loops |

All conditions use **Minimax M2.7** via OpenRouter. The core thesis comparison
is A vs F: same model, dramatically different results with wg coordination.

## Running the Benchmark

```bash
# Install Harbor
pip install harbor-framework

# Single task test (development)
harbor run -d terminal-bench/terminal-bench-2 \
  --agent-import-path wg.adapter:ConditionAAgent \
  -m minimax/minimax-m2.7 \
  --task-ids task-42 \
  -k 1

# Full submission run (89 tasks x 5 trials)
harbor run -d terminal-bench/terminal-bench-2 \
  --agent-import-path wg.adapter:ConditionAAgent \
  -m minimax/minimax-m2.7 \
  -k 5
```

Replace `ConditionAAgent` with the appropriate agent class for each condition.

## Preparing Submissions

Use the prepare script to copy trial data into submission format:

```bash
bash terminal-bench/prepare-leaderboard.sh [--dry-run]
```

This copies `result.json` and `config.json` files from experiment result directories
into the leaderboard submission structure.

## Submitting to HuggingFace

1. Fork: `https://huggingface.co/datasets/harborframework/terminal-bench-2-leaderboard`
2. Copy each condition directory into `submissions/terminal-bench/2.0/`
3. Open a Pull Request
4. Bot auto-validates:
   - `timeout_multiplier` must be `1.0`
   - No timeout or resource overrides
   - Minimum **5 trials per task** (hard requirement)
   - Valid `result.json` in every trial directory
5. Maintainer reviews and merges
6. Results appear at https://www.tbench.ai/leaderboard/terminal-bench/2.0

## Rules

- Use default timeouts (per-task, defined in task.toml: 15 min to 3.3 hours)
- Use default resources (1-2 CPUs, 2-4 GB RAM, 10 GB storage per task)
- No overrides of any kind
- Multi-agent and orchestrator architectures are allowed
- Retry/convergence loops within a trial are allowed
- Agents cannot access tbench.ai or the terminal-bench GitHub repo
- **Scrub API keys and proprietary prompts** — submissions are public

## Current Status

See `terminal-bench/submissions/` for prepared submission data:
- `condition-a/` — pilot-a-89 results (89 tasks × 1 trial, Harbor format)
- `condition-f/` — pilot-f-89 results (18 tasks × 5 trials, wg runner format)

**Not yet submittable**: Leaderboard requires 89 tasks × 5 trials per condition.
Additional trial runs needed before final submission.

## Contact

- Questions: alexgshaw64@gmail.com
- Discord: https://discord.gg/6xWPKhGDbA
