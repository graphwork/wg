"""
Terminal Bench Condition A Harness: Bare Agent (Control Group)

This adapter implements Harbor's agent protocol for Terminal Bench evaluation.
It provides a minimal "bare agent" configuration with no workgraph features.

Condition A characteristics:
- Native executor, single session
- Tools: bash, read_file, write_file, edit_file, glob, grep
- NO wg tools, no graph awareness, no journal/resume
- No task decomposition, no external memory
- System prompt: minimal (tool descriptions + task instruction)

This is the CONTROL GROUP - what everyone else has.
"""

import json
import os
import subprocess
import tempfile
import uuid
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional

import yaml


# ─────────────────────────────────────────────────────────────────────────────
# Harbor Agent Protocol Interface
# ─────────────────────────────────────────────────────────────────────────────

class Agent:
    """
    Terminal Bench agent adapter implementing Harbor's agent protocol.
    
    This is the Condition A (bare agent) harness that:
    - Uses the native Rust executor via `wg native-exec`
    - Runs with minimal tools (bash, file ops) - no wg tools
    - Single session, no resume capability
    - Minimal system prompt focused on task completion
    """
    
    def __init__(
        self,
        model: str = "minimax/minimax-m2.7",
        max_turns: int = 100,
        timeout_seconds: int = 1800,
        openrouter_api_key: Optional[str] = None,
        wg_binary_path: Optional[str] = None,
    ):
        """
        Initialize the Condition A agent adapter.
        
        Args:
            model: Model to use via OpenRouter (e.g., "minimax/minimax-m2.7")
            max_turns: Maximum agent turns before stopping
            timeout_seconds: Task timeout in seconds
            openrouter_api_key: OpenRouter API key (falls back to env)
            wg_binary_path: Path to wg binary (falls back to system PATH)
        """
        self.model = model
        self.max_turns = max_turns
        self.timeout_seconds = timeout_seconds
        self.openrouter_api_key = openrouter_api_key or os.environ.get("OPENROUTER_API_KEY")
        self.wg_binary_path = wg_binary_path or self._find_wg_binary()
        
    def _find_wg_binary(self) -> str:
        """Find the wg binary."""
        # Check common locations
        candidates = [
            "/home/erik/workgraph/target/release/wg",
            "/home/erik/workgraph/target/debug/wg",
            "wg",  # System PATH
        ]
        for path in candidates:
            if os.path.exists(path):
                return path
        # Fall back to system PATH
        return "wg"
    
    def run(
        self,
        task_instruction: str,
        working_dir: Optional[str] = None,
        container_id: Optional[str] = None,
    ) -> Dict[str, Any]:
        """
        Run a Terminal Bench task using the native executor.
        
        Args:
            task_instruction: The task description from Terminal Bench
            working_dir: Working directory for the task (maps to Docker volume mount)
            container_id: Docker container ID if running inside a container
            
        Returns:
            Dict with keys: success, output, error, turns, tokens_used
        """
        task_id = f"tb-condition-a-{uuid.uuid4().hex[:8]}"
        workgraph_dir = tempfile.mkdtemp(prefix="wg-tb-")
        
        try:
            # Build the prompt file with Condition A system prompt
            prompt_file = os.path.join(workgraph_dir, "prompt.txt")
            system_prompt = self._build_condition_a_prompt(task_instruction)
            with open(prompt_file, "w") as f:
                f.write(system_prompt)
            
            # Build the native-exec command
            cmd = self._build_native_exec_command(
                task_id=task_id,
                prompt_file=prompt_file,
                workgraph_dir=workgraph_dir,
                working_dir=working_dir,
            )
            
            # Execute with timeout
            result = self._execute_with_timeout(cmd, container_id)
            
            # Parse output and extract results
            return self._parse_results(
                task_id=task_id,
                result=result,
                workgraph_dir=workgraph_dir,
            )
            
        finally:
            # Cleanup workgraph directory
            import shutil
            shutil.rmtree(workgraph_dir, ignore_errors=True)
    
    def _build_condition_a_prompt(self, task_instruction: str) -> str:
        """
        Build Condition A system prompt: minimal, no graph awareness.
        
        This is intentionally bare - just tool descriptions and the task.
        """
        tools_description = """You have access to the following tools for completing the task:

## Tool: bash
Execute a shell command and return its output (stdout + stderr).
- Input: {"command": "shell command to execute", "timeout": optional_timeout_ms}
- Returns: Command output or error message

## Tool: read_file
Read the contents of a file.
- Input: {"path": "path to file", "offset": optional_line_number, "limit": optional_max_lines}
- Returns: File contents or error

## Tool: write_file
Write content to a file (creates or overwrites).
- Input: {"path": "path to file", "content": "content to write"}
- Returns: Success or error

## Tool: edit_file
Make a targeted edit to an existing file.
- Input: {"path": "path to file", "old_string": "exact text to find", "new_string": "replacement text"}
- Returns: Success or error

## Tool: glob
Find files matching a glob pattern.
- Input: {"path": "base directory", "pattern": "glob pattern (e.g., **/*.py)"}
- Returns: List of matching file paths

## Tool: grep
Search file contents using regex.
- Input: {"path": "file or directory to search", "pattern": "regex pattern"}
- Returns: Matching lines with file paths and line numbers

## Guidelines
- Always prefer precise edits over full file rewrites when possible
- Use glob and grep to explore the codebase before making changes
- Commands are executed in the task working directory
- Keep output concise - prefer summary over raw dump for large outputs
"""
        
        condition_a_prefix = """You are a coding agent completing a Terminal Bench task.
You have access to bash and file tools as described below.
Focus on completing the task efficiently and correctly.
Do not ask for clarification - proceed with your best judgment.
"""
        
        return f"{condition_a_prefix}\n\n{tools_description}\n\n## Task\n\n{task_instruction}"
    
    def _build_native_exec_command(
        self,
        task_id: str,
        prompt_file: str,
        workgraph_dir: str,
        working_dir: Optional[str],
    ) -> List[str]:
        """Build the wg native-exec command for Condition A."""
        # Create Condition A bundle (bash + file tools, NO wg tools)
        # This is the CONTROL group - no graph awareness, no wg tools
        bundles_dir = os.path.join(workgraph_dir, "bundles")
        os.makedirs(bundles_dir, exist_ok=True)
        
        condition_a_bundle = """name = "condition-a"
description = "Terminal Bench Condition A: Bare agent control group. No wg tools, no graph awareness."
tools = ["bash", "read_file", "write_file", "edit_file", "glob", "grep"]
context_scope = "clean"
"""
        bundle_path = os.path.join(bundles_dir, "condition-a.toml")
        with open(bundle_path, "w") as f:
            f.write(condition_a_bundle)
        
        cmd = [
            self.wg_binary_path,
            "native-exec",
            "--dir", workgraph_dir,
            "--prompt-file", prompt_file,
            "--task-id", task_id,
            "--model", self.model,
            "--exec-mode", "condition-a",  # Custom bundle: bash + file tools only
            "--max-turns", str(self.max_turns),
            "--no-resume",  # Single session, no resume
        ]
        
        # Set working directory
        if working_dir:
            cmd.extend(["--working-dir", working_dir])
        
        # Set OpenRouter API key if provided
        if self.openrouter_api_key:
            cmd.extend(["--api-key", self.openrouter_api_key])
        
        return cmd
    
    def _execute_with_timeout(
        self,
        cmd: List[str],
        container_id: Optional[str],
    ) -> subprocess.CompletedProcess:
        """Execute command with timeout."""
        env = os.environ.copy()
        if self.openrouter_api_key:
            env["OPENROUTER_API_KEY"] = self.openrouter_api_key
        
        # If running inside a container, execute via docker exec
        if container_id:
            docker_cmd = [
                "docker", "exec",
                "-w", "/workspace",
                container_id,
            ] + cmd
            process = subprocess.run(
                docker_cmd,
                capture_output=True,
                text=True,
                timeout=self.timeout_seconds,
                env=env,
            )
        else:
            process = subprocess.run(
                cmd,
                capture_output=True,
                text=True,
                timeout=self.timeout_seconds,
                env=env,
            )
        
        return process
    
    def _parse_results(
        self,
        task_id: str,
        result: subprocess.CompletedProcess,
        workgraph_dir: str,
    ) -> Dict[str, Any]:
        """Parse execution results into standardized format."""
        # Look for output log
        output_log = os.path.join(workgraph_dir, "native-exec.ndjson")
        agent_log = os.path.join(workgraph_dir, "agent.ndjson")
        
        turns = 0
        tokens_used = {"input": 0, "output": 0}
        final_text = ""
        error_output = []
        
        # Parse NDJSON log if exists
        for log_path in [output_log, agent_log]:
            if os.path.exists(log_path):
                try:
                    with open(log_path, "r") as f:
                        for line in f:
                            try:
                                event = json.loads(line)
                                if event.get("type") == "Result":
                                    turns = event.get("turns", 0)
                                    usage = event.get("total_usage", {})
                                    tokens_used = {
                                        "input": usage.get("input_tokens", 0),
                                        "output": usage.get("output_tokens", 0),
                                    }
                                    final_text = event.get("final_text", "")
                            except json.JSONDecodeError:
                                continue
                except Exception:
                    pass
        
        # Determine success based on exit code and output
        success = result.returncode == 0
        
        # Check for error indicators in output
        if not success:
            error_output = [result.stderr] if result.stderr else []
        
        return {
            "success": success,
            "task_id": task_id,
            "output": final_text or result.stdout,
            "error": "\n".join(error_output) if error_output else None,
            "turns": turns,
            "tokens_used": tokens_used,
            "exit_code": result.returncode,
            "condition": "A",  # Bare agent, no wg tools
        }


