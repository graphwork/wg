#!/usr/bin/env bash
# Scenario: completion_legacy_environment_drift_renewal
#
# Pins this week's legacy environment-drift fix end to end:
#   1. A Land candidate's validation evidence is captured by the worker in one
#      process; the LandingPending finalization re-validates it in ANOTHER
#      process (the operator `wg resume`). Modern evidence carries the stable
#      environment identity, so the cross-process re-validation succeeds
#      without any renewal (positive control for the stable-identity fix).
#   2. The same evidence downgraded to the legacy v1 shape (the
#      `environment_stable_identity` projection stripped, exactly like
#      historical captures) must FAIL strict re-validation with the
#      environment-drift classification, and Done must then complete through
#      the sanctioned renewal (src/commands/completion_done.rs
#      renew_environment_drifted_validation): every configured command plus the
#      Land baseline re-run in the current environment at the integrated
#      commit, fresh captures stored + ledger-recorded, and the original
#      evidence re-verified under TolerantDrift.
#
# Reviewers are deterministic credential-free stubs; no live model call.
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

cat >"$fakebin/pi" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
: "${FAKE_REVIEW_STATE:?}"
if [[ "$*" == *"--list-models"* ]]; then
  printf 'provider model\ntest fake-review\n'
  exit 0
fi
input=$(cat || true)
n=$(($(cat "${FAKE_REVIEW_STATE}.count" 2>/dev/null || echo 0)+1))
printf '%s\n' "$n" >"${FAKE_REVIEW_STATE}.count"
FAKE_PROMPT="$input" python3 - <<'PY'
import json, os
prompt = os.environ.get("FAKE_PROMPT", "")
if "worksgood-flip-blind-inference-v1" in prompt:
    text = json.dumps({"goal": "reconstruct the requested deliverable",
                       "constraints": [], "invariants": [], "failure_modes": []},
                      separators=(",", ":"))
else:
    text = json.dumps({"verdict": "pass", "findings": []}, separators=(",", ":"))
print(json.dumps({"type": "turn_end", "message": {"role": "assistant", "content": [
    {"type": "text", "text": text}], "provider": "test", "model": "fake-review",
    "stopReason": "stop", "usage": {"input": 2, "output": 1, "cacheRead": 0,
    "cacheWrite": 0, "totalTokens": 3, "cost": {"total": 0.0001}}}},
    separators=(",", ":")))
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
git config user.email drift@test.invalid
git config user.name Drift
echo base > base.txt
git add base.txt && git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
git add .gitignore AGENTS.md CLAUDE.md && git commit -qm init-wg
wgrun(){ env -u WG_TASK_ID -u WG_AGENT_ID -u WG_GRAPH_ID WG_DIR="$repo/.wg" "$WG_BIN" "$@"; }

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

# ── Shared flow: capture evidence in the worker process, park at LandingPending,
#    finalize from the operator process. ────────────────────────────────────
park_landing_task() {
  local id="$1"
  wgrun add "Legacy drift probe $id" --id "$id" \
    --validation-command "test -s result-$id.txt" \
    -d $'Produce the validated result file.\n\n## Validation\n- [ ] exact deterministic command passes' >/dev/null
  wgrun publish "$id" --only >/dev/null
  wgrun claim "$id" --actor drift-worker >/dev/null
  # The worker owns a clean detached worktree; the INTEGRATION checkout is
  # deliberately dirty, so the reviewed candidate parks at LandingPending and
  # the source worker is released — exactly the retained-candidate state the
  # finalizer reconciles.
  printf 'in-flight operator bytes\n' >> base.txt
  [[ -n "$(git status --porcelain)" ]] || loud_fail "$id fixture produced no park dirtiness"
  git worktree add -q "$scratch/wt-$id" -b "worker/$id" refs/heads/main
  (
    cd "$scratch/wt-$id" \
      && printf 'validated\n' > "result-$id.txt" \
      && git add "result-$id.txt" && git commit -qm "$id" \
      && env WG_TASK_ID="$id" WG_AGENT_ID=drift-worker \
         "$WG_BIN" --dir "$repo/.wg" done "$id" \
         >"$scratch/$id.done.out" 2>"$scratch/$id.done.err"
  ) || loud_fail "worker one-shot completion for $id failed: $(cat "$scratch/$id.done.err")"
  local blocker
  blocker=$(wgrun show "$id" --json | python3 -c 'import json,sys; x=json.load(sys.stdin); b=x.get("completion_blocker") or {}; print(b.get("kind",""))')
  [[ "$blocker" == "landing-pending" ]] \
    || loud_fail "$id did not park at LandingPending (blocker=$blocker): $(cat "$scratch/$id.done.err")"
  # The operator resolves the deliberate dirtiness; the retained worker
  # worktree stays in place for the finalizer.
  git restore base.txt
  [[ -z "$(git status --porcelain)" ]] || loud_fail "$id checkout not clean before finalization"
}

