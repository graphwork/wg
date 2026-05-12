# TB Task Set Audit: 89 vs 18/20 Confusion

## Executive Summary

There are **two separate task registries** in use, causing confusion about the TB task count:

1. **Harbor TB 2.0 dataset** (89 tasks) — the canonical benchmark, stored in `~/.cache/harbor/tasks/packages/terminal-bench/`
2. **Local `wg/tasks.py`** (18 tasks) — hand-crafted definitions with instruction files and verify commands

Standalone Python runners (all `run_qwen3_*` and `run_pilot_*` scripts) import from `wg/tasks.py` and only see 18 tasks. Harbor-based configs (`*-config.json`) reference `terminal-bench@2.0` and get all 89.

The "20" in `run_qwen3_hard_20_*.py` was aspirational — only 18 local tasks exist.

---

## The Two Task Registries

### Registry A: Harbor TB 2.0 (89 tasks)

- **Location**: `~/.cache/harbor/tasks/packages/terminal-bench/` (downloaded by `harbor download`)
- **Format**: Each task is a directory with `instruction.md`, `task.toml`, `environment/`, `tests/`, `solution/`
- **Execution**: Via `harbor run` with Docker containers, environment isolation, prebuilt images
- **Configs that use it**: `pilot-m25free-89-1x-config.json`, `pilot-native-m27-89-1x-config.json`, `gpt-oss-120b-condition-*-config.json`, `nemotron-3-super-condition-*-config.json`, `local-qwen3-coder-30b-condition-a-config.json`
- **How it works**: Config specifies `"name": "terminal-bench", "version": "2.0"` in the `datasets` array. `task_names: null` means ALL tasks.

### Registry B: Local wg/tasks.py (18 tasks)

- **Location**: `terminal-bench/wg/tasks.py` + instruction files in `terminal-bench/tasks/`
- **Format**: Python dicts with `id`, `title`, `instruction_file`, `verify_cmd`, `difficulty`
- **Execution**: Via standalone Python runners using `wg native-exec` directly (no Docker, no Harbor)
- **Scripts that use it**: `run_qwen3_hard_20_a.py`, `run_qwen3_hard_20_g.py`, `run_pilot_qwen3_local_10.py`, `run_pilot_qwen3_local_10_g.py`, `run_condition_a.py`, `run_pilot_f_89.py`, `run_pilot_a_vs_g_haiku.py`, etc.
- **How it works**: `from wg.tasks import TASKS_BY_ID, ALL_TASKS` — scripts select from these 18.

---

## The 18 Local Tasks (Full List)

| # | ID | Difficulty | In Harbor TB 2.0? | Origin |
|---|-----|-----------|-------------------|--------|
| 1 | file-ops | easy | NO | Custom calibration |
| 2 | text-processing | easy | NO | Custom calibration |
| 3 | debugging | medium | NO | Custom calibration |
| 4 | shell-scripting | medium | NO | Custom calibration |
| 5 | data-processing | medium | NO | Custom calibration |
| 6 | algorithm | hard | NO | Custom calibration |
| 7 | ml | hard | NO | Custom calibration |
| 8 | sysadmin | hard | NO | Custom calibration |
| 9 | configure-git-webserver | hard | YES | Hard benchmark |
| 10 | mailman | hard | YES | Hard benchmark |
| 11 | multi-source-data-merger | hard | YES | Hard benchmark |
| 12 | financial-document-processor | hard | YES | Hard benchmark |
| 13 | cobol-modernization | hard | YES | Hard benchmark |
| 14 | build-cython-ext | hard | YES | Hard benchmark |
| 15 | fix-code-vulnerability | hard | YES | Hard benchmark |
| 16 | constraints-scheduling | hard | YES | Hard benchmark |
| 17 | multi-module-type-migration | hard | NO | Custom hard benchmark |
| 18 | iterative-test-fix | hard | NO | Custom hard benchmark |

**Breakdown**: 8 custom calibration (not in TB 2.0) + 8 from TB 2.0 + 2 custom hard (not in TB 2.0) = 18 total.

---

## The 89 Harbor TB 2.0 Tasks (Full List)

```
adaptive-rejection-sampler     bn-fit-modify               break-filter-js-from-html
build-cython-ext               build-pmars                 build-pov-ray
caffe-cifar-10                 cancel-async-tasks          chess-best-move
circuit-fibsqrt                cobol-modernization         code-from-image
compile-compcert               configure-git-webserver     constraints-scheduling
count-dataset-tokens           crack-7z-hash               custom-memory-heap-crash
db-wal-recovery                distribution-search         dna-assembly
dna-insert                     extract-elf                 extract-moves-from-video
feal-differential-cryptanalysis feal-linear-cryptanalysis  filter-js-from-html
financial-document-processor   fix-code-vulnerability      fix-git
fix-ocaml-gc                   gcode-to-text               git-leak-recovery
git-multibranch                gpt2-codegolf               headless-terminal
hf-model-inference             install-windows-3-11        kv-store-grpc
large-scale-text-editing       largest-eigenval            llm-inference-batching-scheduler
log-summary-date-ranges        mailman                     make-doom-for-mips
make-mips-interpreter          mcmc-sampling-stan          merge-diff-arc-agi-task
model-extraction-relu-logits   modernize-scientific-stack  mteb-leaderboard
mteb-retrieve                  multi-source-data-merger    nginx-request-logging
openssl-selfsigned-cert        overfull-hbox               password-recovery
path-tracing                   path-tracing-reverse        polyglot-c-py
polyglot-rust-c                portfolio-optimization      protein-assembly
prove-plus-comm                pypi-server                 pytorch-model-cli
pytorch-model-recovery         qemu-alpine-ssh             qemu-startup
query-optimize                 raman-fitting               regex-chess
regex-log                      reshard-c4-data             rstan-to-pystan
sam-cell-seg                   sanitize-git-repo           schemelike-metacircular-eval
sparql-university              sqlite-db-truncate          sqlite-with-gcov
torch-pipeline-parallelism     torch-tensor-parallelism    train-fasttext
tune-mjcf                      video-processing            vulnerable-secret
winning-avg-corewars           write-compressor
```

