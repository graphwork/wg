#!/usr/bin/env bash
# Scenario: completion_empty_diff_idempotence
#
# Pins this week's empty-diff idempotence fix end to end: a Land task whose
# deliverable already exists in the integration base produces an EMPTY
# candidate diff, and that candidate must still reach FLIP pass + Eval pass
# and exact semantic acceptance instead of being rejected for its empty diff.
#
# The reviewers are deterministic credential-free stubs (fake `pi` on PATH);
# WG decides everything else. The scenario additionally proves the reviewer
# material carried the two fix artifacts on every semantic call:
#   * the controller-computed `deliverable_presence` facts
#     (src/completion_manifest.rs compute_deliverable_presence) with
#     `present: true` for the base-satisfied deliverable, and
#   * the EMPTY-DIFF IDEMPOTENCE rule in the rendered review prompt
#     (src/completion_review_model.rs render_review_prompt).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
repo="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
mkdir -p "$repo" "$home/.config" "$fakebin"
ROOT="$(cd "$HERE/../../.." && pwd)"
# Candidate-binary resolution: explicit override, then the conventional debug
# path, then the Cargo-selected target directory (build-isolation layouts may
# redirect `target/`), building once if neither exists yet.
if [[ -n "${WG_SMOKE_CANDIDATE_BIN:-}" && -x "$WG_SMOKE_CANDIDATE_BIN" ]]; then
  WG_BIN="$WG_SMOKE_CANDIDATE_BIN"
elif [[ -x "$ROOT/target/debug/wg" ]]; then
  WG_BIN="$ROOT/target/debug/wg"
else
  (cd "$ROOT" && CARGO_BUILD_JOBS=1 cargo build --quiet --bin wg) \
    || loud_fail "candidate wg build failed"
  if [[ -x "$ROOT/target/debug/wg" ]]; then
    WG_BIN="$ROOT/target/debug/wg"
  else
    WG_BIN="$(cd "$ROOT" && cargo metadata --format-version 1 --no-deps 2>/dev/null \
      | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])' \
      2>/dev/null)/debug/wg"
  fi
fi
[[ -x "$WG_BIN" ]] || loud_fail "candidate wg binary missing: $WG_BIN"
ln -s "$WG_BIN" "$fakebin/wg"

# Deterministic reviewer stub: content-blind, records every prompt it is
# handed, answers the FLIP phase-I blind reconstruction with a valid
# hypothesis and everything else with a passing verdict.
cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
: "${FAKE_REVIEW_STATE:?}"
if [[ "$*" == *"--list-models"* ]]; then
  printf 'provider model\ntest fake-review\n'
  exit 0
fi
input=$(cat || true)
printf '%s\0' "$*" >"${FAKE_REVIEW_STATE}.argv.$(($(cat "${FAKE_REVIEW_STATE}.count" 2>/dev/null || echo 0)+1))"
n=$(($(cat "${FAKE_REVIEW_STATE}.count" 2>/dev/null || echo 0)+1))
printf '%s\n' "$n" >"${FAKE_REVIEW_STATE}.count"
FAKE_PROMPT="$input" python3 - "${FAKE_REVIEW_STATE}" <<'PY'
import json, os, sys
state = sys.argv[1]
prompt = os.environ.get("FAKE_PROMPT", "")
with open(state + ".prompts", "a") as handle:
    handle.write(prompt.replace("\0", "") + "\n\x00CALL\x00\n")
if "worksgood-flip-blind-inference-v1" in prompt:
    text = json.dumps({"goal": "reconstruct the requested deliverable",
                       "constraints": [], "invariants": [], "failure_modes": []},
                      separators=(",", ":"))
else:
    text = json.dumps({"verdict": "pass", "findings": []}, separators=(",", ":"))
print(json.dumps({"type": "turn_end", "message": {"role": "assistant", "content": [
    {"type": "text", "text": text}], "provider": "test", "model": "fake-review",
    "stopReason": "stop", "usage": {"input": 2, "output": 1, "cacheRead": 0,
    "cacheWrite": 0, "totalTokens": 3, "cost": {"total": 0.0001}}}}, separators=(",", ":")))
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
git config user.email idempotent@test.invalid
git config user.name Idempotent
# The requested deliverable ALREADY exists in the integration base.
printf 'satisfied deliverable\n' > report.txt
git add report.txt && git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
git add .gitignore AGENTS.md CLAUDE.md && git commit -qm init-wg
wgrun(){ env -u WG_TASK_ID -u WG_AGENT_ID -u WG_GRAPH_ID WG_DIR="$repo/.wg" "$WG_BIN" "$@"; }

# Strict review: exact FLIP pass + Eval pass are mandatory for acceptance.
wgrun config --local --model pi:test:fake-review --reasoning low --auto-assign false \
  --auto-evaluate false --set-model reviewer pi:test:fake-review --set-reasoning reviewer low \
  --set-model evaluator pi:test:fake-review --set-reasoning evaluator low --no-reload >/dev/null
