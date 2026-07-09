#!/usr/bin/env bash
# Enter or source the Guix environment used to build workgraph from this checkout.
#
# Run a command with the Guix build environment:
#   ./env.sh cargo build --release --locked
#   ./env.sh cargo install --path . --locked
#
# Start an isolated interactive shell:
#   ./env.sh
#
# Or add the Guix profile to the current shell:
#   source ./env.sh
#   cargo build --release --locked

_wg_fail() {
    echo "$*" >&2
    return 1 2>/dev/null || exit 1
}

_wg_sourced=0
if [[ -n "${BASH_VERSION:-}" ]]; then
    _wg_script="${BASH_SOURCE[0]:-$0}"
    if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
        _wg_sourced=1
    fi
elif [[ -n "${ZSH_VERSION:-}" ]]; then
    eval '_wg_script="${(%):-%x}"'
    case ":${ZSH_EVAL_CONTEXT:-}:" in
        *:file:*) _wg_sourced=1 ;;
    esac
else
    _wg_script="$0"
fi

_wg_dir="$(cd "$(dirname "$_wg_script")" && pwd)"
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
    _wg_fail
fi

if [[ "$_wg_sourced" -eq 0 ]]; then
    exec bash --noprofile --norc -c '
set -eo pipefail
source "$1" || exit $?
shift
if [[ "$#" -gt 0 ]]; then
    exec "$@"
fi
exec bash --noprofile --norc -i
' bash "$_wg_dir/env.sh" "$@"
fi

_search_paths="$("$_guix" shell -m "$_manifest" --search-paths)"
if [[ -z "$_search_paths" ]]; then
    echo "error: Guix did not return a usable environment" >&2
    _wg_fail
fi
eval "$_search_paths"

export CC=gcc
export CXX=g++
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=gcc
export LC_ALL=C
export LANG=C
unset LC_CTYPE

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
    _wg_fail
fi

_ld_so="$(gcc -print-file-name=ld-linux-x86-64.so.2)"
_gcc_lib="$(dirname "$(gcc -print-file-name=libgcc_s.so.1)")"
_shim_dir="${TMPDIR:-/tmp}/wg-guix-rust-${USER:-$(id -u)}-${_profile##*/}"

if command -v rustup >/dev/null 2>&1; then
    _cargo_real="$(rustup which cargo)"
    _rustc_real="$(rustup which rustc)"
    _rustdoc_real="$(rustup which rustdoc)"
else
    _cargo_real="$(command -v cargo)"
    _rustc_real="$(command -v rustc)"
    _rustdoc_real="$(command -v rustdoc)"
fi
_rust_lib="$(dirname "$_rustc_real")/../lib"

mkdir -p "$_shim_dir"
_write_rust_wrapper() {
    local _name="$1"
    local _real="$2"
    {
        printf '%s\n' '#!/usr/bin/env bash'
        printf 'exec %q --library-path %q %q "$@"\n' \
            "$_ld_so" "$_profile/lib:$_gcc_lib:$_rust_lib" "$_real"
    } >"$_shim_dir/$_name"
    chmod +x "$_shim_dir/$_name"
}
_write_rust_wrapper cargo "$_cargo_real"
_write_rust_wrapper rustc "$_rustc_real"
_write_rust_wrapper rustdoc "$_rustdoc_real"

export PATH="$_shim_dir:$PATH"
export CARGO="$_shim_dir/cargo"
export RUSTC="$_shim_dir/rustc"
export RUSTDOC="$_shim_dir/rustdoc"

_rust_version="$(rustc --version | cut -d' ' -f2)"

echo "workgraph Guix build env ready: cargo $(cargo --version | cut -d' ' -f2), rustc $_rust_version"

unset _wg_sourced _wg_script _wg_dir _manifest _guix
unset _search_paths _profile_bin _profile
unset _ld_so _gcc_lib _shim_dir _cargo_real _rustc_real _rustdoc_real _rust_lib
unset _rust_version
unset -f _wg_fail _find_guix _version_ge _write_rust_wrapper 2>/dev/null || \
    unfunction _wg_fail _find_guix _version_ge _write_rust_wrapper 2>/dev/null || true
