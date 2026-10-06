#!/usr/bin/env bash
# Proof harness: dependency-only immutable hardlink baseline (Cow survey Option B).
#
# Runs entirely in an isolated instance: its own HOME, its own CARGO_HOME,
# its own CARGO_TARGET_DIR/cache root, and --dir-independent temp dirs. It does
# NOT touch /home/bot/.cache/wg/build-targets, the live .wg, or any worktree.
#
# It proves, with a real Cargo build:
#   1. a cold baseline target tree can be built once and frozen read-only;
#   2. a second attempt's private layer can be composed over it by hardlinking
#      ONLY registry/git dependency artifacts (rlib/rmeta/proc-macro .so/dep-info
#      + immutable fingerprint data) and privately copying everything mutable
#      (workspace crate artifacts, build-script out/, invoked.timestamp,
#      top-level binaries);
#   3. Cargo in a DIFFERENT source root accepts the hardlinked registry deps as
#      fresh and recompiles only the workspace crate(s);
#   4. the second attempt's private physical footprint is a fraction of the cold
#      tree, and the baseline stays byte-identical.
#
# Usage: dependency-hardlink-baseline-proof.sh [ISOLATED_DIR]
set -euo pipefail

ISO="${1:-/tmp/wg-cow-proof}"
REPO_ROOT="${WG_PROOF_SOURCE_REPO:-$(git rev-parse --show-toplevel)}"
RUSTUP_HOME_DIR="${RUSTUP_HOME:-$HOME/.rustup}"
SHARED_CARGO_REGISTRY="${WG_PROOF_SHARED_REGISTRY:-$HOME/.cargo/registry}"
SHARED_CARGO_GIT="${WG_PROOF_SHARED_GIT:-$HOME/.cargo/git}"
# Workspace (own-package) target names — never shared.
WS="${WG_PROOF_WORKSPACE_TARGETS:-worksgood wg nex casa-adapter}"

log() { printf '\n=== %s ===\n' "$*"; }
b() { numfmt --to=iec "${1:-0}"; }

log "isolated instance: $ISO  (source repo: $REPO_ROOT)"
chmod -R u+w "$ISO" 2>/dev/null || true
rm -rf "$ISO"
mkdir -p "$ISO/home" "$ISO/cargo-home" "$ISO/checkout-A" "$ISO/checkout-B" "$ISO/baseline"

# --- private CARGO_HOME (copy the registry so the build is offline+isolated) ---
cp -a "$SHARED_CARGO_REGISTRY" "$ISO/cargo-home/registry"
[ -d "$SHARED_CARGO_GIT" ] && cp -a "$SHARED_CARGO_GIT" "$ISO/cargo-home/git" || true

# --- two independent clean checkouts at the SAME submitted HEAD, DIFFERENT roots ---
git -C "$REPO_ROOT" archive HEAD | tar -x -C "$ISO/checkout-A"
git -C "$REPO_ROOT" archive HEAD | tar -x -C "$ISO/checkout-B"

export HOME="$ISO/home" RUSTUP_HOME="$RUSTUP_HOME_DIR" CARGO_HOME="$ISO/cargo-home"
export CARGO_INCREMENTAL=0

# ---------------------------------------------------------------------------
# 1) cold baseline build
# ---------------------------------------------------------------------------
log "1. cold baseline build (checkout-A)"
BASE_TARGET="$ISO/baseline/target"
t0=$(date +%s)
( cd "$ISO/checkout-A" && CARGO_TARGET_DIR="$BASE_TARGET" cargo build --locked --offline ) \
  >"$ISO/baseline-build.log" 2>&1
t1=$(date +%s)
echo "baseline build: $((t1-t0))s, logical size $(du -sh "$BASE_TARGET" | cut -f1)"
du -sh "$BASE_TARGET"/debug/deps "$BASE_TARGET"/debug/build "$BASE_TARGET"/debug/.fingerprint

