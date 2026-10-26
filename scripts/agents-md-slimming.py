#!/usr/bin/env python3
"""agents-md-slimming.py — render the AGENTS.md/CLAUDE.md slimming patch.

The proposal lives in `docs/design/agents-md-slimming.md`. This script *derives*
the patch instead of storing it, for two reasons:

  * a stored unified diff carries blank context lines (a single space), which
    `git diff --check` — a hard completion gate in this repo — reports as
    trailing whitespace, so it cannot be committed as a checked-in file;
  * the patch is a function of the current `AGENTS.md`, and that file grows
    above the boundary. Deriving it means the patch can never be stale: if the
    split point has moved, the script says so.

The cut point is *semantic*, not a line number: the first level-2 heading that
starts the spark narrative (`## WG-Fed identity …`). Everything from that
heading to EOF is the duplicated block; everything above it is the operational
core and is never touched.

Usage:
    scripts/agents-md-slimming.py            # write the patch to stdout
    scripts/agents-md-slimming.py -o FILE    # write it to FILE
    scripts/agents-md-slimming.py --check    # verify the patch applies cleanly
    scripts/agents-md-slimming.py --stats    # print before/after line counts

Applying is the operator's call (see the proposal's §7):

    scripts/agents-md-slimming.py -o /tmp/slim.patch
    git apply --check /tmp/slim.patch && git apply /tmp/slim.patch
"""

from __future__ import annotations

import argparse
import difflib
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
FILES = ["AGENTS.md", "CLAUDE.md"]
BOUNDARY_PREFIX = "## WG-Fed identity"

STUB = """## Spark-stage features (documented under `docs/spark/`, not inlined)

The federation / content-safety / remote-execution / pilot stack is
**spark-stage**: complete, smoke-pinned, and off the operator's daily path. It is
documented in `docs/spark/` and deliberately **not inlined in this file**, because
this file is loaded into every agent prompt (rationale + patch:
`docs/design/agents-md-slimming.md`).

| Subsystem | Verbs | Compat const | Doc |
|---|---|---|---|
| WG-Fed (Waves 3–6) | `wg identity`, `wg fed-node`, `wg peer`, `wg msg --to wgid:` | `WG_FED_COMPAT_VERSION` | [`docs/spark/wg-fed.md`](docs/spark/wg-fed.md) |
| WG-Review | `wg review` | — (rides WG-Fed envelopes) | [`docs/spark/wg-review.md`](docs/spark/wg-review.md) |
| WG-Exec | `wg provider` | `WG_EXEC_COMPAT_VERSION` | [`docs/spark/wg-exec.md`](docs/spark/wg-exec.md) |
| Family-team e2e | `tests/smoke/scenarios/e2e_family_team.sh` | — | [`docs/spark/e2e-family-team.md`](docs/spark/e2e-family-team.md) |
| WG-Pilot | `wg pilot` | — | [`docs/spark/wg-pilot.md`](docs/spark/wg-pilot.md) |

Read the matching spark doc **before** changing that code (each carries its
compat-const rule and its smoke owner). A spark doc is never the authority for
the daily worker contract, and never a licence to widen a compat const.
"""


def die(msg: str) -> None:
    print(f"agents-md-slimming: {msg}", file=sys.stderr)
    raise SystemExit(2)


def split(text: str) -> tuple[str, int]:
    """Return (slimmed_text, removed_line_count); die if there is nothing to cut."""
    lines = text.splitlines(keepends=True)
    idx = next((i for i, ln in enumerate(lines) if ln.startswith(BOUNDARY_PREFIX)), None)
    if idx is None:
        die(
            f"no '{BOUNDARY_PREFIX}' heading — the file is already slimmed (or the "
            "spark sections were renamed; update BOUNDARY_PREFIX)"
        )
    head = "".join(lines[:idx]).rstrip("\n") + "\n"
    return head + "\n" + STUB, len(lines) - idx


def render_patch() -> str:
    chunks: list[str] = []
    for name in FILES:
        path = REPO_ROOT / name
        if not path.exists():
            die(f"missing {name}")
        original = path.read_text()
        slimmed, _ = split(original)
        diff = difflib.unified_diff(
            original.splitlines(keepends=True),
            slimmed.splitlines(keepends=True),
            fromfile=f"a/{name}",
            tofile=f"b/{name}",
            n=3,
        )
        body = "".join(diff)
        if not body:
            die(f"{name} is already slimmed")
        chunks.append(f"diff --git a/{name} b/{name}\n" + body)
    return "".join(chunks)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("-o", "--output", help="write the patch here instead of stdout")
    ap.add_argument("--check", action="store_true", help="verify the patch applies cleanly")
    ap.add_argument("--stats", action="store_true", help="print before/after line counts")
    ap.add_argument(
        "--preview",
        metavar="FILE",
        help="write the slimmed AGENTS.md content here (review before applying)",
    )
    args = ap.parse_args()

    if args.stats:
        for name in FILES:
            original = (REPO_ROOT / name).read_text()
            slimmed, removed = split(original)
            before, after = len(original.splitlines()), len(slimmed.splitlines())
            print(
                f"{name}: {before} -> {after} lines (-{before - after}), "
                f"removed block {removed} lines, "
                f"{len(original.encode())} -> {len(slimmed.encode())} bytes "
                f"(-{len(original.encode()) - len(slimmed.encode())})"
            )
        return 0

    if args.preview:
        slimmed, _ = split((REPO_ROOT / "AGENTS.md").read_text())
        Path(args.preview).write_text(slimmed)
        print(f"agents-md-slimming: wrote slimmed AGENTS.md preview to {args.preview}")
        return 0

    patch = render_patch()

    if args.check:
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / "slim.patch"
            p.write_text(patch)
            proc = subprocess.run(
                ["git", "-C", str(REPO_ROOT), "apply", "--check", str(p)],
                capture_output=True,
                text=True,
            )
            if proc.returncode != 0:
                die(f"patch does not apply cleanly:\n{proc.stderr.strip()}")
        print("agents-md-slimming: patch applies cleanly to " + " and ".join(FILES))
        return 0

    if args.output:
        Path(args.output).write_text(patch)
        print(f"agents-md-slimming: wrote {args.output}")
    else:
        sys.stdout.write(patch)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
