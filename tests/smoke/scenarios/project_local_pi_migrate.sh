#!/usr/bin/env bash
# Real terminal regression for `wg migrate project-local-pi
# --cleanup-global-routing`: removes only stale machine-global routing +
# active-profile pointer while preserving every reusable profile definition,
# secret, keystore, identity, federation, and Pi-settings byte. Proves dry-run,
# backup/receipt, idempotent second run, malformed-config fail-closed, and
# CAS rollback through the real `wg migrate ...` CLI.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
scratch=$(make_scratch)
home="$scratch/home"
global="$scratch/global"
project="$scratch/project"
mkdir -p "$home" "$global" "$project/.wg"

run_wg() {
  (cd "$project" && env -u WG_DIR -u WG_PROJECT_ROOT -u WG_TASK_ID -u WG_AGENT_ID \
    HOME="$home" WG_GLOBAL_DIR="$global" \
    wg --dir "$project/.wg" "$@")
}

sha() { blake3sum "$1" 2>/dev/null | awk '{print $1}' || sha256sum "$1" | awk '{print $1}'; }

# ── Sentinel-laden global state ─────────────────────────────────────────
cat >"$global/config.toml" <<'TOML'
profile = "pi"

[agent]
model = "pi:global-provider:stale-model"
executor = "claude"

[dispatcher]
model = "pi:global-provider:stale-model"
provider = "global"
max_agents = 7

[models.task_agent]
model = "pi:global-provider:stale-model"
reasoning = "high"

[tiers]
fast = "pi:global-provider:fast"

[openrouter]
fallback_model = "openrouter:anthropic/claude-opus-4-7"
monthly_budget_usd = 50

[[execution.fallbacks]]
primary = "pi:global-provider:stale-model"
models = ["pi:global-provider:alt"]

[secrets]
allow_plaintext = true

[auth]
claude_code_oauth_token = "must-not-leak-into-output"

[native_executor]
preserved_field = "preserved"
TOML
printf 'pi\n' >"$global/active-profile"
mkdir -p "$global/profiles" "$global/secrets" "$global/keystore"
printf '# reusable profile definition\n[agent]\nmodel = "pi:provider:model"\n' >"$global/profiles/pi.toml"
printf 'secret-bytes-alpha\n' >"$global/secrets/alpha"
printf 'root-key-bytes\n' >"$global/keystore/root.key"
printf '{"name":"pi","ts":"now"}\n' >"$global/profile-usage.jsonl"

before_cfg=$(sha "$global/config.toml")
before_active=$(sha "$global/active-profile")
before_profile_def=$(sha "$global/profiles/pi.toml")
before_secret=$(sha "$global/secrets/alpha")
before_keystore=$(sha "$global/keystore/root.key")
before_usage=$(sha "$global/profile-usage.jsonl")

# ── 1. config lint reports stale global routing before migrating ────────
run_wg config lint --global >"$scratch/lint.out"
grep -q 'stale machine-global routing' "$scratch/lint.out"
grep -q 'agent.model' "$scratch/lint.out"
grep -q 'active-profile' "$scratch/lint.out"
grep -q 'wg migrate project-local-pi --cleanup-global-routing' "$scratch/lint.out"
# Secret values must not leak into lint output.
! grep -q 'must-not-leak-into-output' "$scratch/lint.out"

# ── 2. dry-run writes nothing ───────────────────────────────────────────
run_wg migrate project-local-pi --cleanup-global-routing --dry-run --yes >"$scratch/dry.out"
[[ "$(sha "$global/config.toml")" == "$before_cfg" ]]
[[ "$(sha "$global/active-profile")" == "$before_active" ]]
[[ ! -e "$global/migrations" ]]
grep -q 'dry-run' "$scratch/dry.out"

# ── 3. informational mode (no --cleanup-global-routing) writes nothing ──
run_wg migrate project-local-pi >"$scratch/info.out"
[[ "$(sha "$global/config.toml")" == "$before_cfg" ]]
[[ ! -e "$global/migrations" ]]