# ─────────────────────────────────────────────────────────────────────────────
# Harbor Agent Protocol - WorkgraphAgent Class
# ─────────────────────────────────────────────────────────────────────────────

class WorkgraphAgent:
    """
    Harbor agent interface implementation for Terminal Bench.
    
    This class is instantiated by Harbor for each task evaluation.
    It wraps the bare Agent to provide Harbor's expected interface.
    
    Usage:
        harbor run --agent-import-path wg.adapter:WorkgraphAgent -m minimax/minimax-m2.7 ...
    """
    
    def __init__(
        self,
        model: str = "minimax/minimax-m2.7",
        max_turns: int = 100,
        timeout_seconds: int = 1800,
    ):
        """
        Initialize the WorkgraphAgent for Harbor.
        
        Args:
            model: Model identifier for OpenRouter (e.g., "minimax/minimax-m2.7")
            max_turns: Maximum turns per task
            timeout_seconds: Task timeout
        """
        self.agent = Agent(
            model=model,
            max_turns=max_turns,
            timeout_seconds=timeout_seconds,
        )
    
    def run(self, task_instruction: str, **kwargs) -> Dict[str, Any]:
        """
        Run a Terminal Bench task.
        
        Args:
            task_instruction: The task description from Terminal Bench
            **kwargs: Additional Harbor parameters (ignored)
            
        Returns:
            Dict with: success, output, error, turns, tokens_used
        """
        return self.agent.run(task_instruction=task_instruction)