# ── 1. Positive control: modern stable-identity evidence re-validates across
#      the process boundary without renewal. ────────────────────────────────
park_landing_task "modern-evidence"
review_count_before=$(cat "${FAKE_REVIEW_STATE}.count" 2>/dev/null || echo 0)
if ! wgrun resume modern-evidence --only >"$scratch/modern.resume.out" 2>"$scratch/modern.resume.err"; then
  loud_fail "modern-evidence cross-process finalization failed: $(cat "$scratch/modern.resume.err")"
fi
if grep -q 'validation evidence renewed' "$scratch/modern.resume.err"; then
  loud_fail "modern evidence was needlessly renewed: $(cat "$scratch/modern.resume.err")"
fi
wgrun show modern-evidence --json >"$scratch/modern.json"
python3 - "$scratch/modern.json" <<'PY'
import json, sys
x = json.load(open(sys.argv[1]))
assert x['status'] == 'done' and x['completion_disposition'] == 'landed', x
rows = x['completion_review_activity']
assert [(r['reviewer_kind'], r['verdict']) for r in rows] == [('flip', 'pass'), ('eval', 'pass')], rows
PY
# No extra model calls: the finalization reused the exact reviewed candidate.
review_count_after=$(cat "${FAKE_REVIEW_STATE}.count" 2>/dev/null || echo 0)
[[ "$review_count_after" == "$review_count_before" ]] \
  || loud_fail "finalization re-ran model review ($review_count_before -> $review_count_after)"

echo "PASS phase A: stable-identity evidence re-validated cross-process without renewal"

# ── 2. Legacy downgrade: strip the stable environment projection from the
#      captured configured evidence (the exact historical v1 shape), keeping
#      every content-addressed binding consistent (evidence object, create-once
#      capture authority, manifest, review receipt, graph candidate refs).
#      Review policy is advisory with an unavailable FLIP lane so the parked
#      candidate exercises the deterministic validation layer only.
# Flip the reviewer stub into its transport-broken mode for the legacy task.
printf 'broken\n' >"${FAKE_REVIEW_STATE}.mode"

# Advisory policy: deterministic evidence stays authoritative, semantic review
# availability is not a completion gate (the unavailable lane stays surfaced).
# The FLIP lane's routes point at a transport-broken fixture model (rejected
# by the stub above through FAKE_REVIEW_STATE.mode) so the parked candidate
# exercises the deterministic validation layer only.
wgrun config --local --set-model flip_inference pi:test:broken-model \
  --set-reasoning flip_inference low \
  --set-model flip_comparison pi:test:broken-model \
  --set-reasoning flip_comparison low --no-reload >/dev/null
python3 - "$repo/worksgood.toml" <<'PY'
import pathlib, re, sys
path = pathlib.Path(sys.argv[1]); text = path.read_text()
text = re.sub(r'^completion_review_strict\s*=.*$', 'completion_review_strict = false', text, flags=re.M)
path.write_text(text)
PY
git add worksgood.toml && git commit -qm advisory-policy-for-legacy-probe

park_landing_task "legacy-evidence"

# The worker's FLIP lane was deliberately unavailable; the candidate parked
# with exactly one unavailable FLIP receipt and no Eval.
wgrun show legacy-evidence --json >"$scratch/legacy.parked.json"
python3 - "$scratch/legacy.parked.json" <<'PY'
import json, sys
x = json.load(open(sys.argv[1])); c = x['completion_candidate']
rows = [r for r in x['completion_review_activity'] if r['candidate_state'] == 'current']
assert [(r['reviewer_kind'], r['verdict']) for r in rows] == [('flip', 'unavailable')], rows
assert c['flip_receipt'] and not c.get('eval_receipt'), c.keys()
PY

# Downgrade the captured configured evidence to the legacy v1 shape and keep
# the whole content-addressed chain consistent.
python3 - "$repo/.wg" "legacy-evidence" "$WG_BIN" <<'PY'
import json, pathlib, subprocess, sys, tempfile

