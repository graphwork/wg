#!/usr/bin/env bash
# Scenario: launcher_redesign_default_and_addnew (redesign-new-chat)
#
# Regression lock for the 2026-04-30 redesign of the TUI new-chat
# dialog. Pre-fix, the launcher pre-populated an openrouter model dump
# the user never asked for, surfaced a usage-history list, and crammed
# 4+ pickers (executor / model / endpoint / recent) onto one screen.
# Post-fix, the dialog opens in a minimal Default mode with exactly
# two preset radios (codex:gpt-5.5, claude:opus) plus a single
# "+ Add new..." row that flips into a small inline form.
#
# The TUI itself can't be driven from a smoke script. Instead, this
# scenario runs the unit tests that pin the contract:
#
#   * open_launcher_starts_in_default_mode_with_two_presets
#       — open_launcher must initialize the launcher in Default mode
#         with exactly two presets (codex:gpt-5.5 + claude:opus) and
#         nothing else. Locks against accidental reintroduction of
#         the openrouter dump or recent-list.
#
#   * default_mode_resolves_first_preset
#   * default_mode_resolves_claude_preset_when_selected
#   * default_mode_returns_none_when_add_new_highlighted
#       — Default-mode preset rows resolve the right (executor, model)
#         tuple. The "+ Add new..." row resolves to None (caller must
#         flip into Add-new mode rather than launch nothing).
#
#   * enter_add_new_resets_form_and_focuses_executor
#       — Picking "+ Add new..." flips the launcher into Add-new mode
#         with a fresh form (no stale openrouter / endpoint values).
#
#   * add_new_show_endpoint_only_for_nex
#   * add_new_with_nex_resolves_with_endpoint
#   * add_new_claude_omits_endpoint_even_when_filled
#       — Endpoint field is shown ONLY for executor=nex (claude/codex
#         auth themselves). Stale endpoint text from a prior nex
#         session must NOT leak into a claude/codex launch.
#
#   * add_new_returns_none_when_model_missing
#       — Add-new is rejected when the user hasn't filled in a model.
#
#   * next_section_add_new_skips_endpoint_for_claude
#   * next_section_add_new_includes_endpoint_for_nex
#       — Tab-cycling skips the endpoint field for claude/codex but
#         walks through it for nex.
#
#   * launcher_paste_reaches_custom_endpoint_text (event.rs)
#   * launcher_open_does_not_leak_keys_to_underlying_pty (event.rs)
#   * launcher_open_does_not_leak_paste_to_underlying_pty (event.rs)
#       — Paste-events / keystroke-leak regression locks adapted to
#         the redesigned dialog (fix-new-chat-2 + fix-paste-events
#         contracts must still hold against the new state model).
#
# Exit 0  = PASS
# Exit 77 = SKIP (cargo not available)
# Exit 1  = FAIL

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR" && git rev-parse --show-toplevel 2>/dev/null || echo "")"
if [[ -z "$REPO_ROOT" ]]; then
    echo "SKIP: could not locate git repo root" >&2
    exit 77
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "SKIP: cargo not found" >&2
    exit 77
fi

cd "$REPO_ROOT"

echo "=== redesign-new-chat: minimal Default mode + clean Add-new flow ==="

echo "--- Default mode: two presets + Add-new row ---"
cargo test --bin wg --quiet \
    open_launcher_starts_in_default_mode_with_two_presets 2>&1
cargo test --bin wg --quiet \
    default_presets_are_codex_then_claude 2>&1
cargo test --bin wg --quiet \
    default_mode_resolves_first_preset 2>&1
cargo test --bin wg --quiet \
    default_mode_resolves_claude_preset_when_selected 2>&1
cargo test --bin wg --quiet \
    default_mode_returns_none_when_add_new_highlighted 2>&1

echo "--- Add-new flow: executor radio + model + conditional endpoint ---"
cargo test --bin wg --quiet \
    enter_add_new_resets_form_and_focuses_executor 2>&1
cargo test --bin wg --quiet \
    add_new_show_endpoint_only_for_nex 2>&1
cargo test --bin wg --quiet \
    add_new_with_nex_resolves_with_endpoint 2>&1
cargo test --bin wg --quiet \
    add_new_claude_omits_endpoint_even_when_filled 2>&1
cargo test --bin wg --quiet \
    add_new_returns_none_when_model_missing 2>&1
cargo test --bin wg --quiet \
    add_new_executor_choices_match_spec 2>&1

echo "--- Tab-cycle skips endpoint field for claude/codex ---"
cargo test --bin wg --quiet \
    next_section_default_mode_toggles_between_defaults_and_name 2>&1
cargo test --bin wg --quiet \
    next_section_add_new_skips_endpoint_for_claude 2>&1
cargo test --bin wg --quiet \
    next_section_add_new_includes_endpoint_for_nex 2>&1

echo "--- Render-level locks: dialog buffer matches the spec mock ---"
cargo test --bin wg --quiet \
    launcher_default_mode_render_shows_two_presets_and_add_new 2>&1
cargo test --bin wg --quiet \
    launcher_add_new_mode_renders_form_without_endpoint_for_claude 2>&1
cargo test --bin wg --quiet \
    launcher_add_new_mode_renders_endpoint_field_for_nex 2>&1

echo "--- No regression of fix-new-chat-2 / fix-paste-events ---"
cargo test --bin wg --quiet \
    launcher_open_clears_pty_input_routing 2>&1
cargo test --bin wg --quiet \
    launcher_open_clears_paste_routing_to_launcher_field 2>&1
cargo test --bin wg --quiet \
    launcher_paste_reaches_custom_endpoint_text 2>&1

echo "=== All redesign-new-chat tests passed ==="
