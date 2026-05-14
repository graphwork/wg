# Terminal-Bench Inventory & Infrastructure

## 1. Task Inventory

### TB 2.0 (Harbor Dataset)

**Total tasks: 89** (from `terminal-bench@2.0` Harbor package, cached at `~/.cache/harbor/tasks/packages/terminal-bench/`)

100 entries exist in `~/.cache/harbor/tasks/` but 10 are duplicate task names with different Docker image tags (Docker Hub `alexgshaw/*:20251031` vs GHCR `ghcr.io/laude-institute/terminal-bench/*:2.0`). The canonical package directory contains exactly **89 unique task subdirectories**.

#### Difficulty Breakdown (89 tasks)

| Difficulty | Count | Percentage |
|-----------|-------|------------|
| Easy      | 4     | 4.5%       |
| Medium    | 55    | 61.8%      |
| Hard      | 30    | 33.7%      |

#### Category Breakdown (89 tasks)

| Category              | Count |
|-----------------------|-------|
| software-engineering  | 26    |
| system-administration | 9     |
| security              | 8     |
| scientific-computing  | 8     |
| data-science          | 8     |
| file-operations       | 5     |
| debugging             | 5     |
| model-training        | 4     |
| mathematics           | 4     |
| data-processing       | 4     |
| machine-learning      | 3     |
| video-processing      | 1     |
| personal-assistant    | 1     |
| optimization          | 1     |
| games                 | 1     |
| data-querying         | 1     |

#### Full Task List (89 TB 2.0 tasks, alphabetical)

| # | Task Name | Difficulty | Category |
|---|-----------|-----------|----------|
| 1 | adaptive-rejection-sampler | medium | scientific-computing |
| 2 | bn-fit-modify | hard | scientific-computing |
| 3 | break-filter-js-from-html | medium | security |
| 4 | build-cython-ext | medium | debugging |
| 5 | build-pmars | medium | software-engineering |
| 6 | build-pov-ray | medium | software-engineering |
| 7 | caffe-cifar-10 | medium | machine-learning |
| 8 | cancel-async-tasks | hard | software-engineering |
| 9 | chess-best-move | medium | games |
| 10 | circuit-fibsqrt | hard | software-engineering |
| 11 | cobol-modernization | easy | software-engineering |
| 12 | code-from-image | medium | software-engineering |
| 13 | compile-compcert | medium | system-administration |
| 14 | configure-git-webserver | hard | system-administration |
| 15 | constraints-scheduling | medium | personal-assistant |
| 16 | count-dataset-tokens | medium | model-training |
| 17 | crack-7z-hash | medium | security |
| 18 | custom-memory-heap-crash | medium | debugging |
| 19 | db-wal-recovery | medium | file-operations |
| 20 | distribution-search | medium | machine-learning |
| 21 | dna-assembly | hard | scientific-computing |
| 22 | dna-insert | medium | scientific-computing |
| 23 | extract-elf | medium | file-operations |
| 24 | extract-moves-from-video | hard | file-operations |
| 25 | feal-differential-cryptanalysis | hard | mathematics |
| 26 | feal-linear-cryptanalysis | hard | mathematics |
| 27 | filter-js-from-html | medium | security |
| 28 | financial-document-processor | medium | data-processing |
| 29 | fix-code-vulnerability | hard | security |
| 30 | fix-git | easy | software-engineering |
| 31 | fix-ocaml-gc | hard | software-engineering |
| 32 | gcode-to-text | medium | file-operations |
| 33 | git-leak-recovery | medium | software-engineering |
| 34 | git-multibranch | medium | system-administration |
| 35 | gpt2-codegolf | hard | software-engineering |
| 36 | headless-terminal | medium | software-engineering |
| 37 | hf-model-inference | medium | data-science |
| 38 | install-windows-3.11 | hard | system-administration |
| 39 | kv-store-grpc | medium | software-engineering |
| 40 | large-scale-text-editing | medium | file-operations |
| 41 | largest-eigenval | medium | mathematics |
| 42 | llm-inference-batching-scheduler | hard | machine-learning |
| 43 | log-summary-date-ranges | medium | data-processing |
| 44 | mailman | medium | system-administration |
| 45 | make-doom-for-mips | hard | software-engineering |
| 46 | make-mips-interpreter | hard | software-engineering |
| 47 | mcmc-sampling-stan | hard | data-science |
| 48 | merge-diff-arc-agi-task | medium | debugging |
| 49 | model-extraction-relu-logits | hard | mathematics |
| 50 | modernize-scientific-stack | medium | scientific-computing |
| 51 | mteb-leaderboard | medium | data-science |
| 52 | mteb-retrieve | medium | data-science |
| 53 | multi-source-data-merger | medium | data-processing |
| 54 | nginx-request-logging | medium | system-administration |
| 55 | openssl-selfsigned-cert | medium | security |
| 56 | overfull-hbox | easy | debugging |
| 57 | password-recovery | hard | security |
| 58 | path-tracing | hard | software-engineering |
| 59 | path-tracing-reverse | hard | software-engineering |
| 60 | polyglot-c-py | medium | software-engineering |
| 61 | polyglot-rust-c | hard | software-engineering |
| 62 | portfolio-optimization | medium | optimization |
| 63 | protein-assembly | hard | scientific-computing |
| 64 | prove-plus-comm | easy | software-engineering |
| 65 | pypi-server | medium | software-engineering |
| 66 | pytorch-model-cli | medium | model-training |
| 67 | pytorch-model-recovery | medium | model-training |
| 68 | qemu-alpine-ssh | medium | system-administration |
| 69 | qemu-startup | medium | system-administration |
| 70 | query-optimize | medium | data-science |
| 71 | raman-fitting | medium | scientific-computing |
| 72 | regex-chess | hard | software-engineering |
| 73 | regex-log | medium | data-processing |
| 74 | reshard-c4-data | medium | data-science |
| 75 | rstan-to-pystan | medium | data-science |
| 76 | sam-cell-seg | hard | data-science |
| 77 | sanitize-git-repo | medium | security |
| 78 | schemelike-metacircular-eval | medium | software-engineering |
| 79 | sparql-university | hard | data-querying |
| 80 | sqlite-db-truncate | medium | debugging |
| 81 | sqlite-with-gcov | medium | system-administration |
| 82 | torch-pipeline-parallelism | hard | software-engineering |
| 83 | torch-tensor-parallelism | hard | software-engineering |
| 84 | train-fasttext | hard | model-training |
| 85 | tune-mjcf | medium | scientific-computing |
| 86 | video-processing | hard | video-processing |
| 87 | vulnerable-secret | medium | security |
| 88 | winning-avg-corewars | medium | software-engineering |
| 89 | write-compressor | hard | software-engineering |

