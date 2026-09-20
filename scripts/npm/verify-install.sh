#!/usr/bin/env bash
#
# verify-install.sh — local dry-run proof of the npm distribution first slice
# (no publish, no npm token). Implements the task validation:
#
#   1. pack the tarballs (make-packages.sh --pack, bin-dir mode),
#   2. install them into CLEAN prefixes (twice: normal and --ignore-scripts),
#      proving the metapackage pulls the platform package AND
#      @earendil-works/pi-coding-agent,
#   3. prove the shim resolves + execs the binary and `worksgood <version>`
#      works from the npm-installed layout,
#   4. prove the WG_BINARY_PATH override and the cargo-install fallback hint,
#   5. npm publish --dry-run structural validation of every package.
#
# Requires network for the pi dependency (registry.npmjs.org). Exit 77 = loud
# SKIP when a prerequisite (binaries, registry) is unavailable.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

BIN_DIR=
OUT_DIR=
KEEP=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --bin-dir) BIN_DIR="$2"; shift 2 ;;
    --out-dir) OUT_DIR="$2"; shift 2 ;;
    --keep) KEEP=1; shift ;;
    -h|--help) sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "verify-install.sh: unknown argument: $1" >&2; exit 2 ;;
  esac
done

fail() { echo "FAIL: $*" >&2; exit 1; }
pass() { echo "PASS: $*"; }

# ---------------------------------------------------------------- prerequisites
if [[ -z "${BIN_DIR}" ]]; then
  for cand in "${REPO_ROOT}/target/release" "${HOME}/.cargo/bin"; do
    if [[ -x "${cand}/wg" && -x "${cand}/worksgood" && -x "${cand}/nex" ]]; then
      BIN_DIR="${cand}"
      break
    fi
  done
fi
[[ -n "${BIN_DIR}" ]] || {
  echo "SKIP(77): no prebuilt wg/nex/worksgood binaries found (build with cargo build --release --bins or pass --bin-dir)"
  exit 77
}

if ! curl -fsSI --max-time 10 https://registry.npmjs.org/ >/dev/null 2>&1; then
  echo "SKIP(77): npm registry unreachable — the pi dependency cannot be pulled"
  exit 77
fi

# The exec proof needs a slice platform matching the host binaries' platform.
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) NPM_PLATFORM="linux-x64-gnu" ;;
  Darwin-arm64) NPM_PLATFORM="darwin-arm64" ;;
  *) echo "SKIP(77): host $(uname -s)-$(uname -m) is not a first-slice platform (linux-x64-gnu / darwin-arm64)"; exit 77 ;;
esac

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "${REPO_ROOT}/Cargo.toml" | head -n 1)"
[[ -n "${VERSION}" ]] || fail "cannot read version from Cargo.toml"

