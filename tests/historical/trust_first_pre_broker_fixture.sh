#!/usr/bin/env bash
# Executable historical proof against the parent of the scoped IPC broker:
# an ordinary isolated local worker received the graph and coordinated siblings.
set -euo pipefail
: "${WG_BIN:?set WG_BIN to the historical pre-broker binary}"
[[ -x $WG_BIN ]]

scratch=$(mktemp -d "${TMPDIR:-/tmp}/wg-historical-trust-first.XXXXXX")
project="$scratch/project"
home="$scratch/home"
cleanup() {
  env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC \
    WG_DIR="$project/.wg" "$WG_BIN" service stop --force --kill-agents >/dev/null 2>&1 || true
  rm -rf "$scratch"
}
trap cleanup EXIT
mkdir -p "$project" "$home" "$scratch/bin"
ln -s "$WG_BIN" "$scratch/bin/wg"
cat >"$scratch/bin/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
bash "$(dirname "$0")/worker.sh"
printf '%s\n' '{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"historical coordination complete"}],"provider":"test","model":"fake","stopReason":"stop","usage":{"input":1,"output":1,"cacheRead":0,"cacheWrite":0,"totalTokens":2,"cost":{"total":0}}}}'
SH
cat >"$scratch/bin/worker.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ -n ${WG_DIR:-} ]] || { echo "historical WG_DIR missing" >&2; exit 80; }
wg show historical-downstream --json > before.json
wg edit historical-downstream --description 'historical trust-first cross-task edit'
wg add 'Historical child' --id historical-child --after "$WG_TASK_ID"
wg reprioritize historical-downstream critical
wg msg send historical-downstream 'historical trust-first cross-task message'
printf 'old-trusted\n' > historical-trusted.txt
git add before.json historical-trusted.txt
git commit -qm 'historical trust-first evidence'
SH
chmod +x "$scratch/bin/pi" "$scratch/bin/worker.sh"
export PATH="$scratch/bin:$PATH" HOME="$home" XDG_CONFIG_HOME="$home/.config"
unset WG_AGENT_ID WG_TASK_ID WG_WORKER_CAPABILITY WG_WORKER_IPC WG_PROJECT_ROOT WG_WORKTREE_PATH WG_DIR

git -C "$project" init -q -b main
git -C "$project" config user.email historical-trust@test.invalid
git -C "$project" config user.name HistoricalTrust
printf 'base\n' >"$project/README"
git -C "$project" add README
git -C "$project" commit -qm base
(cd "$project" && "$WG_BIN" init --no-agency --route pi --model pi:openrouter:test/model >/dev/null)
wgrun() {
  (cd "$project" && env -u WG_AGENT_ID -u WG_TASK_ID -u WG_WORKER_CAPABILITY -u WG_WORKER_IPC \
    WG_DIR="$project/.wg" "$WG_BIN" "$@")
}
wgrun config --local --auto-evaluate false --no-reload >/dev/null
wgrun add controller --id historical-controller >/dev/null
wgrun add downstream --id historical-downstream --after historical-controller >/dev/null
wgrun publish historical-controller --only >/dev/null
wgrun service start --max-agents 1 --no-coordinator-agent --no-supervise >/dev/null

worktree=""
for _ in $(seq 1 240); do
  worktree=$(find "$project/.wg-worktrees" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | head -1 || true)
  if [[ -n $worktree ]] && git -C "$worktree" show HEAD:historical-trusted.txt 2>/dev/null | grep -qx old-trusted; then
    break
  fi
  sleep 0.25
done
[[ -n $worktree ]]
git -C "$worktree" show HEAD:historical-trusted.txt | grep -qx old-trusted
grep -q 'historical trust-first cross-task edit' "$project/.wg/graph.jsonl"
grep -q '"id":"historical-child"' "$project/.wg/graph.jsonl"
grep -q 'historical trust-first cross-task message' "$project/.wg/messages/historical-downstream.jsonl"
echo "PASS: pre-broker historical worker edited/read/messaged a sibling and created a linked task through normal WG commands"
