#!/usr/bin/env bash
# Candidate-binary regression: task-owned Land wins before a generated wrapper
# reports a later non-zero process exit.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
unset WG_WORKER_CAPABILITY WG_WORKER_IPC WG_AGENT_ID WG_EXECUTOR_TYPE WG_MODEL WG_TIER
require_wg

scratch="$(make_scratch)"
project="$scratch/project"
home="$scratch/home"
evidence="$scratch/evidence"
fakebin="$scratch/fakebin"
mkdir -p "$project" "$home/.config/workgraph" "$evidence" "$fakebin"
: >"$home/.config/workgraph/config.toml"
cat >"$fakebin/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null || true
exec bash worker.sh
FAKE_PI
chmod +x "$fakebin/pi"

cat >"$project/worker.sh" <<'WORKER'
#!/usr/bin/env bash
set -euo pipefail
printf 'durable success precedes wrapper exit\n' >late-exit-payload.txt
git add late-exit-payload.txt
git commit -qm 'late exit payload'
wg log "$WG_TASK_ID" 'task-owned Land begins before forced wrapper exit' >/dev/null
wg artifact "$WG_TASK_ID" late-exit-payload.txt >/dev/null
WG_WORKER_REQUEST_ID=late-exit-done wg done "$WG_TASK_ID" >"$LATE_EXIT_EVIDENCE/done.out"
WG_WORKER_REQUEST_ID=late-exit-diagnostic wg fail "$WG_TASK_ID" --class agent-exit-nonzero \
  --reason 'forced provider-unavailable after durable task-owned Land' >"$LATE_EXIT_EVIDENCE/diagnostic.out"
printf 'wg done returned successfully; forcing wrapper exit 73\n' >"$LATE_EXIT_EVIDENCE/worker-summary"
exit 73
WORKER
chmod +x "$project/worker.sh"

(
  cd "$project"
  git init -q -b main
  git config user.email late-exit@test.invalid
  git config user.name 'Late Exit Smoke'
  git add worker.sh
  git commit -qm baseline
  env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID HOME="$home" XDG_CONFIG_HOME="$home/.config" wg init --no-agency >/dev/null
)
export HOME="$home" XDG_CONFIG_HOME="$home/.config" LATE_EXIT_EVIDENCE="$evidence" PATH="$fakebin:$PATH"
wgrun() {
  (cd "$project" && env -u WG_TASK_ID -u WG_AGENT_ID -u WG_EXECUTOR_TYPE -u WG_WORKTREE_PATH \
    WG_DIR="$project/.wg" LATE_EXIT_EVIDENCE="$evidence" wg "$@")
}
wgrun config --local --model pi:openrouter:test/model --poll-interval 1 --auto-assign false --auto-evaluate false --flip-enabled false --no-reload >/dev/null
wgrun add 'late exit durable source' --id late-exit-source --model pi:openrouter:test/model \
  -d $'Land one committed payload, then let the generated wrapper observe a nonzero exit.\n\n## Deliverables\n- late-exit-payload.txt\n' >/dev/null
wgrun add 'late exit dependent' --id late-exit-dependent --after late-exit-source \
  -d $'Dependency readiness probe.\n\n## Validation\n- source terminal success satisfies this edge exactly once\n' >/dev/null
wgrun publish late-exit-source --only >/dev/null

start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --no-supervise
status=''
for _ in $(seq 1 400); do
  status=$(wgrun show late-exit-source --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
  [[ $status == done || $status == failed || $status == abandoned ]] && break
  sleep .1
done
[[ $status == done ]] || loud_fail "late-exit source did not finish: $(wgrun show late-exit-source)
--- daemon ---
$(tail -80 "$project/.wg/service/daemon.log" 2>/dev/null || true)
--- agents ---
$(find "$project/.wg/agents" -name output.log -type f -exec tail -50 {} \; 2>/dev/null || true)"
wgrun service stop >/dev/null 2>&1 || true

[[ -s "$evidence/worker-summary" ]] || loud_fail 'worker did not return from task-owned wg done before exit 73'
[[ -f "$project/late-exit-payload.txt" ]] || loud_fail 'landed payload missing from main'
[[ $(git -C "$project" log --all --format='%s' | grep -c '^late exit payload$') == 1 ]] \
  || loud_fail 'source commit was duplicated or lost'

wgrun show late-exit-source --json >"$evidence/show.json"
python3 - "$evidence/show.json" <<'PY' || loud_fail 'terminal precedence projection is invalid'
import json,sys
j=json.load(open(sys.argv[1])); events=j['lifecycle']['audit']
assert j['status']=='done', j
assert j.get('completion_disposition')=='landed', j
assert j.get('finish_phase')=='Cleaned', j
assert j.get('retry_count',0)==0, j
assert sum(e['event_kind']=='attempt-succeeded' for e in events)==1, events
assert not [e for e in events if e['event_kind'] in ('attempt-failed','attempt-lost')], events
logs=j.get('log',[])
assert any('late-process-diagnostic' in (e.get('actor') or '') and 'provider-unavailable' in e.get('message','') for e in logs), logs
PY
wgrun finalize status late-exit-source --json >"$evidence/finish.json"
python3 - "$evidence/finish.json" <<'PY' || loud_fail 'finish transaction duplicated or incomplete'
import json,sys
j=json.load(open(sys.argv[1]))
assert j['phase']=='cleaned',j
assert j['candidate']['candidate_version']==1,j
assert j['merge_receipt'] and j['cleanup_receipt'],j
PY
grep -Eq 'exact durable successful finalization|"handoff":"fail"|"outcome":"accepted"' "$evidence/diagnostic.out" \
  || loud_fail "late process failure disappeared instead of remaining diagnostic evidence: $(cat "$evidence/diagnostic.out" 2>/dev/null || true)"

# Publish only after stopping the daemon so readiness can be observed without
# dispatch racing it. Repeated reads contain the one dependent exactly once and
# do not replay completion/promotion.
wgrun publish late-exit-dependent --only >/dev/null
for n in 1 2; do
  ready=$(wgrun ready)
  count=$(grep -c 'late-exit-dependent' <<<"$ready" || true)
  [[ $count == 1 ]] || loud_fail "readiness projection $n contained dependent $count times: $ready"
done
receipt_before=$(wgrun show late-exit-source --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["completion_receipt"])')
wgrun finish cleanup late-exit-source >/dev/null
receipt_after=$(wgrun show late-exit-source --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["completion_receipt"])')
[[ $receipt_before == "$receipt_after" ]] || loud_fail 'cleanup replay minted a second completion receipt'

printf 'PASS: task-owned Land stayed Done/Landed/Cleaned after wrapper exit 73; failure remained diagnostic, promotion and dependency readiness were exact-once\n'
