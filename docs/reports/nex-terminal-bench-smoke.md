# Nex DeepSeek Terminal-Bench Smoke

Date: 2026-06-01

Task: `bench-nex-deepseek-terminal`

## Scope

This was a bounded smoke benchmark of Nex as a Terminal-Bench-style executor
harness. The goal was not to maximize score. I used the small local calibration
fixture at:

`terminal-bench/tasks/condition-a-calibration/01-file-ops-easy.txt`

I did not run Harbor or Docker Terminal-Bench. A runnable local text fixture was
available, and the task explicitly allowed a very small or local
Terminal-Bench-like fixture.

## Key Handling

- I checked only whether `~/.openrouter.key` existed; I did not print or commit
  its contents.
- The Nex eval runs succeeded in reaching OpenRouter through the configured
  `openrouter` endpoint. I did not pass an API key on the command line.
- No OpenRouter key appears in this report.

## Environment

- Repo worktree: `/home/bot/wg/.wg-worktrees/agent-48`
- Nex binary: `/home/bot/.cargo/bin/nex`
- Nex version: `nex 0.1.0`
- Endpoint under test: `openrouter`
- Model under test: `openrouter:deepseek/deepseek-v3.2`
- Fixture: `terminal-bench/tasks/condition-a-calibration/01-file-ops-easy.txt`

## Endpoint Preflight

Command:

```bash
timeout 45s wg endpoints test openrouter
```

Result:

| Field | Value |
| --- | --- |
| Exit status | 0 |
| Elapsed | 3s |
| Connectivity | OK |
| Models | OK |
| Authentication | OK |
| Generation | OK |

## Eval Runs

All eval runs used:

- `nex --wg --eval-mode`
- `-e openrouter`
- `-m openrouter:deepseek/deepseek-v3.2`
- `--minimal-tools`
- `--idle-timeout-secs 60`
- An outer GNU `timeout`

The command wrappers captured `exit_status`, elapsed seconds, and the final Nex
JSON summary line. The process was monitored in the foreground through
completion; no background benchmark process was left running.

### Attempt 1: Worktree CWD, 4 Turns

Command:

```bash
timeout 180s nex --wg --eval-mode \
  -e openrouter \
  -m openrouter:deepseek/deepseek-v3.2 \
  --max-turns 4 \
  --idle-timeout-secs 60 \
  --minimal-tools \
  "$(cat terminal-bench/tasks/condition-a-calibration/01-file-ops-easy.txt)"
```

Result:

| Field | Value |
| --- | --- |
| Exit status | 1 |
| Elapsed | 15s |
| Nex status | `abnormal` |
| Exit reason | `max_turns` |
| Turns | 4 |
| Input tokens | 7,459 |
| Output tokens | 317 |

Observed behavior:

- Nex started the task and created `/tmp/project/src/tests` and
  `/tmp/project/data` with `bash`.
- The model initially tried to use the Nex `write_file` tool for
  `/tmp/project/src/main.py`.
- `write_file` rejected the absolute path because the Nex working directory was
  the repo worktree, not `/tmp`.
- The model recovered by using `bash` writes, but the 4-turn cap stopped it
  after only two files were created under `/tmp/project`.

Independent post-run checks:

| Check | Result |
| --- | --- |
| `/tmp/project` file count | 2 |
| `python3 -m pytest ...` | Failed: system Python had no `pytest` |
| JSON config validation | Failed: `config.json` missing |

### Attempt 2: Worktree CWD, 10 Turns

Command:

```bash
timeout 240s nex --wg --eval-mode \
  -e openrouter \
  -m openrouter:deepseek/deepseek-v3.2 \
  --max-turns 10 \
  --idle-timeout-secs 60 \
  --minimal-tools \
  "$(cat terminal-bench/tasks/condition-a-calibration/01-file-ops-easy.txt)"
```

Result:

| Field | Value |
| --- | --- |
| Exit status | 1 |
| Elapsed | 47s |
| Nex status | `abnormal` |
| Exit reason | `max_turns` |
| Turns | 10 |
| Input tokens | 22,504 |
| Output tokens | 866 |

Observed behavior:

- The same CWD/path issue appeared.
- The model switched to creating a repo-local `project/` directory instead of
  the requested `/tmp/project`.
- It created the six requested fixture files under `project/`.
- It attempted the requested pytest command, found `pytest` missing, and tried
  `pip3 install pytest --quiet`.
- The install failed due the host Python's PEP 668 externally-managed
  environment policy.
- The run hit the 10-turn cap before resolving that package-install issue.

Independent post-run checks before cleanup:

| Check | Result |
| --- | --- |
| `project/` file count | 6 |
| `project/data/config.json` JSON validation | Passed |
| `cd project && python3 -m pytest src/tests/test_utils.py -v` | Failed: system Python had no `pytest` |

The generated repo-local `project/` directory was removed and was not staged.

### Attempt 3: `/tmp` CWD, 14 Turns

This run changed only the working directory so that `/tmp/project` was inside
the Nex tool workspace. `WG_DIR` pointed Nex at the project graph while the
process ran from `/tmp`.

Command:

