# TB2 Runtime Config Compliance Audit

Audited 2026-04-04. Compared our configuration against official Terminal Bench 2.0 constraints.

## Summary

| # | Constraint | Verdict | Severity |
|---|-----------|---------|----------|
| 1 | Turns: no cap | **NON-COMPLIANT** | HIGH |
| 2 | Timeout: per-task defaults | **NON-COMPLIANT** | HIGH |
| 3 | Token limit: none | COMPLIANT | — |
| 4 | Container resources: per-task | COMPLIANT | — |
| 5 | Internet access: yes | COMPLIANT | — |
| 6 | timeout_multiplier = 1.0 | COMPLIANT | — |

**2 of 6 constraints violated.** Both affect correctness of results.

## Detailed Findings

### 1. Turn Cap — NON-COMPLIANT

**Official**: No turn cap (reference agent uses max_turns=1,000,000).

**Ours**: Hard-capped at 50–300 turns depending on path:
- `reproduce.sh` line 30: `MAX_TURNS=50`, passed to `harbor run --max-turns` (line 111)
- `tb-harness.sh` line 24: `MAX_TURNS=50`
- `adapter.py` line 804: `WorkgraphAgent.__init__` default = 100
- `adapter.py` line 1209: `ConditionDAgent` default = 200
- `adapter.py` line 1223: `ConditionEAgent` default = 300

The reproduce.sh cap of 50 is the binding constraint for all leaderboard runs. This prematurely terminates agents on complex tasks.

**Remediation**: Set `MAX_TURNS=1000000` in `reproduce.sh`. Set adapter class defaults to 1,000,000. Remove `--max-turns` from `harbor run` invocation or set it to the official reference value.

### 2. Per-Task Timeout — NON-COMPLIANT

**Official**: Easy=15min, Medium=15–30min, Hard=30–60min, Extreme=up to 3.3hrs. Defined in each task's `task.toml`. Must use exactly per-task defaults.

**Ours**: Flat 30-minute (1800s) override for all tasks:
- `reproduce.sh` line 29: `TIMEOUT=1800`, passed to `harbor run --timeout` (line 112)
- `tb-harness.sh` line 25: `TIMEOUT=1800`

This caps Extreme tasks (which need up to 3.3hrs = 11,880s) at 30 minutes, and may also override Easy tasks that only get 15 minutes (potentially allowing extra time — also non-compliant).

**Remediation**: Remove `--timeout "$TIMEOUT"` from the `harbor run` command in `reproduce.sh`. Let Harbor use each task's `task.toml` timeout. If Harbor requires a timeout flag, investigate whether it respects task.toml when `--timeout` is omitted.

### 3. Token Limit — COMPLIANT

**Official**: No session-level token limit (some tasks consumed ~100M tokens).

**Ours**: No session-level token budget. The per-response `max_tokens=16384` (`adapter.py` line 985) is a standard generation-length parameter, not a session cap. The agent loop runs until `max_turns` or termination — tokens accumulate without limit.

### 4. Container Resources — COMPLIANT

**Official**: 1 CPU / 2GB RAM / 10GB storage (default); 2 CPU / 4GB RAM for harder tasks.

**Ours**: No resource overrides anywhere in `adapter.py`, `reproduce.sh`, or `tb-harness.sh`. Harbor manages container resources from `task.toml` definitions. Confirmed: `leaderboard-submission/README.md` line 39 marks "No resource overrides" as checked.

### 5. Internet Access — COMPLIANT

**Official**: Yes (API calls, downloading deps). Cannot access TB website/repo.

**Ours**: No internet restrictions imposed. Harbor containers have default network access. Our adapter does not modify network settings.

### 6. timeout_multiplier — COMPLIANT

**Official**: Must be exactly 1.0.

**Ours**: Never set or overridden. `leaderboard-submission/README.md` line 38 confirms `timeout_multiplier = 1.0 (default)`. No references to `timeout_multiplier` found in any config file (confirmed via grep).

## Key Files Examined

- `terminal-bench/reproduce.sh` — main experiment runner (MAX_TURNS=50, TIMEOUT=1800)
- `terminal-bench/tb-harness.sh` — single-task harness (MAX_TURNS=50, TIMEOUT=1800)
- `terminal-bench/wg/adapter.py` — Harbor agent adapter (all 5 conditions, turn caps, model config)
- `terminal-bench/leaderboard-submission/README.md` — submission checklist
- `terminal-bench/leaderboard-submission/*/metadata.yaml` — per-condition metadata
