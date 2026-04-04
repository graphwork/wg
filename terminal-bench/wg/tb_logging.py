"""
Structured logging for Terminal Bench trials.

Provides per-turn and per-trial logging across all conditions (A–E).
Writes NDJSON event streams and JSON summary files alongside trial results.

Usage in adapter.py:
    trial_log = TrialLogger(logs_dir, condition, root_task_id, model)
    for turn in range(max_turns):
        trial_log.begin_turn(turn)
        # ... LLM call ...
        trial_log.record_llm_response(response)
        # ... tool calls ...
        trial_log.record_tool_call(name, args, result, elapsed)
        trial_log.end_turn()
    trial_log.write_summary(context_metadata)
"""

import json
import re
import time
from collections import Counter
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Verification command detection (used by D/E tracking)
# ---------------------------------------------------------------------------

_VERIFY_PATTERNS = re.compile(
    r"\b(test|pytest|cargo\s+test|npm\s+test|make\s+test|go\s+test"
    r"|check|verify|\.\/verify|\.\/run_tests|\.\/test)\b",
    re.IGNORECASE,
)


def _is_verification_command(cmd: str) -> bool:
    """Heuristic: does this bash command look like a verification/test step?"""
    return bool(_VERIFY_PATTERNS.search(cmd))


def _summarize_args(args: dict, max_len: int = 200) -> dict:
    """Return a truncated copy of tool args suitable for structured logging."""
    out: dict[str, Any] = {}
    for k, v in args.items():
        if isinstance(v, str) and len(v) > max_len:
            out[k] = v[:max_len] + f"...[{len(v)} chars]"
        else:
            out[k] = v
    return out


def _parse_exit_code(result: str) -> int | None:
    """Extract exit code from tool result string if present."""
    m = re.search(r"\[exit code: (\d+)\]", result)
    return int(m.group(1)) if m else None


