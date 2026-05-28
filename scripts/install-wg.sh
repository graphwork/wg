#!/bin/sh
set -eu

DEFAULT_REPO="graphwork/wg"

CHANNEL="${WG_INSTALL_CHANNEL:-stable}"
VERSION="${WG_INSTALL_VERSION:-}"
INSTALL_DIR="${WG_INSTALL_DIR:-}"
REPO="${WG_INSTALL_REPO:-$DEFAULT_REPO}"
BASE_URL="${WG_INSTALL_BASE_URL:-}"
TARGET="${WG_INSTALL_TARGET:-}"
DEV_DIR="${WG_INSTALL_DEV_DIR:-}"
DRY_RUN="${WG_INSTALL_DRY_RUN:-0}"

WORK_DIR=""
VERIFIED_SHA256=""

usage() {
  cat <<'USAGE'
Install WG native binaries.

Usage:
  sh install-wg.sh [options]

Options:
  --channel stable|nightly|dev   Release channel to install (default: stable).
  --version VERSION              Install an explicit release tag/version.
  --install-dir DIR              Install wg and nex into DIR.
  --dry-run                      Resolve and print actions without installing.
  --repo OWNER/REPO              GitHub repository (default: graphwork/wg).
  --base-url URL                 Mirror/test URL containing release-manifest.json,
                                 SHA256SUMS, and the target archive.
  --target TRIPLE                Override detected release target triple.
  --dev-dir DIR                  Source checkout for --channel dev.
  -h, --help                     Show this help.

Environment variables mirror the long options:
  WG_INSTALL_CHANNEL, WG_INSTALL_VERSION, WG_INSTALL_DIR,
  WG_INSTALL_REPO, WG_INSTALL_BASE_URL, WG_INSTALL_TARGET,
  WG_INSTALL_DEV_DIR, WG_INSTALL_DRY_RUN.
USAGE
}

say() {
  printf '%s\n' "$*"
}

warn() {
  printf 'warning: %s\n' "$*" >&2
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

have() {
  command -v "$1" >/dev/null 2>&1
}

cleanup() {
  if [ -n "$WORK_DIR" ] && [ -d "$WORK_DIR" ]; then
    rm -rf "$WORK_DIR"
  fi
}

trap cleanup EXIT HUP INT TERM

while [ "$#" -gt 0 ]; do
  case "$1" in
    --channel)
      [ "$#" -ge 2 ] || die "--channel requires a value"
      CHANNEL="$2"
      shift 2
      ;;
    --version)
      [ "$#" -ge 2 ] || die "--version requires a value"
      VERSION="$2"
      shift 2
      ;;
    --install-dir)
      [ "$#" -ge 2 ] || die "--install-dir requires a value"
      INSTALL_DIR="$2"
      shift 2
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --repo)
      [ "$#" -ge 2 ] || die "--repo requires a value"
      REPO="$2"
      shift 2
      ;;
    --base-url)
      [ "$#" -ge 2 ] || die "--base-url requires a value"
      BASE_URL="$2"
      shift 2
      ;;
    --target)
      [ "$#" -ge 2 ] || die "--target requires a value"
      TARGET="$2"
      shift 2
      ;;
    --dev-dir)
      [ "$#" -ge 2 ] || die "--dev-dir requires a value"
      DEV_DIR="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

case "$DRY_RUN" in
  1|true|TRUE|yes|YES) DRY_RUN=1 ;;
  0|false|FALSE|no|NO|"") DRY_RUN=0 ;;
  *) die "WG_INSTALL_DRY_RUN must be 0/1, true/false, or yes/no" ;;
esac

case "$CHANNEL" in
  stable|nightly|dev) ;;
  *) die "--channel must be stable, nightly, or dev" ;;
esac

if [ -z "${HOME:-}" ]; then
  die "HOME is not set"
fi

make_work_dir() {
  if [ -z "$WORK_DIR" ]; then
    WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/wg-install.XXXXXX")"
  fi
}

trim_trailing_slashes() {
  printf '%s' "$1" | sed 's:/*$::'
}