---

## Where the Confusion Originated

### Step 1: Harbor-based evaluation (89 tasks)

The initial pilot runs used Harbor configs (`pilot-m25free-89-1x-config.json`, `pilot-native-m27-89-1x-config.json`). These correctly loaded all 89 TB 2.0 tasks via `harbor run -d terminal-bench@2.0`. This is where "89 tasks" comes from — it's the real benchmark.

### Step 2: Local model runners bypass Harbor (18 tasks)

When the project needed to test local models (Qwen3-Coder-30B on SGLang/lambda01), Harbor's Docker+API execution path didn't apply. Instead, standalone Python runners were written that:
- Import task definitions from `wg/tasks.py` (only 18 tasks)
- Create isolated temp directories
- Run `wg native-exec` directly
- Use hand-written `verify_cmd` strings for validation

These runners (`run_qwen3_hard_20_a.py`, etc.) can only run tasks that exist in `wg/tasks.py`.

### Step 3: Naming mismatch

`run_qwen3_hard_20_a.py` was named with "20" tasks planned, but `wg/tasks.py` only has 18. The docstring correctly states: *"The full TB 2.0 catalog has 89 tasks but only 18 have local runner definitions."*

### Step 4: wg task names compound the confusion

The wg tasks created to manage these runs (`tb-qwen3-hard-20-g`, `tb-qwen3-local-10-g`) have "20" and "10" in their names, which are different numbers from the actual task counts (18 and 10 respectively). An agent looking at these wg task IDs could easily confuse them with the TB task counts.

### The "18 out of 89" claim is accurate

When agents said "only 18 have local definitions out of 89 total" — they were exactly right. The 18 local tasks in `wg/tasks.py` are the only ones with instruction files, verify commands, and standalone runner support. The remaining 71 TB tasks only exist in Harbor's registry and require `harbor run` with Docker environments.

---

## How to Run the Full 89 Tasks

### Option 1: Harbor framework (existing, works now)

Use the existing Harbor config files with `harbor run`:

```bash
cd terminal-bench
harbor run -c gpt-oss-120b-condition-g-config.json
# or
harbor run -c nemotron-3-super-condition-a-config.json
```

These load `terminal-bench@2.0` from the registry and run all 89 tasks in Docker containers with their native verifiers.

### Option 2: Extend wg/tasks.py (requires work)

To run all 89 tasks via standalone runners:

1. **Extract instruction text** from each Harbor task's `instruction.md`
2. **Extract verify commands** from each Harbor task's `tests/run-tests.sh`
3. **Add all 89 entries** to `wg/tasks.py` with instruction text (inline or file-referenced) and verify commands
4. **Handle Docker dependencies**: Many of the 71 missing tasks require specific Docker environments (prebuilt images, custom Dockerfiles). The standalone runners would need to either:
   - Run inside Docker containers (matching Harbor's approach)
   - Adapt instructions for bare-metal execution (significant effort, not all tasks can run bare-metal)

### Option 3: Hybrid — Harbor for full runs, local for dev/debug

Keep the current setup but make it clear:
- **Full 89-task evaluations**: Use Harbor configs (`harbor run -c <config>.json`)
- **Quick local testing (18 tasks)**: Use standalone runners (`python run_qwen3_hard_20_a.py`)
- Document this distinction prominently

---

## Recommendation

**Option 3 (hybrid)** is the pragmatic path. The 18 local tasks are useful for quick iteration and debugging. The full 89-task runs should go through Harbor. The key fix is:

1. **Rename misleading scripts**: `run_qwen3_hard_20_a.py` → `run_qwen3_local_18_a.py` (or keep the name but fix the docstring)
2. **Add a comment to wg/tasks.py** explaining this is a subset, with a pointer to Harbor for the full 89
3. **Update wg task descriptions** for future TB runs to clearly state whether they target the 18-task local set or the 89-task Harbor set
4. **For local model evals on all 89**: Create a Harbor config that routes through the local SGLang endpoint (like `local-qwen3-coder-30b-condition-a-config.json` already does — this config correctly references `terminal-bench@2.0` and would run all 89 tasks if executed via `harbor run`)
