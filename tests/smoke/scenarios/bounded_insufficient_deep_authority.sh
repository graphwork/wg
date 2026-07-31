#!/usr/bin/env bash
# Bounded truncation is infrastructure-only while exact-candidate deep FLIP decides.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v tmux >/dev/null 2>&1 || loud_skip "MISSING TMUX" "tmux is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
REPO_ROOT="$(cd "$HERE/../../.." && pwd)"
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" ]]; then WG_BIN="$WG_SMOKE_CANDIDATE_BIN"; else
  export CARGO_TARGET_DIR="$scratch/candidate-target"
  (cd "$REPO_ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
  WG_BIN="$CARGO_TARGET_DIR/debug/wg"
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"
project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$project/src" "$home/.config" "$fakebin"
ln -s "$WG_BIN" "$fakebin/wg"
cat >"$fakebin/pi" <<EOF
#!/usr/bin/env bash
set -euo pipefail
model=""; argv=("\$@")
while ((\$#)); do case "\$1" in --model) model="\$2"; shift 2;; *) shift;; esac; done
case "\$model" in
  source-worker)
    cat >/dev/null || true
    python3 -c 'from pathlib import Path; Path("src/api.rs").write_text("pub const MODE: &str = \\"deep\\";\\n" + "pub const TABLE: &str = \\"" + "exact-candidate-byte-" * 5000 + "\\";\\n")'
    wg artifact "\$WG_TASK_ID" src/api.rs >/dev/null
    wg done "\$WG_TASK_ID" >/dev/null
    printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"candidate complete"}],"provider":"test","model":"source-worker","stopReason":"stop","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
    ;;
  bounded-must-not-run)
    echo 'bounded semantic model invoked despite deterministic truncation' >&2
    exit 97
    ;;
  deep-pass) exec '$HERE/../../fixtures/fake-pi-deep/pi' "\${argv[@]}";;
  *) echo "unexpected model \$model" >&2; exit 88;;
