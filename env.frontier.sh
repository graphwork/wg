#!/usr/bin/env bash
# Sourceable build environment for Frontier login/build nodes.
#
# env.sh is the Guix-backed environment used on hosts where Guix is installed.
# Frontier already provides the compiler/runtime stack through the login shell,
# but bindgen still needs LIBCLANG_PATH to point at a real libclang.so.

_wg_frontier_fail() {
    echo "$*" >&2
    return 1 2>/dev/null || exit 1
}

_wg_frontier_version_ge() {
    test "$(printf "%s\n%s\n" "$1" "$2" | sort -V | head -n1)" = "$2"
}

_wg_frontier_has_libclang() {
    local dir="$1"
    [[ -n "$dir" ]] || return 1
    compgen -G "$dir/libclang.so" >/dev/null || \
        compgen -G "$dir/libclang-*.so" >/dev/null || \
        compgen -G "$dir/libclang.so.*" >/dev/null || \
        compgen -G "$dir/libclang-*.so.*" >/dev/null
}

_wg_frontier_find_libclang() {
    local dir

    if _wg_frontier_has_libclang "${LIBCLANG_PATH:-}"; then
        printf '%s\n' "$LIBCLANG_PATH"
        return
    fi

    if command -v llvm-config >/dev/null 2>&1; then
        dir="$(llvm-config --libdir 2>/dev/null || true)"
        if _wg_frontier_has_libclang "$dir"; then
            printf '%s\n' "$dir"
            return
        fi
    fi

    for dir in \
        /opt/rocm-7.13.0/lib/llvm/lib \
        /opt/rocm-7.2.0/lib/llvm/lib \
        /opt/rocm-7.1.1/lib/llvm/lib \
        /opt/rocm-7.0.2/lib/llvm/lib \
        /opt/rocm-6.4.1/lib/llvm/lib \
        /opt/rocm-6.4.0/lib/llvm/lib \
        /usr/lib64
    do
        if _wg_frontier_has_libclang "$dir"; then
            printf '%s\n' "$dir"
            return
        fi
    done
}

if ! command -v cargo >/dev/null 2>&1 || ! command -v rustc >/dev/null 2>&1; then
    _wg_frontier_fail "error: cargo and rustc must be on PATH before sourcing env.frontier.sh"
fi

_wg_frontier_rust_version="$(rustc --version | cut -d' ' -f2)"
if ! _wg_frontier_version_ge "$_wg_frontier_rust_version" "1.85.0"; then
    _wg_frontier_fail "error: WG requires rustc >= 1.85.0, but PATH resolved rustc $_wg_frontier_rust_version"
fi

_wg_frontier_libclang="$(_wg_frontier_find_libclang)"
if [[ -z "$_wg_frontier_libclang" ]]; then
    _wg_frontier_fail "error: unable to find libclang.so; set LIBCLANG_PATH to the directory containing it"
fi

export LIBCLANG_PATH="$_wg_frontier_libclang"
case ":${LD_LIBRARY_PATH:-}:" in
    *:"$_wg_frontier_libclang":*) ;;
    *) export LD_LIBRARY_PATH="$_wg_frontier_libclang${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ;;
esac

export CC="${CC:-gcc}"
export CXX="${CXX:-g++}"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER:-$CC}"
_wg_frontier_gcc_include="$($CC -print-file-name=include 2>/dev/null || true)"
_wg_frontier_gcc_include_fixed="$($CC -print-file-name=include-fixed 2>/dev/null || true)"
_wg_frontier_bindgen_args=()
if [[ -d "$_wg_frontier_gcc_include" ]]; then
    _wg_frontier_bindgen_args+=("-I$_wg_frontier_gcc_include")
fi
if [[ -d "$_wg_frontier_gcc_include_fixed" ]]; then
    _wg_frontier_bindgen_args+=("-I$_wg_frontier_gcc_include_fixed")
fi
if [[ -d /usr/include ]]; then
    _wg_frontier_bindgen_args+=("-I/usr/include")
fi
if [[ "${#_wg_frontier_bindgen_args[@]}" -gt 0 ]]; then
    export BINDGEN_EXTRA_CLANG_ARGS="${_wg_frontier_bindgen_args[*]}${BINDGEN_EXTRA_CLANG_ARGS:+ $BINDGEN_EXTRA_CLANG_ARGS}"
fi
export LC_ALL=C
export LANG=C
unset LC_CTYPE

echo "workgraph Frontier build env ready: cargo $(cargo --version | cut -d' ' -f2), rustc $_wg_frontier_rust_version, libclang $LIBCLANG_PATH"

unset _wg_frontier_rust_version _wg_frontier_libclang
unset _wg_frontier_gcc_include _wg_frontier_gcc_include_fixed _wg_frontier_bindgen_args
unset -f _wg_frontier_fail _wg_frontier_version_ge _wg_frontier_has_libclang _wg_frontier_find_libclang 2>/dev/null || true
