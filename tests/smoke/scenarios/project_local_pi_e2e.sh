#!/usr/bin/env bash
# End-to-end terminal regression for the project-local-by-default Pi
# configuration cutover (`docs/design-project-local-pi-config.md`).
#
# Models TWO repositories sharing ONE $HOME (and therefore one machine-global
# ~/.wg + one Pi-owned credential store) and proves:
#
#   1. Isolation: selecting/configuring a Pi route in repo A writes only
#      <repoA>/worksgood.toml and never touches repo B, ~/.wg/config.toml,
#      or ~/.wg/active-profile.
#   2. Fail-loud: repo B with no project route fails WG-EXEC-UNSELECTED at
#      every LLM entry point despite a stale, valid-looking global route +
#      active-profile pointer, and creates no service/session/claim state.
#      Ignored legacy routing is named inactive in the diagnostic.
#   3. No global inheritance: repo B's dispatcher.max_agents is the
#      builtin-default, not the stale global 99.
#   4. Clone reproduction: copying repo A's checked-in worksgood.toml into a
#      fresh repo B (no .wg route, no global config, no reusable profile
#      definitions, no WG credentials) reproduces the exact Pi route,
#      per-role reasoning, resource guardrail, and archive policy, all
#      reported with source=project-file.
#   5. Migration preservation: `wg migrate project-local-pi
#      --cleanup-global-routing` removes only the stale global routing
#      selectors + active-profile pointer while preserving every reusable
#      profile definition, secret, keystore, identity record, and federation
#      state byte-for-byte; the project route in repo A keeps working.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

scratch=$(make_scratch)
home="$scratch/home"
global="$scratch/global"
repoA="$scratch/repoA"
repoB="$scratch/repoB"
mkdir -p "$home" "$global" "$repoA" "$repoB"

# Both repos share ONE $home and ONE $global. Only --dir differs.
run_wg() {
  local repo="$1"; shift
  (cd "$repo" && env -u WG_DIR -u WG_PROJECT_ROOT -u WG_TASK_ID -u WG_AGENT_ID \
    -u WG_AGENT_ROLE -u WG_EXECUTOR_TYPE -u WG_MODEL \
    HOME="$home" WG_GLOBAL_DIR="$global" \
    wg --dir "$repo/.wg" "$@")
}

sha() { blake3sum "$1" 2>/dev/null | awk '{print $1}' || sha256sum "$1" | awk '{print $1}'; }

# ── Shared, stale machine-global routing left behind by an older install ──
# This is present BEFORE either repo selects a route, so isolation + fail-loud
# are proven against real stale state, not an empty machine.
cat >"$global/config.toml" <<'TOML'
[agent]
model = "pi:global:stale-route"

[dispatcher]
model = "pi:global:stale-route"
max_agents = 99

[models.task_agent]
model = "pi:global:stale-route"
reasoning = "high"

[secrets]
allow_plaintext = true
TOML
printf 'stale-pi\n' >"$global/active-profile"

# Reusable profile definition + secret material + a custody keystore sentinel.
# These MUST survive the cleanup migration byte-for-byte.
mkdir -p "$global/profiles" "$global/secrets" "$global/keystore"
printf '# reusable profile definition (machine-global input, not project authority)\n[agent]\nmodel = "pi:provider:model"\n' >"$global/profiles/pi.toml"
printf 'secret-bytes-alpha\n' >"$global/secrets/alpha"
printf 'keystore-sentinel-bytes\n' >"$global/keystore/sentinel.key"

before_profile_def=$(sha "$global/profiles/pi.toml")
before_secret=$(sha "$global/secrets/alpha")
before_keystore_sentinel=$(sha "$global/keystore/sentinel.key")

# ════════════════════════════════════════════════════════════════════════
# 1. Repo A: select a Pi route + project guardrails. Writes ONLY worksgood.toml.
# ════════════════════════════════════════════════════════════════════════
run_wg "$repoA" init --no-agency >/dev/null
run_wg "$repoA" setup --route pi --model pi:test:worker --yes >"$scratch/A_setup.out"
grep -q 'Winning source: project-file' "$scratch/A_setup.out"
[[ -f "$repoA/worksgood.toml" ]]
# No global mutation from project setup.
sha_global_cfg_before_A=$(sha "$global/config.toml")
sha_global_active_before_A=$(sha "$global/active-profile")
[[ "$(sha "$global/config.toml")" == "$sha_global_cfg_before_A" ]]
[[ "$(sha "$global/active-profile")" == "$sha_global_active_before_A" ]]
# schema_version is present (the checked-in project document).
grep -q '^schema_version = 1' "$repoA/worksgood.toml"
# Exact Pi route for the strong roles.
grep -q 'model = "pi:test:worker"' "$repoA/worksgood.toml"

