#!/usr/bin/env bash
# Historical scenario name retained because the smoke manifest is grow-only.
#
# Native Codex live chat no longer launches an interactive Codex PTY. The TUI
# stays on its native composer and the daemon's existing codex-handler invokes
# `codex exec --dangerously-bypass-approvals-and-sandbox`, preserving one
# inbox/outbox transcript plus the native thread ID. These focused tests pin
# both sides of that evolved contract; explicit_codex_chat_resume supplies the
# full fake-Codex terminal/TUI human flow.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/_helpers.sh"
require_wg

run_test() {
  local filter="$1"
  echo "running $filter ..."
  cargo test --bin wg "$filter"
}

run_test 'tui::viz_viewer::state::chat_pty_executor_resolution_tests::explicit_codex_chat_uses_supervised_handler_not_vendor_pty'
run_test 'commands::codex_handler::tests::canonical_chat_refs_are_coordinator_sessions'
run_test 'commands::chat_cmd::tests::create_explicit_codex_chat_is_supported_without_pi_preflight'
run_test 'tui::viz_viewer::state::build_codex_chat_pty_args_tests::fresh_session_includes_bypass_flag'

echo 'PASS: historical Codex PTY contract evolved to native composer + daemon codex-handler; bypass and explicit route remain pinned'
