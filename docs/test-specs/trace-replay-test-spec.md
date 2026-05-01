# Test Specification: wg trace, wg replay, wg runs

Comprehensive test coverage spec for the trace/replay/runs subsystem.

**Source files:**
- `src/commands/trace.rs` — trace command logic
- `src/commands/replay.rs` — replay command logic
- `src/commands/runs_cmd.rs` — runs list/show/restore/diff commands
- `src/runs.rs` — run snapshot storage layer

**Test files:**
- `tests/integration_trace_replay.rs` — original integration tests (15 tests)
- `tests/integration_trace_exhaustive.rs` — exhaustive trace tests (34 tests)
- `tests/integration_replay_exhaustive.rs` — exhaustive replay/trace/runs tests (63 tests)
- `tests/integration_runs_exhaustive.rs` — exhaustive runs tests (28 tests)

**Total: 140 tests** (full suite: 1,966 tests, 0 failures)

**Last updated:** 2026-02-19 — added edge cases 1.21–1.25, 2.33–2.38, 3.29–3.31, 4.8–4.9, 5.12–5.14

---

## Coverage Summary

Legend: [COVERED] = tested, [EXISTS] = original tests exist, [GAP] = no test coverage.

---

## 1. TRACE

### 1.1 trace_no_agent_runs_manual_done [COVERED]

**Description:** Trace a task that was completed manually (no agent was ever spawned). The agent_runs list should be empty and summary.agent_run_count should be 0.

**Setup:**
1. Create a workgraph with one task `t1` (status: Done, started_at and completed_at set).
2. Record provenance entries for `add_task`, `claim` (by human), and `done`.
3. Do NOT create any `log/agents/t1/` directory.

**Expected outcome:**
- `wg trace t1` succeeds.
- Output contains `Agent runs: (none)`.
- `wg trace t1 --json` returns `agent_runs: []` and `summary.agent_run_count: 0`.
- Operations section lists the provenance entries.

**Coverage:** Unit test `test_trace_basic_task_summary`. Exhaustive tests `test_trace_no_agent_runs_summary_output`, `test_trace_no_agent_runs_json_output` (trace_exhaustive), `test_trace_no_agent_runs_output_content` (replay_exhaustive).

---

### 1.2 trace_multiple_agent_runs_retried_task [COVERED]

**Description:** Trace a task that was retried, producing multiple agent run archive directories.

**Setup:**
1. Create a workgraph with task `t1`.
2. Create two archive directories under `log/agents/t1/`:
   - `2026-02-18T10:00:00Z/` with `prompt.txt` and `output.txt`
   - `2026-02-18T11:00:00Z/` with `prompt.txt` and `output.txt`

**Expected outcome:**
- `wg trace t1` shows `Agent runs (2):` with both runs listed chronologically.
- `wg trace t1 --json` returns `agent_runs` array of length 2, sorted by timestamp.
- `summary.agent_run_count` equals 2.

**Coverage:** Exhaustive tests `test_trace_multiple_agent_runs_summary`, `test_trace_multiple_agent_runs_json_sorted` (trace_exhaustive), `test_trace_multiple_agent_runs` (replay_exhaustive).

---

### 1.3 trace_json_structure_validation [COVERED]

**Description:** Validate that `--json` output contains all required fields and is parseable.

**Setup:**
1. Create a task with all optional fields populated (assigned, created_at, started_at, completed_at).
2. Record provenance entries.
3. Create an agent archive with stream-json output containing tool_use and assistant messages.

**Expected outcome:**
JSON output contains:
- `id` (string)
- `title` (string)
- `status` (string: "open"|"done"|"failed"|"abandoned"|"blocked")
- `assigned` (string or absent)
- `created_at` (string or absent)
- `started_at` (string or absent)
- `completed_at` (string or absent)
- `operations` (array of objects, each with `timestamp`, `op`, `actor`, `detail`, `task_id`)
- `agent_runs` (array of objects, each with `timestamp`, and conditionally: `prompt_bytes`, `output_bytes`, `prompt_lines`, `output_lines`, `prompt`, `output`, `tool_calls`, `turns`)
- `summary` (object with `operation_count`, `agent_run_count`, and conditionally: `duration_secs`, `duration_human`, `total_tool_calls`, `total_turns`, `total_output_bytes`)

**Coverage:** Exhaustive tests `test_trace_json_full_structure` (trace_exhaustive), `test_trace_json_structure_validation` (replay_exhaustive).

---

### 1.4 trace_full_output_contains_conversation [COVERED]

**Description:** `--full` mode should print the complete prompt and output content from each agent run.

**Setup:**
1. Create task `t1` with an agent archive containing multi-line prompt.txt and output.txt.

**Expected outcome:**
- Output contains `[Prompt]` and `[Output]` headers.
- The actual prompt and output text appear verbatim in the output.
- Byte counts in brackets match actual content size.

**Coverage:** Exhaustive tests `test_trace_full_shows_prompt_and_output_content`, `test_trace_full_multiple_runs_shows_all` (trace_exhaustive), `test_trace_full_output_contains_conversation` (replay_exhaustive).

---

### 1.5 trace_ops_only_shows_only_provenance [COVERED]

**Description:** `--ops-only` mode shows only provenance log entries, no agent runs, no summary.

**Setup:**
1. Create task with provenance entries (add_task, claim, done).
2. Create an agent archive directory (should be ignored).

**Expected outcome:**
- Output contains `Operations for 'task-id'` header.
- Lists each operation with timestamp, op name, and actor.
- Does NOT contain `Agent runs`, `Summary`, or `Duration`.

**Coverage:** Exhaustive tests `test_trace_ops_only_shows_operations`, `test_trace_ops_only_no_operations` (trace_exhaustive), `test_trace_ops_only_excludes_agent_runs` (replay_exhaustive).

---

### 1.6 trace_nonexistent_task [COVERED]

**Description:** Tracing a task that doesn't exist should return an error.

**Setup:**
1. Create a workgraph with task `t1`.

**Expected outcome:**
- `wg trace nonexistent` returns non-zero exit code.
- Error message contains "not found".

**Coverage:** Unit test `test_trace_nonexistent_task`. Integration test `test_trace_nonexistent_task` (trace_exhaustive).

---

### 1.7 trace_in_progress_task [COVERED]

**Description:** Trace a task that is currently in-progress (status: Open with assigned and started_at set, but no completed_at).

**Setup:**
1. Create task `t1` with status=Open, assigned="agent-1", started_at=now, completed_at=None.
2. Create a partial agent archive.

**Expected outcome:**
- `wg trace t1` succeeds and shows status as "open".
- `summary.duration_secs` is None (no completed_at).
- `summary.duration_human` is absent.
- Agent runs (if archive exists) are still listed.

**Coverage:** Exhaustive tests `test_trace_in_progress_task_summary`, `test_trace_in_progress_task_json_no_duration` (trace_exhaustive), `test_trace_in_progress_task` (replay_exhaustive).

---

### 1.8 trace_with_rotated_log_files [COVERED]

**Description:** Trace should read operations from both the active `operations.jsonl` and rotated compressed `.jsonl.zst` files.

**Setup:**
1. Create task `t1`.
2. Create a rotated file at `log/<timestamp>.jsonl.zst` containing zstd-compressed JSONL with operations for `t1`.
3. Create the active `log/operations.jsonl` with additional operations for `t1`.

**Expected outcome:**
- `wg trace t1` shows all operations from both rotated and active files.
- `--json` output `operations` array contains entries from both files.
- Operations are ordered chronologically.

**Coverage:** Exhaustive test `test_trace_with_rotated_operations_logs` (trace_exhaustive).

---

### 1.9 trace_output_size_accuracy [COVERED]

**Description:** Verify that the reported output size (KB/MB) in trace summary matches actual file sizes.

**Setup:**
1. Create task with agent archive.
2. Write a known-size output.txt (e.g., exactly 10240 bytes).

**Expected outcome:**
- `wg trace t1` summary shows `Total output: 10.0 KB`.
- `wg trace t1 --json` shows `summary.total_output_bytes: 10240`.

**Coverage:** Exhaustive tests `test_trace_output_size_accuracy_summary`, `test_trace_output_size_accuracy_json`, `test_trace_output_size_accuracy_megabytes`, `test_trace_output_size_multiple_runs_summed` (trace_exhaustive), `test_trace_output_size_accuracy` (replay_exhaustive).

---

### 1.10 trace_turn_count_accuracy [COVERED]

**Description:** Verify turn count and tool call count are accurately parsed from stream-json output.

**Setup:**
1. Create task with agent archive containing output.txt with known stream-json content:
   - 3 `{"type":"assistant",...}` lines (= 3 turns)
   - 5 `{"type":"tool_use",...}` lines (= 5 tool calls)
   - 1 `{"type":"result",...}` line

**Expected outcome:**
- `wg trace t1 --json` returns `agent_runs[0].turns: 3` and `agent_runs[0].tool_calls: 5`.
- `summary.total_turns: 3` and `summary.total_tool_calls: 5`.

**Coverage:** Exhaustive tests `test_trace_turn_count_accuracy`, `test_trace_turn_count_summary_display`, `test_trace_turn_count_multiple_runs_summed` (trace_exhaustive), `test_trace_turn_count_accuracy` (replay_exhaustive).

