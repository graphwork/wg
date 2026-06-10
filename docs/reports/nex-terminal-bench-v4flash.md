# Nex DeepSeek v4-flash Terminal-Bench (clean rerun)

Date: 2026-06-10

Task: `benchmark-nex-deepseek` (clean re-run of the failed
`bench-nex-v4flash-terminal`)

## Scope

A bounded Terminal-Bench-style smoke benchmark of Nex as the executor harness,
running `openrouter:deepseek/deepseek-v4-flash`. Claude (the chat agent) only
orchestrated the WG task and set up the endpoint; it did not solve the fixture.
The measured run went through `nex --wg --eval-mode`.

This is a clean re-run of `bench-nex-v4flash-terminal`, which FAILED on harness
wrapper bugs (a zsh read-only `status` variable and a `/tmp/project` collision
with a concurrent Minimax run) rather than on model behavior. This run fixed
both: the wrapper is `bash` and uses no `status` variable, and the workdir is an
isolated `/tmp/wg-nex-v4flash-clean` (not the shared `/tmp/project`).

Same fixture family as the comparable benches:
`terminal-bench/tasks/condition-a-calibration/01-file-ops-easy.txt`. The only
change is the target directory: every `/tmp/project` in the prompt was rewritten
to `/tmp/wg-nex-v4flash-clean` for isolation. The directory structure, file
contents, and verification steps are byte-for-byte identical to the Minimax and
DeepSeek v3.2 baselines, so the run remains directly comparable.

No Harbor, Docker Terminal-Bench, or external agent CLI was used.

## Key Handling

- OpenRouter availability was checked with `wg endpoints test openrouter`
  (200 OK, auth OK).
- The OpenRouter API key was never printed, echoed, passed on a command line, or
  committed. No `set -x`, `env`, `printenv`, `ps e`, or `/proc/*/environ` was
  used.
- The endpoint resolved the key from a `0600` file (`api_key_file`,
  materialized once from the keystore with output redirected to the file, never
  to a terminal). No `-k/--api-key` was passed to Nex.
- The key file and all temporary endpoint config edits were removed after the
  run (see Cleanup).

## Environment

| Field | Value |
| --- | --- |
| Repo worktree | `/home/bot/wg/.wg-worktrees/agent-5200` |
| WG dir used by Nex (`WG_DIR`) | `/home/bot/wg/.wg` |
| Isolated workdir (CWD) | `/tmp/wg-nex-v4flash-clean` |
| Nex binary | `/home/bot/.cargo/bin/nex` |
| Nex version | `nex 0.1.0` |
| Endpoint under test | `openrouter` |
| WG/Nex model under test | `openrouter:deepseek/deepseek-v4-flash` |
| Expected raw OpenRouter model | `deepseek/deepseek-v4-flash` |
| Fixture | `terminal-bench/tasks/condition-a-calibration/01-file-ops-easy.txt` (paths rewritten to the isolated workdir) |

The model id `deepseek/deepseek-v4-flash` was confirmed present in the live
OpenRouter model list before the run (alongside `deepseek/deepseek-v4-pro`).

## Endpoint wiring note (why the prior run could not even authenticate)

Beyond the wrapper bug, getting Nex to authenticate at all required a config
fix worth recording for future benches. Under `nex --wg`, Nex applies
`apply_wg_endpoint_inheritance_policy` (`src/nex_runtime.rs`): if the WG-dir
config (`/home/bot/wg/.wg/config.toml`) neither declares its own
`[[llm_endpoints.endpoints]]` nor sets `[llm_endpoints] inherit_global = true`,
Nex **strips** the `endpoints` array inherited from `~/.wg/config.toml`. So an
`openrouter` endpoint added only to the user-global config is invisible to a
`--wg` Nex session, and the request goes out with no `Authorization` header →
HTTP 401 ("No cookie auth credentials found"). The `--config <file>` flag does
not help here either: it is only consulted in standalone eval-runtime
resolution, not in `--wg` integrated mode.

Fix used: declare the `openrouter` endpoint (with `api_key_file`) directly in
`/home/bot/wg/.wg/config.toml`, which makes the inheritance policy keep it.
Nex's native path resolves endpoint keys via `resolve_api_key_strict`
(`src/config.rs`), which honors `api_key` / `api_key_file` / `api_key_env` but
**not** `api_key_ref` — so the keystore ref that `wg endpoints test` accepts is
not enough for the native executor; a file (or inline/env) key is required.

