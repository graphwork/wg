#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

require() {
  local pattern=$1 file=$2
  grep -Eq "$pattern" "$file" || fail "$file missing release contract: $pattern"
}

# Cargo's default target set contains the concierge unconditionally. Keep the
# compatibility feature harmless, but never make a normal install request it.
python3 - <<'PY'
from pathlib import Path
text = Path("Cargo.toml").read_text()
blocks = text.split('[[bin]]')[1:]
block = next(block for block in blocks if 'name = "worksgood"' in block)
assert 'path = "src/bin/worksgood.rs"' in block, block
assert "required-features" not in block, block
assert 'name = "worksg"' not in text
PY

release=.github/workflows/release.yml
require 'cargo build --release --locked --bins' "$release"
require 'for bin in worksgood wg nex; do' "$release"
require 'foreach \(\$exe in @\("worksgood\.exe", "wg\.exe", "nex\.exe"\)\)' "$release"
require 'release/worksgood\$\{EXE_EXT\}' "$release"
require 'binaries: \["worksgood", "wg", "nex"\]' "$release"
require 'attended `worksgood` lifecycle concierge' "$release"
require 'notarytool submit' "$release"
require 'actions/attest@v4' "$release"

test -f tests/install/installer_smoke.ps1 || fail 'missing Windows PowerShell installer smoke'
require 'worksgood\.exe.*wg\.exe.*nex\.exe' tests/install/installer_smoke.ps1

for installer in scripts/install-wg.sh scripts/install-wg.ps1; do
  require 'worksgood.*wg.*nex' "$installer"
  require 'archive is missing worksgood' "$installer"
  require 'binaries = \[.worksgood., .wg., .nex.\]' "$installer"
  require '[Uu]ninstall' "$installer"
  require 'refusing to overwrite' "$installer"
done

require 'for bin in \["worksgood", "wg", "nex"\]' src/commands/upgrade.rs
require 'binary_name\("worksgood"\)' src/commands/upgrade.rs
require 'cargo build --quiet --bin wg --bin worksgood' tests/smoke/scenarios/worksgood_concierge.sh
if grep -q -- '--features worksgood-trial' tests/smoke/scenarios/worksgood_concierge.sh; then
  fail 'promoted concierge smoke still requires worksgood-trial'
fi

# pi-worksgood deliberately keeps the full expert backend: the concierge has no
# ready/show/add/publish/done/fail/msg/pi-plugin verbs.
require 'this\.host\.exec\("wg"' worksgood-pi/src/wg-backend.ts
if grep -Eq 'host\.exec\("worksgood"|exec\("worksgood"' worksgood-pi/src/*.ts; then
  fail 'pi-worksgood was repointed to the limited concierge'
fi

echo 'PASS: Cargo, Linux/macOS/Windows archives/signing/attestation, installers, receipts, upgrade/rollback/uninstall, concierge smoke, and Pi expert-backend contracts enumerate worksgood/wg/nex correctly'
