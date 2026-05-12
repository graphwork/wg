#!/bin/bash
# wg-connect.sh - Connection dispatcher for all transports
#
# Determines the wg user and attaches to (or creates) a tmux session
# running `wg tui`. Used as the entry point for:
#   - ttyd launch command
#   - SSH ForceCommand
#   - mosh connection command
#
# User resolution order:
#   1. WG_USER environment variable (explicit override)
#   2. SSH_USER / TTYD_USER (set by transport layer)
#   3. $USER (login user fallback)
#
# Usage:
#   wg-connect.sh              # auto-detect user, launch wg tui
#   WG_USER=alice wg-connect.sh  # explicit user

set -euo pipefail

# --- User resolution ---

resolve_user() {
    if [ -n "${WG_USER:-}" ]; then
        echo "$WG_USER"
    elif [ -n "${SSH_USER:-}" ]; then
        echo "$SSH_USER"
    elif [ -n "${TTYD_USER:-}" ]; then
        echo "$TTYD_USER"
    else
        echo "${USER:-unknown}"
    fi
}

WG_USER="$(resolve_user)"
export WG_USER

SESSION_NAME="${WG_USER}-wg"

# --- Dependency checks ---

if ! command -v wg >/dev/null 2>&1; then
    cat <<'SETUP'
wg is not installed or not in PATH.

To install:
  1. Install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
  2. Install wg:   cargo install --git https://github.com/graphwork/wg
  3. Re-run this script

If wg is already installed, make sure it is on your PATH:
  export PATH="$HOME/.cargo/bin:$PATH"
SETUP
    exit 1
fi

if ! command -v tmux >/dev/null 2>&1; then
    echo "tmux is required but not found. Install it with your package manager:"
    echo "  apt install tmux   # Debian/Ubuntu"
    echo "  brew install tmux  # macOS"
    exit 1
fi

# --- Session management ---

# tmux new-session -A: attach if session exists, create otherwise.
# This makes the script idempotent — running it twice attaches to the
# existing session rather than creating a duplicate.
exec tmux new-session -A -s "$SESSION_NAME" "wg tui"
