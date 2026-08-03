#!/usr/bin/env bash
# Candidate-binary broker handoff plus exact-root WorkSave fault checks.
set -euo pipefail
export WG_SMOKE_ROOT="/tmp/wg-as-broker-${BASHPID}"
HERE="$(cd "$(dirname "$0")" && pwd)"; . "$HERE/_helpers.sh"
command -v cargo >/dev/null || loud_skip "MISSING CARGO" "candidate build requires cargo"
ROOT=$(git -C "$HERE" rev-parse --show-toplevel) || loud_fail "cannot find repository root"
(cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg) || loud_fail "candidate build failed"
WG_BIN="$ROOT/target/debug/wg"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH WG_DIR WG_BRANCH
scratch=$(make_scratch); project="$scratch/project"; home="$scratch/home"; fake="$scratch/fake"; evidence="$scratch/evidence"
mkdir -p "$project" "$home" "$fake" "$evidence"
cat >"$fake/pi" <<'SH'
#!/usr/bin/env bash
exec bash worker.sh
SH
chmod +x "$fake/pi"
export PATH="$fake:$(dirname "$WG_BIN"):$PATH" HOME="$home" XDG_CONFIG_HOME="$home/.config" OPENROUTER_API_KEY=fake BROKER_EVIDENCE="$evidence"
cd "$project"; git init -q -b main; git config user.email broker@test.invalid; git config user.name Broker
cat >worker.sh <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ ! -e .wg ]] || { echo 'worker saw graph control plane' >&2; exit 81; }
mkdir -p docs
printf 'exact retained worktree bytes\n' >docs/atomic-save.md
printf 'uncommitted WIP survives broker handoff\n' >broker-untracked.txt
git add docs/atomic-save.md
git commit -qm 'brokered deliverable'
git rev-parse HEAD >"$BROKER_EVIDENCE/commit"
printf '%s\n' "$PWD" >"$BROKER_EVIDENCE/worktree"
WG_WORKER_REQUEST_ID=atomic-save-stable-done wg done "$WG_TASK_ID" >"$BROKER_EVIDENCE/first"
WG_WORKER_REQUEST_ID=atomic-save-stable-done wg done "$WG_TASK_ID" >"$BROKER_EVIDENCE/replay"
printf ok >"$BROKER_EVIDENCE/finished"
SH
chmod +x worker.sh; git add worker.sh; git commit -qm base
"$WG_BIN" init --no-agency --route pi --model pi:openrouter:test/model >/dev/null
wgrun(){ env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC WG_DIR="$project/.wg" "$WG_BIN" "$@"; }
wgrun config --local --model pi:openrouter:test/model --auto-assign false --auto-evaluate false --flip-enabled false --no-reload >/dev/null
wgrun add broker --id broker --model pi:openrouter:test/model -d $'broker fixture\n\n## Deliverables\n- docs/atomic-save.md\n\n## Validation\n- exact handoff' >/dev/null
wgrun finalize contract broker report >/dev/null; wgrun publish broker --only >/dev/null
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --no-supervise
for _ in $(seq 1 360); do [[ -s "$evidence/finished" ]] && break; sleep .2; done
[[ -s "$evidence/finished" ]] || loud_fail "broker worker did not finish: $(tail -60 "$project/.wg/service/daemon.log" 2>/dev/null || true)"
# Stop before a no-finalization child can be considered for replacement: this
# scenario targets the authenticated broker reservation/replay cut, while the
# dead-owner scenario separately owns replacement convergence.
wgrun service stop >/dev/null 2>&1 || true
status=$(wgrun show broker --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["status"])')
[[ $status != done ]] || loud_fail "broker handoff alone manufactured Done before GraphSave"
wt=$(cat "$evidence/worktree"); [[ $wt == *'/.wg-worktrees/'* ]] || loud_fail "broker did not bind retained worktree"
[[ ! -e "$project/docs/atomic-save.md" ]] || loud_fail "broker copied deliverable into ambient graph root"
oid=$(cat "$evidence/commit"); [[ $(git show "$oid:docs/atomic-save.md") == 'exact retained worktree bytes' ]] || loud_fail "retained candidate commit missing"
# Both calls used one stable request. Output representation may change, but the
# broker audit is the durable replay authority and must record exact replay.
grep -q '"outcome":"replayed"' "$project/.wg/service/worker-capability-audit.jsonl" || loud_fail "broker replay audit missing"
[[ $(grep -c '"request_id":"atomic-save-stable-done"' "$project/.wg/service/worker-capability-audit.jsonl") -ge 2 ]] || loud_fail "stable request ID not journaled twice"
(cd "$ROOT" && cargo test --quiet --test atomic_save_faults broker_handoff_requires_bound_worktree -- --exact) || loud_fail "bound-worktree WorkSave fault failed"
(cd "$ROOT" && cargo test --quiet --test atomic_save_faults lost_done_response_replays_graphsave_intent -- --exact) || loud_fail "lost response fault failed"
echo "PASS: brokered candidate used its authenticated retained worktree, stable done replayed exactly, dirty WorkSave captured WIP, and mismatched root was refused"
