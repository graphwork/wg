#!/usr/bin/env bash
# Candidate-binary regression for brokered deliverable preflight root binding.
set -euo pipefail
# Unix-domain sockets cap path length; keep this long-named scenario's root short.
export WG_SMOKE_ROOT="/tmp/wg-bd-${BASHPID}"
source "$(dirname "$0")/_helpers.sh"
require_wg
WG_BIN=$(command -v wg)
[[ -x $WG_BIN ]] || loud_fail "candidate WG_BIN is not executable: $WG_BIN"

scratch=$(make_scratch)
project="$scratch/project"
home="$scratch/home"
evidence="$scratch/evidence"
mkdir -p "$project" "$home" "$scratch/bin" "$evidence"
ln -s "$WG_BIN" "$scratch/bin/wg"
cat >"$scratch/bin/pi" <<'SH'
#!/usr/bin/env bash
exec bash worker.sh
SH
chmod +x "$scratch/bin/pi"
export PATH="$scratch/bin:$PATH" HOME="$home" XDG_CONFIG_HOME="$home/.config" EVIDENCE_DIR="$evidence"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH WG_DIR

git -C "$project" init -q -b main
git -C "$project" config user.email brokered-deliverable@test.invalid
git -C "$project" config user.name BrokeredDeliverable
cat >"$project/worker.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ ! -e .wg ]] || { echo 'worker unexpectedly sees graph control plane' >&2; exit 81; }
mkdir -p docs
printf 'atomic graph/work save design\n' > docs/atomic-save.md
git add docs/atomic-save.md
git commit -qm 'committed brokered deliverable'
git rev-parse HEAD >"$EVIDENCE_DIR/commit"
# Use a stable id so a lost response can be replayed without re-executing done.
WG_WORKER_REQUEST_ID=brokered-deliverable-done wg done "$WG_TASK_ID" >"$EVIDENCE_DIR/first"
WG_WORKER_REQUEST_ID=brokered-deliverable-done wg done "$WG_TASK_ID" >"$EVIDENCE_DIR/replay"
printf 'ok\n' >"$EVIDENCE_DIR/worker-finished"
SH
chmod +x "$project/worker.sh"
git -C "$project" add worker.sh
git -C "$project" commit -qm base
(
  cd "$project"
  env -u WG_DIR "$WG_BIN" init --no-agency --route pi --model pi:openrouter:test/model >/dev/null
)
wgrun() {
  (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC \
    WG_DIR="$project/.wg" "$WG_BIN" "$@")
}
wgrun add "brokered deliverable preflight" --id brokered-deliverable-preflight \
  -d $'Commit the design only in the retained worker worktree.\n\n## Deliverables\n- docs/atomic-save.md\n' >/dev/null
wgrun finalize contract brokered-deliverable-preflight report >/dev/null
wgrun publish brokered-deliverable-preflight --only >/dev/null
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --no-supervise

for _ in $(seq 1 360); do
  [[ -s "$evidence/worker-finished" ]] && break
  status=$(wgrun show brokered-deliverable-preflight --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
  [[ $status == failed || $status == abandoned ]] && loud_fail "brokered worker terminal status: $status"
  sleep 0.25
done
if [[ ! -s "$evidence/worker-finished" ]]; then
  diagnostics=$(find "$project/.wg/agents" -type f -maxdepth 3 -print -exec tail -30 {} \; 2>/dev/null || true)
  loud_fail "brokered worker did not complete; wrapper=$(tail -30 "$project/daemon.log" 2>/dev/null || true); daemon=$(tail -60 "$project/.wg/service/daemon.log" 2>/dev/null || true); agents=$diagnostics"
fi
[[ ! -e "$project/docs/atomic-save.md" ]] || loud_fail "deliverable was copied into graph root"
oid=$(cat "$evidence/commit")
git -C "$project" show "$oid:docs/atomic-save.md" | grep -qx 'atomic graph/work save design' \
  || loud_fail "committed worktree-only deliverable is missing"
grep -q '"handoff":"done"' "$evidence/first" || loud_fail "first brokered done was not accepted"
cmp -s "$evidence/first" "$evidence/replay" || loud_fail "stable broker request did not replay idempotently"
grep -q '"outcome":"replayed"' "$project/.wg/service/worker-capability-audit.jsonl" \
  || loud_fail "broker replay audit evidence missing"

echo "PASS: committed deliverable present only in the authenticated retained worktree passed brokered preflight, task-owned completion ran without copying to root, and the exact lost-response retry replayed idempotently"