# freeze the baseline read-only WITHOUT stripping exec bits
find "$ISO/baseline" -type d -exec chmod a-w {} +
find "$ISO/baseline" -type f -exec chmod a-w {} +

# content snapshot for later immutability check
( cd "$BASE_TARGET" && find . -type f -printf '%P\0' | sort -z | xargs -0 sha256sum ) \
  > "$ISO/baseline.content.sha"
echo "frozen; snapshot sha: $(sha256sum "$ISO/baseline.content.sha" | cut -d' ' -f1)"

# ---------------------------------------------------------------------------
# 2) compose the second attempt's private layer (seed-dependency-layer)
# ---------------------------------------------------------------------------
log "2. compose private layer over baseline (hardlink deps, copy the rest)"
_is_ws() { local n="${1//-/_}" w; for w in $WS; do [[ "$n" == "${w//-/_}" ]] && return 0; done; return 1; }
_strip_hash() { sed -E 's/-[0-9a-f]{16}$//'; }
declare -A SEEN=()
_link() { mkdir -p "$(dirname "$2")"; ln "$1" "$2"; }
_priv() {
  local ino; ino="$(stat -c '%d:%i' "$1")"; mkdir -p "$(dirname "$2")"
  if [[ -n "${SEEN[$ino]:-}" ]]; then ln "${SEEN[$ino]}" "$2"
  else cp -a "$1" "$2"; chmod u+w "$2"; SEEN[$ino]="$2"; fi
}
seed_dependency_layer() {
  local BASE="$1" DEST="$2"
  mkdir -p "$DEST/debug/deps" "$DEST/debug/.fingerprint"
  # deps/ : registry -> hardlink, workspace -> private copy
  local f n stem name
  for f in "$BASE"/debug/deps/*; do
    [[ -e "$f" ]] || continue; n="$(basename "$f")"; stem="${n%.*}"
    name="$(printf '%s' "$stem" | sed -E 's/^lib//' | _strip_hash)"
    if _is_ws "$name"; then _priv "$f" "$DEST/debug/deps/$n"; else _link "$f" "$DEST/debug/deps/$n"; fi
  done
  # .fingerprint/ : immutable registry fingerprint data -> hardlink; timestamp+ws -> copy
  local d fn
  for d in "$BASE"/debug/.fingerprint/*/; do
    [[ -d "$d" ]] || continue; n="$(basename "$d")"; name="$(printf '%s' "$n" | _strip_hash)"
    mkdir -p "$DEST/debug/.fingerprint/$n"
    for f in "$d"*; do
      [[ -e "$f" ]] || continue; fn="$(basename "$f")"
      if _is_ws "$name" || [[ "$fn" == "invoked.timestamp" ]]; then _priv "$f" "$DEST/debug/.fingerprint/$n/$fn"
      else _link "$f" "$DEST/debug/.fingerprint/$n/$fn"; fi
    done
  done
  # build/ : mutable build-script output -> private copy
  [[ -d "$BASE/debug/build" ]] && { cp -a "$BASE/debug/build" "$DEST/debug/build"; chmod -R u+w "$DEST/debug/build"; }
  # every other top-level debug entry (binaries, dep-info, misc dirs) -> private
  local e
  for e in "$BASE"/debug/*; do
    [[ -e "$e" ]] || continue; fn="$(basename "$e")"
    case "$fn" in .fingerprint|deps|build|.rustc_info.json|.cargo-lock) continue;; esac
    if [[ -d "$e" ]]; then cp -a "$e" "$DEST/debug/$fn"; chmod -R u+w "$DEST/debug/$fn"
    else _priv "$e" "$DEST/debug/$fn"; fi
  done
  for e in "$BASE"/*; do [[ -f "$e" ]] || continue; _priv "$e" "$DEST/$(basename "$e")"; done
}
LAYER_TARGET="$ISO/layer/target"
mkdir -p "$ISO/layer"
seed_dependency_layer "$BASE_TARGET" "$LAYER_TARGET"

# ---------------------------------------------------------------------------
# 3) accurate physical accounting (unique private inodes) + free-block delta
# ---------------------------------------------------------------------------
private_bytes() { # $1=baseline root  $2=candidate root
python3 - "$1" "$2" <<'PY'
import os,sys
base,cand=sys.argv[1],sys.argv[2]
def inodes(r):
    s=set()
    for dp,_,fns in os.walk(r):
        for fn in fns:
            try: st=os.lstat(os.path.join(dp,fn))
            except OSError: continue
            s.add((st.st_dev,st.st_ino))
    return s
bi=inodes(base); seen=set(); priv=0; shared=0
for dp,_,fns in os.walk(cand):
    for fn in fns:
        try: st=os.lstat(os.path.join(dp,fn))
        except OSError: continue
        k=(st.st_dev,st.st_ino)
        if k in bi: shared+=1
        elif k in seen: pass
        else: seen.add(k); priv+=st.st_blocks*512
print(f"{priv} {len(seen)} {shared}")
PY
}
read -r SEED_PRIV SEED_INODES SHARED_INODES <<<"$(private_bytes "$BASE_TARGET" "$LAYER_TARGET")"
echo "seed: shared(hardlinked) files=$SHARED_INODES  private inodes=$SEED_INODES  private bytes=$SEED_PRIV ($(b "$SEED_PRIV"))"

# ---------------------------------------------------------------------------
# 4) real rebuild in a DIFFERENT source root over the composed layer
# ---------------------------------------------------------------------------
log "3. real rebuild in checkout-B (different source root) over the layer"
echo "// proof touch $(date +%s%N)" >> "$ISO/checkout-B/src/lib.rs"
t0=$(date +%s)
( cd "$ISO/checkout-B" && CARGO_TARGET_DIR="$LAYER_TARGET" cargo build --locked --offline ) \
  >"$ISO/layer-build.log" 2>&1
t1=$(date +%s)
echo "layer build: $((t1-t0))s"
echo "crates compiled: $(grep -c '^   Compiling' "$ISO/layer-build.log" || true)"
grep '^   Compiling' "$ISO/layer-build.log" | head
grep -E 'Finished|error' "$ISO/layer-build.log" | tail -3

read -r FIN_PRIV FIN_INODES SHARED_INODES <<<"$(private_bytes "$BASE_TARGET" "$LAYER_TARGET")"
echo "final: private bytes=$FIN_PRIV ($(b "$FIN_PRIV"))  baseline logical=$(du -sh "$BASE_TARGET" | cut -f1)"

# binary works?
HOME="$ISO/home" "$LAYER_TARGET/debug/wg" --version 2>&1 | head -1 || echo "wg --version: (non-fatal)"

# ---------------------------------------------------------------------------
# 5) baseline immutability check
# ---------------------------------------------------------------------------
log "4. baseline immutability"
( cd "$BASE_TARGET" && find . -type f -printf '%P\0' | sort -z | xargs -0 sha256sum ) \
  > "$ISO/baseline.content.after.sha"
if cmp -s "$ISO/baseline.content.sha" "$ISO/baseline.content.after.sha"; then
  echo "PASS: every baseline file byte-identical after the second attempt"
else
  echo "FAIL: baseline content changed"; diff <(head -3 "$ISO/baseline.content.sha") <(head -3 "$ISO/baseline.content.after.sha")
fi
echo "baseline shared-inode count (nlink>1, expected from hardlink farm): $(find "$BASE_TARGET" -type f -links +1 | wc -l)"

log "summary"
echo "cold baseline logical:        $(du -sh "$BASE_TARGET" | cut -f1)"
echo "second-attempt private bytes: $(b "$FIN_PRIV")"
echo "hardlinked (0 new bytes) files: $SHARED_INODES"
