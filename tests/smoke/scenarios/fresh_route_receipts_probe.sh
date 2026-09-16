#!/usr/bin/env bash
# Scenario: fresh_route_receipts_probe
#
# Pins the fresh-route hardening end to end on the DEFAULT (non-experimental)
# configuration: a worker task dispatched on the project routes
# `pi:lunaroute:glm-5.3-flash` (strong/task) with the agency weak tier
# `pi:lunaroute:deepseek-4.1-flash` (reviewer/evaluator/FLIP roles) completes
# with route receipts intact.
#
# The lunaroute provider is opaque to WG: `pi` owns provider:model resolution,
# so WG must pass the exact inner dialect through unsplit and unnormalized
# (the pi CLI receives its native --provider/--model pair with the byte-exact
# suffix; WG never consults its registry and never rewrites the route), and
# every receipt must carry the byte-exact handler-first route.
# Everything runs against a fake `pi` stub — deterministic and credential-free.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# Keep the daemon's AF_UNIX socket below sun_path even with deep build trees.
export WG_SMOKE_ROOT="${WG_SMOKE_ROOT:-/tmp/wgsmoke}"
. "$HERE/_helpers.sh"
command -v git >/dev/null 2>&1 || loud_skip "MISSING GIT" "git is required"
command -v python3 >/dev/null 2>&1 || loud_skip "MISSING PYTHON3" "python3 is required"

scratch=$(make_scratch)
project="$scratch/project"; home="$scratch/home"; fakebin="$scratch/fakebin"
calls="$scratch/pi-calls"
mkdir -p "$project" "$home/.config" "$fakebin" "$calls"
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

# The fake Pi binary records every invocation and behaves by selected model:
#  - the strong worker model performs the task (artifact + wg done),
#  - the weak agency models answer the deterministic review protocol
#    (FLIP phase-I blind reconstruction, then pass verdicts),
#  - the --offline capability probe answers with a bounded model table.
cat >"$fakebin/pi" <<'FAKE_PI'
#!/usr/bin/env bash
set -euo pipefail
n=1
while [[ -e "$FAKE_PI_CALLS/$n.argv" ]]; do n=$((n+1)); done
printf '%s\0' "$@" >"$FAKE_PI_CALLS/$n.argv"
input=$(cat || true)
model=""; previous=""
for arg in "$@"; do
  [[ "$previous" == "--model" || "$previous" == "--list-models" ]] && model="$arg"
  previous="$arg"
done
# The default (non-opaque) wrapper passes the pi route's inner dialect as the
# pi CLI's native --provider/--model pair; the suffix stays byte-exact.
if [[ " $* " == *' --provider lunaroute '* ]]; then model="lunaroute:$model"; fi
if [[ " $* " == *' --offline '* || "$*" == *'--list-models'* ]]; then
  printf 'Provider Model Context Window Max Output\n'
  printf 'lunaroute glm-5.3-flash 1 1\n'
  printf 'lunaroute deepseek-4.1-flash 1 1\n'
  exit 0
fi
FAKE_PROMPT="$input" python3 - "$model" <<'PY'
import json, os, sys
model = sys.argv[1]
prompt = os.environ.get("FAKE_PROMPT", "")
if model == "lunaroute:glm-5.3-flash":
    # The worker does real work and completes through the ordinary CLI.
    import pathlib, subprocess
    pathlib.Path("route-probe-out.txt").write_text("route receipt intact\n")
    env = dict(os.environ)
    subprocess.run(["wg", "artifact", env["WG_TASK_ID"], "route-probe-out.txt"],
                   check=True, capture_output=True)
    subprocess.run(["wg", "done", env["WG_TASK_ID"]], check=True)
    text = "worker finished with route receipts intact"
else:
    if "worksgood-flip-blind-inference-v1" in prompt:
        text = json.dumps({"goal": "reconstruct the requested artifact",
                           "constraints": [], "invariants": [], "failure_modes": []},
                          separators=(",", ":"))
    else:
        text = json.dumps({"verdict": "pass", "findings": []}, separators=(",", ":"))
print(json.dumps({"type": "turn_end", "message": {"role": "assistant", "content": [
    {"type": "text", "text": text}], "provider": "lunaroute", "model": model,
    "stopReason": "stop", "usage": {"input": 2, "output": 1, "cacheRead": 0,
    "cacheWrite": 0, "totalTokens": 3, "cost": {"total": 0.0001}}}},
    separators=(",", ":")))