### Custom Calibration Tasks (8 tasks)

Located in `tasks/condition-a-calibration/`. These are project-authored tasks used in early pilots and calibration runs, NOT part of the TB 2.0 dataset:

| # | ID | Title | Difficulty | Category |
|---|-----|-------|-----------|----------|
| 1 | file-ops | File Operations: create project structure | easy | file-ops |
| 2 | text-processing | Text Processing: word frequency counter | easy | text-processing |
| 3 | debugging | Debugging: fix merge sort bugs | medium | debugging |
| 4 | shell-scripting | Shell Scripting: log file analyzer | medium | shell-scripting |
| 5 | data-processing | Data Processing: JSON to CSV department summary | medium | data-processing |
| 6 | algorithm | Algorithm: key-value store with transactions | hard | algorithm |
| 7 | ml | ML: k-means clustering from scratch | hard | ml |
| 8 | sysadmin | Sysadmin: rate-limited HTTP server | hard | sysadmin |

### Custom Hard Benchmark Tasks (10 tasks)

Located in `tasks/hard-benchmarks/`. These are project-authored multi-step tasks designed to stress graph coordination:

| # | ID | Title | Difficulty |
|---|-----|-------|-----------|
| 1 | configure-git-webserver | Configure Git Webserver: bare repo + post-receive hook + HTTP server | hard |
| 2 | mailman | Mailman: local mail system with mailing list manager | hard |
| 3 | multi-source-data-merger | Multi-Source Data Merger: 3 formats -> merge -> conflict report | hard |
| 4 | financial-document-processor | Financial Document Processor: classify -> extract -> summarize | hard |
| 5 | cobol-modernization | COBOL Modernization: payroll COBOL -> Python with identical output | hard |
| 6 | build-cython-ext | Build Cython Extension: numpy integration, build, test | hard |
| 7 | fix-code-vulnerability | Fix Code Vulnerabilities: analyze -> report -> fix -> test | hard |
| 8 | constraints-scheduling | Constraints Scheduling: ICS parsing + slot finding + meeting gen | hard |
| 9 | multi-module-type-migration | Multi-Module Type Migration: UserId str -> dataclass across 6 modules | hard |
| 10 | iterative-test-fix | Iterative Test Fix: 6 interrelated bugs, 15 tests, fix all | hard |

