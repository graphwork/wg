#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TMP="${TMPDIR:-/tmp}/wg-installer-test.$$"

pass() {
  printf 'PASS: %s\n' "$*"
}

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  rm -rf "$TMP"
}

trap cleanup EXIT

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os:$arch" in
    Linux:x86_64|Linux:amd64) printf 'x86_64-unknown-linux-gnu' ;;
    Linux:aarch64|Linux:arm64) printf 'aarch64-unknown-linux-gnu' ;;
    Darwin:x86_64|Darwin:amd64) printf 'x86_64-apple-darwin' ;;
    Darwin:aarch64|Darwin:arm64) printf 'aarch64-apple-darwin' ;;
    *) fail "unsupported test host $os/$arch" ;;
  esac
}

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

make_release() {
  local dir="$1"
  local version="$2"
  local channel="$3"
  local label="$4"
  local target="$5"
  local root="wg-v${version}-${target}"
  local archive="${root}.tar.gz"

  mkdir -p "$dir/$root"
  cat > "$dir/$root/worksgood" <<EOF
#!/usr/bin/env sh
printf 'worksgood ${label}\n'
EOF
  cat > "$dir/$root/wg" <<EOF
#!/usr/bin/env sh
printf 'wg ${label}\n'
EOF
  cat > "$dir/$root/nex" <<EOF
#!/usr/bin/env sh
printf 'nex ${label}\n'
EOF
  chmod +x "$dir/$root/worksgood" "$dir/$root/wg" "$dir/$root/nex"
  printf 'test license\n' > "$dir/$root/LICENSE"
  printf 'test readme\n' > "$dir/$root/README-install.txt"

  tar -C "$dir" -czf "$dir/$archive" "$root"
  local digest
  digest="$(sha256_of "$dir/$archive")"
  printf '%s  %s\n' "$digest" "$archive" > "$dir/SHA256SUMS"

  cat > "$dir/release-manifest.json" <<EOF
{
  "schema_version": 1,
  "package": "workgraph",
  "release_name": "wg",
  "version": "${version}",
  "tag": "v${version}",
  "channel": "${channel}",
  "dry_run": false,
  "publish": true,
  "repository": "graphwork/wg",
  "binaries": ["worksgood", "wg", "nex"],
  "checksums": {
    "algorithm": "sha256",
    "file": "SHA256SUMS"
  },
  "assets": [
    {
      "target": "${target}",
      "archive": "${archive}",
      "format": "tar.gz",
      "sha256": "${digest}",
      "url": null,
      "github_release_url_template": "file://${dir}/${archive}",
      "checksum": "${archive}.sha256",
      "attestation_bundle": "${archive}.intoto.jsonl",
      "gh_attestation_verify": "gh attestation verify ${archive} --repo graphwork/wg"
    }
  ]
}
EOF
}

run_installer() {
  HOME="$1" PATH="/usr/bin:/bin" sh "$ROOT/scripts/install-wg.sh" "${@:2}"
}

assert_file() {
  [[ -f "$1" ]] || fail "expected file: $1"
}

assert_executable() {
  [[ -x "$1" ]] || fail "expected executable: $1"
}

mkdir -p "$TMP"
TARGET="$(detect_target)"

RELEASE_ONE="$TMP/release-one"
RELEASE_TWO="$TMP/release-two"
BAD_RELEASE="$TMP/bad-release"
make_release "$RELEASE_ONE" "0.99.0" "stable" "first" "$TARGET"
make_release "$RELEASE_TWO" "1.2.3" "nightly" "second" "$TARGET"
cp -R "$RELEASE_ONE" "$BAD_RELEASE"
printf '0000000000000000000000000000000000000000000000000000000000000000  wg-v0.99.0-%s.tar.gz\n' "$TARGET" > "$BAD_RELEASE/SHA256SUMS"

HOME_ONE="$TMP/home-one"
INSTALL_ONE="$HOME_ONE/.local/bin"
mkdir -p "$HOME_ONE"
OUTPUT_ONE="$TMP/install-one.out"
run_installer "$HOME_ONE" \
  --base-url "file://$RELEASE_ONE" \
  --install-dir "$INSTALL_ONE" \
  --channel stable > "$OUTPUT_ONE" 2>&1

assert_executable "$INSTALL_ONE/worksgood"
assert_executable "$INSTALL_ONE/wg"
assert_executable "$INSTALL_ONE/nex"
[[ "$("$INSTALL_ONE/worksgood")" == "worksgood first" ]] || fail "worksgood did not run from first install"
[[ "$("$INSTALL_ONE/wg")" == "wg first" ]] || fail "wg did not run from first install"
[[ "$("$INSTALL_ONE/nex")" == "nex first" ]] || fail "nex did not run from first install"
grep -q 'checksum: OK' "$OUTPUT_ONE" || fail "install output did not report checksum OK"
grep -q 'attestation: skipped' "$OUTPUT_ONE" || fail "install output did not report attestation status"
grep -q '^  worksgood$' "$OUTPUT_ONE" || fail "install output did not print attended next step"
grep -q 'complete expert CLI' "$OUTPUT_ONE" || fail "install output did not preserve the wg expert boundary"
assert_file "$HOME_ONE/.wg/install-receipt.toml"
grep -q 'version = "0.99.0"' "$HOME_ONE/.wg/install-receipt.toml" || fail "receipt missing installed version"
grep -q 'binaries = \["worksgood", "wg", "nex"\]' "$HOME_ONE/.wg/install-receipt.toml" || fail "receipt missing exact binary set"
pass "shell installer installs worksgood, wg, and nex into temp HOME without sudo"

