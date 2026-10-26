#!/usr/bin/env bash
# Scenario: nex_yolo_banner_and_precedence
#
# Pins `wg nex --yolo`:
#   * a loud "YOLO MODE" startup banner announcing the workspace write
#     sandbox is disabled (write_file/edit_file may write outside cwd), and
#   * the --yolo + --read-only conflict resolution — read-only wins, yolo is
#     forced OFF, and the active YOLO banner is suppressed.
#
# The banner is printed before any model round-trip, so a bogus endpoint is
# fine: we only assert what lands on stderr. This guards against a future
# refactor silently dropping the banner or letting --yolo override the
# conservative --read-only request.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch="$(make_scratch)"
( cd "$scratch" && wg init >/dev/null 2>&1 ) || loud_fail "wg init failed in $scratch"
wg_dir="$scratch/.wg"

# Bogus endpoint (nothing listening on port 9): the connection is refused
# fast, but the startup banner is already on stderr before the call. One
# turn, no MCP, minimal tools, autonomous so EndTurn/abort exits promptly.
common=(--autonomous --no-mcp --minimal-tools
        -m openrouter:bogus/model -e http://127.0.0.1:9
        --max-turns 1 "hi")

yolo_out="$(cd "$scratch" && timeout 30 wg --dir "$wg_dir" nex --yolo "${common[@]}" 2>&1)"
grep -q "YOLO MODE" <<<"$yolo_out" || loud_fail "wg nex --yolo missing loud YOLO MODE banner. stderr:
$yolo_out"
grep -q "All safety gating disabled" <<<"$yolo_out" || \
    loud_fail "yolo banner missing the safety-disabled warning line. stderr:
$yolo_out"

conflict_out="$(cd "$scratch" && timeout 30 wg --dir "$wg_dir" nex --yolo --read-only "${common[@]}" 2>&1)"
grep -q "read-only wins" <<<"$conflict_out" || \
    loud_fail "--yolo --read-only did not warn that read-only wins. stderr:
$conflict_out"
grep -q "yolo mode is OFF" <<<"$conflict_out" || \
    loud_fail "--yolo --read-only did not report yolo OFF. stderr:
$conflict_out"
if grep -q "YOLO MODE" <<<"$conflict_out"; then
    loud_fail "read-only must suppress the active YOLO banner. stderr:
$conflict_out"
fi
grep -q "\[read-only\]" <<<"$conflict_out" || \
    loud_fail "--yolo --read-only did not show the read-only banner. stderr:
$conflict_out"

echo "PASS: wg nex --yolo shows loud banner; --read-only wins over --yolo"
exit 0