PY
FAKE_PI
chmod +x "$fakebin/pi"

export HOME="$home" XDG_CONFIG_HOME="$home/.config" WG_GLOBAL_DIR="$home/.wg"
export PATH="$fakebin:$PATH" FAKE_PI_CALLS="$calls"
unset WG_DIR WG_TASK_ID WG_AGENT_ID WG_GRAPH_ID WG_PROJECT_ROOT WG_WORKTREE_PATH \
  WG_WORKTREE_ACTIVE WG_BRANCH WG_WORKER_ATTEMPT_ID WG_WORKER_ATTEMPT_FENCE \
  WG_WORKER_GENERATION WG_SPAWN_EPOCH WG_SPAWN_RUN_ID WG_WORKER_CONTROL_MODE || true

cd "$project"
git init -q -b main
git config user.email route-probe@test.invalid
git config user.name RouteProbe
printf 'base\n' >base.txt
git add base.txt && git commit -qm base
"$WG_BIN" init --no-agency >/dev/null
G="$project/.wg"
wgrun(){ (cd "$project" && env -u WG_TASK_ID -u WG_AGENT_ID -u WG_GRAPH_ID "$WG_BIN" --dir "$G" "$@"); }

# The fresh project routes: strong/task route for the worker, weak tier for
# every agency one-shot role (reviewer/FLIP/eval resolve through tiers.fast).
wgrun config --local --model pi:lunaroute:glm-5.3-flash --reasoning low \
  --auto-assign false --auto-evaluate false --no-reload >/dev/null
python3 - "$project/worksgood.toml" <<'PY'
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
# Every agency one-shot role rides the weak tier; Pi routes additionally
# require effective reasoning on each role before admission.
wgrun config --local --tier fast=pi:lunaroute:deepseek-4.1-flash \
  --set-reasoning evaluator low --set-reasoning flip_inference low \
  --set-reasoning flip_comparison low --set-reasoning assigner low \
  --set-reasoning reviewer low --no-reload >/dev/null
wgrun config set dispatcher.max_agents 1 >/dev/null
wgrun config set dispatcher.poll_interval 1 >/dev/null
wgrun config set dispatcher.settling_delay_ms 0 >/dev/null
wgrun config set dispatcher.worktree_isolation false >/dev/null
wgrun config set dispatcher.resource_management.disk_sentinel_enabled false >/dev/null
git add worksgood.toml && git commit -qm route-fixture

wgrun add "Fresh-route probe task" --id route-probe \
  -d $'Produce route-probe-out.txt.\n\n## Validation\n- [ ] route receipts stay byte-exact' >/dev/null
wgrun contract route-probe report >/dev/null
wgrun publish route-probe --only >/dev/null
start_wg_daemon "$project" --no-chat-agent --interval 1
rm -f "$project/daemon.log"