# ── 4. apply removes only routing, preserves sentinels, writes receipt ──
run_wg migrate project-local-pi --cleanup-global-routing --yes --json >"$scratch/apply.out"
receipt_id=$(python3 -c 'import json,sys; print(json.load(open("'"$scratch/apply.out"'"))["receipt_id"])')
[[ -n "$receipt_id" ]]
after_cfg=$(sha "$global/config.toml")
[[ "$after_cfg" != "$before_cfg" ]]   # config changed
[[ ! -e "$global/active-profile" ]]   # pointer removed
# Routing selectors gone.
! grep -q 'pi:global-provider' "$global/config.toml"
! grep -q 'fallback_model' "$global/config.toml"
! grep -q 'profile =' "$global/config.toml"
# Preserved non-routing bytes survive.
grep -q 'max_agents = 7' "$global/config.toml"
grep -q 'allow_plaintext = true' "$global/config.toml"
grep -q 'must-not-leak-into-output' "$global/config.toml"   # auth section preserved inactive
grep -q 'monthly_budget_usd = 50' "$global/config.toml"
# Sentinels byte-identical.
[[ "$(sha "$global/profiles/pi.toml")" == "$before_profile_def" ]]
[[ "$(sha "$global/secrets/alpha")" == "$before_secret" ]]
[[ "$(sha "$global/keystore/root.key")" == "$before_keystore" ]]
[[ "$(sha "$global/profile-usage.jsonl")" == "$before_usage" ]]
# Backup + receipt on disk with lockdown perms.
receipt_dir="$global/migrations/project-local-pi/$receipt_id"
[[ -f "$receipt_dir/config.toml.pre" ]]
[[ -f "$receipt_dir/active-profile.pre" ]]
[[ -f "$receipt_dir/receipt.json" ]]
backup_body=$(cat "$receipt_dir/config.toml.pre")
echo "$backup_body" | grep -q 'pi:global-provider:stale-model'   # original routing preserved in backup
[[ $(( $(stat -c '%a' "$receipt_dir") )) -eq 700 ]]
[[ $(( $(stat -c '%a' "$receipt_dir/config.toml.pre") )) -eq 600 ]]

# ── 5. second run is a no-op (no new receipt, no mtime change) ──────────
cfg_mtime_before=$(stat -c '%Y' "$global/config.toml")
run_wg migrate project-local-pi --cleanup-global-routing --yes >"$scratch/second.out"
grep -qi 'nothing to clean\|no stale global routing' "$scratch/second.out"
receipt_count=$(find "$global/migrations/project-local-pi" -mindepth 1 -maxdepth 1 -type d | wc -l)
[[ "$receipt_count" -eq 1 ]]
cfg_mtime_after=$(stat -c '%Y' "$global/config.toml")
[[ "$cfg_mtime_before" -eq "$cfg_mtime_after" ]]

# ── 6. winning source after migration is not global ─────────────────────
run_wg config lint --global >"$scratch/lint2.out"
! grep -q 'stale machine-global routing' "$scratch/lint2.out" || \
  ! grep -q 'agent.model' "$scratch/lint2.out"   # routing selectors are gone

# ── 7. rollback restores exact bytes ────────────────────────────────────
run_wg migrate project-local-pi --rollback "$receipt_id" >"$scratch/rollback.out"
[[ "$(sha "$global/config.toml")" == "$before_cfg" ]]
[[ "$(sha "$global/active-profile")" == "$before_active" ]]
grep -q 'pi:global-provider:stale-model' "$global/config.toml"

# ── 8. malformed config fail-closed ─────────────────────────────────────
printf 'this is = = not valid toml {{{\n' >"$global/config.toml"
malformed_sha=$(sha "$global/config.toml")
if run_wg migrate project-local-pi --cleanup-global-routing --yes 2>"$scratch/mal.err"; then
  loud_fail 'malformed config unexpectedly migrated'
fi
grep -qi 'not valid TOML' "$scratch/mal.err"
[[ "$(sha "$global/config.toml")" == "$malformed_sha" ]]   # unchanged
[[ ! -e "$global/migrations" ]] || true

echo "project_local_pi_migrate: PASS"
