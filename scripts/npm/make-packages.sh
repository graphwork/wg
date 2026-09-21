#!/usr/bin/env bash
#
# make-packages.sh — assemble the @worksgood npm packages from WG release
# artifacts. See scripts/npm/README.md and docs/research/npm-distribution.md.
#
# Two modes:
#   archive mode (CI):  unpack the existing release.yml archives
#                       (wg-v<archive-version>-<target>.tar.gz) into platform
#                       packages. Binaries are prebuilt release artifacts
#                       (GitHub-attested; macOS binaries are not yet code-signed
#                       or notarized) — this is pure packaging.
#   bin-dir mode (local dry-run / verify-install.sh): assemble one platform
#                       package straight from a directory containing the
#                       wg/nex/worksgood binaries.
#
# Versioning: Cargo.toml's `version` is the single source of truth (the same
# value release.yml derives). All packages in one run share that exact version.
#
# OPERATOR-GATED: this script never publishes. It packs tarballs and prints the
# operator checklist for the npm org/token/trusted-publisher steps.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"

usage() {
  cat <<'USAGE'
Usage:
  make-packages.sh --archive-dir DIR [--out-dir DIR] [--pack]
  make-packages.sh --bin-dir DIR --npm-platform NAME [--out-dir DIR] [--pack]

Options:
  --archive-dir DIR     Directory containing wg-v<version>-<target>.tar.gz
                        release archives (CI mode).
  --bin-dir DIR         Directory containing wg, nex, worksgood binaries
                        (local dry-run mode).
  --npm-platform NAME   Platform package suffix for --bin-dir mode:
                        linux-x64-gnu | darwin-arm64.
  --out-dir DIR         Output directory (default: <repo>/dist/npm).
  --version V           npm version for all packages
                        (default: read from Cargo.toml — the single source).
  --archive-version V   Version token used to LOCATE the release archives
                        (default: same as --version). release.yml names
                        archives with its `version` output, which for
                        release-test tags is `<cargo_version>-<tag>`, while
                        the npm version stays the Cargo.toml version.
  --pack                Also run `npm pack` per package (tarballs in out-dir).
  -h, --help            This help.
USAGE
}

MODE=archive
ARCHIVE_DIR=
BIN_DIR=
NPM_PLATFORM=
OUT_DIR="${REPO_ROOT}/dist/npm"
VERSION=
ARCHIVE_VERSION=
DO_PACK=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --archive-dir) ARCHIVE_DIR="$2"; MODE=archive; shift 2 ;;
    --bin-dir) BIN_DIR="$2"; MODE=bin-dir; shift 2 ;;
    --npm-platform) NPM_PLATFORM="$2"; shift 2 ;;
    --out-dir) OUT_DIR="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --archive-version) ARCHIVE_VERSION="$2"; shift 2 ;;
    --pack) DO_PACK=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "make-packages.sh: unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

# --- the first-slice release matrix (docs/research/npm-distribution.md §1) ---
SLICE_TARGETS=(x86_64-unknown-linux-gnu aarch64-apple-darwin)
SLICE_PKGS=(linux-x64-gnu darwin-arm64)
SLICE_OS=(linux darwin)
SLICE_CPU=(x64 arm64)
# npm>=10 `libc` field: glibc vs musl is otherwise invisible to npm's platform
# matching, and a glibc package installed on musl fails at exec time. Only
# meaningful on Linux; darwin targets omit the field (empty => line removed).
SLICE_LIBC=(glibc "")

version_from_cargo() {
  sed -n 's/^version = "\(.*\)"/\1/p' "${REPO_ROOT}/Cargo.toml" | head -n 1
}

[[ -z "${VERSION}" ]] && VERSION="$(version_from_cargo)"
[[ -z "${VERSION}" ]] && { echo "make-packages.sh: cannot determine version from Cargo.toml" >&2; exit 1; }
[[ -z "${ARCHIVE_VERSION}" ]] && ARCHIVE_VERSION="${VERSION}"

if [[ "${MODE}" == "bin-dir" ]]; then
  [[ -n "${BIN_DIR}" && -n "${NPM_PLATFORM}" ]] || { echo "make-packages.sh: --bin-dir mode requires --bin-dir and --npm-platform" >&2; usage >&2; exit 2; }
else
  [[ -n "${ARCHIVE_DIR}" ]] || { echo "make-packages.sh: --archive-dir is required (or use --bin-dir/--npm-platform)" >&2; usage >&2; exit 2; }
fi

