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
# Also pins wg-html-publish: opt-in `--mkpath` appends one flag to the
# default; opt-in `--rsync-flags '<custom>'` fully replaces the default;
# the two are mutually exclusive at the CLI; and adding a deployment with
# NEITHER opt-in must keep the legacy '-avz --delete' default unchanged
# (no silent --mkpath upgrade — older rsync still works). The fresh-path
# subscenario at the bottom asserts --mkpath actually does what it says.
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

# ── --mkpath / --rsync-flags opt-ins (wg-html-publish) ────────────────
# 1) Default with no opt-in: rsync flags surface as '-avz --delete' (legacy
#    contract preserved).
# 2) --mkpath: rsync flags surface as '-avz --delete --mkpath' AND a `run`
#    against a NON-EXISTENT remote subdir succeeds (creates the path)
#    instead of hitting rsync exit 11. The non-opt-in default would fail
#    here on a fresh path; this is the user-visible reason --mkpath exists.
# 3) --rsync-flags: full override — '-avzP' surfaces verbatim, NO -avz/--delete
#    silently merged in.
# 4) --mkpath + --rsync-flags together: clap rejects with non-zero exit and
#    a message naming both flags. No deployment is persisted.
# 5) help text advertises both flags.

# (1) default unchanged
if ! wg html publish add legacydefault --rsync "$dest/" >legacy.log 2>&1; then
    loud_fail "default add (no opt-in) failed: $(tail -10 legacy.log)"
fi
default_flags=$(wg html publish show legacydefault 2>&1 | grep -E "^  rsync flags:" || true)
if [[ "$default_flags" != *"-avz --delete"* ]]; then
    loud_fail "default rsync flags must be '-avz --delete'; got: $default_flags"
fi
if [[ "$default_flags" == *"--mkpath"* ]]; then
    loud_fail "default rsync flags must NOT silently include --mkpath; got: $default_flags"
fi
wg html publish remove legacydefault >/dev/null 2>&1

# (2) --mkpath opt-in + fresh-path live run
fresh_parent="$scratch/fresh-parent"
mkdir -p "$fresh_parent"
fresh_path="$fresh_parent/freshly/created/sub/path"
if [[ -e "$fresh_path" ]]; then
    loud_fail "test setup error: $fresh_path should NOT exist before add"
fi
if ! wg html publish add freshpath --rsync "$fresh_path/" --mkpath >fresh-add.log 2>&1; then
    loud_fail "wg html publish add --mkpath failed: $(tail -10 fresh-add.log)"
fi
mk_flags=$(wg html publish show freshpath 2>&1 | grep -E "^  rsync flags:" || true)
if ! echo "$mk_flags" | grep -q -- "--mkpath"; then
    loud_fail "--mkpath must surface in show; got: $mk_flags"
fi
if ! echo "$mk_flags" | grep -q -- "-avz"; then
    loud_fail "--mkpath must append to default ('-avz --delete'); got: $mk_flags"
fi
if ! wg html publish run freshpath >fresh-run.log 2>&1; then
    loud_fail "wg html publish run (fresh path + --mkpath) failed: $(tail -20 fresh-run.log)"
fi
if [[ ! -f "$fresh_path/index.html" ]]; then
    loud_fail "expected $fresh_path/index.html after --mkpath run; got: $(ls -la "$fresh_path" 2>&1 || echo missing)"
fi
wg html publish remove freshpath >/dev/null 2>&1

# (3) --rsync-flags full override
if ! wg html publish add overrideflags --rsync "$dest/" --rsync-flags='-avzP' >ov.log 2>&1; then
    loud_fail "wg html publish add --rsync-flags failed: $(tail -10 ov.log)"
fi
ov_flags=$(wg html publish show overrideflags 2>&1 | grep -E "^  rsync flags:" || true)
if [[ "$ov_flags" != *"-avzP"* ]]; then
    loud_fail "--rsync-flags must round-trip; got: $ov_flags"
fi
if echo "$ov_flags" | grep -q -- "--delete"; then
    loud_fail "--rsync-flags must FULLY REPLACE the default; got: $ov_flags"
fi
wg html publish remove overrideflags >/dev/null 2>&1

# (4) --mkpath + --rsync-flags is a CLI-level mutex
mutex_out=$(wg html publish add conflict --rsync "$dest/" --mkpath --rsync-flags='-avzP' 2>&1) && mutex_rc=0 || mutex_rc=$?
if [[ "$mutex_rc" == "0" ]]; then
    loud_fail "--mkpath + --rsync-flags must be rejected; instead exit=0 out=$mutex_out"
fi
if [[ "$mutex_out" != *"--mkpath"* || "$mutex_out" != *"--rsync-flags"* ]]; then
    loud_fail "mutex error must name both flags; got: $mutex_out"
fi
# Must NOT have persisted the rejected deployment.
if wg html publish list 2>&1 | grep -q '^  conflict$'; then
    loud_fail "rejected --mkpath+--rsync-flags add must NOT persist 'conflict' deployment"
fi

# (5) --help advertises both
help_out=$(wg html publish add --help 2>&1)
if ! echo "$help_out" | grep -qE "^\s+--mkpath"; then
    loud_fail "wg html publish add --help must advertise --mkpath; got:\n$help_out"
fi
if ! echo "$help_out" | grep -qE "^\s+--rsync-flags"; then
    loud_fail "wg html publish add --help must advertise --rsync-flags; got:\n$help_out"
fi

echo "PASS: wg html publish add/list/show/run/remove + scheduled lifecycle + default unchanged + --mkpath opt-in + --rsync-flags override + mutex + help advertised + wg publish task surface intact"
exit 0
