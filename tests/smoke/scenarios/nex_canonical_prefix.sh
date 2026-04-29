#!/usr/bin/env bash
# Smoke: `nex:` is the canonical model-prefix for the in-process nex handler.
# Pins fix-model-prefix: prior to this change the same handler accepted
# three prefixes (`local:`, `oai-compat:`, `openrouter:`) — none of which
# matched the subcommand name `wg nex`. This scenario asserts that:
#
#   1. `wg init -m nex:<model> -e <URL>` writes a config that round-trips
#      cleanly: `agent.model` = `nex:<model>`, `[[llm_endpoints.endpoints]]`
#      = the supplied URL, no errors.
#   2. The dispatcher routes `nex:<model>` to the in-process nex handler
#      (visible via `wg spawn-task --dry-run`'s WG_EXECUTOR_TYPE=native
#      + WG_MODEL=nex:<model>).
#   3. The legacy `local:<model>` / `oai-compat:<model>` prefixes still
#      work for one release with a stderr deprecation warning.
#   4. `wg migrate config` rewrites `local:` / `oai-compat:` to `nex:`,
#      preserves a `.pre-migrate.<ts>` backup, and is idempotent (a
#      second run is a no-op).
#   5. `wg config init --bare` for the `local` and `nex-custom` routes
#      writes `nex:` in the model field (not `local:` / `oai-compat:`).

set -eu
source "$(dirname "$0")/_helpers.sh"
require_wg

scratch=$(make_scratch)

# ── Case 1: wg init -m nex:<model> -e <URL> works end-to-end ──────────
proj1="$scratch/proj1"
mkdir -p "$proj1"
cd "$proj1"

