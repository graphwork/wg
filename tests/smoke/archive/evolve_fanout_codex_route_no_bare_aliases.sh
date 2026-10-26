#!/usr/bin/env bash
# Scenario: evolve_fanout_codex_route_no_bare_aliases
#
# Regression for bug-evolve-run-bypasses-codex-route:
# `wg evolve run --force-fanout` was hard-coding analyzer / synthesizer /
# apply / evaluate task models to bare anthropic aliases (`sonnet`, `haiku`,
# `opus`) regardless of the project's `[tiers]` and `[models.evolver]`
# config. On a codex-routed repo (`wg init --route codex-cli` or
# `wg config -m codex:gpt-5.5`) the resulting tasks dispatched through the
# claude CLI handler — even though `wg config --show` reported codex
# everywhere.
#
# This scenario drives the actual CLI to verify the fix:
#   1. Init the codex-cli route (codex tiers, codex evolver model).
#   2. Seed agency data so `wg evolve run` has roles + tradeoffs to act on.
#   3. Force fanout via `--force-fanout` (pre-set the eval threshold) and
#      ask for JSON.
#   4. Assert every task model in the resulting graph is codex-prefixed
#      and no bare anthropic alias survived.
#
# Pure config-and-graph-shape test: no LLM is invoked. The fanout creates
# the task graph synchronously and `wg list` reads it back.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

scratch=$(make_scratch)
fake_home="$scratch/home"
mkdir -p "$fake_home/.config/workgraph"
: >"$fake_home/.config/workgraph/config.toml"

run_wg() {
    env -u WG_EXECUTOR_TYPE -u WG_MODEL -u WG_TIER -u WG_AGENT_ID -u WG_TASK_ID \
        HOME="$fake_home" XDG_CONFIG_HOME="$fake_home/.config" \
        wg "$@"
}

cd "$scratch"

# ── Init codex-cli route ──────────────────────────────────────────────
if ! run_wg init --route codex-cli >init.log 2>&1; then
    loud_fail "wg init --route codex-cli failed: $(tail -10 init.log)"
fi

# Sanity: agency primitives exist for evolve to chew on.
if ! run_wg agency init >agency_init.log 2>&1; then
    loud_fail "wg agency init failed: $(tail -10 agency_init.log)"
fi

# ── Seed evaluations so fanout has signal ─────────────────────────────
# Fanout runs unconditionally when `--force-fanout` is passed, but the
# partition phase still needs roles + tradeoffs to filter against — both
# are seeded by `agency init`.

# ── Run evolve fanout ─────────────────────────────────────────────────
if ! run_wg evolve run --force-fanout --json >evolve.log 2>err.log; then
    loud_fail "wg evolve run --force-fanout failed.
stdout: $(cat evolve.log)
stderr: $(cat err.log)"
fi

# ── Assert no bare anthropic alias and no claude:* in fanout output ──
out=$(cat evolve.log)

if grep -qE '"model":[[:space:]]*"(sonnet|haiku|opus)"' <<<"$out"; then
    loud_fail "fanout output contains bare anthropic alias for model. Output:
$out"
fi

if grep -qE '"model":[[:space:]]*"claude:' <<<"$out"; then
    loud_fail "fanout output contains a claude: model on a codex-routed repo. Output:
$out"
fi

# Every analyzer slice's model must be codex-prefixed.
if ! grep -qE '"model":[[:space:]]*"codex:' <<<"$out"; then
    loud_fail "fanout output has no codex-prefixed model — codex routing was not honored. Output:
$out"
fi

# coord_model (synthesize/apply/evaluate) must be codex-prefixed too.
if ! grep -qE '"coord_model":[[:space:]]*"codex:' <<<"$out"; then
    loud_fail "fanout coord_model is not codex-prefixed. Output:
$out"
fi

# ── Confirm via the on-disk graph that every evolve task carries codex ──
# `wg list` hides dot-prefixed system tasks and has no JSON mode, so read
# the canonical graph file directly. The fanout writes synchronously.
graph_path="$scratch/.wg/graph.jsonl"
if [[ ! -f "$graph_path" ]]; then
    graph_path="$scratch/.workgraph/graph.jsonl"
fi
if [[ ! -f "$graph_path" ]]; then
    loud_fail "no graph.jsonl found under $scratch after evolve run"
fi

evolve_models=$(python3 - "$graph_path" <<'PY'
import json, sys
out = []
with open(sys.argv[1]) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        node = json.loads(line)
        # graph.jsonl wraps tasks in a "task" envelope.
        if "task" in node:
            t = node["task"]
        elif node.get("kind") == "task":
            t = node
        else:
            continue
        tid = t.get("id", "")
        if not tid.startswith(".evolve-"):
            continue
        if "evolve-partition" in tid:
            continue  # pre-completed, no LLM
        out.append(f"{tid} {t.get('model') or '<unset>'}")
print("\n".join(out))
PY
)

if [[ -z "$evolve_models" ]]; then
    loud_fail "no .evolve-* tasks found in $graph_path after fanout. graph file:
$(head -50 "$graph_path")"
fi

while IFS=' ' read -r tid model; do
    [[ -z "$tid" ]] && continue
    if [[ "$model" != codex:* ]]; then
        loud_fail "evolve task $tid has model='$model' (expected codex:* for codex-routed repo)"
    fi
    if [[ "$model" == sonnet || "$model" == haiku || "$model" == opus ]]; then
        loud_fail "evolve task $tid has bare anthropic alias '$model'"
    fi
done <<<"$evolve_models"

echo "PASS: wg evolve run --force-fanout honored codex-cli routing for every task"
echo "evolve task models:"
echo "$evolve_models" | sed 's/^/  /'
exit 0