esac
EOF
chmod +x "$fakebin/pi"
export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
unset WG_TASK_ID WG_AGENT_ID WG_TIER WG_EXECUTOR_TYPE WG_MODEL TMUX TMUX_TMPDIR
unset OPENAI_API_KEY OPENROUTER_API_KEY ANTHROPIC_API_KEY AWS_SECRET_ACCESS_KEY
base_env=(env -u WG_TASK_ID -u WG_AGENT_ID -u WG_TIER -u WG_EXECUTOR_TYPE -u WG_MODEL \
  -u OPENAI_API_KEY -u OPENROUTER_API_KEY -u ANTHROPIC_API_KEY -u AWS_SECRET_ACCESS_KEY \
  HOME="$HOME" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" WG_GLOBAL_DIR="$WG_GLOBAL_DIR" PATH="$fakebin:$PATH")
(cd "$project" && git init -q -b main && git config user.email bounded@test.invalid && git config user.name Bounded \
  && printf 'pub const MODE: &str = "legacy";\n' > src/api.rs \
  && printf 'pub const MODES: &[&str] = &["legacy", "deep"];\n' > src/registry.rs \
  && git add src && git commit -qm base && "${base_env[@]}" "$WG_BIN" init --no-agency >/dev/null)
G="$project/.wg"
wgrun(){ (cd "$project" && "${base_env[@]}" "$WG_BIN" --dir "$G" "$@"); }
wgrun config --local --model pi:test:source-worker --reasoning high --auto-assign false \
  --auto-evaluate true --eval-gate-all true --eval-gate-threshold 0.8 --flip-enabled false \
  --set-model evaluator pi:test:bounded-must-not-run --set-reasoning evaluator low \
  --set-model flip_inference pi:test:deep-pass --set-model flip_comparison pi:test:deep-pass --no-reload >/dev/null
wgrun add 'Implement source code larger than the bounded evidence budget' --id bounded-source \
  -d $'Change src/api.rs while preserving the full generated lookup table. Correctness depends on bytes beyond any truncated prefix.\n\n## Validation\n- [ ] exact candidate table is preserved' >/dev/null
wgrun publish bounded-source --only >/dev/null

session="wg-bounded-insufficient-$$"
cleanup(){ tmux kill-session -t "$session" 2>/dev/null || true; wgrun service stop >/dev/null 2>&1 || true; }
add_cleanup_hook cleanup
tmux new-session -d -x 180 -y 55 -s "$session" "cd '$project' && env -u WG_TASK_ID -u WG_AGENT_ID HOME='$HOME' XDG_CONFIG_HOME='$XDG_CONFIG_HOME' WG_GLOBAL_DIR='$WG_GLOBAL_DIR' PATH='$fakebin:$PATH' WG_TUI_APPEARANCE=none '$WG_BIN' --dir '$G' tui"
dump(){ local raw; raw=$(wgrun --json tui-dump 2>/dev/null || true); [[ -n "$raw" ]] && python3 -c 'import json,sys; print(json.load(sys.stdin).get("text", ""))' <<<"$raw"; }
(cd "$project" && "${base_env[@]}" "$WG_BIN" --dir "$G" service start --max-agents 1 --model pi:test:source-worker --no-coordinator-agent --no-supervise >/dev/null)

details=''
for _ in $(seq 1 600); do
  details=$(wgrun show bounded-source --json 2>/dev/null || true)
  if python3 -c 'import json,sys; x=json.load(sys.stdin); rs=x["evaluation_records"]; b=next(r for r in rs if r["product"]=="bounded"); d=next(r for r in rs if r["product"]=="deep-readonly-flip"); assert x["status"]=="done" and b["state"]=="retry-backoff" and d["state"]=="consumed"' <<<"$details" 2>/dev/null; then break; fi
  sleep .05
done
DETAILS="$details" python3 - "$project" "$G" <<'PY' || loud_fail "bounded/deep lifecycle invalid: $details"
import json,os,pathlib,stat,subprocess,sys
x=json.loads(os.environ['DETAILS']); rs=x['evaluation_records']
b=next(r for r in rs if r['product']=='bounded'); d=next(r for r in rs if r['product']=='deep-readonly-flip')
assert b['policy']['applicability']=='advisory' and b.get('verdict') is None and b.get('consumed_verdict_id') is None
assert len(b['attempts'])==1 and b['attempts'][0]['failure']['kind']=='insufficient-evidence'
f=b['attempts'][0]['failure']
assert 'candidate-source' in f['safe_evidence_categories'] and 'candidate-source' in f['safe_evidence_ids']
assert x['status']=='done' and x.get('retry_count',0)==0 and x.get('spawn_failures',0)==0
assert x['lifecycle']['current_attempt']['disposition']=='succeeded'
assert not any('acceptance-rejected' in e['event_kind'] for e in x['lifecycle']['audit'])
assert d['policy']['applicability']=='required' and d['state']=='consumed' and d['deep_report']['outcome']=='pass'
source=b['source']; objects=pathlib.Path(sys.argv[2])/'finalization'/'objects'
descriptor=json.loads((objects/source['candidate_digest'].replace(':','_')).read_text())
candidate=subprocess.check_output(['git','-C',sys.argv[1],'show',descriptor['candidate_commit_oid']+':src/api.rs'])
main=subprocess.check_output(['git','-C',sys.argv[1],'show','main:src/api.rs'])
assert candidate==main and b'exact-candidate-byte-' in candidate and len(candidate)>80000
attempt=d['attempts'][0]['attempt_id']
materialized=pathlib.Path(sys.argv[2])/'evaluation'/'runtime'/(d['evaluation_id']+'-'+attempt)/'bundle'/'repository'/'src'/'api.rs'
assert materialized.read_bytes()==candidate
assert not (materialized.stat().st_mode & stat.S_IWUSR)
PY
[[ ! -e "$home/fake-pi-invocations.log" ]] || loud_fail "bounded semantic evaluator was invoked"
grep -Fq 'deep-pass' "$home/fake-pi-deep-invocations.log" || loud_fail "required exact-candidate deep FLIP did not run"

show=$(wgrun show bounded-source)
for needle in 'InsufficientEvidence' 'bounded evidence categories: candidate-source' 'candidate-source'; do
  grep -Fq "$needle" <<<"$show" || loud_fail "terminal show hid safe bounded evidence state $needle: $show"
done

tmux send-keys -t "$session" /; tmux send-keys -t "$session" -l bounded-source; sleep .1; tmux send-keys -t "$session" Enter; sleep .1; tmux send-keys -t "$session" Enter
seen=''
for _ in $(seq 1 220); do
  frame=$(dump); seen+=$'\n'"$frame"
  if grep -Fq 'InsufficientEvidence' <<<"$seen" && grep -Fq 'Evidence categories: candidate-source' <<<"$seen" && grep -Fq 'Deep report: Pass' <<<"$seen"; then break; fi
  tmux send-keys -t "$session" PageDown; sleep .02
done
for needle in 'InsufficientEvidence' 'Evidence categories: candidate-source' 'Deep report: Pass'; do
  grep -Fq "$needle" <<<"$seen" || loud_fail "TUI hid bounded/deep evidence state $needle"
done

echo "PASS: truncated bounded evidence stayed infrastructure-only; exact immutable candidate deep FLIP decided; terminal/TUI showed safe evidence diagnostics"
