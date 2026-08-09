#!/usr/bin/env bash
# Golden-path proof for the simplified trusted-local recovery lifecycle.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"

scratch=$(make_scratch)
repo="$scratch/project"; home="$scratch/home"
mkdir -p "$repo" "$home"
ROOT="$(cd "$HERE/../../.." && pwd)"
WG_BIN="${WG_SMOKE_CANDIDATE_BIN:-$ROOT/target/debug/wg}"
[[ -x "$WG_BIN" ]] || (cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg)
[[ -x "$WG_BIN" ]] || loud_fail "candidate binary missing: $WG_BIN"

clean_env=(env -u WG_DIR -u WG_TASK_ID -u WG_AGENT_ID -u WG_PROJECT_ROOT -u WG_WORKTREE_PATH HOME="$home")
run(){ (cd "$repo" && "${clean_env[@]}" "$WG_BIN" --dir "$repo/.wg" "$@"); }

cd "$repo"
git init -q -b main
git config user.email recovery@test.invalid
git config user.name Recovery
echo base > base.txt
git add base.txt && git commit -qm base
"${clean_env[@]}" "$WG_BIN" init --no-agency >/dev/null
git add .gitignore AGENTS.md CLAUDE.md && git commit -qm init-wg

run add "Simple completion" --id simple-finish >/dev/null
run publish simple-finish --only >/dev/null
run claim simple-finish --actor local-worker >/dev/null
git switch -qc worker/simple-finish
echo complete > result.txt
git add result.txt && git commit -qm result
(cd "$repo" && env -u WG_DIR HOME="$home" WG_TASK_ID=simple-finish WG_AGENT_ID=local-worker \
  "$WG_BIN" --dir "$repo/.wg" done simple-finish >"$scratch/done.out" 2>"$scratch/done.err")
run show simple-finish --json >"$scratch/simple.json"
python3 - "$scratch/simple.json" <<'PY'
import json, sys
x=json.load(open(sys.argv[1]))
assert x["status"] == "done", x
assert x["completion_disposition"] == "landed", x
assert x["completion_receipt"], x
activity=x.get("completion_review_activity", [])
assert len(activity) == 1, activity
assert activity[0]["reviewer_kind"] == "flip", activity
assert activity[0]["verdict"] == "unavailable", activity
PY
[[ "$(git rev-parse main)" == "$(git rev-parse worker/simple-finish)" ]] || loud_fail "one-operation completion did not publish exact worker commit"
grep -q "Advisory model review did not pass" "$scratch/done.err" || loud_fail "advisory finding was not visible"

# Explicit operator recovery is forbidden inside worker authority, then succeeds
# outside it with an immutable receipt and reason.
run add "Dead owner recovery" --id operator-recovery >/dev/null
run publish operator-recovery --only >/dev/null
run claim operator-recovery --actor dead-worker >/dev/null
if (cd "$repo" && env -u WG_DIR HOME="$home" WG_AGENT_ID=dead-worker \
  "$WG_BIN" --dir "$repo/.wg" done operator-recovery --operator-accept --reason bad \
  >"$scratch/operator-worker.out" 2>"$scratch/operator-worker.err"); then
  loud_fail "worker environment acquired operator acceptance authority"
fi
grep -q "operator acceptance is refused inside a worker process" "$scratch/operator-worker.err" || loud_fail "worker refusal was not explicit"
run done operator-recovery --operator-accept --reason "operator verified preserved result" >"$scratch/operator.out"
run show operator-recovery --json >"$scratch/operator.json"
python3 - "$scratch/operator.json" "$repo/.wg/completion/v3/objects" <<'PY'
import json, pathlib, sys
x=json.load(open(sys.argv[1]))
assert x["status"] == "done", x
receipt=x["completion_receipt"]
assert receipt and receipt.startswith("b3:"), x
p=pathlib.Path(sys.argv[2]) / receipt.removeprefix("b3:")
assert p.is_file(), p
body=json.loads(p.read_text())
assert body["reason"] == "operator verified preserved result", body
PY

echo "PASS: simple local completion, advisory review visibility, and audited operator recovery"