class TrialLogger:
    """Structured per-turn and per-trial logger for Terminal Bench.

    Writes to two files:
      - <logs_dir>/agent_loop.ndjson  (per-turn events, NDJSON)
      - <logs_dir>/trial_summary.json (per-trial summary, JSON)
    """

    def __init__(
        self,
        logs_dir: Path,
        condition: str,
        root_task_id: str | None = None,
        model: str = "",
    ):
        self.logs_dir = Path(logs_dir)
        self.condition = condition
        self.root_task_id = root_task_id
        self.model = model

        self._log_path = self.logs_dir / "agent_loop.ndjson"
        self._summary_path = self.logs_dir / "trial_summary.json"

        # Trial-level accumulators
        self.trial_start_time = time.monotonic()
        self.total_input_tokens = 0
        self.total_output_tokens = 0
        self.total_cost = 0.0
        self.total_turns = 0

        # Tool call tracking
        self.tool_call_counts: Counter = Counter()  # tool_name -> count
        self.wg_command_counts: Counter = Counter()  # wg subcommand -> count
        self.wg_commands_log: list[dict] = []  # {turn, name, args_summary, exit_code}

        # Verification tracking (D/E)
        self.verification_count = 0
        self.verification_commands: list[str] = []
        self.verification_verdicts: list[dict] = []  # {turn, verdict, message}
        self.termination_type = "max_turns"

        # Decomposition tracking (E)
        self.decomposition_tasks: list[str] = []
        self.triage_count = 0

        # Per-turn state (reset each turn)
        self._turn: int = 0
        self._turn_start: float = 0.0
        self._turn_input_tokens: int = 0
        self._turn_output_tokens: int = 0
        self._turn_tool_calls: list[dict] = []

    # ----- Per-turn lifecycle -----

    def begin_turn(self, turn: int) -> None:
        """Call at the start of each agent turn."""
        self._turn = turn
        self._turn_start = time.monotonic()
        self._turn_input_tokens = 0
        self._turn_output_tokens = 0
        self._turn_tool_calls = []

    def record_llm_response(self, response: Any) -> None:
        """Record LLM response metadata (tokens, content, tool calls).

        Args:
            response: litellm completion response object
        """
        usage = getattr(response, "usage", None)
        if usage:
            prompt_tokens = getattr(usage, "prompt_tokens", 0) or 0
            completion_tokens = getattr(usage, "completion_tokens", 0) or 0
            self._turn_input_tokens = prompt_tokens
            self._turn_output_tokens = completion_tokens
            self.total_input_tokens += prompt_tokens
            self.total_output_tokens += completion_tokens
            cost = getattr(usage, "cost_usd", 0.0) or 0.0
            self.total_cost += cost

        choice = response.choices[0]
        message = choice.message

        # Build the turn event
        turn_event: dict[str, Any] = {
            "type": "turn",
            "turn": self._turn,
            "finish_reason": choice.finish_reason,
            "wall_clock_s": round(time.monotonic() - self._turn_start, 3),
            "input_tokens": self._turn_input_tokens,
            "output_tokens": self._turn_output_tokens,
        }

        # Agent reasoning / plan text
        if message.content:
            turn_event["content"] = message.content

        # Tool calls with args summary
        if message.tool_calls:
            turn_event["tool_calls"] = [
                {
                    "name": tc.function.name,
                    "arguments": tc.function.arguments,
                }
                for tc in message.tool_calls
            ]
        else:
            turn_event["tool_calls"] = None

        self._write_event(turn_event)

    def record_tool_call(
        self,
        tool_name: str,
        args: dict,
        result: str,
        elapsed_s: float,
    ) -> None:
        """Record a single tool execution with its result.

        Args:
            tool_name: Name of the tool called
            args: Parsed arguments dict
            result: Tool output string
            elapsed_s: Wall-clock seconds for tool execution
        """
        self.tool_call_counts[tool_name] += 1
        is_wg = tool_name.startswith("wg_")

        exit_code = _parse_exit_code(result) if is_wg else None

        # Track wg commands specifically
        if is_wg:
            # wg subcommand is the part after "wg_"
            wg_subcmd = tool_name[3:]  # e.g. "show", "add", "done"
            self.wg_command_counts[wg_subcmd] += 1
            wg_entry = {
                "turn": self._turn,
                "command": wg_subcmd,
                "args_summary": _summarize_args(args),
                "exit_code": exit_code if exit_code is not None else 0,
            }
            self.wg_commands_log.append(wg_entry)

        # Track verification commands (bash test/check commands)
        if tool_name == "bash":
            cmd = args.get("command", "")
            if _is_verification_command(cmd):
                self.verification_count += 1
                self.verification_commands.append(cmd[:200])

        # Track wg_log verification verdicts (for E)
        if tool_name == "wg_log":
            msg = args.get("message", "")
            if "VERIFY:" in msg:
                verdict = "PASS" if "PASS" in msg else "FAIL" if "FAIL" in msg else "UNKNOWN"
                self.verification_verdicts.append({
                    "turn": self._turn,
                    "verdict": verdict,
                    "message": msg[:300],
                })

        # Track decomposition (wg_add calls)
        if tool_name == "wg_add":
            title = args.get("title", "")
            self.decomposition_tasks.append(title)
            if title.startswith("Fix:"):
                self.triage_count += 1

        # Track termination signals
        if tool_name == "wg_done" and args.get("task_id") == self.root_task_id:
            self.termination_type = "wg_done"
        elif tool_name == "wg_fail" and args.get("task_id") == self.root_task_id:
            self.termination_type = "wg_fail"

        # Write per-tool-call event
        tool_event: dict[str, Any] = {
            "type": "tool_result",
            "turn": self._turn,
            "tool": tool_name,
            "args_summary": _summarize_args(args),
            "result_length": len(result),
            "elapsed_s": round(elapsed_s, 3),
        }
        if is_wg:
            tool_event["exit_code"] = exit_code if exit_code is not None else 0
        if tool_name == "bash":
            tool_event["is_verification"] = _is_verification_command(
                args.get("command", "")
            )

        self._write_event(tool_event)

    def end_turn(self, had_tool_calls: bool = True) -> None:
        """Call at the end of each turn. Updates turn count and termination tracking."""
        self.total_turns = self._turn + 1
        # If no tool calls were made, agent stopped naturally
        if not had_tool_calls and self.termination_type == "max_turns":
            self.termination_type = "no_tool_calls"

    def record_error(self, error: str) -> None:
        """Record an LLM or tool error event."""
        self._write_event({
            "type": "error",
            "turn": self._turn,
            "error": error,
            "wall_clock_s": round(time.monotonic() - self._turn_start, 3),
        })

    def record_wg_snapshot(self, label: str, snapshot_data: str) -> None:
        """Record a wg state snapshot (e.g. after init, after decomposition).

        Args:
            label: Descriptive label (e.g. "after_init", "before_done")
            snapshot_data: Output of `wg list` or similar
        """
        self._write_event({
            "type": "wg_snapshot",
            "turn": self._turn,
            "label": label,
            "data": snapshot_data[:5000],  # cap size
        })

    # ----- Trial-level summary -----

    def write_summary(self, extra_metadata: dict | None = None) -> dict:
        """Write the per-trial summary JSON and return it.

        Args:
            extra_metadata: Additional fields to merge into the summary
                (e.g. agent_identity for D/E)

        Returns:
            The summary dict
        """
        total_wall_clock = round(time.monotonic() - self.trial_start_time, 3)

        summary: dict[str, Any] = {
            "condition": self.condition,
            "model": self.model,
            "root_task_id": self.root_task_id,
            "total_turns": self.total_turns,
            "total_wall_clock_s": total_wall_clock,
            "total_input_tokens": self.total_input_tokens,
            "total_output_tokens": self.total_output_tokens,
            "total_tokens": self.total_input_tokens + self.total_output_tokens,
            "total_cost_usd": round(self.total_cost, 6),
            "termination_type": self.termination_type,
            "tool_call_counts": dict(self.tool_call_counts),
            "wg_command_counts": dict(self.wg_command_counts),
            "wg_commands_log": self.wg_commands_log[:200],  # cap for size
            "verification_count": self.verification_count,
            "verification_commands": self.verification_commands[:50],
        }

        # D/E specific fields
        if self.condition in ("D", "E"):
            summary["verification_verdicts"] = self.verification_verdicts
            summary["decomposition_task_count"] = len(self.decomposition_tasks)
            summary["decomposition_tasks"] = self.decomposition_tasks[:50]
            summary["triage_count"] = self.triage_count

        if extra_metadata:
            summary.update(extra_metadata)

        # Also write the final event to the NDJSON stream
        self._write_event({
            "type": "trial_summary",
            **summary,
        })

        # Write standalone summary file
        try:
            with open(self._summary_path, "w") as f:
                json.dump(summary, f, indent=2, default=str)
        except Exception as e:
            import logging
            logging.getLogger(__name__).warning(
                f"Failed to write trial summary: {e}"
            )

        return summary

    # ----- Internal -----

    def _write_event(self, event: dict) -> None:
        """Append an NDJSON event to the log file."""
        event["timestamp"] = time.time()
        try:
            with open(self._log_path, "a") as f:
                f.write(json.dumps(event, default=str) + "\n")
        except Exception as e:
            import logging
            logging.getLogger(__name__).warning(
                f"Failed to write log event: {e}"
            )