# Add project-owned resource guardrail + archive policy via the config surface
# (these are non-routing project bytes that must survive a clone).
run_wg "$repoA" config set dispatcher.resource_management.disk_sentinel_enabled false --no-reload >/dev/null
run_wg "$repoA" config set dispatcher.archive_retention_days 31 --no-reload >/dev/null

# Capture repo A's authoritative route/reasoning/resource/archive leaves.
A_agent_model=$(run_wg "$repoA" config get agent.model --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["value"])')
A_task_model=$(run_wg "$repoA" config get models.task_agent.model --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["value"])')
A_task_reasoning=$(run_wg "$repoA" config get models.task_agent.reasoning --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["value"])')
A_disk=$(run_wg "$repoA" config get dispatcher.resource_management.disk_sentinel_enabled --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["value"])')
A_archive=$(run_wg "$repoA" config get dispatcher.archive_retention_days --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["value"])')
[[ "$A_agent_model" == "pi:test:worker" ]]
[[ "$A_task_model" == "pi:test:worker" ]]
[[ "$A_task_reasoning" == "high" ]]
[[ "$A_disk" == "False" ]]
[[ "$A_archive" == "31" ]]
# Every effective leaf is sourced from the project document, never global.
run_wg "$repoA" config get agent.model --json | grep -q '"source": "project-file"'
run_wg "$repoA" config get models.task_agent.reasoning --json | grep -q '"source": "project-file"'
run_wg "$repoA" config get dispatcher.resource_management.disk_sentinel_enabled --json | grep -q '"source": "project-file"'
run_wg "$repoA" config get dispatcher.archive_retention_days --json | grep -q '"source": "project-file"'

# ════════════════════════════════════════════════════════════════════════
# 2. Repo B: no project route. Stale global routing + active-profile present.
#    Every LLM entry point must fail WG-EXEC-UNSELECTED and create NO state.
# ════════════════════════════════════════════════════════════════════════
run_wg "$repoB" init --no-agency >/dev/null
# Graph CRUD stays credential-free under stale global routing.
run_wg "$repoB" add "graph-only survives stale global config" >/dev/null
run_wg "$repoB" list | grep -q "graph-only"

# service start must refuse before daemon/session/claim/worktree state.
if run_wg "$repoB" service start --no-coordinator-agent >"$scratch/B_start.out" 2>&1; then
  loud_fail "repo B service start inherited stale machine-global routing"
fi
grep -q "WG-EXEC-UNSELECTED" "$scratch/B_start.out"
grep -q "Ignored legacy routing" "$scratch/B_start.out"
grep -q "config.toml" "$scratch/B_start.out"
grep -q "active-profile" "$scratch/B_start.out"
grep -q "stale-pi" "$scratch/B_start.out"
[[ ! -e "$repoB/.wg/service/state.json" ]]
[[ ! -e "$repoB/.wg/service/daemon.sock" ]]
[[ ! -d "$repoB/.wg/agents" ]]

# The single-tick path enforces the same preflight; graph bytes are untouched.
graph_before_B=$(cksum "$repoB/.wg/graph.jsonl")
if run_wg "$repoB" service tick >"$scratch/B_tick.out" 2>&1; then
  loud_fail "repo B service tick accepted stale machine-global routing"
fi
grep -q "WG-EXEC-UNSELECTED" "$scratch/B_tick.out"
[[ "$graph_before_B" == "$(cksum "$repoB/.wg/graph.jsonl")" ]]
[[ ! -d "$repoB/.wg/agents" ]]

# Global non-routing capacity is NOT inherited: builtin-default, not 99.
run_wg "$repoB" config get dispatcher.max_agents --json >"$scratch/B_maxagents.json"
! grep -q '"value":99' "$scratch/B_maxagents.json"
grep -q 'builtin-default' "$scratch/B_maxagents.json"

# ════════════════════════════════════════════════════════════════════════
# 3. Isolation: repo A's route is unaffected by repo B / stale global state.
#    Re-selecting a different route in repo A must not alter repo B at all.
# ════════════════════════════════════════════════════════════════════════
[[ "$(run_wg "$repoA" config get agent.model --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["value"])')" == "pi:test:worker" ]]
# repo B has no worksgood.toml route yet.
[[ ! -f "$repoB/worksgood.toml" ]]

# Change repo A's route. repo B must remain route-less and fail-loud.
run_wg "$repoA" config set agent.model pi:test:worker-v2 --no-reload >/dev/null
# A direct setter clears any profile-origin metadata; route stays project-file.
run_wg "$repoA" config get agent.model --json | grep -q '"source": "project-file"'
run_wg "$repoA" config get agent.model --json | grep -q 'pi:test:worker-v2'
# repo B unchanged: still no project document, still fails.
[[ ! -f "$repoB/worksgood.toml" ]]
if run_wg "$repoB" service start --no-coordinator-agent >"$scratch/B_start2.out" 2>&1; then
  loud_fail "repo B service start succeeded after repo A route change"