### Task Set Summary

| Set | Count | Source | Used In |
|-----|-------|--------|---------|
| TB 2.0 (Harbor) | 89 | `terminal-bench@2.0` | pilot-a-89, reproduce.sh, leaderboard |
| Calibration | 8 | `tasks/condition-a-calibration/` | All condition pilots (A-F), calibration runs |
| Hard Benchmarks | 10 | `tasks/hard-benchmarks/` | run_hard_benchmarks.py, pilot-f-89 |
| **Combined (pilot-f-89)** | **18** | calibration + hard | pilot-f-89 (18 tasks x 5 replicas = 90 trials) |

---

## 2. Runner Scripts

### Primary Runners

| Script | Purpose | Execution Mode | Tasks | Conditions |
|--------|---------|---------------|-------|------------|
| `reproduce.sh` | Full reproduction of paper experiment | Harbor framework (Docker containers) | All 89 TB 2.0 | A, B, C |
| `run_condition_a.py` | Isolated wg service per problem, 8 parallel agents | Native WG executor (host-side) | 8 calibration | A |
| `run_pilot_f_89.py` | 18-task pilot with surveillance loops | Native WG executor (temp dirs) | 18 (8 cal + 10 hard) | F |
| `run_pilot_f_5x1.py` | 5-task pilot for condition F surveillance | Native WG executor | 5 calibration subset | F |
| `run_pilot_condition_f.py` | 14-trial condition F lifecycle pilot (no LLM) | Native WG executor + federation | 7 calibration | F |
| `run_pilot_a_prime.py` | A' pilot (bare agent, no turn cap) | Native WG executor + federation | 7 calibration | A' |
| `run_full_a_prime_vs_f.py` | Full A' vs F benchmark | Native WG executor + federation | 7 calibration | A', F |
| `run_hard_benchmarks.py` | Hard benchmark A' vs F comparison | Native WG executor + federation | 10 hard benchmarks | A', F |
| `rerun_pilot_f_89_dns.py` | Re-run 29 DNS-failed trials from pilot-f-89 | Native WG executor | Failed subset of 18 | F |

### Support Scripts

| Script | Purpose |
|--------|---------|
| `tb-harness.sh` | Shell harness for native executor (conditions A, B, C). Wraps `wg native-exec`. |
| `tb_trial_runner.py` | Creates wg tasks from TB definitions + fan-out/fan-in via wg CLI |
| `tb_collect_results.py` | Fan-in analysis: collects FLIP scores, evaluations, verify results |
| `prepare-leaderboard.sh` | Copies results into HuggingFace leaderboard submission format |
| `setup-docker.sh` | Docker + Harbor installation and verification |
| `pre-pull-images.sh` | Pre-caches Docker images to avoid Docker Hub rate limits |

### Adapter (`wg/adapter.py`)

The core adapter bridges Harbor's agent protocol to WG's native executor. Supports two execution modes:
1. **Docker-aware** (Harbor path): LLM agent loop in Python, routes commands through Harbor's `environment.exec()` into Docker containers. Uses LiteLLM for API calls.
2. **Host-native** (standalone runner path): Delegates to `wg service start` + `wg native-exec`. Uses WG's built-in Rust OpenAI-compatible client directly.

Supports **6 conditions** (A through F) with varying tool access, context scope, and agency configuration.

### Execution Mode Comparison

