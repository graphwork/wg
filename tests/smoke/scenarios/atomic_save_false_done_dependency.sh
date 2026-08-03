#!/usr/bin/env bash
# Candidate binary + migration-adapter regression for a naked legacy Done row.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v cargo >/dev/null || loud_skip "MISSING CARGO" "candidate build requires cargo"
ROOT=$(git -C "$HERE" rev-parse --show-toplevel) || loud_fail "cannot find repository root"
(cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg) || loud_fail "candidate build failed"
WG_BIN="$ROOT/target/debug/wg"; export PATH="$(dirname "$WG_BIN"):$PATH"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH WG_DIR WG_BRANCH
scratch=$(make_scratch); project="$scratch/project"; home="$scratch/home"
mkdir -p "$project" "$home"
cd "$project"; git init -q -b main; git config user.email false-done@test.invalid; git config user.name FalseDone
printf 'base\n' >README; git add README; git commit -qm base
HOME="$home" XDG_CONFIG_HOME="$home/.config" "$WG_BIN" init --no-agency >/dev/null
wgrun(){ env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_DIR="$project/.wg" "$WG_BIN" "$@"; }
wgrun add predecessor --id predecessor -d $'legacy source\n\n## Validation\n- evidence required' >/dev/null
wgrun add dependent --id dependent --after predecessor -d $'must remain blocked\n\n## Validation\n- no false dispatch' >/dev/null
# Plant the historical incident directly: status Done with neither a v2 receipt
# nor a lifecycle GraphSave. Preserve these exact bytes as the migration input.
python3 - "$project/.wg/graph.jsonl" <<'PY'
import json,sys
p=sys.argv[1]; rows=[]
for line in open(p):
    j=json.loads(line)
    if j.get('id')=='predecessor':
        j['status']='done'; j.pop('completion_receipt',None); j.pop('completion_disposition',None)
    rows.append(json.dumps(j,separators=(',',':')))
open(p,'w').write('\n'.join(rows)+'\n')
PY
before=$(sha256sum "$project/.wg/graph.jsonl" | cut -d' ' -f1)
shown=$(wgrun show predecessor --json)
python3 -c 'import json,sys;j=json.load(sys.stdin);assert j["status"]=="done";assert not j.get("completion_receipt");assert not any(e.get("event_kind")=="graph-save-committed" for e in j.get("lifecycle",{}).get("audit",[]))' <<<"$shown" || loud_fail "candidate did not expose planted false Done honestly"
# Run the exact migration/dispatch adapter conformance test against the same
# candidate source. It classifies -> persists -> applies NeedsReconciliation,
# then calls production ready_tasks and proves dependent is absent.
(cd "$ROOT" && cargo test --quiet --test atomic_save_faults false_done_dependency_dispatch -- --exact) || loud_fail "false-Done migration/dispatch conformance failed"
[[ $before == "$(sha256sum "$project/.wg/graph.jsonl" | cut -d' ' -f1)" ]] || loud_fail "read-only candidate inspection rewrote planted history"
echo "PASS: candidate exposed a receipt-less Done without blessing it; exact migration adapter quarantined it and production readiness blocked its dependent"