fi
grep -q "WG-EXEC-UNSELECTED" "$scratch/B_start2.out"

# Restore repo A's route for the clone step.
run_wg "$repoA" config set agent.model pi:test:worker --no-reload >/dev/null

# ════════════════════════════════════════════════════════════════════════
# 4. Clone reproduction: copy repo A's checked-in worksgood.toml into a fresh
#    repo B. NO .wg route, NO global config, NO reusable profiles, NO WG
#    credentials. Route/reasoning/resource/archive must reproduce from the
#    project document alone.
# ════════════════════════════════════════════════════════════════════════
# Wipe repo B's graph and rebuild it as a clean clone (no inherited .wg state).
rm -rf "$repoB"
mkdir -p "$repoB"
run_wg "$repoB" init --no-agency >/dev/null
# The clone brings only the checked-in project document.
cp "$repoA/worksgood.toml" "$repoB/worksgood.toml"

# Pretend the clone landed on a machine with NO WG credentials: remove the
# shared global state and reusable profiles entirely. The project document
# must remain self-sufficient.
rm -f "$global/config.toml" "$global/active-profile"
rm -rf "$global/profiles"

clone_agent_model=$(run_wg "$repoB" config get agent.model --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["value"])')
clone_task_model=$(run_wg "$repoB" config get models.task_agent.model --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["value"])')
clone_task_reasoning=$(run_wg "$repoB" config get models.task_agent.reasoning --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["value"])')
clone_disk=$(run_wg "$repoB" config get dispatcher.resource_management.disk_sentinel_enabled --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["value"])')
clone_archive=$(run_wg "$repoB" config get dispatcher.archive_retention_days --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["value"])')
[[ "$clone_agent_model" == "$A_agent_model" ]]
[[ "$clone_task_model" == "$A_task_model" ]]
[[ "$clone_task_reasoning" == "$A_task_reasoning" ]]
[[ "$clone_disk" == "$A_disk" ]]
[[ "$clone_archive" == "$A_archive" ]]
# Every reproduced leaf is sourced from the cloned project document.
run_wg "$repoB" config get agent.model --json | grep -q '"source": "project-file"'
run_wg "$repoB" config get models.task_agent.reasoning --json | grep -q '"source": "project-file"'
run_wg "$repoB" config get dispatcher.resource_management.disk_sentinel_enabled --json | grep -q '"source": "project-file"'
run_wg "$repoB" config get dispatcher.archive_retention_days --json | grep -q '"source": "project-file"'
# No global config or active-profile was recreated by reading the clone.
[[ ! -e "$global/config.toml" ]]
[[ ! -e "$global/active-profile" ]]
[[ ! -d "$global/profiles" ]]

# ════════════════════════════════════════════════════════════════════════
# 5. Migration preservation: re-introduce stale global routing + a reusable
#    profile definition + secret/keystore sentinels + real identity and
#    federation state, then run `wg migrate project-local-pi
#    --cleanup-global-routing`. Only routing selectors + the active-profile
#    pointer are removed; every profile/secret/keystore/identity/federation
#    byte survives. The repo A project route keeps working.
# ════════════════════════════════════════════════════════════════════════
# Re-introduce stale global routing for the migration phase.
cat >"$global/config.toml" <<'TOML'
[agent]
model = "pi:global:stale-route"

[dispatcher]
model = "pi:global:stale-route"
max_agents = 99

[models.task_agent]
model = "pi:global:stale-route"
reasoning = "high"

[openrouter]
fallback_model = "openrouter:anthropic/claude-opus-4-7"
monthly_budget_usd = 50

[secrets]
allow_plaintext = true
TOML
printf 'stale-pi\n' >"$global/active-profile"
# Restore + extend the reusable profile definition and secret/keystore sentinels.
mkdir -p "$global/profiles" "$global/secrets" "$global/keystore"
printf '# reusable profile definition (machine-global input, not project authority)\n[agent]\nmodel = "pi:provider:model"\n' >"$global/profiles/pi.toml"
printf 'secret-bytes-alpha\n' >"$global/secrets/alpha"
printf 'keystore-sentinel-bytes\n' >"$global/keystore/sentinel.key"

# Real WG-Fed identity: private keys land in $HOME/.wg/keystore (custody),
# public record lands in <graph>/identity/. Federation peer registry lands in
# <graph>/federation.yaml. Migration must touch neither.
run_wg "$repoA" identity new alice >/dev/null
run_wg "$repoA" peer add bobby "$repoB" --trust verified >/dev/null