download_file() {
  url="$1"
  dest="$2"

  case "$url" in
    file://*)
      src="${url#file://}"
      cp "$src" "$dest"
      ;;
    /*)
      cp "$url" "$dest"
      ;;
    *)
      if have curl; then
        curl -fsSL --retry 3 --retry-delay 1 "$url" -o "$dest"
      elif have wget; then
        wget -q -O "$dest" "$url"
      else
        die "curl or wget is required to download $url"
      fi
      ;;
  esac
}

json_string_field() {
  key="$1"
  file="$2"
  sed -n 's/^[[:space:]]*"'"$key"'"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$file" | head -n 1
}

version_to_tag() {
  value="$1"
  case "$value" in
    v*|nightly*|release-test-*|dry-run-*) printf '%s' "$value" ;;
    *) printf 'v%s' "$value" ;;
  esac
}

detect_target() {
  if [ -n "$TARGET" ]; then
    printf '%s' "$TARGET"
    return
  fi

  os="$(uname -s 2>/dev/null || printf unknown)"
  arch="$(uname -m 2>/dev/null || printf unknown)"

  case "$os" in
    Linux)
      case "$arch" in
        x86_64|amd64) printf 'x86_64-unknown-linux-gnu' ;;
        aarch64|arm64) printf 'aarch64-unknown-linux-gnu' ;;
        *) die "unsupported Linux architecture: $arch" ;;
      esac
      ;;
    Darwin)
      case "$arch" in
        x86_64|amd64) printf 'x86_64-apple-darwin' ;;
        aarch64|arm64) printf 'aarch64-apple-darwin' ;;
        *) die "unsupported macOS architecture: $arch" ;;
      esac
      ;;
    MINGW*|MSYS*|CYGWIN*)
      case "$arch" in
        x86_64|amd64) printf 'x86_64-pc-windows-msvc' ;;
        *) die "unsupported Windows shell architecture: $arch; use install-wg.ps1 on Windows" ;;
      esac
      ;;
    *)
      die "unsupported OS: $os"
      ;;
  esac
}

archive_ext_for_target() {
  case "$1" in
    x86_64-pc-windows-msvc) printf '.zip' ;;
    *) printf '.tar.gz' ;;
  esac
}

exe_ext_for_target() {
  case "$1" in
    x86_64-pc-windows-msvc) printf '.exe' ;;
    *) printf '' ;;
  esac
}

choose_install_dir() {
  if [ -n "$INSTALL_DIR" ]; then
    printf '%s' "$INSTALL_DIR"
    return
  fi

  if [ -d "$HOME/.local/bin" ] && [ -w "$HOME/.local/bin" ]; then
    printf '%s' "$HOME/.local/bin"
    return
  fi

  if [ ! -e "$HOME/.local/bin" ] && { [ -w "$HOME" ] || [ -w "$HOME/.local" ] 2>/dev/null; }; then
    printf '%s' "$HOME/.local/bin"
    return
  fi

  if [ -d "$HOME/bin" ] && [ -w "$HOME/bin" ]; then
    printf '%s' "$HOME/bin"
    return
  fi

  if [ ! -e "$HOME/bin" ] && [ -w "$HOME" ]; then
    printf '%s' "$HOME/bin"
    return
  fi

  die "no user-writable install directory found; pass --install-dir DIR"
}

ensure_install_dir() {
  dir="$1"

  if [ "$DRY_RUN" = 1 ]; then
    say "dry-run: would create/use install dir $dir"
    return
  fi

  mkdir -p "$dir"
  [ -d "$dir" ] || die "install dir is not a directory: $dir"
  [ -w "$dir" ] || die "install dir is not writable: $dir"
}

sha256_file() {
  file="$1"
  if have sha256sum; then
    sha256sum "$file" | awk '{print $1}'
  elif have shasum; then
    shasum -a 256 "$file" | awk '{print $1}'
  else
    die "sha256sum or shasum is required for checksum verification"
  fi
}

expected_checksum() {
  archive_name="$1"
  checksums_file="$2"
  awk -v name="$archive_name" '
    {
      file = $NF
      sub(/^\*/, "", file)
      if (file == name) {
        print tolower($1)
        exit
      }
    }
  ' "$checksums_file"
}

verify_checksum() {
  archive_name="$1"
  archive_path="$2"
  checksums_file="$3"

  expected="$(expected_checksum "$archive_name" "$checksums_file")"
  [ -n "$expected" ] || die "checksum for $archive_name not found in SHA256SUMS"

  actual="$(sha256_file "$archive_path" | tr '[:upper:]' '[:lower:]')"
  if [ "$actual" != "$expected" ]; then
    die "checksum verification failed for $archive_name: expected $expected, got $actual"
  fi

  say "checksum: OK ($actual)"
  VERIFIED_SHA256="$actual"
}

verify_attestations() {
  archive_name="$1"
  repo="$2"
  base_url="$3"

  if ! have gh; then
    say "attestation: skipped (gh not installed)"
    return
  fi

  if [ -n "$base_url" ]; then
    say "attestation: skipped (custom base URL/mirror)"
    return
  fi

  say "attestation: verifying release-manifest.json, SHA256SUMS, and $archive_name with gh"
  (
    cd "$WORK_DIR"
    gh attestation verify release-manifest.json --repo "$repo"
    gh attestation verify SHA256SUMS --repo "$repo"
    gh attestation verify "$archive_name" --repo "$repo"
  )
  say "attestation: OK"
}

