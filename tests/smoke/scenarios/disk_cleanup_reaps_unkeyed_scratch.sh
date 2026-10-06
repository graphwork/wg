#!/usr/bin/env bash
# Regression for fix-disk-cleanup: `wg disk cleanup --execute` must reap a
# stale *unkeyed* build-scratch directory — a `build-tmp/<name>` that was never
# written into the ownership registry (e.g. the survey's orphaned
# `manual-service-merge`) — while never removing a directory a live process is
# standing in. Pre-fix, cleanup only swept keyed, live-owner-tracked targets, so
# an unkeyed orphan was invisible to both `wg disk doctor` and the reaper.
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON" "python3 required for JSON assertions"

scratch=$(make_scratch)
project="$scratch/project"
mkdir -p "$project"
cd "$project"
wg init --no-agency >/dev/null

scratch_root=".wg/build-tmp"
orphan="$scratch_root/manual-service-merge"
live_open="$scratch_root/agent-orphan-open"
fresh="$scratch_root/just-created"

# 1) Stale, unkeyed orphan — no ownership row, so only age + the live-open
#    guard authorize reaping. Force an age beyond the 7-day default floor.
mkdir -p "$orphan"
echo payload >"$orphan/blob"
touch -d '2020-01-01T00:00:00Z' "$orphan"

# 2) Unkeyed directory held open by a live process cwd: aged, but a live pid
#    must preserve it.
mkdir -p "$live_open"
echo payload >"$live_open/blob"
touch -d '2020-01-01T00:00:00Z' "$live_open"
(
    cd "$live_open"
    exec sleep 120
) &
live_pid=$!
for _ in $(seq 1 50); do
    [ -e "/proc/$live_pid/cwd" ] && break
    sleep 0.1
done
resolved=$(readlink -f "/proc/$live_pid/cwd" 2>/dev/null || true)
expected=$(readlink -f "$live_open")
[ "$resolved" = "$expected" ] \
    || loud_fail "live process cwd did not resolve to the fixture scratch dir ($resolved != $expected)"

# 3) Fresh, unkeyed directory (within the age floor) must be preserved.
mkdir -p "$fresh"
echo payload >"$fresh/blob"

# The snapshot must surface the stale unkeyed dir as a target (owner `-`,
# key legacy/unkeyed) and must not mark the live-open/fresh dirs stale.
doctor=$(wg disk doctor --json)
echo "$doctor" | python3 -c '
import json, sys
from pathlib import Path
report = json.load(sys.stdin)
def r(p): return str(Path(p).resolve())
targets = {r(t["path"]): t for t in report["targets"]}
orphan, live_open, fresh = (r(a) for a in sys.argv[1:4])
for label, path in (("orphan", orphan), ("live-open", live_open), ("fresh", fresh)):
    assert path in targets, f"{label} {path} missing from doctor targets: {sorted(targets)}"
assert targets[orphan].get("cache_key") is None, targets[orphan]
assert targets[orphan]["stale"] is True, targets[orphan]
assert targets[live_open]["stale"] is False, targets[live_open]
assert targets[fresh]["stale"] is False, targets[fresh]
' "$orphan" "$live_open" "$fresh" \
    || loud_fail "doctor snapshot did not classify unkeyed scratch correctly: $doctor"

# Dry run: the stale orphan is eligible; the live-open and fresh dirs are not.
dry=$(wg disk cleanup --json)
echo "$dry" | python3 -c '
import json, sys
from pathlib import Path
report = json.load(sys.stdin)
def r(p): return str(Path(p).resolve())
orphan, live_open, fresh = (r(a) for a in sys.argv[1:4])
eligible = {r(i["path"]) for i in report["eligible"]}
preserved = {r(i["path"]) for i in report["preserved"]}
assert orphan in eligible, report
assert live_open not in eligible and fresh not in eligible, report
assert live_open in preserved, report
assert fresh in preserved, report
' "$orphan" "$live_open" "$fresh" \
    || loud_fail "dry run eligibility was not conservative: $dry"

# Execute: reaps exactly the stale unkeyed orphan; preserves the live-open and
# fresh dirs.
applied=$(wg disk cleanup --execute --json)
echo "$applied" | python3 -c '
import json, sys
from pathlib import Path
report = json.load(sys.stdin)
def r(p): return str(Path(p).resolve())
orphan, live_open, fresh = (r(a) for a in sys.argv[1:4])
reaped = {r(i["path"]) for i in report["reaped_paths"]}
assert reaped == {orphan}, report
assert live_open not in reaped and fresh not in reaped, report
' "$orphan" "$live_open" "$fresh" \
    || loud_fail "execute reaped the wrong unkeyed scratch: $applied"
[ ! -e "$orphan" ] || loud_fail "stale unkeyed scratch was not reaped"
[ -e "$live_open" ] || loud_fail "scratch held open by a live pid was removed"
[ -e "$fresh" ] || loud_fail "fresh unkeyed scratch was removed"

# Idempotence: a second execute pass reaps nothing.
second=$(wg disk cleanup --execute --json)
echo "$second" | python3 -c '
import json, sys
report = json.load(sys.stdin)
assert report["reaped_paths"] == [], report
' || loud_fail "second cleanup pass was not idempotent: $second"

kill "$live_pid" 2>/dev/null || true
wait "$live_pid" 2>/dev/null || true

echo "PASS: stale unkeyed build scratch is listed by doctor and reaped by --execute while live-open and fresh scratch are preserved"