| Path | API Client | Container Execution | Used By |
|------|-----------|--------------------|---------| 
| Harbor (Docker) | LiteLLM (Python) | Docker via Harbor | `reproduce.sh`, leaderboard runs |
| Host-native | Rust `openai_client.rs` | Direct on host (temp dirs) | All `run_*.py` scripts |

---

## 3. Infrastructure Dependencies

### Required Software

| Component | Purpose | Notes |
|-----------|---------|-------|
| **Docker** | Container isolation for TB tasks | `docker.io` + `docker-compose-v2` |
| **Harbor** | Benchmark framework (task download, orchestration, verification) | `pip install harbor-bench` (>= 0.3.0) |
| **wg (`wg`)** | Task graph executor, agent spawning | `cargo install --path .` (Rust binary) |
| **Python 3.10+** | Runner scripts, adapter | System or venv |
| **LiteLLM** | OpenRouter API proxy (Harbor path only) | `pip install litellm` |
| **OpenRouter API** | LLM API gateway | Requires `OPENROUTER_API_KEY` env var |

### Python Dependencies (from `pyproject.toml`)

- `harbor>=0.3.0` — benchmark framework
- `litellm` — LLM API abstraction
- `ddgs>=9.0` — DuckDuckGo search (for web-enabled conditions)
- `httpx>=0.24` — HTTP client
- `trafilatura>=1.0` — Web content extraction

### Docker Images

TB tasks use pre-built Docker images from two registries:
- **Docker Hub**: `alexgshaw/<task-name>:20251031` (primary)
- **GHCR**: `ghcr.io/laude-institute/terminal-bench/<task-name>:2.0` (mirror, no rate limit)

All 89 tasks have Docker images. Resource requirements vary per task (see Section 5).

---

## 4. Rate Limits & Concurrency

### OpenRouter API (M2.7 — `minimax/minimax-m2.7`)

No explicit rate limit documentation was found in the codebase for OpenRouter/M2.7 specifically. The runners use these empirically-determined concurrency settings:

| Runner | Concurrent Trials | Agents/Trial | Max Concurrent Agents | Notes |
|--------|------------------|-------------|----------------------|-------|
| `reproduce.sh` | 4 | 1 | 4 | Harbor-managed |
| `run_condition_a.py` | 4 (default) | 8 | 32 | Configurable `--max-concurrent` |
| `run_pilot_f_89.py` | Sequential (1) | 1 | 1 | Single trial at a time |
| `run_full_a_prime_vs_f.py` | 4 (default) | 1 | 4 | Configurable |
| `run_hard_benchmarks.py` | 4 (default) | 1 | 4 | Configurable |

The pilot-a-89 config (`trial-config-pilot-a-89.json`) specifies `n_concurrent_trials: 4` with retry logic (max 2 retries, excluding timeout errors).

### Docker Hub Rate Limits

| Tier | Pulls/6h | Impact |
|------|---------|--------|
| Anonymous | ~100 | Insufficient for full run (89 x 3 = 267 pulls) |
| Authenticated | ~200 | Marginal for full run |
| GHCR mirror | Unlimited | Preferred for `alexgshaw/*` images |

**Mitigation**: `pre-pull-images.sh` caches all images locally before runs. Use `--no-delete` flag with Harbor to preserve image cache between trials.

### Key Concurrency Constraint

From `DESIGN-condition-a-isolation.md`: "Running N trials fully in parallel would spawn N * 8 = N*8 agents simultaneously. To avoid overwhelming API rate limits or the host machine: `MAX_CONCURRENT_TRIALS = 4` (4 trials * 8 agents = 32 concurrent agents max)."

---

## 5. Resource Requirements Per Trial

### Container Resources (from task.toml metadata)

| Resource Profile | Tasks | CPUs | Memory | Storage |
|-----------------|-------|------|--------|---------|
| Standard | 71 | 1 | 2G | 10G |
| Medium memory | 13 | 1 | 4G | 10G |
| Multi-core (no explicit resources) | 10 | — | — | — |
| High compute | 3 | 2 | 4G | 10G |
| Heavy compute | 2 | 4 | 8G | 10G |

Note: The 10 tasks without explicit CPU/memory are GHCR-mirrored duplicates that reference the same underlying task.

### Timeout Distribution (agent timeout, seconds)

