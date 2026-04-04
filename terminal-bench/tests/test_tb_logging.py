"""Tests for Terminal Bench structured logging (TrialLogger)."""

import json
import tempfile
from pathlib import Path

from wg.tb_logging import (
    TrialLogger,
    _is_verification_command,
    _parse_exit_code,
    _summarize_args,
)


def test_is_verification_command():
    """Test verification command detection heuristic."""
    assert _is_verification_command("cargo test")
    assert _is_verification_command("pytest tests/")
    assert _is_verification_command("npm test")
    assert _is_verification_command("make test")
    assert _is_verification_command("./verify.sh")
    assert _is_verification_command("python3 -m pytest")
    assert _is_verification_command("go test ./...")
    assert not _is_verification_command("cat file.txt")
    assert not _is_verification_command("ls -la")
    assert not _is_verification_command("echo hello")


def test_parse_exit_code():
    """Test exit code extraction from tool result strings."""
    assert _parse_exit_code("some output\n[exit code: 1]") == 1
    assert _parse_exit_code("[exit code: 0]") == 0
    assert _parse_exit_code("[exit code: 127]") == 127
    assert _parse_exit_code("clean output with no exit code") is None
    assert _parse_exit_code("") is None


def test_summarize_args():
    """Test argument truncation for logging."""
    short = {"key": "short"}
    assert _summarize_args(short) == {"key": "short"}

    long_val = "x" * 500
    result = _summarize_args({"content": long_val}, max_len=200)
    assert len(result["content"]) < 300
    assert "500 chars" in result["content"]

    # Non-string values pass through
    assert _summarize_args({"count": 42}) == {"count": 42}


def test_trial_logger_per_turn_events():
    """Test that TrialLogger writes per-turn NDJSON events."""
    with tempfile.TemporaryDirectory() as tmpdir:
        logs_dir = Path(tmpdir)
        tl = TrialLogger(logs_dir, condition="A", root_task_id="test-task", model="test-model")

        # Simulate a turn
        tl.begin_turn(0)

        # Record a tool call
        tl.record_tool_call(
            tool_name="bash",
            args={"command": "echo hello"},
            result="hello",
            elapsed_s=0.5,
        )

        tl.end_turn(had_tool_calls=True)

        # Read events
        log_path = logs_dir / "agent_loop.ndjson"
        assert log_path.exists()

        events = [json.loads(line) for line in log_path.read_text().strip().split("\n")]
        assert len(events) >= 1

        # Check tool_result event
        tool_events = [e for e in events if e["type"] == "tool_result"]
        assert len(tool_events) == 1
        te = tool_events[0]
        assert te["tool"] == "bash"
        assert te["turn"] == 0
        assert te["elapsed_s"] == 0.5
        assert te["result_length"] == 5
        assert "args_summary" in te
        assert te["is_verification"] is False
        assert "timestamp" in te


def test_trial_logger_wg_tracking():
    """Test wg command counting and exit code tracking."""
    with tempfile.TemporaryDirectory() as tmpdir:
        logs_dir = Path(tmpdir)
        tl = TrialLogger(logs_dir, condition="D", root_task_id="test-task", model="m")

        tl.begin_turn(0)

        # wg_log call
        tl.record_tool_call("wg_log", {"task_id": "test-task", "message": "hi"}, "(no output)", 0.1)
        # wg_add call
        tl.record_tool_call("wg_add", {"title": "subtask 1"}, "Created subtask-1", 0.2)
        # wg_done on root
        tl.record_tool_call("wg_done", {"task_id": "test-task"}, "(no output)", 0.1)

        tl.end_turn()

        assert tl.wg_command_counts["log"] == 1
        assert tl.wg_command_counts["add"] == 1
        assert tl.wg_command_counts["done"] == 1
        assert tl.termination_type == "wg_done"
        assert len(tl.decomposition_tasks) == 1