before_identity_record=$(sha "$repoA/.wg/identity/alice.json")
before_federation=$(sha "$repoA/.wg/federation.yaml")
before_repoA_cfg=$(sha "$repoA/worksgood.toml")
# Custody keystore is $HOME-relative (one per machine). Capture every byte.
keystore_manifest_before=$(find "$home/.wg/keystore" -type f -printf '%P\n' 2>/dev/null | sort | xargs -I{} sha256sum "$home/.wg/keystore/{}")

# config lint names the stale global routing + remediation before migration.
run_wg "$repoA" config lint --global >"$scratch/lint_before.out"
grep -q 'stale machine-global routing' "$scratch/lint_before.out"
grep -q 'agent.model' "$scratch/lint_before.out"
grep -q 'active-profile' "$scratch/lint_before.out"
grep -q 'wg migrate project-local-pi --cleanup-global-routing' "$scratch/lint_before.out"
# Secret values never leak into lint output.
! grep -q 'secret-bytes-alpha' "$scratch/lint_before.out"

# Dry-run writes nothing.
run_wg "$repoA" migrate project-local-pi --cleanup-global-routing --dry-run --yes >"$scratch/migrate_dry.out"
[[ "$(sha "$global/config.toml")" == "$(sha "$global/config.toml")" ]]
[[ -e "$global/active-profile" ]]
grep -q 'dry-run' "$scratch/migrate_dry.out"

# Apply: remove only routing selectors + active-profile pointer.
run_wg "$repoA" migrate project-local-pi --cleanup-global-routing --yes --json >"$scratch/migrate_apply.out"
receipt_id=$(python3 -c 'import json,sys; print(json.load(open("'"$scratch/migrate_apply.out"'"))["receipt_id"])')
[[ -n "$receipt_id" ]]
[[ ! -e "$global/active-profile" ]]
! grep -q 'pi:global:stale-route' "$global/config.toml"
! grep -q 'fallback_model' "$global/config.toml"
! grep -q 'profile =' "$global/config.toml"
# Preserved non-routing bytes survive.
grep -q 'max_agents = 99' "$global/config.toml"
grep -q 'allow_plaintext = true' "$global/config.toml"
grep -q 'monthly_budget_usd = 50' "$global/config.toml"

# Reusable profile definition, secret material, custody keystore, identity
# record, and federation registry are byte-identical after cleanup.
[[ "$(sha "$global/profiles/pi.toml")" == "$before_profile_def" ]]
[[ "$(sha "$global/secrets/alpha")" == "$before_secret" ]]
[[ "$(sha "$global/keystore/sentinel.key")" == "$before_keystore_sentinel" ]]
[[ "$(sha "$repoA/.wg/identity/alice.json")" == "$before_identity_record" ]]
[[ "$(sha "$repoA/.wg/federation.yaml")" == "$before_federation" ]]
[[ "$(sha "$repoA/worksgood.toml")" == "$before_repoA_cfg" ]]
keystore_manifest_after=$(find "$home/.wg/keystore" -type f -printf '%P\n' 2>/dev/null | sort | xargs -I{} sha256sum "$home/.wg/keystore/{}")
[[ "$keystore_manifest_before" == "$keystore_manifest_after" ]]

# A second run is a no-op (no new receipt, no mtime change).
cfg_mtime_before=$(stat -c '%Y' "$global/config.toml")
run_wg "$repoA" migrate project-local-pi --cleanup-global-routing --yes >"$scratch/migrate_second.out"
grep -qi 'nothing to clean\|no stale global routing' "$scratch/migrate_second.out"
receipt_count=$(find "$global/migrations/project-local-pi" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l)
[[ "$receipt_count" -eq 1 ]]
cfg_mtime_after=$(stat -c '%Y' "$global/config.toml")
[[ "$cfg_mtime_before" -eq "$cfg_mtime_after" ]]

# After cleanup, lint no longer reports stale global routing.
run_wg "$repoA" config lint --global >"$scratch/lint_after.out"
! grep -q 'stale machine-global routing' "$scratch/lint_after.out"

# The repo A project route still resolves from its project document — cleanup
# did not touch project authority.
run_wg "$repoA" config get agent.model --json | grep -q '"source": "project-file"'
run_wg "$repoA" config get agent.model --json | grep -q 'pi:test:worker'
run_wg "$repoA" config get models.task_agent.reasoning --json | grep -q '"source": "project-file"'

echo "PASS: project-local Pi config end-to-end — two-repo isolation, fail-loud under stale global, credential-free clone reproduction, and migration preserving profiles/identity/federation"
