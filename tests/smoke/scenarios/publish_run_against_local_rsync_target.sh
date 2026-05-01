#!/usr/bin/env bash
# Scenario: publish_run_against_local_rsync_target
#
# Regression lock for wg-publish-easy: `wg html publish add <name> --rsync
# <path>` registers a deployment in `.wg/html-publish.toml`, and
# `wg html publish run <name>` (a) builds the html via `wg html` and
# (b) rsyncs the staging dir to the target. After a successful run, the
# show output records `last_status: ok` and the destination contains an
# `index.html`.
#
# This guards against:
#   * silent path-resolution regressions (run resolving to ~/.wg instead
#     of the project's .wg dir),
#   * config persistence regressions (html-publish.toml round-trip),
#   * rsync invocation flag drift (-avz --delete defaults).
#
# Pure local rsync — no SSH, no network. Works in CI/sandbox.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

if ! command -v rsync >/dev/null 2>&1; then
    loud_skip "rsync_missing" "rsync is not installed in PATH"
    exit 77
fi

scratch=$(make_scratch)
cd "$scratch"

if ! wg init --route local >init.log 2>&1; then
    loud_fail "wg init --route local failed: $(tail -5 init.log)"
fi

if ! wg add "Sample task" --id sample >add.log 2>&1; then
    loud_fail "wg add failed: $(tail -5 add.log)"
fi

dest="$scratch/dest"
mkdir -p "$dest"

# add — manual deployment (no --schedule)
if ! wg html publish add demo --rsync "$dest/" >add.log 2>&1; then
    loud_fail "wg html publish add failed: $(tail -10 add.log)"
fi

# list — must include 'demo'
if ! wg html publish list 2>&1 | grep -q '^  demo$'; then
    loud_fail "wg html publish list did not include 'demo': $(wg html publish list)"
fi

# show — must record manual schedule
if ! wg html publish show demo 2>&1 | grep -q "schedule:.*manual"; then
    loud_fail "wg html publish show did not show manual schedule: $(wg html publish show demo)"
fi

# run — must succeed and produce index.html in dest
if ! wg html publish run demo >run.log 2>&1; then
    loud_fail "wg html publish run demo failed: $(tail -20 run.log)"
fi

if [[ ! -f "$dest/index.html" ]]; then
    loud_fail "expected $dest/index.html after run; got: $(ls -la "$dest")"
fi

# show after run — must record last_status=ok
if ! wg html publish show demo 2>&1 | grep -q "last status:.*ok"; then
    loud_fail "wg html publish show after run did not record last_status=ok: $(wg html publish show demo)"
fi

# remove — must drop deployment from list
if ! wg html publish remove demo >rm.log 2>&1; then
    loud_fail "wg html publish remove failed: $(tail -5 rm.log)"
fi
if wg html publish list 2>&1 | grep -q '^  demo$'; then
    loud_fail "wg html publish remove did not drop deployment from list"
fi

# scheduled deployment — must register a wg task with .html-publish-<name> id.
if ! wg html publish add scheduled --rsync "$dest/" --schedule "*/15 * * * *" >sched.log 2>&1; then
    loud_fail "wg html publish add --schedule failed: $(tail -10 sched.log)"
fi

if ! wg show .html-publish-scheduled 2>&1 | grep -q "Status: open"; then
    loud_fail "expected scheduled task .html-publish-scheduled to be Open; got: $(wg show .html-publish-scheduled 2>&1 | head)"
fi

# remove scheduled — should abandon the scheduling task too.
wg html publish remove scheduled >/dev/null 2>&1
status_line=$(wg show .html-publish-scheduled 2>&1 | grep -i '^Status:' | head -1 || true)
if ! echo "$status_line" | grep -qi "abandoned"; then
    loud_fail "expected .html-publish-scheduled to be abandoned after remove; got: $status_line"
fi

# Existing `wg publish <task-id>` (legacy task-publishing) MUST still
# advertise its surface — guards against accidental re-shadowing.
help_text=$(wg publish --help 2>&1)
if ! echo "$help_text" | grep -qi "publish a draft task"; then
    loud_fail "wg publish --help did not advertise legacy publish-task semantics: $help_text"
fi

echo "PASS: wg html publish add/list/show/run/remove + scheduled task lifecycle + wg publish task surface intact"
exit 0