# ─────────────────────────────────────────────────────────────────────────────
# CLI Entry Point (for Harbor integration)
# ─────────────────────────────────────────────────────────────────────────────

def main():
    """
    CLI entry point for the adapter.
    
    Can be used directly or via Harbor's --agent-import-path option:
        harbor run --agent-import-path wg.adapter:WorkgraphAgent ...
    """
    import argparse
    
    parser = argparse.ArgumentParser(
        description="Terminal Bench Condition A Harness (Bare Agent)"
    )
    parser.add_argument(
        "--model",
        default="minimax/minimax-m2.7",
        help="Model to use via OpenRouter (default: minimax/minimax-m2.7)",
    )
    parser.add_argument(
        "--max-turns",
        type=int,
        default=100,
        help="Maximum agent turns (default: 100)",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=1800,
        help="Task timeout in seconds (default: 1800)",
    )
    
    args = parser.parse_args()
    
    agent = Agent(
        model=args.model,
        max_turns=args.max_turns,
        timeout_seconds=args.timeout,
    )
    
    print(f"Condition A Agent initialized with model: {agent.model}")
    print(f"Tools: bash, read_file, write_file, edit_file, glob, grep")
    print(f"Note: This is the bare agent control group - no wg tools enabled")


# ─────────────────────────────────────────────────────────────────────────────
# Alternative: Direct Python API
# ─────────────────────────────────────────────────────────────────────────────

@dataclass
class TaskResult:
    """Result from a single task execution."""
    success: bool
    task_id: str
    output: str
    error: Optional[str] = None
    turns: int = 0
    tokens_used: Dict[str, int] = field(default_factory=dict)
    exit_code: int = 0
    condition: str = "A"


def run_task(
    task_instruction: str,
    model: str = "minimax/minimax-m2.7",
    max_turns: int = 100,
    timeout_seconds: int = 1800,
    working_dir: Optional[str] = None,
) -> TaskResult:
    """
    Run a single Terminal Bench task with Condition A configuration.
    
    Args:
        task_instruction: The task description from Terminal Bench
        model: Model to use via OpenRouter
        max_turns: Maximum agent turns
        timeout_seconds: Task timeout
        working_dir: Optional working directory
        
    Returns:
        TaskResult with execution details
    """
    agent = Agent(
        model=model,
        max_turns=max_turns,
        timeout_seconds=timeout_seconds,
    )
    
    result = agent.run(
        task_instruction=task_instruction,
        working_dir=working_dir,
    )
    
    return TaskResult(**result)


if __name__ == "__main__":
    main()