# Dispatch happens on the daemon tick; the fake worker completes the task.
probe=''
for _ in $(seq 1 400); do
  status=$(wgrun show route-probe --json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
  [[ "$status" == "done" ]] && probe=1 && break
  sleep .05
done
if [[ -z "$probe" ]]; then
  defer=$(wgrun service status --json 2>/dev/null | python3 -c 'import json,sys; c=json.load(sys.stdin).get("coordinator",{}); print(json.dumps(c.get("admission_deferred")))' 2>/dev/null || true)
  task=$(wgrun show route-probe --json 2>/dev/null | python3 -c 'import json,sys; x=json.load(sys.stdin); print(json.dumps({"status":x.get("status"),"assigned":x.get("assigned"),"failure_reason":x.get("failure_reason"),"log":x.get("log",[])[-3:]}))' 2>/dev/null || true)
  calls=$(ls "$calls" 2>/dev/null | tr '\n' ' ' || true)
  daemon=$(grep -E 'WG-|admission|defer|Error|WARN|spawn|exit' "$G/service/daemon.log" 2>/dev/null | tail -8 || true)
  loud_fail "routed worker task did not complete: deferred=$defer task=$task pi-calls=[$calls] daemon-diag=$daemon"
fi

# Receipt 1: the exact launch argv. The route's inner dialect must arrive
# byte-exact and exactly once, with no WG registry lookup or normalization.
python3 - "$calls" <<'PY'
import glob, sys
argvs = []
for path in sorted(glob.glob(sys.argv[1] + "/*.argv")):
    argvs.append([a.decode() for a in open(path, "rb").read().split(b"\0") if a])
def models_of(a):
    return [a[i+1] for i, v in enumerate(a) if v == "--model"]
def providers_of(a):
    return [a[i+1] for i, v in enumerate(a) if v == "--provider"]

workers = [a for a in argvs if "--offline" not in a and models_of(a) == ["glm-5.3-flash"]]
assert len(workers) == 1, "expected exactly one worker invocation, got %d: %r" % (len(workers), argvs)
worker = workers[0]
assert providers_of(worker) == ["lunaroute"], worker
assert worker.count("--provider") == 1 and worker.count("glm-5.3-flash") == 1, worker
assert "--mode" in worker and worker[worker.index("--mode")+1] == "json", worker
assert worker[worker.index("--thinking")+1] == "low", worker
PY

# Receipt 2: review calls ran on the weak agency route.
python3 - "$calls" <<'PY'
import glob, sys
def models_of(a):
    return [a[i+1] for i, v in enumerate(a) if v == "--model"]
def providers_of(a):
    return [a[i+1] for i, v in enumerate(a) if v == "--provider"]

weak = 0
for path in sorted(glob.glob(sys.argv[1] + "/*.argv")):
    a = [x.decode() for x in open(path, "rb").read().split(b"\0") if x]
    if a and a[-1] != "--list-models" and "--offline" not in a:
        if (models_of(a) == ["deepseek-4.1-flash"]
                and providers_of(a) == ["lunaroute"]):
            weak += 1
assert weak >= 3, "expected >=3 weak-route review calls (FLIP inference, comparison, eval), got %d" % weak
PY

# Receipt 3: immutable review receipts + task attribution + agent registry.
wgrun show route-probe --json >"$scratch/done.json"
python3 - "$scratch/done.json" "$G" <<'PY'
import json, pathlib, sys
x = json.load(open(sys.argv[1])); g = pathlib.Path(sys.argv[2])
assert x['status'] == 'done', x['status']
# Executor-level attribution records the pi inner dialect byte-exact.
assert x.get('actual_model') == 'lunaroute:glm-5.3-flash', x.get('actual_model')
assert x.get('actual_executor') == 'pi', x.get('actual_executor')
rows = x['completion_review_activity']
assert [(r['reviewer_kind'], r['verdict']) for r in rows] == [('flip', 'pass'), ('eval', 'pass')], rows
flip_route = rows[0]['model_route']
assert 'prompt-reconstruction-two-phase-v2' in flip_route, flip_route
assert 'inference=pi:lunaroute:deepseek-4.1-flash' in flip_route, flip_route
assert 'comparison=pi:lunaroute:deepseek-4.1-flash' in flip_route, flip_route
eval_route = rows[1]['model_route']
assert eval_route == 'pi:lunaroute:deepseek-4.1-flash', eval_route
receipt = json.loads((g / 'completion' / 'v3' / 'objects' / x['completion_receipt'].removeprefix('b3:')).read_text())
assert receipt['review_policy'] == 'strict', receipt
assert receipt['semantic_outcome'] == 'approved', receipt
registry = json.loads((g / 'service' / 'registry.json').read_text())
agents = registry.get('agents') or registry
if isinstance(agents, dict):
    agents = list(agents.values())
probe_agents = [a for a in agents if a.get('task_id') == 'route-probe']
assert probe_agents, registry
agent = probe_agents[-1]
assert agent.get('executor') == 'pi', agent
assert agent.get('model') == 'lunaroute:glm-5.3-flash', agent
PY

# Receipt 4: the exact source-provider launch binding persisted before the
# launch gate released provider I/O records the byte-exact route.
python3 - "$G" <<'PY'
import json, pathlib, sys
g = pathlib.Path(sys.argv[1])
bindings = list(g.glob('**/source-provider-recovery/launch-binding.json'))
assert bindings, "no source-provider launch binding was persisted"
recorded = []
for path in bindings:
    binding = json.loads(path.read_text())
    if binding.get('task_id') == 'route-probe':
        recorded.append(binding)
assert recorded, bindings
binding = recorded[-1]
assert binding['executor'] == 'pi', binding
assert binding['model'] == 'lunaroute:glm-5.3-flash', binding
assert binding['exact_route'] == 'pi:lunaroute:glm-5.3-flash', binding
PY

echo "PASS: fresh pi:lunaroute routes dispatched byte-exact, the weak agency tier reviewed, and every receipt kept the exact routes"