def test_trial_logger_verification_tracking():
    """Test verification command and verdict tracking."""
    with tempfile.TemporaryDirectory() as tmpdir:
        logs_dir = Path(tmpdir)
        tl = TrialLogger(logs_dir, condition="E", root_task_id="root", model="m")

        tl.begin_turn(0)
        # Verification bash command
        tl.record_tool_call("bash", {"command": "cargo test"}, "test result\n[exit code: 0]", 5.0)
        # VERIFY verdict via wg_log
        tl.record_tool_call("wg_log", {"task_id": "root", "message": "VERIFY: PASS — all tests pass"}, "(no output)", 0.1)
        # Fix task (triage)
        tl.record_tool_call("wg_add", {"title": "Fix: broken import"}, "Created fix-1", 0.2)
        tl.end_turn()

        assert tl.verification_count == 1
        assert len(tl.verification_verdicts) == 1
        assert tl.verification_verdicts[0]["verdict"] == "PASS"
        assert tl.triage_count == 1


def test_trial_logger_summary():
    """Test per-trial summary generation."""
    with tempfile.TemporaryDirectory() as tmpdir:
        logs_dir = Path(tmpdir)
        tl = TrialLogger(logs_dir, condition="B", root_task_id="tb-abc", model="test-model")

        # Simulate 2 turns
        tl.begin_turn(0)
        tl.record_tool_call("bash", {"command": "ls"}, "file1\nfile2", 0.3)
        tl.record_tool_call("wg_log", {"task_id": "tb-abc", "message": "started"}, "(no output)", 0.1)
        tl.end_turn()

        tl.begin_turn(1)
        tl.record_tool_call("bash", {"command": "cargo test"}, "ok\n[exit code: 0]", 2.0)
        tl.record_tool_call("wg_done", {"task_id": "tb-abc"}, "(no output)", 0.1)
        tl.end_turn()

        summary = tl.write_summary(extra_metadata={"agent_identity": {"name": "solver"}})

        # Check summary file exists
        summary_path = logs_dir / "trial_summary.json"
        assert summary_path.exists()

        with open(summary_path) as f:
            saved = json.load(f)

        assert saved["condition"] == "B"
        assert saved["model"] == "test-model"
        assert saved["root_task_id"] == "tb-abc"
        assert saved["total_turns"] == 2
        assert saved["total_wall_clock_s"] >= 0
        assert saved["termination_type"] == "wg_done"
        assert saved["tool_call_counts"]["bash"] == 2
        assert saved["tool_call_counts"]["wg_log"] == 1
        assert saved["wg_command_counts"]["log"] == 1
        assert saved["wg_command_counts"]["done"] == 1
        assert saved["verification_count"] == 1
        assert saved["agent_identity"]["name"] == "solver"


def test_trial_logger_no_tool_calls_termination():
    """Test termination type when agent stops without tool calls."""
    with tempfile.TemporaryDirectory() as tmpdir:
        logs_dir = Path(tmpdir)
        tl = TrialLogger(logs_dir, condition="A", model="m")

        tl.begin_turn(0)
        tl.end_turn(had_tool_calls=False)

        assert tl.termination_type == "no_tool_calls"


def test_trial_logger_wg_snapshot():
    """Test wg state snapshot recording."""
    with tempfile.TemporaryDirectory() as tmpdir:
        logs_dir = Path(tmpdir)
        tl = TrialLogger(logs_dir, condition="C", model="m")

        tl.begin_turn(0)
        tl.record_wg_snapshot("after_init", "task-1: open\ntask-2: open")
        tl.end_turn()

        log_path = logs_dir / "agent_loop.ndjson"
        events = [json.loads(line) for line in log_path.read_text().strip().split("\n")]
        snapshot_events = [e for e in events if e["type"] == "wg_snapshot"]
        assert len(snapshot_events) == 1
        assert snapshot_events[0]["label"] == "after_init"
        assert "task-1" in snapshot_events[0]["data"]


if __name__ == "__main__":
    test_is_verification_command()
    test_parse_exit_code()
    test_summarize_args()
    test_trial_logger_per_turn_events()
    test_trial_logger_wg_tracking()
    test_trial_logger_verification_tracking()
    test_trial_logger_summary()
    test_trial_logger_no_tool_calls_termination()
    test_trial_logger_wg_snapshot()
    print("All tests passed!")