---

### 1.11 trace_uninitialized_workgraph [COVERED]

**Description:** Tracing when no workgraph exists should return an error.

**Setup:** Empty temp directory (no `.wg/`).

**Expected outcome:** Error message contains "not initialized".

**Coverage:** Unit test `test_trace_not_initialized`. Integration test `test_trace_uninitialized_workgraph` (trace_exhaustive).

---

### 1.12 trace_content_block_tool_use_counting [COVERED]

**Description:** Verify that tool calls are counted from both top-level `{"type":"tool_use"}` messages and `content_block` entries with `"type":"tool_use"`.

**Setup:**
1. Create agent archive with output.txt containing both formats:
   ```json
   {"type":"tool_use","name":"Read","id":"1"}
   {"content_block":{"type":"tool_use","name":"Write","id":"2"}}
   ```

**Expected outcome:**
- Both tool calls are counted (tool_calls = 2).
- No double-counting occurs.

**Coverage:** Exhaustive tests `test_trace_content_block_tool_use_counting`, `test_trace_content_block_no_double_counting` (trace_exhaustive), `test_trace_content_block_tool_use_counting` (replay_exhaustive).

---

### 1.13 trace_json_flag_overrides_full_and_ops_only [COVERED]

**Description:** The global `--json` flag takes priority over `--full` and `--ops-only` in the CLI dispatch (main.rs:1924-1934). When `--json` is set, mode is always `TraceMode::Json` regardless of other flags.

**Setup:**
1. Create task with agent archives and provenance entries.

**Expected outcome:**
- `wg --json trace t1 --full` produces JSON output (not the full text format).
- `wg --json trace t1 --ops-only` produces JSON output (not ops-only text).
- Both outputs are valid JSON with the same `TraceOutput` structure.

**Coverage:** Exhaustive tests `test_trace_json_overrides_full_flag`, `test_trace_json_overrides_ops_only_flag` (trace_exhaustive), `test_trace_json_overrides_full_and_ops_only` (replay_exhaustive).

---

### 1.14 trace_agent_archive_missing_output_txt [COVERED]

**Description:** When an agent archive directory exists with `prompt.txt` but no `output.txt`, trace should handle it gracefully.

**Setup:**
1. Create task `t1`.
2. Create archive directory `log/agents/t1/<timestamp>/` with only `prompt.txt`.

**Expected outcome:**
- `wg trace t1` succeeds without error.
- Agent run shows prompt_bytes but output_bytes is None.
- tool_calls and turns are 0 (no output to parse).

**Code path:** `load_agent_runs` at trace.rs:166 — `fs::read_to_string(&output_path).ok()` returns None.

**Coverage:** Exhaustive tests `test_trace_agent_archive_missing_output` (trace_exhaustive), `test_trace_agent_archive_missing_output` (replay_exhaustive).

---

### 1.15 trace_agent_archive_empty_output [COVERED]

**Description:** When output.txt is empty (0 bytes), trace reports correct sizes and 0 turns/tool_calls.

**Setup:**
1. Create task with archive, output.txt is empty string.

**Expected outcome:**
- `output_bytes: 0`
- `output_lines: 0`
- `tool_calls` and `turns` are None/omitted (0 values are skipped).
- `summary.total_output_bytes` is None/omitted (sum is 0).

**Coverage:** Exhaustive tests `test_trace_agent_archive_empty_output` (trace_exhaustive), `test_trace_agent_archive_empty_output` (replay_exhaustive).

---

### 1.16 trace_operation_detail_truncation [COVERED]

**Description:** In summary and full mode, operation details >120 chars are truncated with "..." (trace.rs:368-371). Details <=120 chars are shown in full.

**Setup:**
1. Create task with two operations:
   - One with short detail (< 120 chars).
   - One with long detail (> 120 chars).

**Expected outcome:**
- Short detail printed in full.
- Long detail truncated to 117 chars + "...".
- JSON mode is unaffected (full detail always present).

**Coverage:** Exhaustive tests `test_trace_operation_detail_truncation` (trace_exhaustive), `test_trace_operation_detail_truncation` (replay_exhaustive).

---

### 1.17 trace_all_task_statuses [COVERED]

**Description:** Trace works correctly for all possible task statuses: Open, Done, Failed, Abandoned.

**Setup:**
1. Create tasks with each status.

**Expected outcome:**
- Each `wg trace <id>` succeeds.
- Status field in output matches the task status.
- `--json` output `status` field is the lowercase string form.

**Coverage:** Exhaustive test `test_trace_various_statuses` (trace_exhaustive).

---

### 1.18 trace_json_omits_zero_stats [COVERED]

**Description:** When tool_calls and turns are 0 (e.g., non-JSON output), they should be omitted from JSON (`skip_serializing_if`) rather than serialized as 0.

**Setup:**
1. Create agent archive with plain text output (no JSON lines).

**Expected outcome:**
- `agent_runs[0].tool_calls` is absent (null in JSON).
- `agent_runs[0].turns` is absent (null in JSON).
- `summary.total_tool_calls` is absent.
- `summary.total_turns` is absent.

**Coverage:** Exhaustive test `test_trace_json_no_tool_calls_or_turns_omitted` (trace_exhaustive).

---

### 1.19 trace_agent_runs_chronological_sort [COVERED]

**Description:** Agent runs are sorted by directory name (timestamp), ensuring chronological order even if filesystem ordering varies.

**Setup:**
1. Create 3 archive directories with timestamps out of filesystem order.

**Expected outcome:**
- Runs appear in chronological order by timestamp.

**Coverage:** Exhaustive tests `test_trace_agent_runs_sort_order` (trace_exhaustive), `test_trace_agent_runs_sort_order` (replay_exhaustive).

---

### 1.20 trace_summary_mode_excludes_content [COVERED]

**Description:** In summary mode (default), agent run content (prompt and output text) is NOT loaded or displayed. Only metadata (bytes, lines, tool_calls, turns) is shown.

**Setup:**
1. Create task with agent archive.

**Expected outcome:**
- Summary output does NOT contain the prompt or output text verbatim.
- Shows "Output: X.X KB (Y lines)" format instead.
- In JSON mode, prompt and output content ARE included.

**Code path:** `load_agent_runs(dir, id, false)` at trace.rs:245 — `include_content=false`.

**Coverage:** Exhaustive tests `test_trace_summary_mode_excludes_content` (trace_exhaustive), `test_trace_summary_mode_excludes_content` (replay_exhaustive).

---

### 1.21 trace_blocked_and_abandoned_and_inprogress_statuses [GAP]

**Description:** Test 1.17 covers Open, Done, and Failed statuses but omits Blocked, Abandoned, and InProgress. The Status enum has 6 variants: Open, InProgress, Done, Blocked, Failed, Abandoned. Trace should handle all of them.

**Setup:**
1. Create tasks with each status: Blocked, Abandoned, InProgress.

**Expected outcome:**
- Each `wg trace <id>` succeeds.
- `--json` output `status` field matches the lowercase display form ("blocked", "abandoned", "in-progress").
- InProgress task shows correctly (distinct from Open with assigned).

**Code path:** `trace.rs:198` — `graph.get_task_or_err(id)` fetches the task regardless of status; `TraceOutput.status` serializes via `Status`'s `Serialize` impl.

---

### 1.22 trace_agent_archive_missing_prompt_txt [GAP]

**Description:** When an agent archive directory exists with `output.txt` but no `prompt.txt`, trace should handle it gracefully. This is the inverse of 1.14 (missing output.txt).

**Setup:**
1. Create task `t1`.
2. Create archive directory `log/agents/t1/<timestamp>/` with only `output.txt`.

**Expected outcome:**
- `wg trace t1` succeeds without error.
- Agent run shows output_bytes but prompt_bytes is None.
- In JSON mode: `prompt_bytes`, `prompt_lines`, and `prompt` are all absent/null.
- tool_calls and turns are parsed from output.txt normally.

**Code path:** `trace.rs:157` — `fs::metadata(&prompt_path).ok()` returns None when prompt.txt doesn't exist. `trace.rs:160-163` — `prompt_content` is None when `include_content` is true. `trace.rs:170-174` — `prompt_lines` falls back to reading the file for line count.

---

### 1.23 trace_agent_archive_empty_directory [GAP]

**Description:** When an agent archive timestamp directory exists but contains no files at all (no prompt.txt, no output.txt), trace should handle it gracefully.

**Setup:**
1. Create task `t1`.
2. Create empty archive directory `log/agents/t1/<timestamp>/` with no files.

**Expected outcome:**
- `wg trace t1` succeeds without error.
- Agent run appears in the list with all file-derived fields absent/null.
- `summary.agent_run_count` counts it.

**Code path:** `trace.rs:157-158` — both `fs::metadata` calls return None. `trace.rs:166` — `fs::read_to_string` returns Err, `.ok()` yields None.

---

### 1.24 trace_operations_filtering_accuracy [GAP]

**Description:** Verify that `wg trace <id>` only shows operations that match the specific task_id, not operations for other tasks or global operations (task_id=None).

