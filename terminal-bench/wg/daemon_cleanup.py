"""
Daemon cleanup registry for Terminal Bench runners.

Tracks active `wg service daemon` processes spawned in temp directories and
ensures they are stopped on exit — whether the parent process completes
normally, crashes, or is killed by a signal (SIGTERM, SIGINT).

Usage:
    from wg.daemon_cleanup import daemon_registry

    # After starting a daemon:
    daemon_registry.register(wg_dir, wg_bin="/usr/local/bin/wg")

    # Before shutil.rmtree() in finally block:
    daemon_registry.stop_one(wg_dir)

    # Or stop all at once (called automatically on atexit/signal):
    daemon_registry.stop_all()
"""

import atexit
import json
import logging
import os
import signal
import subprocess
from pathlib import Path
from threading import Lock

logger = logging.getLogger(__name__)


class DaemonRegistry:
    """Thread-safe registry of active wg service daemons.

    Registers atexit and signal handlers on first use to guarantee
    cleanup even if the parent process is killed.
    """

    def __init__(self) -> None:
        self._lock = Lock()
        # Map from wg_dir (str) → wg_bin (str)
        self._active: dict[str, str] = {}
        self._handlers_installed = False

    def _install_handlers(self) -> None:
        """Install atexit and signal handlers (idempotent)."""
        if self._handlers_installed:
            return
        self._handlers_installed = True
        atexit.register(self.stop_all)

        # Chain onto existing signal handlers so we don't break other code
        for sig in (signal.SIGTERM, signal.SIGINT):
            prev = signal.getsignal(sig)
            def handler(signum, frame, _prev=prev, _sig=sig):
                self.stop_all()
                # Re-raise via previous handler or default behavior
                if callable(_prev):
                    _prev(signum, frame)
                elif _prev == signal.SIG_DFL:
                    # Restore default and re-raise
                    signal.signal(_sig, signal.SIG_DFL)
                    os.kill(os.getpid(), signum)
            signal.signal(sig, handler)

    def register(self, wg_dir: str, wg_bin: str = "wg") -> None:
        """Register an active daemon for cleanup tracking."""
        with self._lock:
            self._install_handlers()
            self._active[wg_dir] = wg_bin
            logger.debug(f"Registered daemon: wg_dir={wg_dir}")

    def stop_one(self, wg_dir: str) -> None:
        """Stop a single daemon and unregister it."""
        with self._lock:
            wg_bin = self._active.pop(wg_dir, None)
        if wg_bin is None:
            return
        _stop_daemon(wg_dir, wg_bin)

    def stop_all(self) -> None:
        """Stop all registered daemons. Safe to call multiple times."""
        with self._lock:
            snapshot = dict(self._active)
            self._active.clear()
        for wg_dir, wg_bin in snapshot.items():
            _stop_daemon(wg_dir, wg_bin)


def _stop_daemon(wg_dir: str, wg_bin: str) -> None:
    """Stop a wg service daemon, with belt-and-suspenders PID kill."""
    # 1. Try graceful stop via CLI
    try:
        env = {k: v for k, v in os.environ.items()
               if not k.startswith("WG_") and k != "CLAUDECODE"}
        subprocess.run(
            [wg_bin, "--dir", wg_dir, "service", "stop", "--force", "--kill-agents"],
            env=env,
            capture_output=True,
            timeout=10,
        )
        logger.debug(f"Stopped daemon via CLI: {wg_dir}")
    except Exception as e:
        logger.debug(f"CLI stop failed for {wg_dir}: {e}")

    # 2. Belt-and-suspenders: read PID from state.json and kill directly
    state_path = os.path.join(wg_dir, "service", "state.json")
    try:
        content = Path(state_path).read_text()
        state = json.loads(content)
        pid = state.get("pid")
        if pid and isinstance(pid, int):
            try:
                os.kill(pid, signal.SIGKILL)
                logger.debug(f"Killed daemon PID {pid} for {wg_dir}")
            except ProcessLookupError:
                pass  # Already dead
            except PermissionError:
                logger.debug(f"Cannot kill PID {pid}: permission denied")
    except (FileNotFoundError, json.JSONDecodeError, KeyError):
        pass


# Module-level singleton
daemon_registry = DaemonRegistry()