```bash
WG_DIR=/home/bot/wg/.wg timeout 300s nex --wg --eval-mode \
  -e openrouter \
  -m openrouter:deepseek/deepseek-v3.2 \
  --max-turns 14 \
  --idle-timeout-secs 60 \
  --minimal-tools \
  "$(cat /home/bot/wg/.wg-worktrees/agent-48/terminal-bench/tasks/condition-a-calibration/01-file-ops-easy.txt)"
```

Result:

| Field | Value |
| --- | --- |
| Exit status | 1 |
| Elapsed | 70s |
| Nex status | `abnormal` |
| Exit reason | `max_turns` |
| Turns | 14 |
| Input tokens | 38,036 |
| Output tokens | 1,352 |

Observed behavior:

- Running from `/tmp` fixed the absolute-path `write_file` issue.
- Nex created all requested top-level fixture files under `/tmp/project`.
- The model encountered missing system `pytest`, attempted a global pip install,
  hit the externally-managed environment policy, attempted `apt-get`, hit
  permission denial, then created a virtual environment under
  `/tmp/project/venv`.
- In that venv, it installed `pytest`, discovered an import error in its test
  file, edited the test, and then passed:

```text
src/tests/test_utils.py::TestUtils::test_add PASSED
```

- It validated `config.json` successfully.
- It then ran `find /tmp/project -type f | wc -l`, but the result was `1955`
  because the venv and pytest cache were inside `/tmp/project`.
- The run hit `max_turns` before printing the final directory tree.

Independent post-run checks before cleanup:

| Check | Result |
| --- | --- |
| Requested top-level files present | Yes |
| Venv pytest run inside trace | Passed |
| `python3 -c "import json; json.load(...)"` | Passed |
| Requested exact file count | Failed: `1955` because venv/cache files were included |
| Exact system pytest command | Failed outside venv because system Python had no `pytest` |

The generated `/tmp/project` directory was removed after measurement.

## Token And Cost Notes

Nex eval-mode emitted token usage but no per-request dollar cost. Aggregate token
usage for the three eval attempts:

| Metric | Value |
| --- | --- |
| Input tokens | 67,999 |
| Output tokens | 2,535 |

`wg spend --today` and `wg openrouter status` did not provide a reliable
per-run cost attribution for these eval attempts. The usable cost proxy from
this smoke run is therefore the Nex token summaries above.

## Process Cleanup

After the eval attempts and cleanup, this command found no matching benchmark
children:

```bash
ps -eo pid,ppid,cmd |
  rg 'nex --wg --eval-mode|terminal-bench/tb|pytest src/tests/test_utils|pip install pytest|apt-get install -y python3-pytest' |
  rg -v 'rg ' || true
```

`git status --short` showed only the pre-existing untracked `.wg` directory
until this report was added. No benchmark-generated `project/` or
`/tmp/project` tree was left behind.

## Harness Assessment

Nex is promising as a bounded eval harness:

- `--eval-mode` produced a machine-readable final JSON summary.
- `--max-turns`, `--idle-timeout-secs`, and an outer `timeout` bounded the run
  cleanly.
- OpenRouter endpoint/model routing worked with `deepseek/deepseek-v3.2`.
- The local tool surface was enough to create files, run shell commands, create
  a venv, install pytest, edit files, and execute tests.

The smoke result does not yet show Nex as a strong out-of-the-box
Terminal-Bench harness relative to mature external CLI harnesses:

- Correct working directory setup matters. From the repo worktree, `write_file`
  rejected `/tmp/project` and the model drifted to a repo-local `project/`.
  A Terminal-Bench adapter should set the process CWD to the task workspace or
  instruct the model to use `bash` for absolute workspace paths.
- Turn budgeting is coarse for file-creation tasks because the model often used
  one tool call per file or verification step. Even this tiny fixture exhausted
  4, 10, and 14 turns.
- Environment setup needs harness policy. The system Python lacked `pytest`;
  the model's venv workaround allowed the test to pass, but polluted the file
  count because the venv lived under the counted project tree.
- Eval-mode reports turns and tokens, but not benchmark score, pass/fail
  criteria, or per-run dollar cost.

Conclusion: Nex can run a cheap DeepSeek/OpenRouter Terminal-Bench-like smoke
task under hard bounds, and the harness mechanics are viable. For comparable
Terminal-Bench scoring against external CLI harnesses, it needs a thin adapter
that sets CWD to the task workspace, keeps dependency caches/venvs outside the
counted project tree, chooses a realistic turn cap, and records benchmark-level
pass/fail results in addition to Nex's eval JSON summary.

## Validation Checklist

- Cheap DeepSeek/OpenRouter Nex eval-mode run attempted with hard timeout: yes,
  three attempts with `timeout` plus `--max-turns`.
- Terminal-Bench-like task exercised: yes,
  `terminal-bench/tasks/condition-a-calibration/01-file-ops-easy.txt`.
- OpenRouter key not printed or committed: yes.
- No lingering Nex/benchmark child processes: yes, checked with `ps`/`rg`.
- Report records commands, status, elapsed time, usage, and assessment: yes.
- Git diff reviewed for unrelated churn: yes; only this report is intended for
  staging, with `.wg` left untracked.