## Eval Attempt

One Nex attempt total (1 of 2 allowed). It reached OpenRouter, completed the
fixture functionally, and produced a definitive turn/token/exit profile, so a
second attempt would only re-confirm the same signal.

| Bound | Value |
| --- | --- |
| External hard timeout | `180s` |
| Nex max turns | `10` |
| Attempts used | 1 of 2 allowed |
| Tool surface | `--minimal-tools` |
| Streaming idle timeout | `60s` |

Command shape (no secret on the command line):

```bash
cd /tmp/wg-nex-v4flash-clean
WG_DIR=/home/bot/wg/.wg timeout 180s nex --wg --eval-mode \
  -e openrouter \
  -m openrouter:deepseek/deepseek-v4-flash \
  --max-turns 10 \
  --idle-timeout-secs 60 \
  --minimal-tools \
  "$(cat /tmp/wg-nex-v4flash-clean.prompt.txt)"
```

Result:

| Field | Value |
| --- | --- |
| Exit status | 1 |
| Elapsed | 36s |
| Nex JSON status | `abnormal` |
| Exit reason | `max_turns` |
| Turns | 10 (hit the cap) |
| Input tokens | 33,507 |
| Output tokens | 2,237 |
| Total tokens | 35,744 |
| Stdout bytes | 101 |
| Stderr bytes | 204 |

Final Nex eval JSON:

```json
{"status":"abnormal","turns":10,"input_tokens":33507,"output_tokens":2237,"exit_reason":"max_turns"}
```

The non-zero exit and `abnormal`/`max_turns` status reflect that the agent never
emitted a clean `end_turn` within the 10-turn cap — not that the task was left
unfinished. All requested artifacts were produced (see below).

## Fixture Result

Independent post-run checks (Claude ran these, not the model) confirmed the
fixture was completed functionally:

| Check | Result |
| --- | --- |
| `src/main.py` | Present; `print("Hello, World!")` |
| `src/utils.py` | Present; `def add(a, b): return a + b` |
| `src/tests/test_utils.py` | Present; imports `add`, asserts `add(2, 3) == 5` |
| `data/config.json` | Present; valid JSON, keys `name`/`version`/`debug` |
| `README.md` | Present with requested heading/body |
| `.gitignore` | Present with `__pycache__/`, `*.pyc`, `.env` |
| `python3 -m pytest src/tests/test_utils.py -v` | Exit 0; 1 passed |
| `python3 -c "import json; json.load(...)"` | Exit 0 (JSON OK) |

Strict file-count result:

| Count | Value |
| --- | --- |
| Requested files (excluding pytest/pycache artifacts) | 6 (exact) |
| `find -type f \| wc -l` after pytest | 12 |

The functional fixture **PASSED**: all six requested files were present with
correct content, JSON validated, and the unit test passed. The strict
"should be 6" count only inflates to 12 *after* running pytest, because
verification creates `.pytest_cache/` and `__pycache__/` — the same
benchmark-design caveat noted in the Minimax and DeepSeek reports, not a model
failure (the pre-verification tree is exactly 6 files).

## Process Cleanup

The generated `/tmp/wg-nex-v4flash-clean` tree was removed after collecting the
checks. A process check by name only (`ps -eo pid,ppid,stat,etime,comm`) found
no lingering `nex`, Terminal-Bench, pytest, pip, or apt-get child processes.

The `0600` key file (`/tmp/.or_v4f_key`) was deleted, and the temporary
`openrouter` endpoint additions to `~/.wg/config.toml` and
`/home/bot/wg/.wg/config.toml` were reverted from backups, returning both
configs to their pre-benchmark state.

## Comparison

All three Nex runs used the same `01-file-ops-easy` fixture through
`nex --wg --eval-mode`. Pricing is OpenRouter live pricing (2026-06-10).

| model spec | attempts | bounds | exit / reason | turns | total tokens | elapsed | functional | LLM eval | in $/M | out $/M | ctx |
|---|---|---|---|---|---|---|---|---|---|---|---|
| `openrouter:deepseek/deepseek-v4-flash` (this run) | 1 | t180 / mt10 | exit 1 / `max_turns` | 10 (cap) | 35,744 | 36s | PASS | n/a | 0.098 | 0.197 | 1,048,576 |
| `openrouter:minimax/minimax-m2.7` (`bench-nex-minimax-terminal`) | 1 | t180 / mt8 | exit 0 / `end_turn` | 8 | 20,703 | 51s | PASS | 0.93 / 0.91 | 0.270 | 1.080 | 204,800 |
| `openrouter:deepseek/deepseek-v3.2` (`bench-nex-deepseek-terminal`) | 3 | t180-300 / mt4-14 | all `max_turns`, pass@3 | 4/10/14 (caps) | — | — | PASS @3 | 0.92 / 0.94 | 0.229 | 0.343 | 131,072 |

