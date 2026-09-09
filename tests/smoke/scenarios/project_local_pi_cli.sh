#!/usr/bin/env bash
# Setup/profile terminal surfaces must write only authoritative project config.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg
scratch=$(make_scratch)
mkdir -p "$scratch/home" "$scratch/global" "$scratch/project"
run_wg() {
  (cd "$scratch/project" && env -u WG_DIR -u WG_PROJECT_ROOT -u WG_TASK_ID -u WG_AGENT_ID \
    HOME="$scratch/home" WG_GLOBAL_DIR="$scratch/global" \
    wg --dir "$scratch/project/.wg" "$@")
}
run_wg init --no-agency >/dev/null
run_wg setup --route pi --model pi:test:worker --yes >"$scratch/setup.out"
grep -q 'Winning source: project-file' "$scratch/setup.out"
[[ -f "$scratch/project/worksgood.toml" ]]
[[ ! -e "$scratch/global/config.toml" ]]
[[ ! -e "$scratch/global/active-profile" ]]

cat >>"$scratch/project/worksgood.toml" <<'TOML'
[dispatcher.resource_management]
disk_sentinel_enabled = false
TOML
run_wg profile select pi --no-reload >"$scratch/select.out"
grep -q 'Winning source: project-profile-import' "$scratch/select.out"
grep -q 'disk_sentinel_enabled = false' "$scratch/project/worksgood.toml"
[[ ! -e "$scratch/global/config.toml" ]]
[[ ! -e "$scratch/global/active-profile" ]]

run_wg profile use pi --no-reload >"$scratch/use.out" 2>"$scratch/use.err"
grep -qi deprecated "$scratch/use.err"
[[ ! -e "$scratch/global/config.toml" ]]
[[ ! -e "$scratch/global/active-profile" ]]

if run_wg setup --route pi --model pi:test:worker --scope global --yes >"$scratch/global.out" 2>&1; then
  loud_fail 'global setup rewrite unexpectedly succeeded'
fi
grep -q 'WG-GLOBAL-CONFIG-WRITE-REFUSED' "$scratch/global.out"
