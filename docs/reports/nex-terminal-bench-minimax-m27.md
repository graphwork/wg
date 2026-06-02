# Nex Minimax M2.7 Terminal-Bench Smoke

Date: 2026-06-02

Task: `bench-nex-minimax-terminal`

## Scope

This was a bounded Terminal-Bench-style smoke benchmark of Nex as the executor
harness under test. Codex only orchestrated the WG task and did not solve the
fixture. The measured run went through `nex --wg --eval-mode`.

I used the same smallest local fixture family as the completed DeepSeek
baseline:

`terminal-bench/tasks/condition-a-calibration/01-file-ops-easy.txt`

I did not run Harbor, Docker Terminal-Bench, or another external agent CLI. The
local text fixture is Terminal-Bench-like and is the same fixture used by
`bench-nex-deepseek-terminal`.

## Key Handling

- I checked OpenRouter availability with `wg endpoints test openrouter`.
- I did not print, echo, pass, or commit the OpenRouter API key.
- I did not use `set -x`, `env`, `printenv`, `ps e`, `/proc/*/environ`, or a
  command-line API key.
- The Nex command used the configured `openrouter` endpoint and did not pass
  `-k/--api-key`.

## Environment

| Field | Value |
| --- | --- |
| Repo worktree | `/home/bot/wg/.wg-worktrees/agent-55` |
| WG dir used by Nex from `/tmp` | `/home/bot/wg/.wg` |
| Nex binary | `/home/bot/.cargo/bin/nex` |
| Nex version | `nex 0.1.0` |
| Endpoint under test | `openrouter` |
| WG/Nex model under test | `openrouter:minimax/minimax-m2.7` |
| Expected raw OpenRouter model | `minimax/minimax-m2.7` |
| Fixture | `terminal-bench/tasks/condition-a-calibration/01-file-ops-easy.txt` |

The eval-mode JSON summary did not include the raw request body/model. Nex
eval-mode also intentionally skips launcher-history recording. The raw model ID
above is therefore recorded as the expected OpenRouter request model for this
provider-prefixed WG/Nex model string, not as a value observed in the final eval
JSON line.

## Endpoint Preflight

Command shape:

```bash
timeout 45s wg endpoints test openrouter
```

Result:

| Field | Value |
| --- | --- |
| Exit status | 0 |
| Elapsed | 1s |
| Connectivity | OK |
| Models | OK |
| Authentication | OK |
| Generation | OK |

The preflight printed the OpenRouter models URL and status, but no credential
value.

## Eval Attempt

I ran one Nex attempt total. I did not run a second attempt because the first
attempt reached OpenRouter, completed within the strict bounds, and produced
enough signal for the matrix while keeping cost bounded.

All attempt bounds:

| Bound | Value |
| --- | --- |
| External hard timeout | `180s` |
| Nex max turns | `8` |
| Attempts used | 1 of 2 allowed |
| Tool surface | `--minimal-tools` |
| Streaming idle timeout | `60s` |

Command shape:

```bash
cd /tmp
WG_DIR=/home/bot/wg/.wg timeout 180s nex --wg --eval-mode \
  -e openrouter \
  -m openrouter:minimax/minimax-m2.7 \
  --max-turns 8 \
  --idle-timeout-secs 60 \
  --minimal-tools \
  "$(cat /home/bot/wg/.wg-worktrees/agent-55/terminal-bench/tasks/condition-a-calibration/01-file-ops-easy.txt)"
```

No secret was present in the command.

Result:

| Field | Value |
| --- | --- |
| Exit status | 0 |
| Elapsed | 51s |
| Nex JSON status | `ok` |
| Exit reason | `end_turn` |
| Turns | 8 |
| Input tokens | 19,542 |
| Output tokens | 1,161 |
| Stdout bytes | 93 |
| Stderr bytes | 0 |

Final Nex eval JSON:

```json
{"status":"ok","turns":8,"input_tokens":19542,"output_tokens":1161,"exit_reason":"end_turn"}
```

## Fixture Result

Independent post-run checks showed the requested files were created and had the
expected functional content:

| Check | Result |
| --- | --- |
| `/tmp/project/src/main.py` | Present; contains `print("Hello, World!")` |
| `/tmp/project/src/utils.py` | Present; contains `def add(a, b): return a + b` |
| `/tmp/project/src/tests/test_utils.py` | Present; imports `utils.add` and tests `add(2, 3) == 5` |
| `/tmp/project/data/config.json` | Present and valid JSON |
| `/tmp/project/README.md` | Present with the requested heading/body |
| `/tmp/project/.gitignore` | Present with the requested ignore lines |
| `python3 -c "import json; json.load(...)"` | Exit 0 |
| `cd /tmp/project && python3 -m pytest src/tests/test_utils.py -v` | Exit 0; 1 test passed |

Strict file-count result:

| Count | Value |
| --- | --- |
| `find /tmp/project -type f \| wc -l` after pytest | 13 |
| Requested files excluding pytest/pycache artifacts | 6 |

The fixture completed functionally: requested source files were present, JSON
validated, and the test passed. The strict "should be 6" count failed after
pytest because verification created `.pytest_cache` and `__pycache__` files.
This is a harness-policy caveat rather than a model-routing failure: the
benchmark should either count before running pytest, disable cache/bytecode
generation, or exclude verification artifacts from the count.

## Process Cleanup

The generated `/tmp/project` tree was removed after collecting the checks.

Final process check used process names only, not environments or command
arguments:

```bash
ps -eo pid,ppid,stat,etime,comm |
  awk 'NR == 1 || $5 ~ /^(nex|tb|pytest|pip|pip3|apt-get)$/ {print}'
```

Result: header row only; no lingering `nex`, Terminal-Bench, pytest, pip, or
apt-get benchmark child processes were present by this check.

## Comparison To DeepSeek Baseline

The completed `bench-nex-deepseek-terminal` baseline used the same
`01-file-ops-easy.txt` fixture with `openrouter:deepseek/deepseek-v3.2`.
That baseline needed three attempts with larger later bounds (`--max-turns` 4,
10, and 14; outer timeouts 180s, 240s, and 300s). All three DeepSeek attempts
exited on `max_turns`; the third reached a passing venv pytest run but failed
the exact file count because the venv/cache artifacts inflated the count to
1,955 files.

This Minimax run was stricter and cheaper: one attempt, `timeout 180s`,
`--max-turns 8`, exit 0 in 51s, and 20,703 total reported tokens. It reached a
clean `end_turn` and passed the functional fixture checks, but it shows the
same benchmark-design issue around counting files after verification has
created cache artifacts.

## Validation Checklist

- OpenRouter endpoint or credential availability checked without printing or
  committing the key: yes, `wg endpoints test openrouter` exited 0.
- Nex eval-mode run attempted with `openrouter:minimax/minimax-m2.7`: yes,
  measured through `nex --wg --eval-mode`.
- External hard timeout no more than 180s and `--max-turns <= 8`: yes,
  `timeout 180s` and `--max-turns 8`.
- No more than two Nex attempts: yes, one attempt total.
- Result captures exit status, elapsed time, model ID, command shape, fixture,
  timeout/max-turn settings, and completion status: yes.
- No lingering benchmark/Nex child processes remain: yes, checked without
  process environments or command arguments.
- Compared briefly to the completed DeepSeek v3.2 baseline: yes.
- Git diff reviewed for unrelated churn: yes; only this report is intended for
  staging, with `.wg` metadata left untracked.