Reading of the comparison:

- **v4-flash vs Minimax M2.7 (the premium pick):** Minimax reached a clean
  `end_turn` in a single attempt at 8 turns and exited 0. v4-flash, like its
  v3.2 sibling, exhausted the turn cap (`max_turns`, exit 1) without a clean
  stop, despite completing the work. That turn-inefficiency is the *same* profile
  that disqualified v3.2 from the premium slot in
  `research-opencode-default`. v4-flash is far cheaper ($0.098/$0.197 vs
  $0.270/$1.080) and has a 5× larger context (1,048,576 vs 204,800), but the
  premium pick was decided on turn efficiency for multi-step impl/research work,
  and v4-flash regresses on exactly that axis.
- **v4-flash vs DeepSeek v3.2:** Same family behavior — turn-inefficient, hits
  `max_turns`, but still lands a functionally passing fixture. v4-flash did it in
  a *single* attempt within tighter bounds (mt10, one run) where v3.2 needed
  three escalating attempts (mt4/10/14) to reach a passing test, so v4-flash is
  at least no worse than v3.2 and arguably more sample-efficient, at roughly half
  the price and with 8× the context.
- **v4-flash vs stepfun/step-3.7-flash (the lightweight pick):** Not directly
  comparable. stepfun was chosen because it is the only candidate with
  *route-confirmed* evidence of dispatching and passing **through the opencode
  executor** (`opencode-live-route-3`). v4-flash has only this Nex eval-mode
  signal — no opencode-route validation exists for it yet.

## Should v4-flash replace either opencode starter pick?

**No — not on current evidence.** Recommendation: keep
`lightweight = openrouter:stepfun/step-3.7-flash` and
`premium = openrouter:minimax/minimax-m2.7`.

- **Premium slot:** v4-flash should *not* displace Minimax M2.7. The premium
  worker does multi-step impl/research, and the deciding criterion was
  turn-efficiency / clean termination. v4-flash hit `max_turns` (exit 1) just
  like v3.2 — the wrong profile — even though it is cheaper with a much larger
  context. Its price/context advantage does not outweigh a regression on the
  exact axis the premium pick optimizes for.
- **Lightweight slot:** v4-flash should *not* displace stepfun/step-3.7-flash
  *yet*, because it has no opencode-executor route confirmation, which was the
  decisive factor for the lightweight pick (and the integration target for
  `fix-opencode-build`). Turn-inefficiency matters far less for short
  chat/triage/eval turns, and v4-flash is the cheapest candidate ($0.098/$0.197)
  with by far the largest context (1,048,576), so it is a strong *future*
  lightweight candidate.

**Suggested follow-up** (not run here): validate
`openrouter:deepseek/deepseek-v4-flash` through the actual **opencode** executor
route (the same shape as `opencode-live-route-3` did for stepfun). If it
dispatches and passes through opencode, v4-flash becomes a serious candidate to
displace the lightweight pick on cost and context, where its only measured
weakness (turn-inefficiency on multi-step work) is least relevant.

## Validation Checklist

- Clean Nex eval-mode run with `openrouter:deepseek/deepseek-v4-flash` on
  `01-file-ops-easy`, isolated `/tmp/wg-nex-v4flash-clean` workdir, no `status`
  variable in any wrapper: yes.
- Result captures exit status (1), elapsed time (36s), turns (10), tokens
  (33,507 in / 2,237 out), exit_reason (`max_turns`), and functional fixture
  pass/fail (PASS): yes.
- OpenRouter key never printed or committed; no lingering nex/benchmark child
  processes: yes (key resolved from a `0600` file, then deleted; process check
  clean).
- `docs/reports/nex-terminal-bench-v4flash.md` exists and compares to the
  DeepSeek v3.2 baseline and the Minimax M2.7 run: yes.
- Report states whether v4-flash should replace either opencode starter pick:
  yes — no for both on current evidence, with an opencode-route follow-up
  recommended for the lightweight slot.
