# Executor Improvements Integration Report

**Date:** 2026-04-05
**Task:** integrate-executor-improvements
**Branches merged:** 5 (all co-committed to main, no conflicts)

## Commits Integrated

| Commit | Branch | Feature |
|--------|--------|---------|
| `97efebd4` | impl-executor-test-discovery | Pre-task test discovery + auto `--verify` gates |
| `97efebd4` | impl-executor-context-injection | Inject wg context for non-Claude models (shared commit) |
| `97efebd4` | impl-executor-decomp-templates | Adaptive decomposition intelligence (shared commit) |
| `87a11ef0` | impl-executor-separate-verify | Separate-agent verification for `--verify` tasks |
| `0c402496` | fix-f-prompt-test-read | F prompt: explicit test file reading (cat, not find) |

## Merge Status

All 5 branches were co-committed to main via shared worktrees. No merge conflicts.

## Test Results

- **cargo test**: 1510 tests, 0 failures, 5 doc-tests pass
- **cargo build --release**: Success (1 warning: unused variable `dir` in coordinator.rs:1817)
- **cargo install --path .**: Binary updated

## Feature Verification

### 1. verify_mode = "separate" (impl-executor-separate-verify)
- Config field `verify_mode: String` in `CoordinatorConfig` (src/config.rs:2196-2197)
- Options: "inline" (default) and "separate"
- Separate mode spawns a new agent context for verification, preventing false-PASS rates
- **Status: FUNCTIONAL**

### 2. auto_test_discovery (impl-executor-test-discovery)
- Config field `auto_test_discovery: bool` (src/config.rs:2227-2228, default: true)
- `discover_test_files()` scans project for test files (Python, Rust, JS patterns)
- Discovered tests injected into agent context as `## Discovered Test Files` section
- Auto-generates `--verify` commands from discovered test files
- **Status: FUNCTIONAL**

### 3. decomp_guidance (impl-executor-decomp-templates)
- Config field `decomp_guidance: bool` (src/config.rs:222-223, default: true)
- `classify_task_complexity()` categorizes tasks: Atomic, Pipeline, FanOut, Integration, Ambiguous
- `build_decomposition_guidance()` injects pattern-matched templates
- **Status: FUNCTIONAL**

### 4. wg context injection for non-Claude models (impl-executor-context-injection)
- `read_wg_guide()` reads `.wg/wg-guide.md` or falls back to built-in default
- Injected when `executor_type == "native"` (src/commands/spawn/execution.rs:311-316)
- **Status: FUNCTIONAL**

### 5. F prompt fix (fix-f-prompt-test-read)
- `build_condition_f_prompt()` now uses `cat /tests/test_outputs.py` instead of `find`
- Includes explicit pytest command and "ground truth" language
- **Status: FUNCTIONAL**

## Condition F Smoke Test

**Task:** cancel-async-tasks
**Model:** minimax/minimax-m2.7 (via OpenRouter)
**Result:** PASS (score: 1.0)

### Key Finding: Agent Reads test_outputs.py
Turn 0 (first action): agent executes `cat /tests/test_outputs.py` — the updated F prompt successfully directs the agent to read (not search for) the test file.

Trial details:
- 17 turns, 79.5s wall clock
- 110K input tokens, 4.6K output tokens
- Tool calls: 13 bash, 2 write_file, 1 read_file, 1 edit_file
- 10 verification iterations
- Termination: no_tool_calls (agent completed naturally)

## Summary

All 5 executor improvement tracks are integrated, tested, and functional. The Condition F smoke test confirms end-to-end operation with the improved prompt directing the agent to read test files on its first turn.
