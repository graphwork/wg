#!/usr/bin/env bash
# Enter or source the Guix environment used to build workgraph from this checkout.
#
# Run a command in an isolated shell:
#   ./env.sh cargo build --release --locked
#   ./env.sh cargo install --path . --locked
#
# Start an isolated interactive shell:
#   ./env.sh
#
# Or add the Guix profile to the current shell:
#   source ./env.sh
#   cargo build --release --locked

set -euo pipefail

_wg_dir="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
_manifest="$_wg_dir/manifest.scm"

_find_guix() {
    if [[ -n "${GUIX:-}" ]]; then
        printf '%s\n' "$GUIX"
        return
    fi

    for candidate in \
        /usr/local/guix-profiles/guix-pull-dc0fffec/bin/guix \
        /usr/local/guix-profiles/guix-pull-20250606/bin/guix \
        /usr/local/guix-profiles/guix/bin/guix
    do
        if [[ -x "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return
        fi
    done

    command -v guix 2>/dev/null || true
}

_guix="$(_find_guix)"

if [[ -z "$_guix" || ! -x "$_guix" ]]; then
    echo "error: Guix command not found or not executable" >&2
    echo "set GUIX=/path/to/guix or add guix to PATH" >&2
    return 1 2>/dev/null || exit 1
fi

_wg_inner='
set -euo pipefail
export CC=gcc
export CXX=g++
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export LIBCLANG_PATH="${GUIX_ENVIRONMENT}/lib"
export SSL_CERT_FILE="${GUIX_ENVIRONMENT}/etc/ssl/certs/ca-certificates.crt"

_version_ge() {
    test "$(printf "%s\n%s\n" "$1" "$2" | sort -V | head -n1)" = "$2"
}

_rust_version="$(rustc --version | cut -d" " -f2)"
if ! _version_ge "$_rust_version" "1.85.0"; then
    echo "error: WG requires rustc >= 1.85.0, but this Guix shell resolved rustc $_rust_version" >&2
    echo "hint: set GUIX=/usr/local/guix-profiles/guix-pull-dc0fffec/bin/guix on this system" >&2
    exit 1
fi

if [[ "$#" -gt 0 ]]; then
    exec "$@"
fi

echo "workgraph Guix build shell ready: cargo $(cargo --version | cut -d" " -f2), rustc $_rust_version"
exec bash --noprofile --norc -i
'

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    exec "$_guix" shell --pure -m "$_manifest" -- \
        bash --noprofile --norc -c "$_wg_inner" bash "$@"
fi

set +u
eval "$("$_guix" shell -m "$_manifest" --search-paths)"
set -u

export CC=gcc
export CXX=g++
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc

_profile_bin="${PATH%%:*}"
_profile="${_profile_bin%/bin}"
export LIBCLANG_PATH="$_profile/lib"
export SSL_CERT_FILE="$_profile/etc/ssl/certs/ca-certificates.crt"

_version_ge() {
    test "$(printf "%s\n%s\n" "$1" "$2" | sort -V | head -n1)" = "$2"
}

_rust_version="$(rustc --version | cut -d' ' -f2)"
if ! _version_ge "$_rust_version" "1.85.0"; then
    echo "error: WG requires rustc >= 1.85.0, but this Guix env resolved rustc $_rust_version" >&2
    echo "hint: set GUIX=/usr/local/guix-profiles/guix-pull-dc0fffec/bin/guix on this system" >&2
    return 1 2>/dev/null || exit 1
fi

echo "workgraph Guix build env ready: cargo $(cargo --version | cut -d' ' -f2), rustc $_rust_version"

unset _wg_dir _manifest _find_guix _guix _wg_inner _profile_bin _profile _version_ge _rust_version