**Setup:**
1. Create tasks t1, t2.
2. Record operations: `add_task` for t1, `add_task` for t2, `replay` with task_id=None.

**Expected outcome:**
- `wg trace t1` shows only the t1 operations (not t2's or the global replay op).
- `wg trace t1 --json` operations array contains only entries where `task_id == "t1"`.

**Code path:** `trace.rs:202-205` — `.filter(|e| e.task_id.as_deref() == Some(id))`.

---

### 1.25 trace_with_unparseable_timestamps [GAP]

**Description:** When a task has `started_at` and `completed_at` that are not valid RFC3339 timestamps, `build_summary` should return None for duration instead of panicking.

**Setup:**
1. Create task with `started_at: "not-a-timestamp"`, `completed_at: "also-not-a-timestamp"`.

**Expected outcome:**
- `wg trace t1` succeeds.
- No duration displayed in summary.
- `--json` output: `summary.duration_secs` and `summary.duration_human` are absent/null.

**Code path:** `trace.rs:265-274` — `.parse::<DateTime<chrono::FixedOffset>>().ok()` returns None for bad timestamps, so duration computation falls through to None.

---

## 2. REPLAY

### 2.1 replay_failed_only_basic [COVERED]

**Description:** `--failed-only` resets only Failed/Abandoned tasks, preserves Done tasks.

**Setup:**
1. Graph with t1 (Done), t2 (Failed), t3 (Open).

**Expected outcome:**
- t2 reset to Open, t1 preserved as Done, t3 remains Open.
- Snapshot created in `runs/`.

**Coverage:** Unit test `test_replay_failed_only`. Integration test `test_replay_failed_only_resets_and_preserves`.

---

### 2.2 replay_failed_only_with_abandoned [COVERED]

**Description:** `--failed-only` should also reset Abandoned tasks (both Failed and Abandoned match the `Status::Failed | Status::Abandoned` pattern).

**Setup:**
1. Graph with t1 (Failed), t2 (Abandoned), t3 (Done).

**Expected outcome:**
- Both t1 and t2 reset to Open. t3 preserved.

**Coverage:** Exhaustive test `test_replay_failed_only_with_abandoned` (replay_exhaustive).

---

### 2.3 replay_below_score_various_thresholds [COVERED]

**Description:** `--below-score <threshold>` resets terminal tasks scoring below the threshold. Tasks with no evaluation score are also reset.

**Setup:**
1. Graph with tasks: high-score (0.95), medium-score (0.6), low-score (0.2), no-score (Done, no eval).
2. Evaluation files for high, medium, low.

**Expected outcome with `--below-score 0.5`:**
- low-score (0.2 < 0.5) reset.
- no-score (no eval, terminal) reset.
- medium-score (0.6 >= 0.5) preserved.
- high-score (0.95 >= 0.5) preserved.

**Expected outcome with `--below-score 1.0`:**
- All terminal tasks reset (nothing scores >= 1.0).

**Coverage:** Exhaustive tests `test_replay_below_score_threshold_0_5`, `test_replay_below_score_threshold_1_0` (replay_exhaustive). Integration test `test_replay_below_score_with_evaluations`.

---

### 2.4 replay_tasks_explicit_list_with_dependents [COVERED]

**Description:** `--tasks a,b` resets exactly those tasks plus their transitive dependents.

**Setup:**
1. Graph: t1 -> t2 -> t3 (dependency chain), t4 (unrelated).
2. All tasks Done.

**Expected outcome with `--tasks t1`:**
- t1, t2, t3 all reset (t2 and t3 are transitive dependents).
- t4 preserved.

**Coverage:** Unit test `test_replay_specific_tasks_with_dependents`. Integration test `test_replay_explicit_tasks`.

---

### 2.5 replay_tasks_multiple_explicit [COVERED]

**Description:** `--tasks a,b` with multiple comma-separated task IDs.

**Setup:**
1. Graph with t1, t2, t3 (all Done, independent — no dependency edges).

**Expected outcome with `--tasks t1,t3`:**
- t1 and t3 reset.
- t2 preserved (not listed, not a dependent).

**Coverage:** Exhaustive test `test_replay_tasks_multiple_explicit` (replay_exhaustive).

---

### 2.6 replay_keep_done_preserves_high_scoring [COVERED]

**Description:** `--keep-done <threshold>` preserves Done tasks whose evaluation score meets or exceeds the threshold, even if they would otherwise be reset as transitive dependents.

**Setup:**
1. Graph: parent (Failed) -> child (Done, score 0.95).
2. `--failed-only --keep-done 0.9`.

**Expected outcome:**
- parent is reset (failed, seed).
- child is a transitive dependent of parent, so it would be reset. BUT child is Done with score 0.95 >= 0.9, so `--keep-done` preserves it.

**Coverage:** Exhaustive test `test_replay_keep_done_preserves_high_scoring` (replay_exhaustive).

---

### 2.7 replay_plan_only_no_side_effects [COVERED]

**Description:** `--plan-only` shows what would happen without modifying the graph, creating snapshots, or recording provenance.

**Setup:**
1. Graph with failed task and dependent.

**Expected outcome:**
- Graph unchanged after command.
- No `runs/` directory created.
- No provenance entries for "replay".
- Output mentions "dry run".

**Coverage:** Unit test `test_replay_plan_only`. Integration tests `test_replay_plan_only_no_side_effects`, `test_replay_plan_only_json_output`.

---

### 2.8 replay_plan_only_json_output [COVERED]

**Description:** `--plan-only --json` returns structured plan with `plan_only: true` and `run_id: "(dry run)"`.

**Coverage:** Integration test `test_replay_plan_only_json_output`.

---

### 2.9 replay_model_override [COVERED]

**Description:** `--model <model>` sets the model field on all reset tasks (including transitive dependents, not just seeds).

**Setup:**
1. Graph with root (Failed) -> mid (Done) -> leaf (Done).
2. `--failed-only --model different-model`.

**Expected outcome:**
- root, mid, leaf all reset to Open.
- All three have `model: Some("different-model")`.
- Run metadata records the model override.

**Coverage:** Integration test `test_replay_failed_only_resets_and_preserves` checks model on reset tasks. Unit test `test_replay_specific_tasks_with_dependents` also checks model.

---

### 2.10 replay_subgraph_scope [COVERED]

**Description:** `--subgraph <root>` only considers tasks in the subgraph rooted at the given task (following `blocks` edges forward).

**Setup:**
1. Graph: root -> child (subgraph), unrelated (outside subgraph). All failed.

**Expected outcome with `--failed-only --subgraph root`:**
- root and child are reset.
- unrelated is NOT reset (outside subgraph).

**Coverage:** Unit test `test_replay_subgraph`. Integration test `test_replay_subgraph_scopes_correctly`.

---

### 2.11 replay_records_provenance [COVERED]

**Description:** Replay records a "replay" provenance entry with run_id, model, reset_count, and reset_tasks.

**Setup:**
1. Run replay on a graph with failed tasks.

**Expected outcome:**
- Provenance log contains a "replay" entry.
- Entry detail has `run_id`, `model`, `reset_count`, `reset_tasks`.
- `task_id` is None (replay is a graph-level operation).

**Coverage:** Integration test `test_replay_creates_provenance_entries`.

---

### 2.12 replay_preserves_log_and_blocked_by_on_reset [COVERED]

**Description:** When tasks are reset, structural and metadata fields are preserved while execution state is cleared.

**Setup:**
1. Create a task with all fields populated: blocked_by, blocks, description, tags, skills, log entries, artifacts, assigned, started_at, completed_at, failure_reason, loop_iteration=3, paused=true.

**Expected outcome after reset:**
- **Cleared:** status→Open, assigned→None, started_at→None, completed_at→None, artifacts→empty, loop_iteration→0, failure_reason→None, paused→false
- **Preserved:** blocked_by, blocks, title, description, tags, skills, log

**Coverage:** Unit test `test_reset_task_clears_fields`. Exhaustive test `test_replay_field_clearing_and_preservation` (replay_exhaustive).

---

### 2.13 replay_clears_assigned_started_completed_artifacts [COVERED]

**Description:** Verify the specific fields cleared by `reset_task()`.

**Coverage:** Unit test `test_reset_task_clears_fields`.

---

### 2.14 replay_tasks_with_loop_edges [COVERED]

**Description:** Replay correctly handles tasks that have `loops_to` edges. The loop edges should be preserved, and `loop_iteration` should be reset to 0.

**Setup:**
1. Graph with loop: src (Done, loop_iteration=3) -> tgt (Done, loop_iteration=3), src.loops_to=[LoopEdge{target: tgt, max_iterations: 5}].

**Expected outcome after `replay` (default: all terminal):**
- Both tasks reset to Open.
- loop_iteration reset to 0 on both.
- loops_to edges preserved on src.
- blocked_by/blocks edges preserved.

**Coverage:** Exhaustive test `test_replay_tasks_with_loop_edges` (replay_exhaustive).

---

### 2.15 replay_empty_graph [COVERED]

**Description:** Replay on a graph with no tasks.

**Setup:**
1. Initialize workgraph with empty graph (no tasks).

**Expected outcome:**
- `wg replay --failed-only` succeeds with "No tasks match" message.
- No snapshot created.

**Coverage:** Exhaustive test `test_replay_empty_graph` (replay_exhaustive).

---

### 2.16 replay_no_matching_tasks [COVERED]

**Description:** When no tasks match the filter criteria, replay reports "no matching tasks" and creates no snapshot.

**Coverage:** Unit test `test_replay_no_matching_tasks`. Integration test `test_replay_no_matching_tasks`.

---

### 2.17 replay_subgraph_nonexistent_root [COVERED]

**Description:** `--subgraph nonexistent` should return an error mentioning "not found".

**Coverage:** Exhaustive test `test_replay_subgraph_nonexistent_root` (replay_exhaustive).

---

### 2.18 replay_default_all_terminal [COVERED]

**Description:** Without any filter flags, replay resets all terminal (Done, Failed, Abandoned) tasks.

**Coverage:** Unit test `test_replay_all_terminal`.

---

### 2.19 replay_below_score_no_eval_resets_terminal [COVERED]

**Description:** When using `--below-score`, terminal tasks with no evaluation data are also reset (no evidence of quality).

**Coverage:** Integration test `test_replay_below_score_with_evaluations` includes `no-score` task.

---

### 2.20 replay_json_output_structure [COVERED]

**Description:** `--json` output contains run_id, model, reset_tasks, preserved_tasks, plan_only.

**Coverage:** Integration test `test_replay_json_output`.

---

### 2.21 replay_below_score_non_terminal_behavior [COVERED]

**Description:** `--below-score` behavior on non-terminal (Open) tasks. The code at replay.rs:87-98 adds tasks with score < threshold regardless of terminal status. For non-terminal tasks with a score, they DO get added to seeds if score < threshold. However, since they're already Open, the reset is a no-op on status.

**Setup:**
1. Graph with: open_task (Open, score=0.1), done_task (Done, score=0.1).

**Expected outcome with `--below-score 0.5`:**
- done_task reset (terminal + low score).
- open_task: included in seeds but already Open — status unchanged.

**Coverage:** Exhaustive test `test_replay_below_score_non_terminal_ignored` (replay_exhaustive).

---

### 2.22 replay_transitive_dependents_chain [COVERED]

**Description:** Transitive dependent collection follows multi-hop dependency chains (4-deep).

**Setup:**
1. Graph: a -> b -> c -> d (chain of 4). a=Failed, b,c,d=Done.

**Expected outcome with `--failed-only`:**
- a is reset (failed = seed).
- b, c, d are all reset (transitive dependents of a).

**Coverage:** Exhaustive test `test_replay_transitive_dependents_deep_chain` (replay_exhaustive).

---

### 2.23 replay_diamond_dependency [COVERED]

**Description:** Transitive dependents correctly handles diamond-shaped dependency graphs without duplication.

**Setup:**
1. Graph: a -> b, a -> c, b -> d, c -> d (diamond). a=Failed, b,c,d=Done.

**Expected outcome with `--failed-only`:**
- All four tasks reset.
- d appears only once in reset_tasks (no duplication).

**Coverage:** Exhaustive test `test_replay_diamond_dependency` (replay_exhaustive).

---

### 2.24 replay_keep_done_with_no_evaluations [COVERED]

**Description:** `--keep-done 0.8` when no evaluations exist. Since no task has a score, keep_done cannot preserve anything.

**Setup:**
1. Graph with t1 (Done), no evaluation files.

**Expected outcome with default filter + `--keep-done 0.8`:**
- t1 is reset (no score, so it's not kept).

**Coverage:** Exhaustive test `test_replay_keep_done_with_no_evaluations` (replay_exhaustive).

---

### 2.25 replay_filter_description_in_metadata [COVERED]

**Description:** The run metadata `filter` field accurately describes the replay filter options used.

**Setup:**
1. Run replay with `--failed-only --model opus --subgraph root`.

**Expected outcome:**
- Run metadata `filter` contains `--failed-only`, `--model opus`, `--subgraph root`.

**Coverage:** Exhaustive test `test_replay_filter_description_in_metadata` (replay_exhaustive).

---

### 2.26 replay_filter_priority [COVERED]

**Description:** The replay filter has an implicit priority order due to `continue` statements in the loop (replay.rs:63-104): `--tasks` > `--failed-only` > `--below-score` > default (all terminal). When multiple conflicting flags are provided, only the highest-priority one takes effect.

**Setup:**
1. Graph with t1 (Failed), t2 (Done).
2. Test case A: `--tasks t2 --failed-only` — should only reset t2 (explicit list wins).
3. Test case B: `--failed-only --below-score 0.5` — should only apply --failed-only (ignores below-score).

**Expected outcome:**
- A: t2 is reset (explicitly listed), t1 is NOT reset (not in explicit list).
- B: t1 is reset (failed), t2 is NOT reset (Done, not failed).

**Coverage:** Exhaustive tests `test_replay_filter_priority_tasks_over_failed_only`, `test_replay_filter_priority_failed_only_over_below_score` (replay_exhaustive).

---

### 2.27 replay_below_score_exact_boundary [COVERED]

**Description:** When a task's evaluation score exactly equals the threshold, it should NOT be reset (the check is `score < threshold` at replay.rs:89).

**Setup:**
1. Graph with task "boundary" (Done, score=0.7).
2. `--below-score 0.7`.

**Expected outcome:**
- "boundary" is preserved (0.7 is NOT < 0.7).

**Coverage:** Exhaustive test `test_replay_below_score_exact_boundary` (replay_exhaustive).

---

### 2.28 replay_tasks_with_nonexistent_id [COVERED]

**Description:** `--tasks nonexistent` where the task ID doesn't exist in the graph. The code iterates `graph.tasks()` and checks `if opts.tasks.contains(&task.id)`, so nonexistent IDs are silently ignored (never match any graph task).

**Setup:**
1. Graph with t1 (Done).
2. `--tasks nonexistent`.

**Expected outcome:**
- No tasks are reset (no match in graph).
- "No tasks match" message.
- No snapshot created.

**Coverage:** Exhaustive test `test_replay_tasks_nonexistent_id` (replay_exhaustive).

---

### 2.29 replay_subgraph_single_node [COVERED]

**Description:** `--subgraph <id>` where the root task has no `blocks` edges (single-node subgraph).

**Setup:**
1. Graph with standalone (Failed, no blocks edges) and other (Failed).
2. `--failed-only --subgraph standalone`.

**Expected outcome:**
- standalone is reset (in subgraph + failed).
- other is NOT reset (outside subgraph).

**Coverage:** Exhaustive test `test_replay_subgraph_single_node` (replay_exhaustive).

---

### 2.30 replay_model_override_not_applied_to_preserved [COVERED]

**Description:** The `--model` flag only sets model on reset tasks, not on preserved tasks.

**Setup:**
1. Graph with t1 (Failed) and t2 (Done, model=None).
2. `--failed-only --model new-model`.

**Expected outcome:**
- t1.model = Some("new-model") (was reset).
- t2.model remains None (was preserved, not touched).

**Code path:** replay.rs:211-218 — model override only applied inside the `for task_id in &reset_ids` loop.

**Coverage:** Exhaustive test `test_replay_model_override_not_applied_to_preserved` (replay_exhaustive).

---

### 2.31 replay_below_score_zero_threshold [COVERED]

**Description:** `--below-score 0.0` means nothing is "below" 0.0 since all scores are >= 0. However, tasks with NO evaluation are still reset (replay.rs:93-96: terminal tasks without a score are reset).

**Setup:**
1. Graph with: scored (Done, score=0.1), unscored (Done, no eval).
2. `--below-score 0.0`.

**Expected outcome:**
- scored is preserved (0.1 is NOT < 0.0).
- unscored is reset (no score + terminal).

**Coverage:** Exhaustive test `test_replay_below_score_zero_threshold` (replay_exhaustive).

---

### 2.32 replay_multiple_evals_uses_highest [COVERED]

**Description:** When multiple evaluations exist for the same task, `build_score_map` keeps the highest score for threshold comparison.

**Setup:**
1. Two evaluations for task t1: score 0.4 and score 0.8.
2. `--below-score 0.5`.

**Expected outcome:**
- t1 preserved (highest score 0.8 >= 0.5).

**Coverage:** Exhaustive test `test_replay_multiple_evals_keeps_highest_score` (replay_exhaustive).

---

### 2.33 replay_tasks_and_subgraph_combined [GAP]

**Description:** When both `--tasks` and `--subgraph` are provided, the subgraph filter runs first (replay.rs:66-69), then explicit task matching runs inside the filtered set. Tasks outside the subgraph should be skipped even if explicitly listed.

**Setup:**
1. Graph: root (Done) -> child (Done), outside (Done, no blocks edges to root).
2. `--tasks outside,child --subgraph root`.

**Expected outcome:**
- `child` is in the subgraph and is in the explicit list → reset.
- `outside` is NOT in the subgraph → skipped by subgraph filter, even though it's in --tasks.
- `root` is in the subgraph but NOT in the explicit task list → preserved.

**Code path:** replay.rs:66-69 checks subgraph first (`if !sg.contains(&task.id) { continue; }`), then replay.rs:72-78 checks `opts.tasks.contains(&task.id)`.

---

### 2.34 replay_config_default_keep_done_threshold [GAP]

**Description:** When `--keep-done` is not explicitly passed, the config's `replay.keep_done_threshold` is used as the default. This means if config has `keep_done_threshold = 0.9`, Done tasks with score >= 0.9 are preserved even without `--keep-done` on the command line.

**Setup:**
1. Create a config.toml with `[replay]\nkeep_done_threshold = 0.9`.
2. Graph: parent (Failed) -> child (Done, score=0.95).
3. Run `wg replay --failed-only` (no explicit --keep-done).

**Expected outcome:**
- parent is reset (failed seed).
- child is a transitive dependent of parent but its score (0.95) >= 0.9 (from config), so it's preserved by the default keep_done_threshold.

**Code path:** replay.rs:41 — `let keep_done_threshold = opts.keep_done.unwrap_or(config.replay.keep_done_threshold);`

---

### 2.35 replay_blocked_task_behavior [GAP]

**Description:** Blocked tasks are NOT terminal (`is_terminal()` returns false for Status::Blocked — see graph.rs:135-137). So `--failed-only` should NOT reset Blocked tasks, and the default filter (all terminal) should NOT include them.

**Setup:**
1. Graph with t1 (Blocked), t2 (Failed), t3 (Done).

**Expected outcome with `--failed-only`:**
- t2 reset (Failed is matched by `Status::Failed | Status::Abandoned`).
- t1 preserved (Blocked is NOT Failed or Abandoned).
- t3 preserved (Done is not Failed or Abandoned).

**Expected outcome with default (no filter flags):**
- t2 reset (Failed is terminal).
- t3 reset (Done is terminal).
- t1 preserved (Blocked is NOT terminal).

**Code path:** replay.rs:81 — `matches!(task.status, Status::Failed | Status::Abandoned)` for --failed-only. replay.rs:101 — `task.status.is_terminal()` for default filter.

---

### 2.36 replay_inprogress_task_behavior [GAP]

**Description:** InProgress tasks are NOT terminal (`is_terminal()` returns false for Status::InProgress). The default filter and `--failed-only` should not touch them.

**Setup:**
1. Graph with t1 (InProgress, assigned="agent-1"), t2 (Failed).

**Expected outcome with `--failed-only`:**
- t2 reset.
- t1 preserved (InProgress is not Failed or Abandoned).

**Expected outcome with default (no filter flags):**
- t2 reset (terminal).
- t1 preserved (InProgress is NOT terminal).

---

### 2.37 replay_tasks_with_duplicates_in_list [GAP]

**Description:** `--tasks a,a` where the same task ID appears twice. The code iterates `graph.tasks()` and checks `opts.tasks.contains(&task.id)`, so duplicates in the task list should be a no-op (the set-based seed collection prevents duplication).

**Setup:**
1. Graph with t1 (Done).
2. `--tasks t1,t1`.

**Expected outcome:**
- t1 reset once (not duplicated in reset_tasks output).
- Replay succeeds normally.

**Code path:** replay.rs:74 — `seeds.insert(task.id.clone())` — HashSet naturally deduplicates.

---

### 2.38 replay_keep_done_only_applies_to_done [GAP]

**Description:** `--keep-done` should only preserve tasks with status `Done` that have a high enough score. Failed/Abandoned tasks with high scores should NOT be preserved by keep-done (they are not Done).

**Setup:**
1. Graph: t1 (Failed, score=0.95), t2 (Done, score=0.95).
2. `--keep-done 0.9` (all terminal filter, no --failed-only).

**Expected outcome:**
- t1 reset (Failed status, keep-done only checks `task.status == Status::Done` at replay.rs:118).
- t2 preserved (Done + score 0.95 >= 0.9).

**Code path:** replay.rs:118 — `if task.status == Status::Done { ... if score >= keep_done_threshold { to_keep.push(task_id.clone()); } }`.

---

## 3. RUNS

### 3.1 runs_list_empty [COVERED]

**Description:** `wg runs list` with no snapshots shows "No run snapshots found."

**Coverage:** Unit test `test_list_empty`. Integration test `test_runs_list_empty` (runs_exhaustive).

---

### 3.2 runs_list_chronological [COVERED]

**Description:** `wg runs list` shows all runs sorted by ID (which is chronological).

**Setup:**
1. Create 3 run snapshots (run-001, run-002, run-003).

**Expected outcome:**
- List shows all three in order.
- Each entry shows ID, timestamp, model (if set), filter, and task counts.

**Coverage:** Exhaustive tests `test_runs_list_chronological_three_runs` (runs_exhaustive), `test_runs_list_three_runs_chronological` (replay_exhaustive).

---

### 3.3 runs_show_metadata [COVERED]

**Description:** `wg runs show <id>` displays correct metadata including ID, timestamp, model, filter, reset_tasks, preserved_tasks.

**Coverage:** Unit test `test_show_run`. Integration test `test_runs_list_and_show`. Exhaustive test `test_runs_show_json_full_metadata` (runs_exhaustive).

---

### 3.4 runs_show_json [COVERED]

**Description:** `wg runs show <id> --json` returns parseable JSON with all metadata fields.

**Coverage:** Integration test `test_runs_list_and_show`. Exhaustive test `test_runs_show_json_full_metadata` (runs_exhaustive).

---

### 3.5 runs_show_nonexistent [COVERED]

**Description:** `wg runs show nonexistent` returns an error.

**Coverage:** Exhaustive tests `test_runs_show_nonexistent` in both runs_exhaustive and replay_exhaustive.

---

### 3.6 runs_restore_restores_graph [COVERED]

**Description:** `wg runs restore <id>` replaces current graph.jsonl with the snapshot's copy, restoring task statuses.

**Setup:**
1. Create graph with t1 (Failed).
2. Replay (creates run-001 with t1=Failed, then resets t1 to Open).
3. Restore from run-001.

**Expected outcome:**
- t1 is back to Failed (pre-replay state).

**Coverage:** Exhaustive tests `test_runs_restore_restores_graph_status` (runs_exhaustive), `test_runs_restore_actual_task_status` (replay_exhaustive).

---

### 3.7 runs_restore_creates_safety_snapshot [COVERED]

**Description:** Before restoring, a safety snapshot of the current graph is created automatically. This allows undoing the restore.

**Coverage:** Unit test `test_restore_creates_safety_snapshot`. Exhaustive test `test_restore_safety_snapshot_distinct` (runs_exhaustive).

---

### 3.8 runs_restore_provenance [COVERED]

**Description:** `wg runs restore` records a "restore" provenance entry with `restored_from` and `safety_snapshot` fields.

**Coverage:** Exhaustive tests `test_runs_restore_provenance` in both runs_exhaustive and replay_exhaustive.

---

### 3.9 runs_restore_nonexistent [COVERED]

**Description:** `wg runs restore nonexistent-run` returns an error.

**Coverage:** Exhaustive tests `test_runs_restore_nonexistent` in both runs_exhaustive and replay_exhaustive.

---

### 3.10 runs_diff_shows_changes [COVERED]

**Description:** `wg runs diff <id>` compares snapshot to current graph, showing status changes, added tasks, and removed tasks.

**Setup:**
1. Create graph with t1 (Done), t2 (Failed).
2. Take snapshot (run-001).
3. Change t2 to Open, add t3.

**Expected outcome:**
- Diff shows t2: Failed -> Open (status_changed).
- Diff shows t3: added.
- t1 shows no change.

**Coverage:** Exhaustive tests `test_runs_diff_shows_status_change`, `test_runs_diff_with_removed_task`, `test_runs_diff_with_added_task` (runs_exhaustive), `test_runs_diff_with_status_change_added_removed`, `test_runs_diff_removed_task`, `test_runs_diff_added_task` (replay_exhaustive).

---

### 3.11 runs_diff_no_changes [COVERED]

**Description:** When current graph matches snapshot, diff reports no differences.

**Coverage:** Exhaustive tests `test_runs_diff_no_changes` in both runs_exhaustive and replay_exhaustive.

---

### 3.12 runs_diff_json_output [COVERED]

**Description:** `wg runs diff <id> --json` returns structured JSON with `run_id`, `changes` array (with id, snapshot_status, current_status, change), and `total_changes`.

**Coverage:** Exhaustive tests `test_runs_diff_json_output` in both runs_exhaustive and replay_exhaustive.

---

### 3.13 runs_diff_nonexistent_run [COVERED]

**Description:** `wg runs diff nonexistent` returns an error.

**Coverage:** Exhaustive tests `test_runs_diff_nonexistent_run` in both runs_exhaustive and replay_exhaustive.

---

### 3.14 runs_list_json [COVERED]

**Description:** `wg runs list --json` returns a JSON array of run metadata objects with id, timestamp, model, filter, reset_tasks, preserved_tasks.

**Coverage:** Exhaustive tests `test_runs_list_json_metadata_structure` (runs_exhaustive), `test_runs_list_json` (replay_exhaustive).

---

### 3.15 run_id_generation_sequential [COVERED]

**Description:** Run IDs are generated as `run-001`, `run-002`, etc., incrementing from the max existing ID.

**Coverage:** Unit test `test_next_run_id_empty` and `test_next_run_id_increments`. Integration test `test_run_id_generation_sequential` (runs_exhaustive).

---

### 3.16 run_id_generation_gap_filling [COVERED]

**Description:** If run-001 and run-003 exist (gap at 002), next ID is `run-004` (max+1, NOT gap-filling).

**Setup:**
1. Create directories run-001 and run-003.

**Expected outcome:**
- `next_run_id` returns `run-004`.

**Coverage:** Unit test `test_next_run_id_increments` tests exactly this case.

---

### 3.17 snapshot_integrity [COVERED]

**Description:** Snapshot copies graph.jsonl faithfully (byte-for-byte match), and meta.json is valid JSON.

**Coverage:** Exhaustive test `test_snapshot_integrity_content_match` (runs_exhaustive).

---

### 3.18 snapshot_without_config_toml [COVERED]

**Description:** Snapshot should succeed even if config.toml doesn't exist. config.toml is absent from snapshot.

**Coverage:** Exhaustive tests `test_snapshot_without_config_toml` in both runs_exhaustive and replay_exhaustive.

---

### 3.19 runs_restore_json_output [COVERED]

**Description:** `wg runs restore <id> --json` returns JSON with `restored_from`, `safety_snapshot`, and `timestamp`.

**Coverage:** Exhaustive tests `test_runs_restore_json_output` in both runs_exhaustive and replay_exhaustive.

---

### 3.20 concurrent_replay_safety [COVERED]

**Description:** Two replays running concurrently should not corrupt the graph or produce duplicate run IDs.

**Setup:**
1. Create graph with multiple failed tasks.
2. Launch two `wg replay --failed-only` commands concurrently.

**Expected outcome:**
- At least one completes without error.
- Run IDs are distinct.
- Graph is in a valid state (parseable).

**Note:** The current `next_run_id` has a TOCTOU race that could theoretically cause duplicate IDs. The test documents actual behavior.

**Coverage:** Exhaustive test `test_concurrent_replay_safety` (runs_exhaustive).

---

### 3.21 runs_list_ignores_non_run_directories [COVERED]

**Description:** Directories in `runs/` that don't match the `run-` prefix are ignored.

**Coverage:** Unit test `test_list_runs` includes a `not-a-run` directory. Integration test `test_runs_list_ignores_non_run_directories` (runs_exhaustive).

---

### 3.22 runs_diff_with_removed_task [COVERED]

**Description:** Diff detects tasks that exist in the snapshot but not in the current graph (change type: "removed").

**Coverage:** Exhaustive tests `test_runs_diff_with_removed_task` in both runs_exhaustive and replay_exhaustive.

---

### 3.23 runs_diff_with_added_task [COVERED]

**Description:** Diff detects tasks that exist in the current graph but not in the snapshot (change type: "added").

**Coverage:** Exhaustive tests `test_runs_diff_with_added_task` in both runs_exhaustive and replay_exhaustive.

---

### 3.24 runs_diff_two_runs_not_implemented [N/A]

**Description:** `wg runs diff <a> <b>` (comparing two snapshots) is NOT implemented. The `diff` subcommand takes a single run ID and compares that snapshot against the current graph. A two-run comparison would require loading two snapshot graphs.

**CLI:** `Diff { id: String }` — single ID only.

**Status:** Feature not implemented. If added, the test should:
1. Create two snapshots at different points.
2. `wg runs diff run-001 run-002`.
3. Verify changes between the two snapshots are correctly identified.

---

### 3.25 runs_list_with_corrupted_metadata [COVERED]

**Description:** `wg runs list` should handle corrupted meta.json gracefully (print warning, skip the entry, continue listing valid runs). See runs_cmd.rs:28-31.

**Setup:**
1. Create run-001 with valid meta.json.
2. Create run-002 directory with an invalid meta.json (not valid JSON).

**Expected outcome:**
- run-001 listed normally.
- Warning printed to stderr about run-002.
- No crash or panic.

**Code path:** runs_cmd.rs:29 — `Err(e) => { eprintln!("Warning: ..."); }`

**Coverage:** Exhaustive test `test_runs_list_with_corrupted_metadata` (runs_exhaustive).

---

### 3.26 runs_restore_missing_snapshot_graph [COVERED]

**Description:** `wg runs restore <id>` when the snapshot's graph.jsonl is missing or corrupted.

**Setup:**
1. Create run-001 with valid meta.json but delete graph.jsonl from the run directory.

**Expected outcome:**
- Error mentioning "Snapshot graph.jsonl not found".

**Code path:** runs.rs:129-131.

**Coverage:** Exhaustive test `test_runs_restore_missing_snapshot_graph` (runs_exhaustive).

---

### 3.27 runs_restore_safety_snapshot_incrementing [COVERED]

**Description:** Multiple restores from the same snapshot create incrementing safety snapshot IDs.

**Coverage:** Exhaustive test `test_multiple_restores_incrementing_ids` (runs_exhaustive).

---

### 3.28 runs_diff_sorted_output [COVERED]

**Description:** Diff output shows changes sorted alphabetically by task ID (runs_cmd.rs:176 — `all_ids.sort()`).

**Setup:**
1. Create graph with tasks z1, a1, m1 (all with status changes).

**Expected outcome:**
- Changes listed in order: a1, m1, z1.

**Coverage:** Exhaustive test `test_runs_diff_sorted_output` (runs_exhaustive).

---

### 3.29 runs_snapshot_without_graph_jsonl [GAP]

**Description:** If `graph.jsonl` doesn't exist at snapshot time (edge case — normally impossible since we just loaded it), the snapshot skips the copy. This means the run directory has `meta.json` but no `graph.jsonl`.

**Setup:**
1. Create a workgraph, then manually delete graph.jsonl before triggering snapshot via the runs API.
2. Alternatively: test via `runs::snapshot()` directly when graph.jsonl is absent.

**Expected outcome:**
- Snapshot directory created with `meta.json` but no `graph.jsonl`.
- `wg runs restore <id>` from this snapshot should fail with "Snapshot graph.jsonl not found".

**Code path:** runs.rs:73 — `if graph_src.exists() { fs::copy(...) }`.

---

### 3.30 run_id_above_999 [GAP]

**Description:** Run IDs above 999 use `{:03}` formatting, so `run-1000` will have 4 digits (not zero-padded to 3). Verify that `next_run_id` and `list_runs` handle this correctly.

**Setup:**
1. Create directory `runs/run-999/`.
2. Call `next_run_id` → should return `run-1000`.
3. Create `runs/run-1000/` with valid meta.json.
4. `wg runs list` should include `run-1000`.

**Expected outcome:**
- `next_run_id` returns `"run-1000"` (format `run-{:03}` with value 1000 = `run-1000`).
- `list_runs` includes "run-1000" and sorts it correctly after "run-999".

**Code path:** runs.rs:56 — `format!("run-{:03}", max + 1)`.

---

### 3.31 runs_diff_only_compares_status [DOCUMENTATION]

**Description:** `run_diff` only compares task `status` between snapshot and current graph. Changes to other fields (title, description, tags, assigned, etc.) are NOT detected. This is by design but worth documenting.

**Setup:**
1. Create graph with t1 (Done, title="Original").
2. Take snapshot.
3. Change t1's title to "Modified" but keep status as Done.

**Expected outcome:**
- `wg runs diff run-001` reports "No differences".
- The diff is status-only; field changes are invisible.

**Code path:** runs_cmd.rs:147-154 — maps are `task_id -> status`, only status is compared.

---

### 3.32 runs_restore_does_not_restore_config [DOCUMENTATION]

**Description:** `restore_graph` (runs.rs:127-134) only restores `graph.jsonl`, NOT `config.toml`. Even though config.toml is snapshotted, restore does not copy it back. This is by design — config changes should be deliberate.

**Setup:**
1. Create workgraph with config.toml containing custom settings.
2. Replay to create snapshot (captures both graph.jsonl and config.toml).
3. Modify config.toml.
4. Restore from snapshot.

**Expected outcome:**
- graph.jsonl is restored to snapshot state.
- config.toml retains its modified state (NOT restored from snapshot).

**Code path:** runs.rs:127-134 — `restore_graph` only copies `graph.jsonl`, no mention of `config.toml`.

---

## 4. CROSS-CUTTING / INTEGRATION

### 4.1 full_replay_restore_round_trip [COVERED]

**Description:** End-to-end: replay resets tasks, runs restore to go back, verify graph matches pre-replay state.

**Setup:**
1. Create graph with t1 (Done), t2 (Failed).
2. Run `wg replay --failed-only` (creates run-001).
3. Verify t2 is now Open.
4. Run `wg runs restore run-001`.
5. Verify t2 is back to Failed.

**Coverage:** Exhaustive tests `test_full_replay_restore_round_trip` in both runs_exhaustive and replay_exhaustive.

---

### 4.2 replay_then_diff [COVERED]

**Description:** After replay, `wg runs diff run-001` shows the changes made by the replay.

**Coverage:** Exhaustive tests `test_replay_then_diff` in both runs_exhaustive and replay_exhaustive.

---

### 4.3 multiple_sequential_replays [COVERED]

**Description:** Multiple replays produce incrementing run IDs and each snapshot is independent.

**Coverage:** Integration test `test_multiple_replays_increment_run_ids`.

---

### 4.4 trace_after_replay [COVERED]

**Description:** After a replay, `wg trace` for a reset task should still work. Agent archives from before replay survive. Note: the replay provenance has `task_id: None`, so per-task trace won't show the replay operation.

**Coverage:** Exhaustive test `test_trace_after_replay` (replay_exhaustive).

---

### 4.5 replay_notify_graph_changed [COVERED]

**Description:** Both replay and restore call `notify_graph_changed(dir)` after modifying the graph. This should work without error even when no service is running.

**Coverage:** Exhaustive test `test_replay_and_restore_without_service` (replay_exhaustive).

---

### 4.6 replay_preserves_agent_archives [COVERED]

**Description:** Agent archives (under `log/agents/<task-id>/`) are NOT modified by replay. They persist across replays and are available via `wg trace` after reset.

**Setup:**
1. Create task with agent archive.
2. Run replay (resets task to Open).
3. Verify agent archive directories still exist on disk.
4. Verify `wg trace <id>` still shows the archived agent runs.

**Coverage:** Exhaustive test `test_replay_preserves_agent_archives` (replay_exhaustive) — verifies both filesystem persistence (prompt.txt, output.txt exist after replay) and trace accessibility (agent_run_count=1).

---

### 4.7 restore_then_diff_shows_no_changes [COVERED]

**Description:** After restoring from a snapshot, diffing against that same snapshot should show no changes.

**Setup:**
1. Create graph, replay to create run-001.
2. Restore from run-001.
3. Diff against run-001.

**Expected outcome:**
- "No differences" (graph matches snapshot).

**Coverage:** Exhaustive tests `test_restore_then_diff_shows_no_changes` in both runs_exhaustive and replay_exhaustive.

---

### 4.8 trace_after_restore [GAP]

**Description:** After restoring from a snapshot, `wg trace` should correctly reflect the restored task state (status, assigned, timestamps etc. from the snapshot).

**Setup:**
1. Create task t1 (Failed, with provenance entries and agent archive).
2. Replay to create run-001, resetting t1 to Open.
3. Restore from run-001 (t1 back to Failed).
4. Run `wg trace t1`.

**Expected outcome:**
- Trace shows t1 with status "failed" (restored state).
- Agent archives from before replay are still accessible.
- Provenance log contains all operations (add_task, replay, restore).

---

### 4.9 multiple_replay_then_trace_shows_all_archives [GAP]

**Description:** After multiple replay cycles (fail → replay → re-run agent → fail → replay), all agent archives from every cycle should be preserved and visible in trace.

**Setup:**
1. Create task t1 (Failed).
2. Create agent archive #1 for t1.
3. Replay (resets t1).
4. Create agent archive #2 for t1 (simulating a new agent run).
5. Set t1 back to Failed, replay again.
6. Run `wg trace t1`.

**Expected outcome:**
- Trace shows both agent archives (agent_run_count: 2).
- Archives are in chronological order.
- Replay does not delete or modify agent archives.

---

### 4.10 replay_with_concurrent_service_notification [DOCUMENTATION]

**Description:** `replay` and `restore` both call `notify_graph_changed(dir)` after modifying the graph. When a service daemon is running, this should trigger the coordinator to re-evaluate the graph. Test 4.5 covers the case when no service is running. This documents the expected behavior when a service IS running.

**Expected behavior:**
- After replay, the coordinator should pick up the newly reset tasks.
- After restore, the coordinator should re-evaluate all task readiness.
- No deadlock or race condition between replay modifying graph.jsonl and service reading it.

**Note:** Hard to test in unit tests without a running service. Consider integration test with `wg service start` + `wg replay` if deterministic timing can be achieved.

---

## 5. HELPER / UTILITY FUNCTIONS

### 5.1 format_duration_edge_cases [COVERED]

**Description:** Duration formatting for edge values.

| Input | Expected |
|-------|----------|
| 0 | "0s" |
| 59 | "59s" |
| 60 | "1m 0s" |
| 3599 | "59m 59s" |
| 3600 | "1h 0m" |
| 7261 | "2h 1m" |

**Coverage:** Exhaustive test `test_trace_duration_boundary_values` (trace_exhaustive).

---

### 5.2 parse_stream_json_result_only [COVERED]

**Description:** When output contains only a `{"type":"result"}` message (no assistant turns), turns should be 1 (fallback at trace.rs:103).

**Coverage:** Exhaustive tests `test_trace_result_only_output` (trace_exhaustive), `test_trace_result_only_stream_json` (replay_exhaustive).

---

### 5.3 build_reverse_index [COVERED]

**Description:** Correctly maps each blocker to the tasks that depend on it.

**Coverage:** Unit test `test_build_reverse_index`.

---

### 5.4 build_score_map_multiple_evals_per_task [COVERED]

**Description:** When multiple evaluations exist for the same task, `build_score_map` keeps the highest score.

**Coverage:** Exhaustive test `test_replay_multiple_evals_keeps_highest_score` (replay_exhaustive).

---

### 5.5 collect_subgraph_deep_tree [COVERED]

**Description:** `collect_subgraph` follows `blocks` edges through multiple levels.

**Coverage:** Exhaustive test `test_replay_subgraph_deep_tree` (replay_exhaustive).

---

### 5.6 collect_subgraph_with_cycles [COVERED]

**Description:** `collect_subgraph` handles cycles in blocks edges without infinite looping (uses a visited set).

**Coverage:** Exhaustive test `test_replay_subgraph_with_cycles` (replay_exhaustive).

---

### 5.7 load_agent_runs_sort_order [COVERED]

**Description:** Agent runs are sorted by directory name (timestamp), ensuring chronological order even if filesystem ordering varies.

**Coverage:** Exhaustive tests `test_trace_agent_runs_sort_order` in both trace_exhaustive and replay_exhaustive.

---

### 5.8 parse_stream_json_malformed_lines [COVERED]

**Description:** Non-JSON lines in stream output are silently ignored (no parsing error, no panic). Tool call and turn counts are 0 for pure non-JSON output.

**Setup:**
```
not json
also not json
```

**Expected outcome:** `(0 tool_calls, 0 turns)`.

**Coverage:** Unit test `test_parse_stream_json_non_json_lines_ignored`.

---

### 5.9 parse_stream_json_empty_input [COVERED]

**Description:** Empty string input returns `(0, 0)`.

**Coverage:** Unit test `test_parse_stream_json_stats_empty`.

---

### 5.10 load_agent_runs_no_archive_directory [COVERED]

**Description:** When no `log/agents/<task-id>/` directory exists, `load_agent_runs` returns an empty vector.

**Coverage:** Unit test `test_load_agent_runs_no_archive_dir`.

---

### 5.11 build_filter_desc [COVERED]

**Description:** `build_filter_desc` builds a human-readable string from replay options. When no flags are set, returns "all tasks".

**Setup:**
1. Test with no flags → "all tasks".
2. Test with `--failed-only --model opus --keep-done 0.9 --subgraph root --tasks a,b --below-score 0.5` → string containing all flags.

**Expected outcome:** Each flag is represented in the output string.

**Coverage:** Exhaustive tests `test_build_filter_desc_all_flags`, `test_build_filter_desc_default_all_tasks`, `test_build_filter_desc_tasks_and_below_score` (replay_exhaustive).

---

### 5.12 parse_stream_json_mixed_valid_and_invalid [GAP]

**Description:** When stream-json output contains a mix of valid JSON lines and non-JSON lines (e.g., stderr mixed in, or plain text progress output), the parser should count only valid JSON entries and silently skip invalid lines.

**Setup:**
```
some random text
{"type":"assistant","message":"hello"}
WARNING: something happened
{"type":"tool_use","name":"Read","id":"1"}
more random text
{"type":"result","cost":{"input":100,"output":50}}
```

**Expected outcome:** `tool_calls: 1, turns: 1`.

**Code path:** `trace.rs:91` — `serde_json::from_str::<serde_json::Value>(line)` returns Err for non-JSON, and the `if let Ok(val)` silently skips it.

---

### 5.13 build_summary_with_only_started_no_completed [COVERED]

**Description:** When a task has `started_at` set but `completed_at` is None (in-progress task), `build_summary` returns None for duration. Already covered by test 1.7 but documented here for completeness.

**Code path:** `trace.rs:263-275` — outer match on `(Some(s), Some(c))` falls through to `_ => None` when completed_at is None.

---

### 5.14 collect_subgraph_disconnected_blocks [GAP]

**Description:** `collect_subgraph` follows `blocks` edges forward from the root. If the graph has tasks where A blocks B and B blocks C, but B doesn't have A in its `blocked_by` (inconsistent edges), `collect_subgraph` should still follow A's `blocks` to reach B and C.

**Setup:**
1. Create tasks: root (blocks: ["child"]), child (blocks: ["leaf"], blocked_by: []), leaf (blocked_by: []).
2. Note: blocked_by is empty on child/leaf — edges are only in root/child's `blocks`.

**Expected outcome:**
- `collect_subgraph("root")` returns {root, child, leaf}.
- The subgraph follows `blocks` edges, not `blocked_by`.

**Code path:** `replay.rs:323-324` — `for blocked in &task.blocks { queue.push(blocked.clone()); }` — follows blocks edges only.

---

### 5.15 next_run_id_with_non_numeric_suffix [GAP]

**Description:** If a directory in `runs/` has a `run-` prefix but a non-numeric suffix (e.g., `run-abc`), `next_run_id` should ignore it when computing the next ID.

**Setup:**
1. Create directories: `runs/run-001/`, `runs/run-abc/`, `runs/run-003/`.

**Expected outcome:**
- `next_run_id` returns `"run-004"` (max of parseable IDs is 3, so next is 4).
- `run-abc` is ignored because `num_str.parse::<u32>()` fails.

**Code path:** `runs.rs:49-51` — `if let Some(num_str) = name.strip_prefix("run-") { if let Ok(num) = num_str.parse::<u32>() { ... } }`.

---

## Appendix: Coverage Matrix

| Category | Spec ID | Status | Priority |
|----------|---------|--------|----------|
| TRACE | 1.1 | COVERED | High |
| TRACE | 1.2 | COVERED | High |
| TRACE | 1.3 | COVERED | High |
| TRACE | 1.4 | COVERED | High |
| TRACE | 1.5 | COVERED | High |
| TRACE | 1.6 | COVERED | High |
| TRACE | 1.7 | COVERED | High |
| TRACE | 1.8 | COVERED | Medium |
| TRACE | 1.9 | COVERED | Medium |
| TRACE | 1.10 | COVERED | Medium |
| TRACE | 1.11 | COVERED | High |
| TRACE | 1.12 | COVERED | Medium |
| TRACE | 1.13 | COVERED | Low |
| TRACE | 1.14 | COVERED | Low |
| TRACE | 1.15 | COVERED | Low |
| TRACE | 1.16 | COVERED | Low |
| TRACE | 1.17 | COVERED | Low |
| TRACE | 1.18 | COVERED | Medium |
| TRACE | 1.19 | COVERED | Medium |
| TRACE | 1.20 | COVERED | Low |
| REPLAY | 2.1 | COVERED | High |
| REPLAY | 2.2 | COVERED | High |
| REPLAY | 2.3 | COVERED | High |
| REPLAY | 2.4 | COVERED | High |
| REPLAY | 2.5 | COVERED | Medium |
| REPLAY | 2.6 | COVERED | High |
| REPLAY | 2.7 | COVERED | High |
| REPLAY | 2.8 | COVERED | Medium |
| REPLAY | 2.9 | COVERED | High |
| REPLAY | 2.10 | COVERED | High |
| REPLAY | 2.11 | COVERED | High |
| REPLAY | 2.12 | COVERED | High |
| REPLAY | 2.13 | COVERED | Medium |
| REPLAY | 2.14 | COVERED | Medium |
| REPLAY | 2.15 | COVERED | Medium |
| REPLAY | 2.16 | COVERED | Medium |
| REPLAY | 2.17 | COVERED | Medium |
| REPLAY | 2.18 | COVERED | Medium |
| REPLAY | 2.19 | COVERED | Medium |
| REPLAY | 2.20 | COVERED | Medium |
| REPLAY | 2.21 | COVERED | Medium |
| REPLAY | 2.22 | COVERED | Medium |
| REPLAY | 2.23 | COVERED | Medium |
| REPLAY | 2.24 | COVERED | Low |
| REPLAY | 2.25 | COVERED | Low |
| REPLAY | 2.26 | COVERED | Medium |
| REPLAY | 2.27 | COVERED | Medium |
| REPLAY | 2.28 | COVERED | Low |
| REPLAY | 2.29 | COVERED | Low |
| REPLAY | 2.30 | COVERED | Low |
| REPLAY | 2.31 | COVERED | Low |
| REPLAY | 2.32 | COVERED | Medium |
| RUNS | 3.1 | COVERED | High |
| RUNS | 3.2 | COVERED | High |
| RUNS | 3.3 | COVERED | High |
| RUNS | 3.4 | COVERED | High |
| RUNS | 3.5 | COVERED | High |
| RUNS | 3.6 | COVERED | High |
| RUNS | 3.7 | COVERED | High |
| RUNS | 3.8 | COVERED | Medium |
| RUNS | 3.9 | COVERED | Medium |
| RUNS | 3.10 | COVERED | High |
| RUNS | 3.11 | COVERED | Medium |
| RUNS | 3.12 | COVERED | Medium |
| RUNS | 3.13 | COVERED | Medium |
| RUNS | 3.14 | COVERED | Medium |
| RUNS | 3.15 | COVERED | Medium |
| RUNS | 3.16 | COVERED | Low |
| RUNS | 3.17 | COVERED | Medium |
| RUNS | 3.18 | COVERED | Low |
| RUNS | 3.19 | COVERED | Low |
| RUNS | 3.20 | COVERED | High |
| RUNS | 3.21 | COVERED | Low |
| RUNS | 3.22 | COVERED | Medium |
| RUNS | 3.23 | COVERED | Medium |
| RUNS | 3.24 | N/A | — |
| RUNS | 3.25 | COVERED | Low |
| RUNS | 3.26 | COVERED | Low |
| RUNS | 3.27 | COVERED | Low |
| RUNS | 3.28 | COVERED | Low |
| CROSS | 4.1 | COVERED | High |
| CROSS | 4.2 | COVERED | High |
| CROSS | 4.3 | COVERED | Medium |
| CROSS | 4.4 | COVERED | Medium |
| CROSS | 4.5 | COVERED | Low |
| CROSS | 4.6 | COVERED | Low |
| CROSS | 4.7 | COVERED | Low |
| HELPER | 5.1 | COVERED | Low |
| HELPER | 5.2 | COVERED | Low |
| HELPER | 5.3 | COVERED | Low |
| HELPER | 5.4 | COVERED | Medium |
| HELPER | 5.5 | COVERED | Low |
| HELPER | 5.6 | COVERED | Low |
| HELPER | 5.7 | COVERED | Medium |
| HELPER | 5.8 | COVERED | Low |
| HELPER | 5.9 | COVERED | Low |
| HELPER | 5.10 | COVERED | Low |
| HELPER | 5.11 | COVERED | Low |
| TRACE | 1.21 | GAP | Medium |
| TRACE | 1.22 | GAP | Low |
| TRACE | 1.23 | GAP | Low |
| TRACE | 1.24 | GAP | Medium |
| TRACE | 1.25 | GAP | Low |
| REPLAY | 2.33 | GAP | Medium |
| REPLAY | 2.34 | GAP | Medium |
| REPLAY | 2.35 | GAP | Medium |
| REPLAY | 2.36 | GAP | Low |
| REPLAY | 2.37 | GAP | Low |
| REPLAY | 2.38 | GAP | Medium |
| RUNS | 3.29 | GAP | Low |
| RUNS | 3.30 | GAP | Low |
| RUNS | 3.31 | DOCUMENTATION | Low |
| RUNS | 3.32 | DOCUMENTATION | Low |
| CROSS | 4.8 | GAP | Medium |
| CROSS | 4.9 | GAP | Low |
| CROSS | 4.10 | DOCUMENTATION | Low |
| HELPER | 5.12 | GAP | Low |
| HELPER | 5.13 | COVERED | Low |
| HELPER | 5.14 | GAP | Low |
| HELPER | 5.15 | GAP | Low |

## Appendix: Gap Summary

**18 testable gaps remaining** (plus 3 documentation-only items). The original 73 scenarios remain fully covered. New gaps were identified through source-level analysis of untested code paths:

### High-priority gaps (0):
None — all high-priority scenarios are covered.

### Medium-priority gaps (7):
- **1.21** — Trace with Blocked/Abandoned/InProgress statuses (only Open/Done/Failed tested)
- **1.24** — Trace operations filtering accuracy (verify per-task filtering)
- **2.33** — Replay --tasks + --subgraph combined (subgraph filter takes precedence)
- **2.34** — Replay config default keep_done_threshold (config vs CLI interaction)
- **2.35** — Replay with Blocked tasks (Blocked is NOT terminal, verify it's skipped)
- **2.38** — Replay --keep-done only applies to Done status (not Failed with high score)
- **4.8** — Trace after restore (verify restored state is reflected)

### Low-priority gaps (11):
- **1.22** — Trace with missing prompt.txt (inverse of 1.14)
- **1.23** — Trace with empty archive directory (no files at all)
- **1.25** — Trace with unparseable timestamps (graceful None duration)
- **2.36** — Replay with InProgress tasks (not terminal, not reset)
- **2.37** — Replay --tasks with duplicate IDs (HashSet deduplication)
- **3.29** — Snapshot without graph.jsonl (edge case in storage layer)
- **3.30** — Run ID above 999 (format string boundary)
- **4.9** — Multiple replay cycles preserve all agent archives
- **5.12** — parse_stream_json with mixed valid/invalid lines
- **5.14** — collect_subgraph follows blocks edges (not blocked_by)
- **5.15** — next_run_id ignores non-numeric run- prefixes

### Documentation-only items (3):
- **3.31** — Diff only compares status (not other fields)
- **3.32** — Restore does not restore config.toml
- **4.10** — Replay with concurrent service notification
