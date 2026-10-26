#!/usr/bin/env bash
# Scenario: nex_chat_in_tui_via_tmux
#
# Permanent regression lock for implement-nex-chat path A. The TUI's
# native/nex chat pane must use the same tmux-wrapped PTY path as claude
# and codex, but the inner command argv must mirror the user's working
# CLI shape:
#
#   wg nex -m <model> -e <endpoint>
#
# In particular, the TUI path must not pass `--chat`, `--role`, or
# `--resume`; those were the divergent args identified by
# diagnose-nex-chat.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"

require_wg

cd "$(cd "$HERE/../../.." && pwd)"

run_test() {
    local name="$1"
    local out
    if ! out=$(cargo test "$name" -- --exact --nocapture 2>&1); then
        loud_fail "cargo test $name failed:
$out"
    fi
}

run_test "tui::viz_viewer::state::build_nex_chat_pty_args_tests::mirrors_working_cli_shape"
run_test "tui::viz_viewer::state::build_nex_chat_pty_args_tests::does_not_use_chat_or_resume_mode"
run_test "tui::viz_viewer::state::build_nex_chat_pty_args_tests::native_chat_pty_spawn_uses_cli_shape_and_endpoint_override"

echo "PASS: nex TUI chat queues tmux-capable PTY spawn with wg nex CLI argv"
exit 0