extract_archive() {
  archive_path="$1"
  extract_dir="$2"

  mkdir -p "$extract_dir"
  case "$archive_path" in
    *.tar.gz)
      tar -xzf "$archive_path" -C "$extract_dir"
      ;;
    *.zip)
      have unzip || die "unzip is required to extract Windows release archives"
      unzip -q "$archive_path" -d "$extract_dir"
      ;;
    *)
      die "unsupported archive format: $archive_path"
      ;;
  esac
}

install_binary() {
  src="$1"
  dest="$2"

  tmp_dest="${dest}.wg-install.$$"
  cp "$src" "$tmp_dest"
  chmod 0755 "$tmp_dest"
  mv -f "$tmp_dest" "$dest"
}

toml_escape() {
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

write_receipt() {
  version="$1"
  channel="$2"
  target="$3"
  install_dir="$4"
  release_url="$5"
  artifact_sha256="$6"
  archive_name="$7"

  if [ "$DRY_RUN" = 1 ]; then
    say "dry-run: would write receipt $HOME/.wg/install-receipt.toml"
    return
  fi

  receipt_dir="$HOME/.wg"
  mkdir -p "$receipt_dir"
  receipt_tmp="$receipt_dir/.install-receipt.toml.$$"
  installed_at="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"

  {
    printf 'manager = "wg-installer"\n'
    printf 'version = "%s"\n' "$(toml_escape "$version")"
    printf 'channel = "%s"\n' "$(toml_escape "$channel")"
    printf 'target = "%s"\n' "$(toml_escape "$target")"
    printf 'installed_at = "%s"\n' "$(toml_escape "$installed_at")"
    printf 'binary_dir = "%s"\n' "$(toml_escape "$install_dir")"
    printf 'release_url = "%s"\n' "$(toml_escape "$release_url")"
    printf 'artifact_sha256 = "%s"\n' "$(toml_escape "$artifact_sha256")"
    printf 'archive = "%s"\n' "$(toml_escape "$archive_name")"
    printf 'repository = "%s"\n' "$(toml_escape "$REPO")"
  } > "$receipt_tmp"
  chmod 0600 "$receipt_tmp"
  mv -f "$receipt_tmp" "$receipt_dir/install-receipt.toml"
}

print_next_steps() {
  install_dir="$1"
  exe_ext="$2"

  say ""
  say "WG installed:"
  say "  wg  $install_dir/wg$exe_ext"
  say "  nex $install_dir/nex$exe_ext"
  say ""

  case ":${PATH:-}:" in
    *:"$install_dir":*) ;;
    *)
      warn "$install_dir is not on PATH; add it before running wg"
      ;;
  esac

  say "Next:"
  say "  wg setup"
  say "  cd your-project"
  say "  wg init"
  say "  wg service start"
  say "  wg tui"
}

install_dev_channel() {
  target="$1"
  exe_ext="$2"
  install_dir="$3"

  dev_dir="$DEV_DIR"
  if [ -z "$dev_dir" ]; then
    dev_dir="$(pwd)"
  fi

  [ -f "$dev_dir/Cargo.toml" ] || die "--channel dev requires a WG source checkout; pass --dev-dir DIR"
  version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$dev_dir/Cargo.toml" | head -n 1)"
  [ -n "$version" ] || version="dev"

  say "channel: dev"
  say "source: $dev_dir"
  say "target: $target"
  say "install dir: $install_dir"
  say "checksum: skipped (dev channel builds local source)"
  say "attestation: skipped (dev channel builds local source)"

  if [ "$DRY_RUN" = 1 ]; then
    say "dry-run: would run cargo install --path $dev_dir --locked --root <temp>"
    print_next_steps "$install_dir" "$exe_ext"
    return
  fi

  have cargo || die "cargo is required for --channel dev"
  make_work_dir
  cargo_root="$WORK_DIR/cargo-root"
  cargo install --path "$dev_dir" --locked --root "$cargo_root" --force

  install_binary "$cargo_root/bin/wg$exe_ext" "$install_dir/wg$exe_ext"
  install_binary "$cargo_root/bin/nex$exe_ext" "$install_dir/nex$exe_ext"
  write_receipt "$version" "dev" "$target" "$install_dir" "file://$dev_dir" "dev" "dev"
  print_next_steps "$install_dir" "$exe_ext"
}

