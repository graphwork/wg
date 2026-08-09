#!/usr/bin/env bash
# Credential-free terminal canary for host-captured completion validation.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"

scratch=$(make_scratch)
repo="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$repo" "$home/.config" "$fakebin"
ROOT="$(cd "$HERE/../../.." && pwd)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$ROOT/target/debug/wg}"
[[ -x "$WG_BIN" ]] || (cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
ln -s "$WG_BIN" "$fakebin/wg"

# The fake provider is deliberately content-blind: it can only return the
# selected semantic verdict. WG, not this fixture/model, decides whether exact
# deterministic evidence is complete enough to reach FLIP and then Eval.
cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
: "${FAKE_REVIEW_STATE:?}"
args="$*"
cat >/dev/null || true
n=$(($(cat "$FAKE_REVIEW_STATE.count" 2>/dev/null || echo 0)+1))
printf '%s\n' "$n" >"$FAKE_REVIEW_STATE.count"
printf '%s\n' "$args" >>"$FAKE_REVIEW_STATE.argv"
verdict=$(cat "$FAKE_REVIEW_STATE.mode" 2>/dev/null || echo pass)
if [[ "$verdict" == reject ]]; then
  response='{"verdict":"reject","findings":[{"code":"canary.semantic","message":"advisory disagreement"}]}'
else
  response='{"verdict":"pass","findings":[]}'
fi
python3 - "$response" <<'PY'
import json,sys
response=sys.argv[1]
print(json.dumps({"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":response}],"provider":"test","model":"fake-review","stopReason":"stop","usage":{"input":2,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":3,"cost":{"total":0.0001}}}}))
PY
SH
chmod +x "$fakebin/pi"

export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
export PATH="$fakebin:$PATH" FAKE_REVIEW_STATE="$scratch/review"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_GRAPH_ID WG_PROJECT_ROOT WG_WORKTREE_PATH \
  WG_WORKTREE_ACTIVE WG_BRANCH WG_WORKER_ATTEMPT_ID WG_WORKER_ATTEMPT_FENCE \
  WG_WORKER_GENERATION WG_SPAWN_EPOCH WG_SPAWN_RUN_ID WG_WORKER_CONTROL_MODE || true

cd "$repo"
git init -q -b main
git config user.email validation@test.invalid
git config user.name Validation
echo base > base.txt
git add base.txt && git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
git add .gitignore AGENTS.md CLAUDE.md && git commit -qm init-wg
wgrun(){ env -u WG_TASK_ID -u WG_AGENT_ID -u WG_GRAPH_ID WG_DIR="$repo/.wg" "$WG_BIN" "$@"; }
wgrun config --local --model pi:test:fake-review --reasoning low --auto-assign false \
  --auto-evaluate false --set-model reviewer pi:test:fake-review --set-reasoning reviewer low \
  --set-model evaluator pi:test:fake-review --set-reasoning evaluator low --no-reload >/dev/null

# Start the latest daemon before any reviewed task exists. Its in-memory graph
# projections are therefore older than the rows written by Done below; any
# later full save must merge (rather than erase) immutable review activity.
start_wg_daemon "$repo" --no-chat-agent --max-agents 0 --interval 1
# The helper's wrapper log is intentionally inside its argument directory;
# unlink it after startup so Land's clean-candidate predicate stays exact.
rm -f "$repo/daemon.log"

# Valid host-captured evidence must admit both model stages, land, and derive
# Done. The command emits enough output to prove stdout and stderr are distinct.
wgrun add "Validation evidence pass" --id evidence-pass \
  --validation-command "test -s result.txt && printf 'validated stdout\\n' && printf 'validated stderr\\n' >&2" \
  -d $'Produce result.txt.\n\n## Validation\n- [ ] exact deterministic command passes' >/dev/null
wgrun publish evidence-pass --only >/dev/null
wgrun claim evidence-pass --actor validation-worker >/dev/null
git switch -qc worker/evidence-pass
echo result > result.txt
git add result.txt && git commit -qm evidence-pass
if ! env WG_TASK_ID=evidence-pass WG_AGENT_ID=validation-worker \
  "$WG_BIN" --dir "$repo/.wg" done evidence-pass >"$scratch/pass.out" 2>"$scratch/pass.err"; then
  loud_fail "valid one-step completion failed: $(cat "$scratch/pass.err")"
fi
wgrun show evidence-pass --json >"$scratch/pass.json"
python3 - "$scratch/pass.json" "$repo/.wg/completion/v3/objects" <<'PY'
import json,pathlib,sys
x=json.load(open(sys.argv[1])); objects=pathlib.Path(sys.argv[2])
assert x['status']=='done' and x['completion_disposition']=='landed',x
rows=x['completion_review_activity']
assert [(r['reviewer_kind'],r['verdict']) for r in rows]==[('flip','pass'),('eval','pass')],rows
binding=rows[0]['binding']
assert binding['task_id']=='evidence-pass' and binding['attempt_id'] and binding['attempt_fence']>0,binding
mref=x['completion_candidate']['manifest']['content_digest']
manifest=json.loads((objects/mref.removeprefix('b3:')).read_text())
assert len(manifest['validation_evidence'])==2,manifest
seen=set()
for ref in manifest['validation_evidence']:
    body=json.loads((objects/ref['content_digest'].removeprefix('b3:')).read_text())
    seen.add(body['purpose'])
    assert body['capture_origin']=='wg_done' and body['evidence_version']==1,body
    assert body['lifecycle']['task_id']=='evidence-pass',body
    assert body['lifecycle']['generation']==binding['generation'],body
    assert body['lifecycle']['attempt_id']==binding['attempt_id'],body
    assert body['lifecycle']['attempt_fence']==binding['attempt_fence'],body
    assert body['exit']=={'code':0,'success':True,'timed_out':False},body['exit']
    assert body['command']['command_digest'].startswith('b3:'),body
    assert body['repository']['before_head_oid']==manifest['source_revision'],body
    assert body['repository']['before_head_oid']==body['repository']['after_head_oid'],body
    assert body['repository']['before_tree_oid']==body['repository']['after_tree_oid'],body
    assert body['repository']['before_status_digest']==body['repository']['after_status_digest'],body
    assert body['repository']['cwd_relative']=='.',body
    assert body['duration_ms']>=0 and body['started_at'] and body['finished_at'],body
    for stream in ('stdout','stderr'):
        assert body[stream]['digest'].startswith('b3:'),body[stream]
        assert body[stream]['captured_bytes']<=32768,body[stream]
assert seen=={'configured','baseline'},seen
PY
[[ "$(cat "$scratch/review.count")" == 2 ]] || loud_fail "valid evidence did not reach exactly FLIP then Eval"

# A configured command cannot be replaced by worker prose. Resolver rejection
# occurs before any model call and remains a hard, visible deterministic gate.
wgrun add "Missing deterministic evidence" --id evidence-missing \
  --validation-command "test -s missing-report.txt" \
  -d $'Produce a report.\n\n## Validation\n- [ ] exact command evidence required' >/dev/null
wgrun contract evidence-missing report >/dev/null
wgrun publish evidence-missing --only >/dev/null
wgrun claim evidence-missing --actor missing-worker >/dev/null
git switch -qc worker/evidence-missing refs/heads/main
printf 'summary\n' > missing-summary.txt
printf 'report\n' > missing-report.txt
printf 'worker claims validation passed\n' > worker-prose.txt
wgrun completion-object missing-report.txt --media-type text/plain > missing-output.json
wgrun completion-object worker-prose.txt --media-type text/plain --evidence-kind worker-prose > missing-evidence.json
wgrun completion-manifest evidence-missing --summary missing-summary.txt \
  --output-ref missing-output.json --evidence-ref missing-evidence.json > missing-manifest.json
if env WG_TASK_ID=evidence-missing WG_AGENT_ID=missing-worker \
  "$WG_BIN" --dir "$repo/.wg" submit evidence-missing --manifest missing-manifest.json \
  --summary missing-summary.txt >"$scratch/missing.out" 2>"$scratch/missing.err"; then
  loud_fail "worker prose satisfied configured deterministic validation"
fi
grep -q 'incomplete deterministic evidence' "$scratch/missing.err" \
  || loud_fail "missing evidence rejection was not visible: $(cat "$scratch/missing.err")"
[[ "$(cat "$scratch/review.count")" == 2 ]] || loud_fail "missing evidence reached a model reviewer"
wgrun show evidence-missing --json >"$scratch/missing.json"
python3 - "$scratch/missing.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); rows=x['completion_review_activity']
assert x['status']=='in-progress',x
assert len(rows)==1 and rows[0]['reviewer_kind']=='flip' and rows[0]['verdict']=='incomplete_evidence',rows
assert rows[0]['failure_class']=='incomplete_evidence',rows
assert rows[0]['findings'][0]['code'].startswith('resolver.'),rows
PY

# Mutating a named immutable evidence object must be detected by its full CAS
# digest before FLIP. Restore the fixture bytes afterward so the earlier Done
# remains independently resolvable during the restart checks below.
wgrun add "Tampered deterministic evidence" --id evidence-tampered \
  --validation-command "test -s tampered-report.txt" \
  -d $'Produce a report.\n\n## Validation\n- [ ] tampered evidence is refused' >/dev/null
wgrun contract evidence-tampered report >/dev/null
wgrun publish evidence-tampered --only >/dev/null
wgrun claim evidence-tampered --actor tampered-worker >/dev/null
printf 'summary\n' > tampered-summary.txt
printf 'report\n' > tampered-report.txt
wgrun completion-object tampered-report.txt --media-type text/plain > tampered-output.json
python3 - "$scratch/pass.json" "$repo/.wg/completion/v3/objects" > tampered-evidence.json <<'PY'
import json,pathlib,sys
x=json.load(open(sys.argv[1])); objects=pathlib.Path(sys.argv[2])
manifest=json.loads((objects/x['completion_candidate']['manifest']['content_digest'].removeprefix('b3:')).read_text())
ref=next(r for r in manifest['validation_evidence'] if r['evidence_kind'].endswith('configured/v1'))
print(json.dumps(ref))
PY
wgrun completion-manifest evidence-tampered --summary tampered-summary.txt \
  --output-ref tampered-output.json --evidence-ref tampered-evidence.json > tampered-manifest.json
tampered_digest=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["content_digest"].removeprefix("b3:"))' tampered-evidence.json)
tampered_object="$repo/.wg/completion/v3/objects/$tampered_digest"
cp "$tampered_object" "$scratch/original-evidence"
python3 - "$tampered_object" <<'PY'
import pathlib,sys
path=pathlib.Path(sys.argv[1]); data=bytearray(path.read_bytes())
data[0] = ord('[') if data[0] != ord('[') else ord('{')
path.write_bytes(data)
PY
if env WG_TASK_ID=evidence-tampered WG_AGENT_ID=tampered-worker \
  "$WG_BIN" --dir "$repo/.wg" submit evidence-tampered --manifest tampered-manifest.json \
  --summary tampered-summary.txt >"$scratch/tampered.out" 2>"$scratch/tampered.err"; then
  loud_fail "digest-tampered deterministic validation evidence was accepted"
fi
cp "$scratch/original-evidence" "$tampered_object"
grep -q 'incomplete deterministic evidence' "$scratch/tampered.err" \
  || loud_fail "tampered evidence rejection was not visible: $(cat "$scratch/tampered.err")"
[[ "$(cat "$scratch/review.count")" == 2 ]] || loud_fail "tampered evidence reached a model reviewer"
wgrun show evidence-tampered --json >"$scratch/tampered.json"
python3 - "$scratch/tampered.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); rows=x['completion_review_activity']
assert x['status']=='in-progress',x
assert len(rows)==1 and rows[0]['verdict']=='incomplete_evidence',rows
assert 'digest' in rows[0]['findings'][0]['code'],rows
PY

# Reusing a real result from another candidate/attempt is stale evidence, not a
# pass. The exact task/requirements/generation/attempt/fence binding rejects it
# before FLIP.
wgrun add "Stale deterministic evidence" --id evidence-stale \
  --validation-command "test -s stale-report.txt" \
  -d $'Produce a report.\n\n## Validation\n- [ ] current candidate evidence required' >/dev/null
wgrun contract evidence-stale report >/dev/null
wgrun publish evidence-stale --only >/dev/null
wgrun claim evidence-stale --actor stale-worker >/dev/null
git switch -qc worker/evidence-stale refs/heads/main
printf 'summary\n' > stale-summary.txt
printf 'report\n' > stale-report.txt
wgrun completion-object stale-report.txt --media-type text/plain > stale-output.json
python3 - "$scratch/pass.json" "$repo/.wg/completion/v3/objects" > stale-evidence.json <<'PY'
import json,pathlib,sys
x=json.load(open(sys.argv[1])); objects=pathlib.Path(sys.argv[2])
manifest=json.loads((objects/x['completion_candidate']['manifest']['content_digest'].removeprefix('b3:')).read_text())
ref=next(r for r in manifest['validation_evidence'] if r['evidence_kind'].endswith('configured/v1'))
print(json.dumps(ref))
PY
wgrun completion-manifest evidence-stale --summary stale-summary.txt \
  --output-ref stale-output.json --evidence-ref stale-evidence.json > stale-manifest.json
if env WG_TASK_ID=evidence-stale WG_AGENT_ID=stale-worker \
  "$WG_BIN" --dir "$repo/.wg" submit evidence-stale --manifest stale-manifest.json \
  --summary stale-summary.txt >"$scratch/stale.out" 2>"$scratch/stale.err"; then
  loud_fail "stale cross-candidate validation evidence was accepted"
fi
grep -q 'incomplete deterministic evidence' "$scratch/stale.err" \
  || loud_fail "stale evidence rejection was not visible: $(cat "$scratch/stale.err")"
[[ "$(cat "$scratch/review.count")" == 2 ]] || loud_fail "stale evidence reached a model reviewer"

# A failing configured command is authoritative regardless of advisory model
# policy. Its bounded failure receipt is logged; no candidate/reviewer exists.
wgrun add "Failing deterministic evidence" --id evidence-fail \
  --validation-command "printf 'expected failure\\n' >&2; exit 9" \
  -d $'Produce failure.txt.\n\n## Validation\n- [ ] deterministic command must pass' >/dev/null
wgrun publish evidence-fail --only >/dev/null
wgrun claim evidence-fail --actor fail-worker >/dev/null
git switch -qc worker/evidence-fail refs/heads/main
echo failure > failure.txt
git add failure.txt && git commit -qm evidence-fail
if env WG_TASK_ID=evidence-fail WG_AGENT_ID=fail-worker \
  "$WG_BIN" --dir "$repo/.wg" done evidence-fail >"$scratch/fail.out" 2>"$scratch/fail.err"; then
  loud_fail "failing deterministic command completed"
fi
grep -q 'configured deterministic validation rejected completion' "$scratch/fail.err" \
  || loud_fail "failing command rejection was not visible: $(cat "$scratch/fail.err")"
grep -q 'expected failure' "$scratch/fail.err" || loud_fail "bounded stderr was not surfaced"
wgrun show evidence-fail --json >"$scratch/fail.json"
python3 - "$scratch/fail.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1]))
assert x['status']=='in-progress' and x.get('completion_candidate') is None,x
assert any('exit=Some(9)' in row['message'] and 'evidence=b3:' in row['message'] for row in x['log']),x['log']
PY
[[ "$(cat "$scratch/review.count")" == 2 ]] || loud_fail "failing command reached model review"

# Semantic disagreement remains advisory by default once deterministic evidence
# is valid: FLIP rejects, Eval is correctly skipped, and lifecycle still lands.
printf 'reject\n' >"$scratch/review.mode"
wgrun add "Advisory semantic disagreement" --id evidence-advisory \
  --validation-command "test -s advisory.txt" \
  -d $'Produce advisory.txt.\n\n## Validation\n- [ ] deterministic command passes' >/dev/null
wgrun publish evidence-advisory --only >/dev/null
wgrun claim evidence-advisory --actor advisory-worker >/dev/null
rm -f missing-* worker-prose.txt stale-* tampered-*
git switch -qc worker/evidence-advisory refs/heads/main
echo advisory > advisory.txt
git add advisory.txt && git commit -qm evidence-advisory
env WG_TASK_ID=evidence-advisory WG_AGENT_ID=advisory-worker \
  "$WG_BIN" --dir "$repo/.wg" done evidence-advisory >"$scratch/advisory.out" 2>"$scratch/advisory.err" \
  || loud_fail "advisory semantic rejection changed lifecycle authority: $(cat "$scratch/advisory.err")"
wgrun show evidence-advisory --json >"$scratch/advisory.json"
python3 - "$scratch/advisory.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); rows=x['completion_review_activity']
assert x['status']=='done' and x['completion_disposition']=='landed',x
assert [(r['reviewer_kind'],r['verdict']) for r in rows]==[('flip','reject')],rows
PY
[[ "$(cat "$scratch/review.count")" == 3 ]] || loud_fail "advisory FLIP unexpectedly reached Eval"

# Let the pre-Done daemon complete another loop/save, then restart it. Neither
# a stale full save nor process restart may lose the immutable FLIP/Eval rows.
# This exercises real service persistence, not a copied in-memory task fixture.
rm -f "$scratch/review.mode"
sleep 2
wgrun service status >/dev/null
wgrun service stop >/dev/null
start_wg_daemon "$repo" --no-chat-agent --max-agents 0 --interval 1
rm -f "$repo/daemon.log"
sleep 1
wgrun show evidence-pass --json >"$scratch/restarted.json"
python3 - "$scratch/restarted.json" <<'PY'
import json,sys
x=json.load(open(sys.argv[1])); rows=x['completion_review_activity']
assert x['status']=='done',x
assert [(r['reviewer_kind'],r['verdict'],r['candidate_state']) for r in rows]==[
    ('flip','pass','current'),('eval','pass','current')],rows
assert len({r['activity_id'] for r in rows})==2,rows
PY

echo "PASS: deterministic evidence → FLIP Pass → Eval; missing/tampered/stale/failing rejected; advisory neutral; restart durable"
