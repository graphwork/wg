#!/usr/bin/env bash
# Enter or source the Guix environment used to build WG from this checkout.
#
# Start an isolated shell:
#   ./env.sh
#   cargo build --release --locked --bins
#
# Or run a command inside the Guix shell:
#   ./env.sh cargo build --release --locked --bins

set -euo pipefail

_wg_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
_guix="${GUIX:-/usr/local/guix-profiles/guix-pull/bin/guix}"
_manifest="$_wg_dir/manifest.scm"

if [[ ! -x "$_guix" ]]; then
    echo "error: Guix command not found or not executable: $_guix" >&2
    return 1 2>/dev/null || exit 1
fi

if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
    echo "error: source mode is not supported; run ./env.sh [command]" >&2
    return 1
fi

if [[ $# -eq 0 ]]; then
    exec "$_guix" shell -m "$_manifest" -- \
        bash --noprofile --norc -c '
            export CC=gcc
            export CXX=g++
            export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
            export LIBCLANG_PATH="${GUIX_ENVIRONMENT}/lib"
            echo "WG Guix build shell ready: cargo $(cargo --version | cut -d" " -f2), rustc $(rustc --version | cut -d" " -f2)"
            exec bash --noprofile --norc -i
        '
fi

exec "$_guix" shell -m "$_manifest" -- \
    bash --noprofile --norc -c '
        export CC=gcc
        export CXX=g++
        export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
        export LIBCLANG_PATH="${GUIX_ENVIRONMENT}/lib"
        exec "$@"
    ' bash "$@"
