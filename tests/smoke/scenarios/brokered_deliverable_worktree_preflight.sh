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
launches=0
[[ -f "$EVIDENCE_DIR/launch-count" ]] && launches=$(cat "$EVIDENCE_DIR/launch-count")
printf '%s\n' "$((launches + 1))" >"$EVIDENCE_DIR/launch-count"
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
wgrun finalize contract brokered-deliverable-preflight deliver >/dev/null
wgrun add "brokered completion dependent" --id brokered-completion-dependent \
  --after brokered-deliverable-preflight \
  -d $'The exact completion/v2 GraphSave must satisfy this typed contribution dependency.\n\n## Validation\n- source is Done and Cleaned before readiness\n' >/dev/null
wgrun finalize input brokered-completion-dependent --from brokered-deliverable-preflight >/dev/null
wgrun publish brokered-deliverable-preflight --only >/dev/null
start_wg_daemon "$project" --max-agents 1 --no-coordinator-agent --no-supervise

status=''
finish_phase=''
for _ in $(seq 1 360); do
  read -r status finish_phase < <(wgrun show brokered-deliverable-preflight --json 2>/dev/null | python3 -c 'import json,sys; j=json.load(sys.stdin); print(j.get("status",""), j.get("finish_phase",""))' 2>/dev/null || true)
  [[ $status == failed || $status == abandoned ]] && loud_fail "brokered worker terminal status: $status"
  [[ $status == done && $finish_phase == Cleaned && -s "$evidence/worker-finished" ]] && break
  sleep 0.25
done
if [[ $status != done || $finish_phase != Cleaned || ! -s "$evidence/worker-finished" ]]; then
  diagnostics=$(find "$project/.wg/agents" -type f -maxdepth 3 -print -exec tail -30 {} \; 2>/dev/null || true)
  loud_fail "brokered worker did not reach Done/Cleaned; status=$status finish=$finish_phase wrapper=$(tail -30 "$project/daemon.log" 2>/dev/null || true); daemon=$(tail -60 "$project/.wg/service/daemon.log" 2>/dev/null || true); agents=$diagnostics"
fi
[[ ! -e "$project/docs/atomic-save.md" ]] || loud_fail "deliverable was copied into graph root"
oid=$(cat "$evidence/commit")
git -C "$project" show "$oid:docs/atomic-save.md" | grep -qx 'atomic graph/work save design' \
  || loud_fail "committed worktree-only deliverable is missing"
grep -q '"handoff":"accepted"' "$evidence/first" || loud_fail "first brokered done was not accepted"
cmp -s "$evidence/first" "$evidence/replay" || loud_fail "stable broker request did not replay idempotently"
grep -q '"outcome":"replayed"' "$project/.wg/service/worker-capability-audit.jsonl" \
  || loud_fail "broker replay audit evidence missing"
[[ $(cat "$evidence/launch-count") == 1 ]] || loud_fail "Prepared completion respawned its source: $(cat "$evidence/launch-count") launches"
python3 - "$project/.wg/completion/v2/transactions" <<'PY' \
  || loud_fail "exact completion/v2 transaction did not reach GraphSaved"
import json, pathlib, sys
heads=list(pathlib.Path(sys.argv[1]).glob('*/head.json'))
rows=[json.loads(p.read_text()) for p in heads]
rows=[r for r in rows if r.get('source',{}).get('task_id')=='brokered-deliverable-preflight']
assert len(rows)==1, rows
assert rows[0]['phase']=='graph-saved', rows[0]
PY
wgrun service stop >/dev/null 2>&1 || true
wgrun publish brokered-completion-dependent --only >/dev/null
ready=$(wgrun ready)
[[ $(grep -c 'brokered-completion-dependent' <<<"$ready" || true) == 1 ]] \
  || loud_fail "Done/Cleaned source did not unlock its dependent exactly once: $ready"

echo "PASS: real brokered wg done advanced its exact Prepared completion/v2 intent to GraphSaved + Done/Cleaned without respawn, preserved the authenticated worktree deliverable, replayed response loss idempotently, and unlocked its dependent exactly once"