# `npm pack --pack-destination` runs after `cd`-ing into each package dir, so a
# relative out-dir would be resolved against that dir and fail. Pin it to an
# absolute path now (relative args are interpreted against the invocation cwd).
if [[ "${OUT_DIR}" != /* ]]; then
  OUT_DIR="$(pwd)/${OUT_DIR}"
fi

pkg_index_by_target() {
  local i
  for i in "${!SLICE_TARGETS[@]}"; do
    [[ "${SLICE_TARGETS[$i]}" == "$1" ]] && { echo "$i"; return 0; }
  done
  return 1
}
pkg_index_by_name() {
  local i
  for i in "${!SLICE_PKGS[@]}"; do
    [[ "${SLICE_PKGS[$i]}" == "$1" ]] && { echo "$i"; return 0; }
  done
  return 1
}

emit_platform_metadata() {
  # emit_platform_metadata <dest-dir> <pkg-suffix> <target>
  local dest="$1" pkg_suffix="$2" target="$3"
  local i; i="$(pkg_index_by_name "${pkg_suffix}")"
  local pkg_name="@worksgood/${pkg_suffix}"
  local sed_args=(
    -e "s|__PKG_NAME__|${pkg_name}|g"
    -e "s|__WG_VERSION__|${VERSION}|g"
    -e "s|__TARGET__|${target}|g"
    -e "s|__OS__|${SLICE_OS[$i]}|g"
    -e "s|__CPU__|${SLICE_CPU[$i]}|g"
  )
  if [[ -n "${SLICE_LIBC[$i]}" ]]; then
    sed_args+=(-e "s|__LIBC__|${SLICE_LIBC[$i]}|g")
  else
    # No libc constraint for this target (non-Linux): drop the whole line.
    sed_args+=(-e '/"libc": \["__LIBC__"\],/d')
  fi
  sed "${sed_args[@]}" \
      "${SCRIPT_DIR}/platform/package.json.in" > "${dest}/package.json"
  sed -e "s|__PKG_SUFFIX__|${pkg_suffix}|g" \
      -e "s|__WG_VERSION__|${VERSION}|g" \
      -e "s|__TARGET__|${target}|g" \
      -e "s|__ARCHIVE_SUFFIX__|-${target}|g" \
      "${SCRIPT_DIR}/platform/README.md.in" > "${dest}/README.md"
}

finish_package() {
  # finish_package <dir>: every WG npm package is pure payload — no scripts,
  # ever. Guard the invariant instead of trusting the templates.
  local dir="$1"
  node -e '
    const fs = require("fs");
    const pkg = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
    const s = pkg.scripts || {};
    const forbidden = Object.keys(s).filter((k) =>
      /^(pre|post)?install$|^(pre|post)?prepare$|^prepack$|^postpack$/.test(k));
    if (forbidden.length > 0) {
      console.error(`FATAL: ${pkg.name} declares install-hook scripts: ${forbidden.join(", ")}`);
      process.exit(1);
    }
  ' "${dir}/package.json"
  chmod 755 "${dir}"/bin/* 2>/dev/null || true
}

rm -rf "${OUT_DIR}"
mkdir -p "${OUT_DIR}"

# ---------------------------------------------------------------- metapackage
META_DIR="${OUT_DIR}/@worksgood/cli"
mkdir -p "${META_DIR}"
cp -R "${SCRIPT_DIR}/cli/bin" "${SCRIPT_DIR}/cli/lib" "${SCRIPT_DIR}/cli/README.md" "${META_DIR}/"
sed -e "s/__WG_VERSION__/${VERSION}/g" "${SCRIPT_DIR}/cli/package.json" > "${META_DIR}/package.json"
cp "${REPO_ROOT}/LICENSE" "${META_DIR}/LICENSE"
finish_package "${META_DIR}"
echo "assembled ${META_DIR} (version ${VERSION})"

# ----------------------------------------------------------- platform packages
built=0
if [[ "${MODE}" == "archive" ]]; then
  tmp="$(mktemp -d)"
  trap 'rm -rf "${tmp}"' EXIT
  for target in "${SLICE_TARGETS[@]}"; do
    archive="${ARCHIVE_DIR}/wg-v${ARCHIVE_VERSION}-${target}.tar.gz"
    if [[ ! -f "${archive}" ]]; then
      echo "note: no release archive for ${target} (${archive}) — skipping (phase-2 target or not built this run)" >&2
      continue
    fi
    i="$(pkg_index_by_target "${target}")"
    pkg="${SLICE_PKGS[$i]}"
    extract="${tmp}/${target}"
    mkdir -p "${extract}"
    # Archives carry a single wg-v<version>-<target>/ root directory.
    tar -xzf "${archive}" -C "${extract}" --strip-components=1
    dest="${OUT_DIR}/@worksgood/${pkg}"
    mkdir -p "${dest}/bin"
    for binname in wg nex worksgood; do
      cp "${extract}/${binname}" "${dest}/bin/${binname}"
    done
    cp "${REPO_ROOT}/LICENSE" "${dest}/LICENSE"
    emit_platform_metadata "${dest}" "${pkg}" "${target}"
    finish_package "${dest}"
    echo "assembled ${dest} (from ${archive})"
    built=$((built + 1))
  done
  [[ ${built} -gt 0 ]] || { echo "make-packages.sh: no slice-target archives found in ${ARCHIVE_DIR}" >&2; exit 1; }
else
  i="$(pkg_index_by_name "${NPM_PLATFORM}")" || {
    echo "make-packages.sh: unknown --npm-platform ${NPM_PLATFORM} (slice: ${SLICE_PKGS[*]})" >&2; exit 2; }
  target="${SLICE_TARGETS[$i]}"
  for binname in wg nex worksgood; do
    [[ -x "${BIN_DIR}/${binname}" ]] || { echo "make-packages.sh: missing binary ${BIN_DIR}/${binname}" >&2; exit 1; }
  done
  dest="${OUT_DIR}/@worksgood/${NPM_PLATFORM}"
  mkdir -p "${dest}/bin"
  for binname in wg nex worksgood; do
    cp "${BIN_DIR}/${binname}" "${dest}/bin/${binname}"
  done
  cp "${REPO_ROOT}/LICENSE" "${dest}/LICENSE"
  emit_platform_metadata "${dest}" "${NPM_PLATFORM}" "${target}"
  finish_package "${dest}"
  echo "assembled ${dest} (from binaries in ${BIN_DIR})"
  built=$((built + 1))
fi

# ------------------------------------------------------------------ npm pack
if [[ ${DO_PACK} -eq 1 ]]; then
  for dir in "${OUT_DIR}"/@worksgood/*/; do
    (cd "${dir}" && npm pack --pack-destination "${OUT_DIR}" --loglevel=error >/dev/null)
  done
  echo "packed:"
  ls -1 "${OUT_DIR}"/*.tgz | sed 's/^/  /'
fi

# ------------------------------------------------- operator publish checklist
# npm publishing is OPERATOR-GATED: the CLI/tooling never holds an npm token.
cat <<'CHECKLIST'

===============================================================================
OPERATOR CHECKLIST — npm publish (run once, then per release)
===============================================================================
One-time setup (NOT automated; do not attempt from tooling):
  [ ] 1. Create/claim the npm org `@worksgood` (npmjs.com -> Add Organization).
  [ ] 2. Reserve ALL package names before first publish (typosquat defense):
         @worksgood/cli, @worksgood/linux-x64-gnu, @worksgood/darwin-arm64,
         plus the phase-2 names @worksgood/linux-arm64-gnu,
         @worksgood/darwin-x64, @worksgood/win32-x64,
         @worksgood/linux-x64-musl, @worksgood/linux-arm64-musl
         (cheap: publish a 0.0.0 placeholder or use npm org package settings).
  [ ] 3. Configure trusted publishing on npm for each package: link the
         graphwork/wg repository + workflow `.github/workflows/release.yml`
         (OIDC; enables `npm publish --provenance` with no token). Alternative:
         a granular automation token scoped to @worksgood packages only,
         stored as the repository secret NPM_TOKEN.
  [ ] 4. Arm the gated publish step: set the repository variable
         NPM_PUBLISH_ENABLED=true (GitHub repo -> Settings -> Variables).
         The npm-package job publishes only when BOTH the tag is a publishable
         `v*` release AND this variable is true; otherwise it stops at the
         dry-run validation.
Per release:
  [ ] 5. Rehearse on a `release-test-*` tag: the npm-package job packs and
         npm-publish-dry-runs everything; download the npm-packages artifact
         and inspect the tarballs (`tar -tzf`, `npm publish --dry-run`).
  [ ] 6. Publish order is platform packages FIRST, metapackage LAST (the
         metapackage exact-pins the platform version; see the research doc §4).
  [ ] 7. After publish, verify: `npm audit signatures`, install
         `npm i -g @worksgood/cli` in a clean container, run `wg --version`,
         and confirm the provenance link (registry provenance tab ->
         repo/commit/workflow run).
===============================================================================
CHECKLIST

echo "make-packages.sh: done (out-dir ${OUT_DIR})"
