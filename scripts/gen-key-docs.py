#!/usr/bin/env python3
"""gen-key-docs.py — render docs/KEY_DOCS.md from the curated docs/key-docs.tsv.

The documentation index used to be hand-maintained. It went stale on 2026-04-29
and stayed stale through an entire feature wave (federation / review / exec /
pilot), which is exactly the failure mode this generator removes:

  * the curated part is a small TSV (zone, path, purpose, audience);
  * the rest of the tree is reported as a **census** (per-directory counts), not
    enumerated, so the index stays readable as the repo grows;
  * every TSV path is existence-checked — a move or archive that forgets to
    update the TSV fails loudly here instead of silently rotting.

Usage:
    scripts/gen-key-docs.py            # rewrite docs/KEY_DOCS.md
    scripts/gen-key-docs.py --check    # exit 1 (with a diff) if it is stale
    scripts/gen-key-docs.py --print    # write to stdout, touch nothing

See docs/archive/POLICY.md §9 for the policy this implements.
"""

from __future__ import annotations

import argparse
import difflib
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
TSV = REPO_ROOT / "docs" / "key-docs.tsv"
OUT = REPO_ROOT / "docs" / "KEY_DOCS.md"

ZONES = [
    ("core", "Start here", "The entry points and the policy that governs the doc tree."),
    ("agent", "Rules for AI agents", "Read before claiming or dispatching work."),
    ("user", "Using `wg`", "Install, configure a route, run the graph."),
    ("operator", "Running it", "Daemon, lifecycle, recovery, security, logging."),
    ("design", "Design of record", "Why a shipping behaviour is shaped the way it is."),
    ("adr", "Decisions of record (ADR)", "Ratified decisions. Never archived because a feature went quiet."),
    ("spark", "Spark-stage subsystems", "Live, smoke-pinned, off the daily path. Read before touching that code."),
    ("embedded", "Bundled / generated", "Content shipped inside the binary or derived from another source."),
]

HISTORIC = [
    ("docs/archive/", "Archived: root reports, historical research, audits, superseded designs"),
    ("docs/reports/", "Dated reports and closeouts (wave-2 archive candidates)"),
    ("docs/research/", "Research notes and studies"),
    ("docs/audit/", "Dated audit trees"),
    ("docs/plans/", "Implementation plans for work in flight"),
    ("docs/design/", "Additional design docs (subset curated above)"),
    ("docs/designs/", "Earlier design drafts"),
    ("docs/prod-audit/", "Production-readiness audits"),
    ("docs/studies/", "Study documents"),
]


def die(msg: str) -> "NoReturn":  # type: ignore[name-defined]
    print(f"gen-key-docs: {msg}", file=sys.stderr)
    raise SystemExit(2)