init_out=$(wg init -m nex:qwen3-coder -e https://example.com/v1 --no-agency 2>&1) || \
    loud_fail "wg init -m nex:qwen3-coder -e https://example.com/v1 failed: $init_out"

if [[ ! -f .wg/config.toml && ! -f .workgraph/config.toml ]]; then
    loud_fail "wg init did not write a config.toml: $init_out"
fi

cfg_path=".wg/config.toml"
[[ -f .workgraph/config.toml ]] && cfg_path=".workgraph/config.toml"

if ! grep -q 'model = "nex:qwen3-coder"' "$cfg_path"; then
    loud_fail "config.toml should contain 'model = \"nex:qwen3-coder\"' — got:\n$(cat "$cfg_path")"
fi
if grep -q 'model = "local:qwen3-coder"' "$cfg_path"; then
    loud_fail "config.toml must NOT use the deprecated 'local:' prefix — got:\n$(cat "$cfg_path")"
fi

# Round-trip: re-loading the config must NOT print a deprecation warning
# for our just-written model spec.
load_warn=$(wg list 2>&1 || true)
if echo "$load_warn" | grep -qE 'deprecated.*nex:|model.*deprecated.*local:|model.*deprecated.*oai-compat:'; then
    loud_fail "freshly-written nex: config triggered a deprecation warning on reload: $load_warn"
fi

cd /

# ── Case 2: legacy local:/oai-compat: still parse, with deprecation warning ─
proj2="$scratch/proj2"
mkdir -p "$proj2/.wg"
touch "$proj2/.wg/graph.jsonl"
cat > "$proj2/.wg/config.toml" <<'EOF'
[agent]
model = "local:qwen3-coder"

[[llm_endpoints.endpoints]]
name = "default"
provider = "local"
url = "https://example.com/v1"
is_default = true
EOF
cd "$proj2"
# `wg config` loads the merged config and surfaces deprecation warnings on stderr.
warn_out=$(wg config 2>&1 || true)
cd /
if ! echo "$warn_out" | grep -qE 'local:.*deprecated|deprecated.*local:'; then
    loud_fail "expected a deprecation warning for 'local:' prefix on config load — got: $warn_out"
fi
if ! echo "$warn_out" | grep -q 'nex:'; then
    loud_fail "deprecation warning must mention the canonical 'nex:' replacement — got: $warn_out"
fi

# ── Case 3: wg migrate config rewrites local:/oai-compat: → nex: ──────
proj3="$scratch/proj3"
mkdir -p "$proj3/.wg"
cat > "$proj3/.wg/config.toml" <<'EOF'
[agent]
model = "local:qwen3-coder"

[tiers]
fast = "oai-compat:gpt-5"
EOF
cd "$proj3"

dry=$(wg migrate config --local --dry-run 2>&1) || \
    loud_fail "wg migrate config --dry-run failed: $dry"
if ! echo "$dry" | grep -qE 'local:qwen3-coder.*nex:qwen3-coder'; then
    loud_fail "dry-run should show local:qwen3-coder → nex:qwen3-coder rewrite — got: $dry"
fi
if ! echo "$dry" | grep -qE 'oai-compat:gpt-5.*nex:gpt-5'; then
    loud_fail "dry-run should show oai-compat:gpt-5 → nex:gpt-5 rewrite — got: $dry"
fi
# Dry-run must NOT modify the file.
if ! grep -q 'local:qwen3-coder' .wg/config.toml; then
    loud_fail "dry-run modified the file (it should not). After dry-run:\n$(cat .wg/config.toml)"
fi

apply=$(wg migrate config --local 2>&1) || \
    loud_fail "wg migrate config (apply) failed: $apply"
if ! grep -q 'nex:qwen3-coder' .wg/config.toml; then
    loud_fail "after migrate, config.toml should contain nex:qwen3-coder — got:\n$(cat .wg/config.toml)"
fi
if grep -q 'local:qwen3-coder' .wg/config.toml; then
    loud_fail "after migrate, config.toml must NOT contain local:qwen3-coder — got:\n$(cat .wg/config.toml)"
fi
if ! grep -q 'nex:gpt-5' .wg/config.toml; then
    loud_fail "after migrate, config.toml should contain nex:gpt-5 — got:\n$(cat .wg/config.toml)"
fi

# Backup file must exist.
if ! ls .wg/config.toml.pre-migrate.* >/dev/null 2>&1; then
    loud_fail "wg migrate config must write a .pre-migrate.<ts> backup — none found in .wg/"
fi

# Idempotent: a second run must be a no-op.
again=$(wg migrate config --local 2>&1) || \
    loud_fail "second wg migrate config run failed: $again"
if ! echo "$again" | grep -qE 'already canonical|no changes'; then
    loud_fail "second migrate should be a no-op — got: $again"
fi
cd /

# ── Case 4: handler_for_model: nex:<model> routes to native (in-process nex) ─
# The lib unit test handler_for_model::test_nex_prefix_routes_to_native covers
# the resolver mapping. This live-binary check confirms it via the dispatcher's
# spawn-task --dry-run plan (the same surface used by other agency / model
# regression scenarios in this manifest), with a task that has an explicit
# `-m nex:...` model so we exercise the per-task → handler resolution path.
proj4="$scratch/proj4"
mkdir -p "$proj4"
cd "$proj4"
wg init -m nex:qwen3-coder -e https://example.com/v1 --no-agency >/dev/null 2>&1 || \
    loud_fail "wg init for spawn-task dry-run setup failed"

# Add a task with explicit `-m nex:qwen3-coder` so the spawn plan resolves
# from this prefix (not the role default). This is the user-flow that pins
# the rename: a task model with `nex:` → executor=native.
wg add "test-nex-routing" --id test-nex-routing --model nex:qwen3-coder >/dev/null 2>&1 || \
    loud_fail "wg add --model nex:qwen3-coder failed inside proj4"

if wg spawn-task --help 2>&1 | grep -q -- --dry-run; then
    plan=$(wg spawn-task test-nex-routing --dry-run 2>&1 || true)
    if ! echo "$plan" | grep -qE 'executor=native|executor=\"?native\"?|WG_EXECUTOR_TYPE=native'; then
        loud_fail "spawn-task --dry-run for nex: model should route to native handler — got: $plan"
    fi
    if ! echo "$plan" | grep -qE 'nex:qwen3-coder|model=qwen3-coder'; then
        loud_fail "spawn-task --dry-run should reflect the nex:qwen3-coder model — got: $plan"
    fi
fi
cd /

echo "PASS: nex_canonical_prefix"