wg = pathlib.Path(sys.argv[1]); task_id = sys.argv[2]; wg_bin = sys.argv[3]
objects = wg / "completion" / "v3" / "objects"
authority = wg / "completion" / "v3" / "validation-authority"

def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()

def read_object(ref):
    return (objects / ref["content_digest"].removeprefix("b3:")).read_bytes()

# WG itself computes every content digest and stores every new immutable
# object; this fixture never re-implements the hash.
def store_object(data, media_type):
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as handle:
        handle.write(data); path = pathlib.Path(handle.name)
    result = subprocess.run(
        [wg_bin, "--dir", str(wg), "completion-object", str(path),
         "--media-type", media_type],
        capture_output=True, text=True, check=True)
    ref = json.loads(result.stdout)
    return ref

# Locate the task row and its selected candidate.
rows = [json.loads(line) for line in (wg / "graph.jsonl").read_text().splitlines()]
task = next(r for r in rows if r.get("id") == task_id)
candidate = task["completion_candidate"]
manifest_ref = candidate["manifest"]
manifest = json.loads(read_object(manifest_ref))

# Downgrade the captured configured evidence: strip the stable environment
# projection exactly like historical v1 captures.
entry = next(e for e in manifest["validation_evidence"]
             if e["evidence_kind"] == "deterministic-validation/configured/v1")
evidence = json.loads(read_object(entry))
env = evidence["environment"]
assert env.get("environment_stable_identity") is not None, "fixture expected modern evidence"
del env["environment_stable_identity"]
e2 = store_object(canonical(evidence), entry["media_type"])
e2_digest = e2["content_digest"]

# Re-derive the create-once capture authority for the downgraded object from
# its own fields (the authority covers lifecycle/command/repository binding,
# never the environment projection).
auth = {
    "authority_version": 1,
    "evidence_digest": e2_digest,
    "task_id": evidence["lifecycle"]["task_id"],
    "requirements_digest": evidence["lifecycle"]["requirements_digest"],
    "generation": evidence["lifecycle"]["generation"],
    "attempt_id": evidence["lifecycle"].get("attempt_id"),
    "attempt_fence": evidence["lifecycle"]["attempt_fence"],
    "command_digest": evidence["command"]["command_digest"],
    "repository_identity": evidence["repository"]["repository_identity"],
    "source_revision": evidence["repository"]["before_head_oid"],
    "source_tree": evidence["repository"]["before_tree_oid"],
}
authority.mkdir(parents=True, exist_ok=True)
(authority / e2_digest.removeprefix("b3:")).write_bytes(canonical(auth))

# Rebind the manifest to the downgraded evidence object.
evidence_kind = entry["evidence_kind"]
entry.clear()
entry.update({"content_digest": e2_digest,
              "evidence_kind": evidence_kind,
              "immutable_locator": {"kind": "completion_object", "digest": e2_digest},
              "media_type": e2["media_type"], "size": e2["size"]})
m2 = store_object(canonical(manifest), "application/vnd.worksgood.completion+json")
m2_digest = m2["content_digest"]

# Rebind the review receipt(s) to the new manifest digest.
new_receipts = {}
for key in ("flip_receipt", "eval_receipt"):
    ref = candidate.get(key)
    if not ref:
        continue
    receipt = json.loads(read_object(ref))
    assert receipt["manifest_digest"] == manifest_ref["content_digest"]
    receipt["manifest_digest"] = m2_digest
    stored = store_object(canonical(receipt), ref["media_type"])
    new_receipts[key] = {"content_digest": stored["content_digest"],
                         "immutable_locator": {"kind": "completion_object",
                                               "digest": stored["content_digest"]},
                         "media_type": ref["media_type"], "size": stored["size"]}

# The persisted FIFO landing-turn lease binds the exact candidate digest; the
# rebound candidate must replace it there too.
old_candidate = manifest_ref["content_digest"]
for state in (wg / "landing-turns").glob("*.json"):
    body = state.read_text()
    if old_candidate in body:
        patched = json.loads(body)

        def walk(node):
            if isinstance(node, dict):
                for key, value in node.items():
                    if key == "candidate_oid" and value == old_candidate:
                        node[key] = m2_digest
                    else:
                        walk(value)
            elif isinstance(node, list):
                for item in node:
                    walk(item)

        walk(patched)
        state.write_text(json.dumps(patched, indent=2) + "\n")