def git_ls_files() -> list[str]:
    out = subprocess.run(
        ["git", "-C", str(REPO_ROOT), "ls-files"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return [line for line in out.splitlines() if line]


def read_tsv() -> list[tuple[str, str, str, str]]:
    if not TSV.exists():
        die(f"missing curated input {TSV.relative_to(REPO_ROOT)}")
    rows: list[tuple[str, str, str, str]] = []
    for lineno, line in enumerate(TSV.read_text().splitlines(), start=1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        parts = [p.strip() for p in line.split("\t")]
        if len(parts) != 4:
            die(
                f"{TSV.relative_to(REPO_ROOT)}:{lineno}: expected 4 tab-separated "
                f"fields (zone, path, purpose, audience), got {len(parts)}"
            )
        zone, path, purpose, audience = parts
        if zone not in {z for z, _, _ in ZONES}:
            die(f"{TSV.relative_to(REPO_ROOT)}:{lineno}: unknown zone {zone!r}")
        rows.append((zone, path, purpose, audience))
    return rows


def census(tracked: list[str]) -> list[tuple[str, int]]:
    md = [p for p in tracked if p.endswith(".md")]
    out = []
    for path, _label in HISTORIC:
        n = sum(1 for p in md if p.startswith(path))
        out.append((path, n))
    return out


def render() -> str:
    tracked = git_ls_files()
    tracked_set = set(tracked)
    rows = read_tsv()

    missing = [p for _, p, _, _ in rows if p not in tracked_set and not (REPO_ROOT / p).exists()]
    if missing:
        die(
            "curated path(s) no longer exist — update docs/key-docs.tsv in the same "
            "commit that moves/archives them:\n  " + "\n  ".join(missing)
        )

    md_count = sum(1 for p in tracked if p.endswith(".md"))
    by_zone: dict[str, list[tuple[str, str, str]]] = {}
    for zone, path, purpose, audience in rows:
        by_zone.setdefault(zone, []).append((path, purpose, audience))

    out: list[str] = []
    out.append("# Key Documentation Files")
    out.append("")
    out.append("**Generated — do not hand-edit.** This file is rendered from")
    out.append("`docs/key-docs.tsv` by `scripts/gen-key-docs.py`.")
    out.append("")
    out.append(
        "The curated zones below are the docs a reader is expected to reach; the census at the"
    )
    out.append(
        "end accounts for the rest of the tree. Archive/keep rules live in"
    )
    out.append("`docs/archive/POLICY.md`; the gate for moving a doc is")
    out.append("`scripts/doc-archive-audit.sh`.")
    out.append("")
    out.append(f"- curated docs: **{len(rows)}**")
    out.append(f"- tracked markdown in the repo: **{md_count}**")
    out.append(
        "- regenerate: `scripts/gen-key-docs.py` · verify: `scripts/gen-key-docs.py --check`"
    )
    out.append("")
    out.append("---")
    out.append("")

    for zone, title, blurb in ZONES:
        entries = by_zone.get(zone, [])
        out.append(f"## {title} (`{zone}`)")
        out.append("")
        out.append(blurb)
        out.append("")
        if not entries:
            out.append("_None registered yet._")
            out.append("")
            continue
        out.append("| File | Purpose | Audience |")
        out.append("|---|---|---|")
        for path, purpose, audience in entries:
            out.append(f"| `{path}` | {purpose} | {audience} |")
        out.append("")

    out.append("## Not indexed — historical zones (census)")
    out.append("")
    out.append(
        "These trees are historical (or not-yet-curated) and are deliberately **not**"
    )
    out.append(
        "enumerated here: an index that lists 600 stale files is how this file rotted"
    )
    out.append("last time. Counts are live from `git ls-files`.")
    out.append("")
    out.append("| Directory | Markdown files | What it holds |")
    out.append("|---|---:|---|")
    for path, n in census(tracked):
        label = dict(HISTORIC)[path]
        out.append(f"| `{path}` | {n} | {label} |")
    out.append("")
    out.append("To find a specific doc in those trees:")
    out.append("")
    out.append("```bash")
    out.append("git ls-files 'docs/reports/*.md' | xargs grep -l '<topic>'")
    out.append("scripts/doc-archive-audit.sh docs/reports/<file>.md   # is it safe to archive?")
    out.append("```")
    out.append("")
    out.append("## Adding a doc to this index")
    out.append("")
    out.append("1. Add one row to `docs/key-docs.tsv` (`zone`, `path`, `purpose`, `audience`).")
    out.append("2. Run `scripts/gen-key-docs.py`.")
    out.append("3. Commit the TSV **and** the regenerated `docs/KEY_DOCS.md` together.")
    out.append("")
    out.append(
        "A doc that is not canonical does **not** belong in the TSV — it belongs in the"
    )
    out.append("census, or in `docs/archive/` if `scripts/doc-archive-audit.sh` says so.")
    out.append("")
    return "\n".join(out)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="fail if docs/KEY_DOCS.md is stale")
    ap.add_argument("--print", dest="to_stdout", action="store_true", help="render to stdout")
    args = ap.parse_args()

    rendered = render()

    if args.to_stdout:
        sys.stdout.write(rendered)
        return 0

    current = OUT.read_text() if OUT.exists() else ""

    if args.check:
        if current == rendered:
            print(f"gen-key-docs: {OUT.relative_to(REPO_ROOT)} is current")
            return 0
        diff = difflib.unified_diff(
            current.splitlines(keepends=True),
            rendered.splitlines(keepends=True),
            fromfile=str(OUT.relative_to(REPO_ROOT)) + " (on disk)",
            tofile=str(OUT.relative_to(REPO_ROOT)) + " (regenerated)",
        )
        sys.stderr.writelines(diff)
        print(
            "gen-key-docs: docs/KEY_DOCS.md is STALE — run scripts/gen-key-docs.py",
            file=sys.stderr,
        )
        return 1

    OUT.write_text(rendered)
    print(f"gen-key-docs: wrote {OUT.relative_to(REPO_ROOT)} ({len(rendered)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
