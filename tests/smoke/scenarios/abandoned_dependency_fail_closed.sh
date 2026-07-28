#!/usr/bin/env bash
# Installed-binary regression: Abandoned is terminal history, never required-success evidence.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
scratch=$(make_scratch)
cd "$scratch"
cleanup(){ wg service stop --force >/dev/null 2>&1 || true; }
trap cleanup EXIT

wg init -x shell >/dev/null 2>&1 || loud_fail "init failed"
wg config --local --auto-assign false --no-reload >/dev/null 2>&1 || loud_fail "config failed"
wg add prerequisite --id prerequisite --assign shell-worker >/dev/null
wg add packaging --id packaging --after prerequisite --assign shell-worker >/dev/null
wg add launch --id launch --after packaging --assign shell-worker >/dev/null
wg publish prerequisite --only >/dev/null
wg publish packaging --only >/dev/null
wg publish launch --only >/dev/null

before=$(wg ready 2>&1)
packaging_before=$(wg --json show packaging 2>&1)
[[ "$before" == *"prerequisite"* && "$before" != *"packaging -"* ]] \
  || loud_fail "before abandon readiness wrong: $before"

abandon=$(wg abandon prerequisite --reason obsolete --superseded-by replacement 2>&1) \
  || loud_fail "abandon failed"
[[ "$abandon" == *"Affected ordinary dependents remain blocked"* ]] \
  || loud_fail "abandon did not report affected dependent: $abandon; packaging=$packaging_before"
[[ "$abandon" == *"provenance only"* ]] \
  || loud_fail "supersession was not identified as provenance-only: $abandon"

after=$(wg ready 2>&1)
[[ "$after" != *"packaging -"* && "$after" != *"launch -"* ]] \
  || loud_fail "abandoned prerequisite authorized downstream readiness: $after"
why=$(wg why-blocked packaging 2>&1)
[[ "$why" == *"blocked: prerequisite prerequisite was abandoned"* ]] \
  || loud_fail "why-blocked lost abandoned blocker: $why"
why_json=$(wg --json why-blocked packaging 2>&1)
[[ "$why_json" == *'"total_blockers": 1'* && "$why_json" == *'"superseded_by"'* ]] \
  || loud_fail "stable why-blocked JSON lost blocker/provenance: $why_json"

if wg claim packaging >claim.log 2>&1; then loud_fail "manual claim bypassed abandoned prerequisite"; fi
if wg spawn packaging --executor shell >spawn.log 2>&1; then loud_fail "spawn bypassed abandoned prerequisite"; fi
if wg done packaging >done.log 2>&1; then loud_fail "worker completion bypassed abandoned prerequisite"; fi

# Repeated daemon polls and restart must create no packaging/launch attempt surface.
for pass in 1 2; do
  wg service start --max-agents 1 --no-coordinator-agent --no-supervise >"service-$pass.log" 2>&1 \
    || loud_fail "service start $pass failed: $(cat service-$pass.log)"
  sleep 3
  wg service stop --force >/dev/null 2>&1 || true
done
for id in packaging launch; do
  show=$(wg --json show "$id")
  [[ "$show" != *'"status": "in-progress"'* && "$show" != *'"assigned"'* ]] \
    || loud_fail "$id gained a live attempt: $show"
  [[ ! -e ".wg/output/$id" && ! -e ".workgraph/output/$id" ]] \
    || loud_fail "$id gained output/runtime state"
done

# Archived status is authoritative: archived Abandoned remains blocked across restart and undo.
wg archive prerequisite -y >/dev/null 2>&1 || loud_fail "archive abandoned prerequisite failed"
[[ "$(wg ready 2>&1)" != *"packaging -"* ]] || loud_fail "archived Abandoned boundary satisfied"
wg service start --max-agents 0 --no-coordinator-agent --no-supervise >/dev/null 2>&1 || true
sleep 1
wg service stop --force >/dev/null 2>&1 || true
[[ "$(wg ready 2>&1)" != *"packaging -"* ]] || loud_fail "restart made archived Abandoned satisfy"
wg archive --undo >/dev/null 2>&1 || loud_fail "archive undo failed"
[[ "$(wg ready 2>&1)" != *"packaging -"* ]] || loud_fail "undo made restored Abandoned satisfy"

# Only an explicit audited graph mutation authorizes progress.
wg rm-dep packaging prerequisite >/dev/null
ready_after_waiver=$(wg ready 2>&1)
[[ "$ready_after_waiver" == *"packaging"* ]] \
  || loud_fail "explicit edge removal did not permit progress: $ready_after_waiver"
rg -q '"op"[[:space:]]*:[[:space:]]*"unlink"|rm-dep|Removed dependency' .wg .workgraph 2>/dev/null \
  || loud_fail "edge removal lacks durable operation provenance"

echo "PASS: abandoned required-success prerequisites fail closed across admission, completion, archive, and daemon restart"
exit 0
