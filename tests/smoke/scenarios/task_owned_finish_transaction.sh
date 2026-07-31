#!/usr/bin/env bash
# Task-owned land/deliver/synthesis transaction through the installed binary.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
scratch="$(make_scratch)"; project="$scratch/project"; home="$scratch/home"
mkdir -p "$project" "$home/.config/workgraph"; : >"$home/.config/workgraph/config.toml"
(cd "$project" && git init -q -b main && git config user.email finish@test.invalid && git config user.name Finish && printf 'base\n' >shared.txt && git add . && git commit -qm base && env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH -u WG_BRANCH HOME="$home" XDG_CONFIG_HOME="$home/.config" wg init --no-agency >/dev/null)
wgrun(){ (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKTREE_PATH -u WG_BRANCH WG_DIR="$project/.wg" HOME="$home" XDG_CONFIG_HOME="$home/.config" wg "$@"); }
new_task(){
  local id=$1 contract=${2:-land} base=${3:-main}
  local wt="$scratch/$id-wt" branch="wg/source/$id"
  wgrun add "$id" --id "$id" -d $'Task-owned finish fixture.\n\n## Validation\n- durable transaction receipt' >/dev/null
  [[ $contract == land ]] || wgrun finish contract "$id" "$contract" >/dev/null
  wgrun claim "$id" --actor "source-$id" >/dev/null
  (cd "$project" && git worktree add -q -b "$branch" "$wt" "$base")
  printf '%s|%s' "$wt" "$branch"
}
finish_env(){
  local id=$1 wt=$2 branch=$3; local agent
  agent=$(wgrun show "$id" --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["assigned"])')
  printf 'WG_DIR=%q WG_TASK_ID=%q WG_AGENT_ID=%q WG_WORKTREE_PATH=%q WG_PROJECT_ROOT=%q WG_BRANCH=%q HOME=%q XDG_CONFIG_HOME=%q' "$project/.wg" "$id" "$agent" "$wt" "$project" "$branch" "$home" "$home/.config"
}
begin(){ local id=$1 wt=$2 branch=$3; eval "$(finish_env "$id" "$wt" "$branch") wg finish begin '$id' --ttl-seconds 90 --json" | python3 -c 'import json,sys;print(json.load(sys.stdin)["lease_id"])'; }
submit_land(){ local id=$1 wt=$2 branch=$3 lease=$4; (cd "$wt" && eval "$(finish_env "$id" "$wt" "$branch") wg finish submit '$id' --lease '$lease' --commit HEAD --wait-seconds 2" >/dev/null); }
cleanup(){ local id=$1 wt=$2; (cd "$project" && WG_DIR="$project/.wg" HOME="$home" XDG_CONFIG_HOME="$home/.config" wg finish cleanup "$id" >/dev/null); [[ ! -e $wt ]] || loud_fail "$id worktree survived terminal cleanup"; }
assert_landed(){ local id=$1; local j; j=$(wgrun show "$id" --json); python3 - "$id" "$j" <<'PY'
import json,sys
j=json.loads(sys.argv[2]); assert j['status']=='done' and j['completion_disposition']=='landed' and j['finish_phase']=='Cleaned', (sys.argv[1],j)
PY
  ! wgrun finalize status "$id" | grep -Eq 'RepairNeeded|merge.target_moved' || loud_fail "$id entered legacy repair flow"
}

# Two source-owned land tasks start at the same base. The repository lease
# serializes promotion while allowing the second worktree to keep editing.
base=$(cd "$project" && git rev-parse main)
IFS='|' read -r awt abr <<<"$(new_task land-a land "$base")"
IFS='|' read -r bwt bbr <<<"$(new_task land-b land "$base")"
(cd "$awt" && printf 'a\n' >a.txt && git add . && git commit -qm 'source a')
(cd "$bwt" && printf 'b\n' >b.txt && git add . && git commit -qm 'source b')
la=$(begin land-a "$awt" "$abr")
set +e
blocked=$(begin land-b "$bwt" "$bbr" 2>&1); blocked_rc=$?
set -e
[[ $blocked_rc -ne 0 ]] && grep -q 'finish.lease_busy' <<<"$blocked" || loud_fail "second land bypassed repository finish lease: $blocked"
submit_land land-a "$awt" "$abr" "$la"; cleanup land-a "$awt"
lb=$(begin land-b "$bwt" "$bbr"); submit_land land-b "$bwt" "$bbr" "$lb"; cleanup land-b "$bwt"
assert_landed land-a; assert_landed land-b
[[ -f $project/a.txt && -f $project/b.txt ]] || loud_fail "serialized land commits did not both reach main"
for id in land-a land-b; do
  status=$(wgrun finalize status "$id")
  grep -q 'evaluation-receipt=wgcid:' <<<"$status" || loud_fail "$id lacks exact-candidate evaluation receipt"
done