WORK_DIR="${OUT_DIR:-$(mktemp -d /tmp/wg-npm-verify.XXXXXX)}"
cleanup() {
  if [[ "${KEEP:-0}" != "1" && -n "${WORK_DIR:-}" && "${WORK_DIR}" == /tmp/* ]]; then
    rm -rf "${WORK_DIR}"
  fi
}
trap cleanup EXIT
PACK_DIR="${WORK_DIR}/packages"
mkdir -p "${PACK_DIR}"

echo "== verify-install: version=${VERSION} platform=${NPM_PLATFORM} bins=${BIN_DIR} work=${WORK_DIR}"

# ------------------------------------------------------- 1. assemble + pack
bash "${SCRIPT_DIR}/make-packages.sh" \
  --bin-dir "${BIN_DIR}" --npm-platform "${NPM_PLATFORM}" \
  --out-dir "${PACK_DIR}" --pack
CLI_TGZ="$(ls "${PACK_DIR}"/worksgood-cli-"${VERSION}".tgz 2>/dev/null | head -n 1)"
PLAT_TGZ="$(ls "${PACK_DIR}"/worksgood-"${NPM_PLATFORM}"-"${VERSION}".tgz 2>/dev/null | head -n 1)"
[[ -n "${CLI_TGZ}" && -f "${CLI_TGZ}" ]] || fail "metapackage tarball missing"
[[ -n "${PLAT_TGZ}" && -f "${PLAT_TGZ}" ]] || fail "platform tarball missing"
pass "tarballs packed: $(basename "${CLI_TGZ}"), $(basename "${PLAT_TGZ}")"

# ------------------------------------------- 2. zero-install-scripts invariant
for pj in "${PACK_DIR}"/@worksgood/*/package.json; do
  node -e '
    const pkg = JSON.parse(require("fs").readFileSync(process.argv[1], "utf8"));
    const bad = Object.keys(pkg.scripts || {}).filter((k) => /install|prepare|pack/.test(k));
    if (bad.length) { console.error(`${pkg.name} has script(s): ${bad}`); process.exit(1); }
  ' "${pj}"
done
pass "zero install scripts in all package.json files (structural, non-negotiable)"

# ------------------------------------------- 3. clean-prefix install (normal)
PREFIX_A="${WORK_DIR}/prefix-a"
mkdir -p "${PREFIX_A}"
# Alias-install the platform tarball under its real name first so the
# metapackage's exact-pinned optionalDependencies dedupes to it (the packages
# are NOT on the registry in this dry run).
npm install --prefix "${PREFIX_A}" --no-audit --no-fund --loglevel=error \
  "@worksgood/${NPM_PLATFORM}@file:${PLAT_TGZ}" >/dev/null
npm install --prefix "${PREFIX_A}" --no-audit --no-fund --loglevel=error \
  "file:${CLI_TGZ}" >/dev/null

for d in "@worksgood/cli" "@worksgood/${NPM_PLATFORM}" "@earendil-works/pi-coding-agent"; do
  [[ -d "${PREFIX_A}/node_modules/${d}" ]] || fail "expected ${d} in clean-prefix install"
done
[[ -x "${PREFIX_A}/node_modules/.bin/worksgood" ]] || fail ".bin/worksgood not created"
[[ -x "${PREFIX_A}/node_modules/@worksgood/${NPM_PLATFORM}/bin/wg" ]] || fail "platform package binaries missing"
pass "clean-prefix install pulled @worksgood/cli + @worksgood/${NPM_PLATFORM} + @earendil-works/pi-coding-agent"

# --------------------------------- 4. shim resolves + execs; worksgood works
WG_VERSION_OUT="$("${PREFIX_A}/node_modules/.bin/wg" --version 2>&1)"
echo "${WG_VERSION_OUT}" | grep -q "wg ${VERSION}" || fail "wg --version from npm layout got: ${WG_VERSION_OUT}"
NEX_VERSION_OUT="$("${PREFIX_A}/node_modules/.bin/nex" --version 2>&1)"
echo "${NEX_VERSION_OUT}" | grep -q "nex ${VERSION}" || fail "nex --version from npm layout got: ${NEX_VERSION_OUT}"
WSG_VERSION_OUT="$("${PREFIX_A}/node_modules/.bin/worksgood" --version 2>&1)"
echo "${WSG_VERSION_OUT}" | grep -q "worksgood ${VERSION}" || fail "worksgood --version from npm layout got: ${WSG_VERSION_OUT}"
pass "shim execs real binaries from npm-installed layout: ${WSG_VERSION_OUT} / ${WG_VERSION_OUT} / ${NEX_VERSION_OUT}"

# ------------------------------------------- 5. --ignore-scripts compatibility
PREFIX_B="${WORK_DIR}/prefix-b"
mkdir -p "${PREFIX_B}"
npm install --prefix "${PREFIX_B}" --ignore-scripts --no-audit --no-fund --loglevel=error \
  "@worksgood/${NPM_PLATFORM}@file:${PLAT_TGZ}" >/dev/null
npm install --prefix "${PREFIX_B}" --ignore-scripts --no-audit --no-fund --loglevel=error \
  "file:${CLI_TGZ}" >/dev/null
WSG_B="$("${PREFIX_B}/node_modules/.bin/worksgood" --version 2>&1)"
echo "${WSG_B}" | grep -q "worksgood ${VERSION}" || fail "--ignore-scripts install broke the shim: ${WSG_B}"
pass "--ignore-scripts install fully functional (zero-scripts design): ${WSG_B}"

# --------------------------------------------- 6. WG_BINARY_PATH escape hatch
STUB="${WORK_DIR}/override-stub"
cat > "${STUB}" <<'EOF'
#!/bin/sh
echo "OVERRIDE-OK $*"
EOF
chmod 755 "${STUB}"
OVER_OUT="$(WG_BINARY_PATH="${STUB}" "${PREFIX_A}/node_modules/.bin/wg" hello-world 2>&1)"
echo "${OVER_OUT}" | grep -q "OVERRIDE-OK hello-world" || fail "WG_BINARY_PATH override did not exec the stub: ${OVER_OUT}"
if WG_BINARY_PATH="${WORK_DIR}/does-not-exist" "${PREFIX_A}/node_modules/.bin/wg" x >/dev/null 2>&1; then
  fail "WG_BINARY_PATH pointing at a missing file must fail"
fi
WG_MISSING_MSG="$(WG_BINARY_PATH="${WORK_DIR}/does-not-exist" "${PREFIX_A}/node_modules/.bin/wg" x 2>&1 || true)"
echo "${WG_MISSING_MSG}" | grep -q "WG_BINARY_PATH" || fail "missing WG_BINARY_PATH target error not loud: ${WG_MISSING_MSG}"
pass "WG_BINARY_PATH override honored (exec + loud missing-file error)"

# ------------------------------------------------- 7. cargo-install fallback
HIDDEN="${PREFIX_A}/node_modules/@worksgood/${NPM_PLATFORM}.hidden"
mv "${PREFIX_A}/node_modules/@worksgood/${NPM_PLATFORM}" "${HIDDEN}"
FALLBACK_MSG="$( "${PREFIX_A}/node_modules/.bin/worksgood" --version 2>&1 || true )"
mv "${HIDDEN}" "${PREFIX_A}/node_modules/@worksgood/${NPM_PLATFORM}"
echo "${FALLBACK_MSG}" | grep -q "no prebuilt binary" || fail "fallback message missing: ${FALLBACK_MSG}"
echo "${FALLBACK_MSG}" | grep -q "cargo install --git https://github.com/graphwork/wg --locked" \
  || fail "fallback must print the cargo-install hint: ${FALLBACK_MSG}"
pass "missing optional dep -> friendly cargo-install fallback (not a stack trace)"

# --------------------------------------------- 8. npm publish --dry-run proof
for dir in "${PACK_DIR}"/@worksgood/*/; do
  (cd "${dir}" && npm publish --dry-run --loglevel=error >/dev/null) \
    || fail "npm publish --dry-run failed for ${dir}"
done
pass "npm publish --dry-run validates every package (registry-independent)"

echo
echo "== verify-install: ALL CHECKS PASSED (dry-run only; nothing was published) =="
echo "Artifacts kept at: ${WORK_DIR}${OUT_DIR:-} (auto-cleaned unless --keep/--out-dir)"