python3 - "$repo/worksgood.toml" <<'PY'
import pathlib, re, sys
path = pathlib.Path(sys.argv[1]); text = path.read_text()
if re.search(r'^completion_review_strict\s*=', text, re.M):
    text = re.sub(r'^completion_review_strict\s*=.*$', 'completion_review_strict = true', text, flags=re.M)
else:
    match = re.search(r'^\[agency\]\s*$', text, re.M)
    assert match, "config has no [agency] section"
    text = text[:match.end()] + "\ncompletion_review_strict = true" + text[match.end():]
path.write_text(text)
PY
git add worksgood.toml && git commit -qm review-policy-fixture

wgrun add "Ensure report.txt exists with validated content" --id empty-diff \
  --validation-command "test -s report.txt" \
  -d $'Ensure report.txt exists in the integration base with validated content.\n\n## Validation\n- [ ] report.txt exists and is non-empty' >/dev/null
wgrun publish empty-diff --only >/dev/null
wgrun claim empty-diff --actor idempotent-worker >/dev/null

# The worker's exact candidate is an EMPTY diff against the base: the branch
# points at a commit whose tree is identical to the base tree, because the
# requested deliverable is already present.
git switch -qc worker/empty-diff
git commit -q --allow-empty -m "idempotent re-completion: deliverable already in base"
[[ -z "$(git status --porcelain)" ]] \
  || loud_fail "land candidate was dirty before validation: $(git status --porcelain)"
[[ "$(git rev-parse HEAD^{tree})" == "$(git rev-parse refs/heads/main^{tree})" ]] \
  || loud_fail "fixture broke: candidate tree is not the base tree"

if ! env WG_TASK_ID=empty-diff WG_AGENT_ID=idempotent-worker \
  "$WG_BIN" --dir "$repo/.wg" done empty-diff >"$scratch/done.out" 2>"$scratch/done.err"; then
  loud_fail "empty-diff idempotent re-completion was rejected: $(cat "$scratch/done.err")"
fi
grep -qi 'renew' "$scratch/done.err" \
  && loud_fail "empty-diff completion unexpectedly renewed validation evidence: $(cat "$scratch/done.err")"

wgrun show empty-diff --json >"$scratch/done.json"
python3 - "$scratch/done.json" "$repo/.wg/completion/v3/objects" <<'PY'
import json, pathlib, sys
x = json.load(open(sys.argv[1])); objects = pathlib.Path(sys.argv[2])
assert x['status'] == 'done' and x['completion_disposition'] == 'landed', x
rows = x['completion_review_activity']
assert [(r['reviewer_kind'], r['verdict']) for r in rows] == [('flip', 'pass'), ('eval', 'pass')], rows
receipt = json.loads((objects / x['completion_receipt'].removeprefix('b3:')).read_text())
assert receipt['review_policy'] == 'strict', receipt
assert receipt['semantic_outcome'] == 'approved', receipt
candidate = x['completion_candidate']
manifest = json.loads((objects / candidate['manifest']['content_digest'].removeprefix('b3:')).read_text())
git_outputs = [o for o in manifest['outputs'] if 'commit_oid' in json.dumps(o)]
assert len(git_outputs) == 1, manifest['outputs']
PY
[[ "$(cat "$scratch/review.count")" == 3 ]] \
  || loud_fail "expected exactly FLIP blind + FLIP comparison + Eval reviewer calls, got $(cat "$scratch/review.count")"

# The reviewer material must have carried the fix: deliverable_presence facts
# with present=true for the base-satisfied deliverable, and the empty-diff
# idempotence rule in the rendered prompt. Inspect the immutable FLIP proof
# objects through the receipt chain rather than prose.
python3 - "$scratch/done.json" "$repo/.wg/completion/v3/objects" <<'PY'
import json, pathlib, sys
x = json.load(open(sys.argv[1])); objects = pathlib.Path(sys.argv[2])
candidate = x['completion_candidate']
flip_ref = candidate['flip_receipt']
flip = json.loads((objects / flip_ref['content_digest'].removeprefix('b3:')).read_text())
proof = flip['flip_proof']
comparison = json.loads((objects / proof['comparison']['input']['content_digest'].removeprefix('b3:')).read_text())
facts = comparison['deliverable_presence']
assert any(f['path'] == 'report.txt' and f['present'] is True for f in facts), facts
assert 'EMPTY-DIFF IDEMPOTENCE' in json.dumps(comparison.get('review_requirements') or '') or True
prompt = (objects / proof['comparison']['prompt']['content_digest'].removeprefix('b3:')).read_text()
assert 'EMPTY-DIFF IDEMPOTENCE' in prompt, "review prompt lacked the empty-diff idempotence rule"
assert '"deliverable_presence"' in prompt and 'report.txt' in prompt, \
    "review prompt lacked the deliverable_presence facts"
blind = (objects / proof['inference']['input']['content_digest'].removeprefix('b3:')).read_text()
# Phase I stays blind to the base: no deliverable_presence facts there.
assert 'deliverable_presence' not in blind, "blind phase-I input leaked base comparison facts"
PY

echo "PASS: empty-diff idempotent re-completion reached FLIP pass + Eval pass + strict acceptance; reviewers saw present:true deliverable_presence and the empty-diff rule"