# A real merge conflict is returned to the same source worktree/attempt. No
# merge or repair task owns the resolution.
conflict_base=$(cd "$project" && git rev-parse main)
IFS='|' read -r c1wt c1br <<<"$(new_task conflict-first land "$conflict_base")"
IFS='|' read -r c2wt c2br <<<"$(new_task conflict-source land "$conflict_base")"
(cd "$c1wt" && printf 'first\n' >shared.txt && git add . && git commit -qm first)
(cd "$c2wt" && printf 'source\n' >shared.txt && git add . && git commit -qm source)
lc1=$(begin conflict-first "$c1wt" "$c1br"); submit_land conflict-first "$c1wt" "$c1br" "$lc1"; cleanup conflict-first "$c1wt"
owner_before=$(wgrun show conflict-source --json | python3 -c 'import json,sys; j=json.load(sys.stdin); print(j["assigned"]+"|"+j["lifecycle"]["current_attempt"]["id"])')
set +e
conflict_out=$(begin conflict-source "$c2wt" "$c2br" 2>&1); conflict_rc=$?
set -e
[[ $conflict_rc -ne 0 ]] && grep -q 'finish.integration_conflict' <<<"$conflict_out" || loud_fail "integration conflict was not returned to source: $conflict_out"
[[ -d $c2wt ]] || loud_fail "conflicted source worktree was removed"
(cd "$c2wt" && printf 'first+source\n' >shared.txt && git add shared.txt && git commit -qm 'source resolves current-main conflict')
lc2=$(begin conflict-source "$c2wt" "$c2br"); submit_land conflict-source "$c2wt" "$c2br" "$lc2"
owner_after=$(wgrun show conflict-source --json | python3 -c 'import json,sys; j=json.load(sys.stdin); print(j["assigned"]+"|"+j["lifecycle"]["current_attempt"]["id"])')
cleanup conflict-source "$c2wt"
[[ $owner_before == "$owner_after" ]] || loud_fail "conflict replaced source ownership: $owner_before -> $owner_after"
assert_landed conflict-source
! wgrun list --json | grep -Eq 'merge-conflict-source|repair-conflict-source' || loud_fail "conflict spawned detached merge/repair owner"

# Delivered children publish retained immutable refs and clean immediately.
# The land synthesis consumes those refs in its own worktree and alone moves main.
deliver_base=$(cd "$project" && git rev-parse main)
IFS='|' read -r d1wt d1br <<<"$(new_task contribution-one deliver "$deliver_base")"
IFS='|' read -r d2wt d2br <<<"$(new_task contribution-two deliver "$deliver_base")"
(cd "$d1wt" && printf 'one\n' >contribution-one.txt && git add . && git commit -qm contribution-one)
(cd "$d2wt" && printf 'two\n' >contribution-two.txt && git add . && git commit -qm contribution-two)
main_before_deliver=$(cd "$project" && git rev-parse main)
for spec in "contribution-one|$d1wt|$d1br" "contribution-two|$d2wt|$d2br"; do
  IFS='|' read -r id wt br <<<"$spec"
  (cd "$wt" && eval "$(finish_env "$id" "$wt" "$br") wg finish submit '$id' --commit HEAD --wait-seconds 2" >/dev/null)
  cleanup "$id" "$wt"
  j=$(wgrun show "$id" --json); python3 - "$j" <<'PY'
import json,sys
j=json.loads(sys.argv[1]); assert j['status']=='done' and j['completion_disposition']=='delivered' and j['finish_phase']=='Cleaned',j
PY
done
[[ $(cd "$project" && git rev-parse main) == "$main_before_deliver" ]] || loud_fail "deliver child advanced main"
r1=$(cd "$project" && git for-each-ref --format='%(refname)' 'refs/wg/contributions/contribution-one/*' | tail -1)
r2=$(cd "$project" && git for-each-ref --format='%(refname)' 'refs/wg/contributions/contribution-two/*' | tail -1)
[[ -n $r1 && -n $r2 ]] || loud_fail "delivered immutable contribution refs missing"

wgrun add synthesis --id synthesis -d $'Combine immutable contribution refs.\n\n## Validation\n- both retained inputs on main' >/dev/null
wgrun finish input synthesis --from contribution-one >/dev/null
wgrun finish input synthesis --from contribution-two >/dev/null
wgrun claim synthesis --actor source-synthesis >/dev/null
swt="$scratch/synthesis-wt"; sbr=wg/source/synthesis
(cd "$project" && git worktree add -q -b "$sbr" "$swt" main)
(cd "$swt" && git merge --no-ff --no-edit "$r1" >/dev/null && git merge --no-ff --no-edit "$r2" >/dev/null)
ls=$(begin synthesis "$swt" "$sbr"); submit_land synthesis "$swt" "$sbr" "$ls"; cleanup synthesis "$swt"
assert_landed synthesis
cleanup_before=$(wgrun show synthesis --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["completion_receipt"])')
wgrun finish cleanup synthesis >/dev/null
cleanup_after=$(wgrun show synthesis --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["completion_receipt"])')
[[ $cleanup_before == "$cleanup_after" ]] || loud_fail "cleanup crash replay produced a second terminal receipt"
[[ -f $project/contribution-one.txt && -f $project/contribution-two.txt ]] || loud_fail "synthesis omitted delivered inputs"
[[ -n $(cd "$project" && git show-ref "$r1") && -n $(cd "$project" && git show-ref "$r2") ]] || loud_fail "synthesis consumed live worktrees instead of retained refs"

echo "task-owned finish transaction passed"
