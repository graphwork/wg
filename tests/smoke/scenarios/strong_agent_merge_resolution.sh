#!/usr/bin/env bash
# Installed-binary strong-agent merge-resolution authority flow.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
scratch="$(make_scratch)"; project="$scratch/project"; home="$scratch/home"; fake="$scratch/fake-strong-merger"; calls="$scratch/calls"
mkdir -p "$project" "$home/.config/workgraph"; : >"$home/.config/workgraph/config.toml"
(cd "$project" && git init -q -b main && git config user.name Test && git config user.email test.invalid@example.com && printf 'base\n' >value.txt && git add . && git commit -qm base && HOME="$home" XDG_CONFIG_HOME="$home/.config" wg init --no-agency >/dev/null)
wgrun(){ (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID WG_DIR="$project/.wg" HOME="$home" XDG_CONFIG_HOME="$home/.config" wg "$@"); }
cat >>"$project/.wg/config.toml" <<'TOML'
[models.merger]
model = "pi:fake:strong-coder"
tier = "premium"
reasoning = "high"
TOML
cat >"$fake" <<SH
#!/usr/bin/env bash
set -euo pipefail
workspace= outcome= route= reasoning= bundle= provider= model=
while ((\$#)); do
 case \$1 in --workspace) workspace=\$2;; --outcome) outcome=\$2;; --route) route=\$2;; --reasoning) reasoning=\$2;; --bundle-cid) bundle=\$2;; --provider) provider=\$2;; --model) model=\$2;; esac
 shift 2
done
[[ \$route == pi:fake:strong-coder && \$provider == fake && \$model == strong-coder && \$reasoning == high && \$bundle == wgcid:* ]]
[[ ! -e \$workspace/.wg/graph.jsonl ]]
printf x >>"$calls"
printf 'resolved\n' >"\$workspace/value.txt"
printf '{"outcome":"resolved","explanation":"bounded fake resolution","generator_commands":[]}\n' >"\$outcome"
SH
chmod +x "$fake"
new_candidate(){
 local id=$1 value=$2
 local wt="$scratch/$id-wt"
 wgrun add "$id" --id "$id" -d $'Immutable candidate.\n\n## Validation\n- exact content binding' >/dev/null
 wgrun claim "$id" >/dev/null
 (cd "$project" && git worktree add -q -b "wg/test/$id" "$wt")
 wgrun pi-watchdog fixture-init "$id" --worktree "$wt" --now 0 >/dev/null
 printf '%s\n' "$value" >"$wt/value.txt"
 wgrun finalize checkpoint "$id" --worktree "$wt" --quiescence-receipt "receipt:$id" >/dev/null
 printf '%s' "$wt"
}
# Mechanical lane proves model independence and zero calls.
new_candidate clean candidate-clean >/dev/null
clean=$(wgrun merge-resolution run clean --adapter "$fake")
grep -q 'MechanicalMerge / MR_MECHANICAL_CLEAN' <<<"$clean" || loud_fail "clean classifier missing: $clean"
[[ ! -e $calls ]] || loud_fail "clean merge invoked strong adapter"
# Real textual conflict -> exact route -> standalone repo -> fresh gates -> CAS.
new_candidate conflict candidate >/dev/null
printf 'target\n' >"$project/value.txt"; (cd "$project" && git add value.txt && git commit -qm target)
before_source=$(wgrun candidate show conflict --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["candidate_commit_oid"])')
out=$(wgrun merge-resolution run conflict --adapter "$fake" --integration-check 'test "$(cat value.txt)" = resolved')
grep -q 'Merge resolution Merged' <<<"$out" || loud_fail "resolution did not merge: $out"
grep -q 'pi:fake:strong-coder strength=Premium reasoning=high.*no fallback' <<<"$out" || loud_fail "exact strong route hidden: $out"
grep -q 'isolated=true' <<<"$out" || loud_fail "isolation probes hidden: $out"
grep -q 'gates: safety=accept validation=true evaluation=true' <<<"$out" || loud_fail "fresh gates missing: $out"
[[ $(cat "$calls") == x && $(cat "$project/value.txt") == resolved ]] || loud_fail "wrong call count/result"
json=$(wgrun merge-resolution status conflict --json)
RESOLUTION_JSON="$json" python3 - <<'PY' || loud_fail "descriptor/receipt tree binding mismatch"
import json,os
x=json.loads(os.environ['RESOLUTION_JSON'])
assert x['runner_invocations']==1
assert x['descriptor']['resolution_tree_oid']==x['merge_receipt']['result_tree_oid']
assert x['gates']['descriptor_id']==x['descriptor']['resolution_candidate_id']
PY
[[ $(wgrun candidate show conflict --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["candidate_commit_oid"])') == "$before_source" ]] || loud_fail "source candidate mutated"
receipt=$(python3 -c 'import json,sys;print(json.load(sys.stdin)["merge_receipt"]["receipt_id"])' <<<"$json")
wgrun merge-resolution run conflict --adapter "$fake" >/dev/null
[[ $(cat "$calls") == x ]] || loud_fail "duplicate delivery charged again"
# Installed service restart leaves the same durable receipt.
wgrun service start --no-chat-agent --force >/dev/null 2>&1 || true; sleep .2; wgrun service stop >/dev/null 2>&1 || true
[[ $(wgrun merge-resolution status conflict --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["merge_receipt"]["receipt_id"])') == "$receipt" ]] || loud_fail "restart drifted receipt"
# Unknown generated ownership is a human stop and creates no satellite/call.
new_candidate generated-ambiguous candidate-generated >/dev/null
printf 'target-generated\n' >"$project/value.txt"; (cd "$project" && git add value.txt && git commit -qm target-generated)
human=$(wgrun merge-resolution run generated-ambiguous --adapter "$fake" --generated)
grep -q 'HumanDecisionRequired' <<<"$human" || loud_fail "generated ambiguity did not stop: $human"
[[ $(cat "$calls") == x ]] || loud_fail "human stop invoked adapter"
wgrun merge-resolution decide generated-ambiguous --rationale 'source ownership must be selected by product owner' >/dev/null
wgrun merge-resolution rollback "$receipt" | grep -q 'compensating immutable candidate' || loud_fail "rollback implied hard reset"
echo 'strong agent merge resolution passed'