DRY_HOME="$TMP/home-dry"
DRY_INSTALL="$DRY_HOME/bin"
mkdir -p "$DRY_HOME"
OUTPUT_DRY="$TMP/dry-run.out"
run_installer "$DRY_HOME" \
  --base-url "file://$RELEASE_ONE" \
  --install-dir "$DRY_INSTALL" \
  --channel stable \
  --version v0.99.0 \
  --dry-run > "$OUTPUT_DRY" 2>&1
[[ ! -e "$DRY_INSTALL/worksgood" && ! -e "$DRY_INSTALL/wg" && ! -e "$DRY_INSTALL/nex" ]] || fail "dry-run installed a binary"
grep -q 'dry-run: would install worksgood, wg, and nex' "$OUTPUT_DRY" || fail "dry-run did not print exact install plan"
pass "shell installer supports dry-run and explicit version"

OUTPUT_UPGRADE="$TMP/upgrade.out"
run_installer "$HOME_ONE" \
  --base-url "file://$RELEASE_TWO" \
  --install-dir "$INSTALL_ONE" \
  --channel nightly \
  --version v1.2.3 > "$OUTPUT_UPGRADE" 2>&1
[[ "$("$INSTALL_ONE/worksgood")" == "worksgood second" ]] || fail "worksgood was not replaced on reinstall/upgrade"
[[ "$("$INSTALL_ONE/wg")" == "wg second" ]] || fail "wg was not replaced on reinstall/upgrade"
[[ "$("$INSTALL_ONE/nex")" == "nex second" ]] || fail "nex was not replaced on reinstall/upgrade"
grep -q 'channel = "nightly"' "$HOME_ONE/.wg/install-receipt.toml" || fail "receipt missing nightly channel"
grep -q 'version = "1.2.3"' "$HOME_ONE/.wg/install-receipt.toml" || fail "receipt missing upgraded version"
pass "shell installer upgrades the exact worksgood/wg/nex set"

FOREIGN_HOME="$TMP/home-foreign"
FOREIGN_INSTALL="$FOREIGN_HOME/bin"
mkdir -p "$FOREIGN_INSTALL"
printf 'foreign command\n' > "$FOREIGN_INSTALL/worksgood"
OUTPUT_FOREIGN="$TMP/foreign.out"
if run_installer "$FOREIGN_HOME" \
  --base-url "file://$RELEASE_ONE" \
  --install-dir "$FOREIGN_INSTALL" \
  --channel stable > "$OUTPUT_FOREIGN" 2>&1; then
  fail "installer overwrote an unreceipted foreign command"
fi
grep -q 'refusing to overwrite existing command' "$OUTPUT_FOREIGN" || fail "foreign collision refusal was not explicit"
[[ "$(cat "$FOREIGN_INSTALL/worksgood")" == "foreign command" ]] || fail "foreign command changed"
pass "shell installer refuses foreign command collisions"

BAD_HOME="$TMP/home-bad"
BAD_INSTALL="$BAD_HOME/bin"
mkdir -p "$BAD_HOME"
OUTPUT_BAD="$TMP/bad.out"
if run_installer "$BAD_HOME" \
  --base-url "file://$BAD_RELEASE" \
  --install-dir "$BAD_INSTALL" \
  --channel stable > "$OUTPUT_BAD" 2>&1; then
  fail "installer accepted checksum mismatch"
fi
grep -q 'checksum verification failed' "$OUTPUT_BAD" || fail "checksum mismatch did not print verification failure"
[[ ! -e "$BAD_INSTALL/wg" ]] || fail "checksum failure installed wg"
pass "shell installer refuses checksum mismatches"

printf 'foreign sibling\n' > "$INSTALL_ONE/not-ours"
OUTPUT_UNINSTALL_DRY="$TMP/uninstall-dry.out"
run_installer "$HOME_ONE" --install-dir "$INSTALL_ONE" --uninstall --dry-run > "$OUTPUT_UNINSTALL_DRY" 2>&1
for bin in worksgood wg nex; do
  assert_executable "$INSTALL_ONE/$bin"
  grep -q "dry-run: would remove $INSTALL_ONE/$bin" "$OUTPUT_UNINSTALL_DRY" || fail "dry-run omitted $bin removal"
done
run_installer "$HOME_ONE" --install-dir "$INSTALL_ONE" --uninstall > "$TMP/uninstall.out" 2>&1
for bin in worksgood wg nex; do
  [[ ! -e "$INSTALL_ONE/$bin" ]] || fail "uninstall left $bin"
done
[[ -f "$INSTALL_ONE/not-ours" ]] || fail "uninstall removed a foreign sibling"
[[ ! -e "$HOME_ONE/.wg/install-receipt.toml" ]] || fail "uninstall left receipt"
pass "shell installer dry-run/uninstall removes only the receipted binary set"

if command -v pwsh >/dev/null 2>&1; then
  PS_HOME="$TMP/home-pwsh"
  PS_INSTALL="$PS_HOME/.local/bin"
  mkdir -p "$PS_HOME"
  OUTPUT_PS="$TMP/pwsh.out"
  HOME="$PS_HOME" pwsh -NoLogo -NoProfile -File "$ROOT/scripts/install-wg.ps1" \
    -BaseUrl "file://$RELEASE_ONE" \
    -InstallDir "$PS_INSTALL" \
    -Channel stable > "$OUTPUT_PS" 2>&1
  assert_executable "$PS_INSTALL/worksgood"
  assert_executable "$PS_INSTALL/wg"
  assert_executable "$PS_INSTALL/nex"
  grep -q 'checksum: OK' "$OUTPUT_PS" || fail "PowerShell installer did not report checksum OK"
  pass "PowerShell installer works on this host"
else
  printf 'SKIP: pwsh not installed; PowerShell installer not executed on this host\n'
fi

printf 'All installer smoke tests passed for %s\n' "$TARGET"