# Rewrite the mutable graph projection: only this task row changes. The
# LandingPending blocker embeds the exact candidate snapshot it froze, so the
# rebound candidate must replace it in both places.
def rebound_candidate(cand):
    cand = dict(cand)
    cand["manifest"] = {"content_digest": m2_digest,
                        "immutable_locator": {"kind": "completion_object",
                                              "digest": m2_digest},
                        "size": m2["size"]}
    for key, ref in new_receipts.items():
        cand[key] = ref
    return cand

out = []
for row in rows:
    if row is task:
        row = dict(row)
        row["completion_candidate"] = rebound_candidate(candidate)
        blocker = row.get("completion_blocker")
        if blocker and "candidate" in blocker:
            row["completion_blocker"] = dict(blocker, candidate=rebound_candidate(blocker["candidate"]))
    out.append(json.dumps(row, separators=(",", ":")))
(wg / "graph.jsonl").write_text("\n".join(out) + "\n")
print("legacy downgrade applied: evidence=%s manifest=%s" % (e2_digest, m2_digest))
PY

# The Done-step finalization re-validates the legacy evidence strictly in the
# operator process, fails closed on the environment binding, renews every
# capture in the current environment, and converges.
if ! wgrun resume legacy-evidence --only >"$scratch/legacy.resume.out" 2>"$scratch/legacy.resume.err"; then
  loud_fail "legacy-evidence finalization failed: $(cat "$scratch/legacy.resume.err")"
fi
grep -q 'validation evidence renewed in the current environment' "$scratch/legacy.resume.err" \
  || loud_fail "legacy drift renewal was not performed: $(cat "$scratch/legacy.resume.err")"

wgrun show legacy-evidence --json >"$scratch/legacy.json"
python3 - "$scratch/legacy.json" "$repo/.wg" <<'PY'
import json, pathlib, sys
x = json.load(open(sys.argv[1])); wg = pathlib.Path(sys.argv[2])
objects = wg / "completion" / "v3" / "objects"
assert x['status'] == 'done' and x['completion_disposition'] == 'landed', x
# Historical (superseded) receipts stay visible; the current candidate's
# current-state rows carry the unavailable FLIP and no Eval.
rows = [r for r in x['completion_review_activity'] if r['candidate_state'] == 'current']
assert [(r['reviewer_kind'], r['verdict']) for r in rows] == [('flip', 'unavailable')], rows
receipt = json.loads((objects / x['completion_receipt'].removeprefix('b3:')).read_text())
assert receipt['review_policy'] == 'advisory', receipt
assert receipt['semantic_outcome'] == 'unavailable', receipt
# Renewed captures were recorded: one extra Configured + one extra Baseline
# capture beyond the worker's originals, each binding the same task.
captures = [m for m in x['log'] if 'Captured deterministic validation purpose=' in m.get('message', '')]
purposes = [m['message'].split('purpose=', 1)[1].split(' ', 1)[0] for m in captures]
assert purposes.count('Configured') == 2, purposes
assert purposes.count('Baseline') == 2, purposes
# The reviewed manifest binds the downgraded (legacy) evidence object.
manifest = json.loads((objects / x['completion_candidate']['manifest']['content_digest'].removeprefix('b3:')).read_text())
configured = next(e for e in manifest['validation_evidence']
                  if e['evidence_kind'] == 'deterministic-validation/configured/v1')
legacy = json.loads((objects / configured['content_digest'].removeprefix('b3:')).read_text())
assert 'environment_stable_identity' not in legacy['environment'], legacy['environment']
assert legacy['capture_origin'] == 'wg_done', legacy
PY

# The renewed evidence objects must carry a fresh stable environment identity
# (they were captured by the current binary in the current environment).
python3 - "$scratch/legacy.json" "$repo/.wg" <<'PY'
import json, pathlib, sys
x = json.load(open(sys.argv[1])); wg = pathlib.Path(sys.argv[2])
objects = wg / "completion" / "v3" / "objects"
log = [m['message'] for m in x['log'] if 'Captured deterministic validation purpose=' in m.get('message', '')]
digests = [m.split('evidence=', 1)[1].split(' ', 1)[0] for m in log]
stable = 0
for d in set(digests):
    body = json.loads((objects / d.removeprefix('b3:')).read_text())
    env = body.get('environment') or {}
    if env.get('environment_stable_identity'):
        stable += 1
assert stable >= 2, "renewed captures lack the stable environment identity: %r" % digests
PY

echo "PASS: legacy v1 evidence failed strict cross-process re-validation; Done-step renewal re-captured configured+baseline evidence and completed"
