#!/usr/bin/env bash
# Enter or source the Guix environment used to build WG from this checkout.
#
# Start an isolated shell:
#   ./env.sh
#   cargo build --release --locked --bins
#
# Or run a command inside the Guix shell:
#   ./env.sh cargo build --release --locked --bins
#
# Or export the build environment into your current shell:
#   source ./env.sh
#   cargo install --force --path . --locked

set -euo pipefail

_wg_sourced=0
if [[ -n "${BASH_VERSION:-}" ]]; then
    _wg_script="${BASH_SOURCE[0]}"
    [[ "${BASH_SOURCE[0]}" != "$0" ]] && _wg_sourced=1
elif [[ -n "${ZSH_VERSION:-}" ]]; then
    _wg_script="${(%):-%x}"
    [[ "${ZSH_EVAL_CONTEXT:-}" == *:file* ]] && _wg_sourced=1
else
    _wg_script="$0"
fi

_wg_dir="$(cd "$(dirname "$_wg_script")" && pwd)"
_guix="${GUIX:-}"
if [[ -z "$_guix" ]]; then
    if [[ -x /usr/local/guix-profiles/guix-pull/bin/guix ]]; then
        _guix=/usr/local/guix-profiles/guix-pull/bin/guix
    elif _wg_guix_found="$(command -v guix 2>/dev/null)"; then
        _guix="$_wg_guix_found"
    else
        _guix=/usr/local/guix-profiles/guix-pull/bin/guix
    fi
fi
_manifest="$_wg_dir/manifest.scm"

if [[ ! -x "$_guix" ]]; then
    echo "error: Guix command not found or not executable: $_guix" >&2
    return 1 2>/dev/null || exit 1
fi

_wg_export_build_env() {
    export CC=gcc
    export CXX=g++
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
    if [[ -n "${GUIX_ENVIRONMENT:-}" ]]; then
        export LIBCLANG_PATH="${GUIX_ENVIRONMENT}/lib"
    elif command -v clang >/dev/null 2>&1; then
        _wg_clang_prefix="$(cd "$(dirname "$(command -v clang)")/.." && pwd)"
        if [[ -d "$_wg_clang_prefix/lib" ]]; then
            export LIBCLANG_PATH="$_wg_clang_prefix/lib"
        fi
        unset _wg_clang_prefix
    fi
}

if [[ "$_wg_sourced" -eq 1 ]]; then
    set +u
    eval "$("$_guix" shell -m "$_manifest" --search-paths)"
    set -u
    _wg_export_build_env
    unset _wg_dir _wg_script _wg_sourced _wg_guix_found _guix _manifest
    unset -f _wg_export_build_env
    return 0
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
