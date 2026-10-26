#!/usr/bin/env bash
# Archived-scenario helper shim.
#
# `tests/smoke/archive/` sits at the SAME depth as `tests/smoke/scenarios/`
# (`tests/smoke/<dir>/<scenario>.sh`), so every `$HERE/..`-relative path in a
# retired script keeps resolving after the move:
#
#   $HERE/_helpers.sh              -> tests/smoke/archive/_helpers.sh (this shim)
#   $HERE/../..                    -> tests/smoke
#   $HERE/../../fixtures           -> tests/fixtures
#   $HERE/../../..                 -> repository root
#
# Only the same-directory `_helpers.sh` source line would break, so this shim
# forwards to the single live helper implementation in `scenarios/`. Retired
# scripts therefore keep their bodies byte-for-byte unchanged, which is what
# makes `wg done --full-smoke` able to run them again with no other edits.
#
# Do not add behaviour here: `scenarios/_helpers.sh` is the one owner of the
# scratch/ownership/cleanup contract (see tests/smoke/README.md).
. "$(cd "$(dirname "${BASH_SOURCE[0]}")/../scenarios" && pwd)/_helpers.sh"