| Timeout | Tasks | Notes |
|---------|-------|-------|
| 360 (6 min) | 1 | overfull-hbox (easy) |
| 600 (10 min) | 1 | modernize-scientific-stack |
| 900 (15 min) | 50 | Most common, default for medium tasks |
| 1200 (20 min) | 6 | Medium-complexity tasks |
| 1800 (30 min) | 15 | Hard tasks, default for runners |
| 2400 (40 min) | 2 | compile-compcert, schemelike-metacircular-eval |
| 3600 (1 hr) | 12 | Complex tasks (regex-chess, sam-cell-seg peer) |
| 7200 (2 hr) | 1 | sam-cell-seg |
| 12000 (3.3 hr) | 1 | build-pov-ray |

### Host-Side Requirements (per runner)

| Resource | Requirement | Notes |
|----------|------------|-------|
| CPU | 1-4 cores per concurrent trial | Most tasks use 1 CPU container |
| Memory | 2-8 GB per concurrent trial | 2 GB typical, 8 GB for mcmc-sampling-stan/rstan-to-pystan |
| Disk | ~10 GB per active container + temp dirs | Docker layer caching amortizes across trials |
| Network | Outbound HTTPS to OpenRouter API | Plus Docker pulls if not pre-cached |

### Cost Estimates

From `reproduce.sh`: **~$20 total for 3 conditions x 89 tasks x 3 trials** with Minimax M2.7.
That's approximately **$0.025 per trial** or **$2.20 per condition per full run**.

### Time Estimates

From `reproduce.sh`: **~15 hours per condition** with `--n-concurrent 4`.
Per-trial timeout default: **1800 seconds (30 minutes)**, configurable per task.

---

## 6. Existing Experiment Runs

Results directories in `terminal-bench/results/` document completed experiments:

| Run | Tasks | Conditions | Replicas | Status |
|-----|-------|-----------|----------|--------|
| pilot-a-89 | 89 (TB 2.0) | A | 1 | Complete (37/89 pass) |
| pilot-f-89 | 18 (custom) | F | 5 | Complete (61/90 pass, 29 DNS failures rerun) |
| pilot-a-5x1 | 5 (calibration) | A | 5 | Complete |
| pilot-f-5x1 | 5 (calibration) | F | 1 | Complete |
| full-a-prime-vs-f | 7 (calibration) | A', F | 3 | Complete |
| hard-benchmarks | 10 (hard) | A', F | 2 | Complete |
| condition-a/b calibration | 8 (calibration) | A, B | multiple | Complete |
| full-condition-a/b/c | 89 (TB 2.0) | A, B, C | 3 | Complete (via Harbor) |
| rerun-condition-a/b | 89 (TB 2.0) | A, B | continuation | Complete |
| smoke-a-20 | 20 (TB 2.0 subset) | A | 1 | Complete |

---

## 7. Key Architecture Notes for Scaling

1. **Two execution paths exist**: Harbor (Docker containers, LiteLLM) and host-native (wg service, Rust client). The host-native path is used by all `run_*.py` scripts and provides per-trial isolation via temp directories.

2. **Trial isolation**: Each trial gets its own temp directory with an independent `.wg/`, config, and service socket. Trials cannot interfere with each other.

3. **Concurrency is semaphore-gated**: `asyncio.Semaphore(MAX_CONCURRENT_TRIALS)` limits parallel trials. The limiting factor is API rate limits, not local compute for most tasks.

4. **Docker image caching is critical**: Pre-pull all images before large runs. Docker Hub rate limits (100/6h anonymous) are easily exceeded. GHCR mirror (`ghcr.io/laude-institute/terminal-bench/*:2.0`) has no rate limit.

5. **The 18-task custom set overlaps with TB 2.0**: Some custom tasks (configure-git-webserver, build-cython-ext, etc.) have TB 2.0 equivalents with different verification commands. The custom tasks have project-specific verify commands; TB 2.0 tasks use Harbor's built-in verifier.

6. **Model routing**: The standard model is `openrouter:minimax/minimax-m2.7`. Some configs override per-difficulty (e.g., Gemini Flash for easy tasks, Claude Sonnet for medium/hard).
