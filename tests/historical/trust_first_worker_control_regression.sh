#!/usr/bin/env bash
# Executable three-point regression: historical trust-first -> scoped regression -> candidate.
set -euo pipefail
: "${WG_BIN:?set WG_BIN to the current candidate binary}"
repo=$(git rev-parse --show-toplevel)
trust_first_commit=1a1e112e1e91a1a36f66c161762f12bc725a76ec
scoped_regression_commit=da286458ac640a6c4a49b269284c39e1d9ff3fdf
scratch=$(mktemp -d "${TMPDIR:-/tmp}/wg-historical-worker-control.XXXXXX")
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/trust-first-source" "$scratch/scoped-source"
git -C "$repo" archive "$trust_first_commit" | tar -x -C "$scratch/trust-first-source"
git -C "$repo" archive "$scoped_regression_commit" | tar -x -C "$scratch/scoped-source"

# Dedicated targets preserve reproducible historical executables without
# replacing the candidate under test. Dependencies remain cached across runs.
CARGO_TARGET_DIR="$repo/target/historical-trust-first" \
  cargo build --quiet --manifest-path "$scratch/trust-first-source/Cargo.toml" --bin wg
trust_first_bin="$repo/target/historical-trust-first/debug/wg"
CARGO_TARGET_DIR="$repo/target/historical-worker-control" \
  cargo build --quiet --manifest-path "$scratch/scoped-source/Cargo.toml" --bin wg
scoped_bin="$repo/target/historical-worker-control/debug/wg"
[[ -x $trust_first_bin && -x $scoped_bin ]]

# The parent of the broker executes a real isolated worker that successfully
# reads/edits/messages a sibling and creates a linked task through normal CLI.
WG_BIN="$trust_first_bin" bash "$repo/tests/historical/trust_first_pre_broker_fixture.sh"

# Strengthen the pinned regression tree's own real-daemon/Fake-Pi fixture with
# an existing unequal target and an explicit cross-task edit attempt. Both show
# and edit must be refused; unequal-target show reproduces cross_task_refused.
python3 - "$scratch/scoped-source/tests/smoke/scenarios/worker_control_capability_broker.sh" <<'PY'
from pathlib import Path
import sys
p = Path(sys.argv[1])
s = p.read_text()
s = s.replace(
    "grep -q 'worker_control.cross_task_refused' cross-task.out\nif wg service status",
    "grep -q 'worker_control.cross_task_refused' cross-task.out\n"
    "if wg edit another-task --description 'must be refused' > cross-edit.out 2>&1; then\n"
    "  echo 'cross-task edit unexpectedly allowed' >&2\n  exit 89\nfi\n"
    "grep -Eq 'worker_control.(cross_task|operation)_refused' cross-edit.out\nif wg service status",
)
s = s.replace(
    "git add scoped-show.json brokered.txt",
    "git add scoped-show.json cross-task.out cross-edit.out brokered.txt",
)
s = s.replace(
    'wgrun add "worker broker probe" --id worker-broker-probe >/dev/null\n',
    'wgrun add "worker broker probe" --id worker-broker-probe >/dev/null\n'
    'wgrun add "another task" --id another-task >/dev/null\n',
)
if "cross-edit.out" not in s or 'id another-task' not in s:
    raise SystemExit("failed to install explicit historical cross-task regression assertions")
p.write_text(s)
PY
WG_BIN="$scoped_bin" bash "$scratch/scoped-source/tests/smoke/scenarios/worker_control_capability_broker.sh"

# The current candidate executes the same class of real worker as trusted and
# completes a quality-pass flow with cross-task mutations and release.
WG_BIN="$WG_BIN" bash "$repo/tests/smoke/scenarios/trust_first_local_worker_coordination.sh"
echo "PASS: historical $trust_first_commit coordinated normally; $scoped_regression_commit reproduced unequal-target show/edit refusal; current candidate restores fenced trust-first coordination"
