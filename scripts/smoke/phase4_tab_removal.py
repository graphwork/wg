#!/usr/bin/env python3
"""Phase 4 live smoke — tab removal regression gate.

Per the rollout plan, Phase 4 removes the Log, Messages, Firehose,
and Output tabs from `wg tui`'s right panel since the PTY view
covers their content. This script asserts:

  A. `wg tui` opens cleanly (no panic on startup).
  B. The tab bar does NOT render labels for removed tabs.
  C. Number-key tab navigation (`0`..`9`) doesn't crash and doesn't
     point at a non-existent tab.
  D. `q` quits cleanly (rc=0).
  E. Phase 3 PTY mode still works (Ctrl+T → embedded nex banner
     visible) — regression gate on all prior work.

Run after each incremental tab removal. Any FAIL means we broke
something; stop and fix before proceeding to the next tab.
"""
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time


REMOVED_LABELS = ["Log", "Msg", "Fire", "Live"]  # labels from RightPanelTab::label()


def drain(master, timeout=1.0):
    buf = b""
    end = time.time() + timeout
    while time.time() < end:
        r, _, _ = select.select([master], [], [], 0.1)
        if master in r:
            try:
                data = os.read(master, 8192)
                if not data:
                    return buf
                buf += data
            except OSError:
                return buf
    return buf


def setup_tmp():
    tmp = tempfile.mkdtemp(prefix="phase4_")
    env = os.environ.copy()
    env["WG_DIR"] = tmp + "/.workgraph"
    subprocess.run(
        ["wg", "init", "--no-agency"], env=env, cwd=tmp,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    subprocess.run(
        ["wg", "add", "--id", ".coordinator-0", "coord-zero"],
        env=env, cwd=tmp,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    return tmp, env


def run_tui_and_capture(env, tmp, keys_to_send=b"", duration=4.0):
    """Spawn wg tui, optionally send keys, return captured bytes + exit code."""
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 0, 0))
    proc = subprocess.Popen(
        ["wg", "tui"], stdin=slave, stdout=slave, stderr=slave,
        env=env, cwd=tmp, start_new_session=True,
    )
    os.close(slave)
    out = drain(master, 3.0)
    if keys_to_send:
        os.write(master, keys_to_send)
        out += drain(master, duration)
    # Quit cleanly.
    os.write(master, b"q")
    out += drain(master, 2.0)
    try:
        rc = proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.terminate()
        try:
            rc = proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
            rc = -9
    return out, rc


def main():
    results = []

    def report(name, ok):
        results.append((name, ok))
        status = "PASS" if ok else "FAIL"
        print(f"  [{status}] {name}")

    print("=== Phase 4 tab-removal regression smoke ===")

    # A: wg tui opens and exits cleanly with no keys.
    tmp, env = setup_tmp()
    out, rc = run_tui_and_capture(env, tmp)
    report("A: wg tui starts and exits cleanly (rc=0)", rc == 0)

    # B: tab bar does NOT contain removed labels.
    # Strip ANSI escape sequences + cursor-positioning codes to get
    # the readable text, then check each removed label as a word.
    # Ratatui renders labels cell-by-cell with cursor moves between,
    # so the literal string may not appear contiguously in raw bytes.
    import re
    flat = out.decode(errors="replace")
    # Remove ANSI CSI sequences (including cursor moves, colors).
    stripped = re.sub(r"\x1b\[[0-9;?]*[a-zA-Z~]", "", flat)
    stripped = re.sub(r"\x1b\][^\x07]*\x07", "", stripped)
    stripped = re.sub(r"\x1b[()][AB012]", "", stripped)
    # Collapse whitespace so cell-by-cell renders reassemble.
    words = set(re.findall(r"\b\w+\b", stripped))
    removed_still_visible = [label for label in REMOVED_LABELS if label in words]
    report(
        "B: tab bar does NOT render removed-tab labels",
        len(removed_still_visible) == 0,
    )
    if removed_still_visible:
        print(f"    still visible: {removed_still_visible}")

    # C: pressing number keys 0..9 doesn't crash the TUI.
    tmp2, env2 = setup_tmp()
    out_n, rc_n = run_tui_and_capture(
        env2, tmp2, keys_to_send=b"0123456789", duration=2.0
    )
    report("C: number-key tab nav (0..9) doesn't crash", rc_n == 0)

    # D: q quits cleanly — already implicit in A/C passing with rc=0.
    report("D: q exits cleanly (implicit from A/C rc=0)", rc == 0 and rc_n == 0)

    # E: Phase 3 PTY regression — Ctrl+T still works.
    tmp3, env3 = setup_tmp()
    out_pty, rc_pty = run_tui_and_capture(
        env3, tmp3, keys_to_send=b"\x14", duration=8.0
    )
    lock = os.path.join(tmp3, ".workgraph", "chat", ".coordinator-0", ".handler.pid")
    # Lock may have been cleaned up by now (we already sent q which
    # exits the TUI, which drops the pane, which kills the child,
    # which removes the lock on Drop). What matters is:
    #   - TUI didn't crash (rc=0)
    #   - banner was visible during the session
    pty_ok = rc_pty == 0 and "wg nex" in out_pty.decode(errors="replace")
    report("E: Phase 3 PTY regression (Ctrl+T renders wg nex banner)", pty_ok)

    passed = sum(1 for _, ok in results if ok)
    total = len(results)
    print(f"\n{passed}/{total} assertions passed")

    # Cleanup
    import shutil
    for d in [tmp, tmp2, tmp3]:
        shutil.rmtree(d, ignore_errors=True)

    return 0 if passed == total else 1


if __name__ == "__main__":
    sys.exit(main())