install_release_channel() {
  target="$1"
  exe_ext="$2"
  archive_ext="$3"
  install_dir="$4"

  make_work_dir
  manifest_path="$WORK_DIR/release-manifest.json"

  base_url="$(trim_trailing_slashes "$BASE_URL")"
  if [ -n "$base_url" ]; then
    manifest_url="$base_url/release-manifest.json"
    release_base="$base_url"
    release_url="$base_url"
  else
    if [ -n "$VERSION" ]; then
      tag="$(version_to_tag "$VERSION")"
      release_base="https://github.com/$REPO/releases/download/$tag"
      manifest_url="$release_base/release-manifest.json"
      release_url="https://github.com/$REPO/releases/tag/$tag"
    elif [ "$CHANNEL" = "stable" ]; then
      manifest_url="https://github.com/$REPO/releases/latest/download/release-manifest.json"
      release_base="https://github.com/$REPO/releases/latest/download"
      release_url="https://github.com/$REPO/releases/latest"
    else
      tag="nightly"
      release_base="https://github.com/$REPO/releases/download/$tag"
      manifest_url="$release_base/release-manifest.json"
      release_url="https://github.com/$REPO/releases/tag/$tag"
    fi
  fi

  say "manifest: $manifest_url"
  download_file "$manifest_url" "$manifest_path"

  manifest_version="$(json_string_field version "$manifest_path")"
  [ -n "$manifest_version" ] || die "release-manifest.json is missing version"

  manifest_tag="$(json_string_field tag "$manifest_path" || true)"
  manifest_channel="$(json_string_field channel "$manifest_path" || true)"
  if [ -n "$manifest_channel" ] && [ "$manifest_channel" != "$CHANNEL" ]; then
    warn "requested channel $CHANNEL but manifest reports $manifest_channel"
  fi

  if [ -z "$base_url" ] && [ -z "$VERSION" ] && [ "$CHANNEL" = "stable" ] && [ -n "$manifest_tag" ]; then
    release_base="https://github.com/$REPO/releases/download/$manifest_tag"
    release_url="https://github.com/$REPO/releases/tag/$manifest_tag"
  fi

  archive_name="wg-v$manifest_version-$target$archive_ext"
  archive_url="$release_base/$archive_name"
  checksums_url="$release_base/SHA256SUMS"
  archive_path="$WORK_DIR/$archive_name"
  checksums_path="$WORK_DIR/SHA256SUMS"

  say "channel: $CHANNEL"
  say "version: $manifest_version"
  say "target: $target"
  say "archive: $archive_url"
  say "install dir: $install_dir"

  if [ "$DRY_RUN" = 1 ]; then
    say "dry-run: would download $archive_url"
    say "dry-run: would verify SHA256 from $checksums_url"
    if have gh && [ -z "$base_url" ]; then
      say "dry-run: would verify GitHub attestations for release-manifest.json, SHA256SUMS, and the archive with gh"
    else
      say "dry-run: attestation would be skipped unless gh and a GitHub release are available"
    fi
    say "dry-run: would install wg$exe_ext and nex$exe_ext into $install_dir"
    say "dry-run: would write receipt $HOME/.wg/install-receipt.toml"
    print_next_steps "$install_dir" "$exe_ext"
    return
  fi

  download_file "$archive_url" "$archive_path"
  download_file "$checksums_url" "$checksums_path"

  verify_checksum "$archive_name" "$archive_path" "$checksums_path"
  artifact_sha256="$VERIFIED_SHA256"
  verify_attestations "$archive_name" "$REPO" "$base_url"

  extract_dir="$WORK_DIR/extract"
  extract_archive "$archive_path" "$extract_dir"
  payload_dir="$(find "$extract_dir" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
  [ -n "$payload_dir" ] || die "archive did not contain a top-level directory"
  [ -f "$payload_dir/wg$exe_ext" ] || die "archive is missing wg$exe_ext"
  [ -f "$payload_dir/nex$exe_ext" ] || die "archive is missing nex$exe_ext"

  install_binary "$payload_dir/wg$exe_ext" "$install_dir/wg$exe_ext"
  install_binary "$payload_dir/nex$exe_ext" "$install_dir/nex$exe_ext"
  write_receipt "$manifest_version" "$CHANNEL" "$target" "$install_dir" "$release_url" "$artifact_sha256" "$archive_name"
  print_next_steps "$install_dir" "$exe_ext"
}

TARGET="$(detect_target)"
ARCHIVE_EXT="$(archive_ext_for_target "$TARGET")"
EXE_EXT="$(exe_ext_for_target "$TARGET")"
INSTALL_DIR="$(choose_install_dir)"
ensure_install_dir "$INSTALL_DIR"

if [ "$CHANNEL" = "dev" ]; then
  install_dev_channel "$TARGET" "$EXE_EXT" "$INSTALL_DIR"
else
  install_release_channel "$TARGET" "$EXE_EXT" "$ARCHIVE_EXT" "$INSTALL_DIR"
fi
