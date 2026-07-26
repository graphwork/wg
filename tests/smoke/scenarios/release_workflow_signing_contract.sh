#!/usr/bin/env bash
# Smoke scenario: pin the OS code-signing contract in the release pipeline.
# Owned by impl-binary-signing-notarization (T6).
#
# This is a STATIC contract test — it cannot run the GitHub Actions workflow
# (that needs real Apple/Azure credentials and a runner), so it asserts the
# invariants the release-engineering change must hold by inspecting the files.
# If a future change silently removes codesign / notarytool / the entitlements
# plist / the Sigstore attestation, this scenario fails before the regression
# ships.
#
# Assertions (mapped to docs/studies/wg-npm-distribution-design.md §6.2/§6.3/§6.4):
#   1. macos-entitlements.plist exists and is a valid plist.
#   2. release.yml has a macOS step that codesigns with hardened runtime +
#      entitlements + timestamp, then notarytool submit --wait, then stapler
#      staple — gated on runner.os == macOS, and BEFORE the archive step.
#   3. release.yml has a Windows signing path (Azure Trusted Signing preferred,
#      signtool fallback), gated on runner.os == Windows.
#   4. The existing Sigstore actions/attest@v4 provenance is still present
#      (signing did not replace provenance — they are complementary layers).
#   5. The certs-absent SKIP-with-banner path exists for BOTH OSes so a release
#      is not blocked on signing, and the gap is surfaced.
#   6. The per-target signing status flows into the Release notes (the assemble
#      job reads signing-status-*.json and emits an unsigned-archives section).

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "${ROOT}"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

REL=".github/workflows/release.yml"
PLIST="macos-entitlements.plist"

[[ -f "${REL}"   ]] || fail "missing ${REL}"
[[ -f "${PLIST}" ]] || fail "missing ${PLIST}"

# --- 1. plist validity ------------------------------------------------------
python3 - <<'PY' || exit 1
import plistlib, sys
with open("macos-entitlements.plist", "rb") as f:
    data = plistlib.load(f)
# Must not enable App Sandbox (which would break network/fs for the CLI) and
# must not disable library validation unnecessarily.
if "com.apple.security.app-sandbox" in data:
    print("FAIL: entitlements enable App Sandbox, which breaks the CLI", file=sys.stderr)
    sys.exit(1)
print("plist OK:", sorted(data.keys()))
PY

# --- 2. macOS signing contract ----------------------------------------------
# Gated on macOS, runs BEFORE "Create archive", and exercises the full chain.
python3 - <<'PY' || exit 1
import sys, re
src = open(".github/workflows/release.yml").read()

steps = re.findall(r"^\ {6}- name: (.+)$", src, re.M)
def idx(needle):
    for i, s in enumerate(steps):
        if needle in s:
            return i
    return -1

# The macOS step must exist and be gated on macOS.
m = re.search(r'name: (Sign and notarize macOS binaries)\n(?:.*\n)*?\s+if: \$\{\{ runner\.os == \'macOS\' \}\}', src)
if not m:
    print("FAIL: macOS signing step missing or not gated on runner.os == macOS", file=sys.stderr)
    sys.exit(1)

body = src
for needle in [
    "codesign --force --options runtime --timestamp",
    "--entitlements macos-entitlements.plist",
    "xcrun notarytool submit",
    "--wait",
    "xcrun stapler staple",
    "xcrun stapler validate",
]:
    if needle not in body:
        print(f"FAIL: macOS step missing '{needle}'", file=sys.stderr)
        sys.exit(1)

# The macOS signing step must run BEFORE the "Create archive" step (so the
# shipped archive carries the signature).
i_sign = idx("Sign and notarize macOS binaries")
i_archive = idx("Create archive")
i_package = idx("Package binaries")
if i_sign < 0 or i_archive < 0:
    print("FAIL: could not locate macOS-sign / Create-archive steps", file=sys.stderr)
    sys.exit(1)
if not (i_sign < i_package < i_archive):
    print("FAIL: macOS signing must run before Package binaries / Create archive", file=sys.stderr)
    sys.exit(1)

# The Sigstore attestation step must come AFTER archiving (provenance over the
# shipped archive bytes) and still exist.
i_attest = idx("Generate artifact attestations")
if i_attest < 0:
    print("FAIL: actions/attest provenance step missing", file=sys.stderr)
    sys.exit(1)
if not (i_archive < i_attest):
    print("FAIL: attestation must follow archiving", file=sys.stderr)
    sys.exit(1)

print("macOS signing contract OK")
PY

# --- 3. Windows signing contract -------------------------------------------
python3 - <<'PY' || exit 1
import sys, re
src = open(".github/workflows/release.yml").read()
for needle in [
    "Azure/trusted-signing-action@v0",
    "runner.os == 'Windows' && steps.win_sign_config.outputs.method == 'azure'",
    "runner.os == 'Windows' && steps.win_sign_config.outputs.method == 'signtool'",
    "/fd SHA256 /tr",            # signtool with RFC3161 timestamp
    "signtool verify /pa /v",
]:
    if needle not in src:
        print(f"FAIL: Windows signing contract missing '{needle}'", file=sys.stderr)
        sys.exit(1)
print("Windows signing contract OK")
PY

# --- 4. Sigstore provenance preserved --------------------------------------
grep -q "actions/attest@v4" "${REL}" \
    || fail "Sigstore actions/attest@v4 provenance layer was removed"

# --- 5. SKIP-with-banner path exists for both OSes -------------------------
grep -q "macOS code-signing SKIPPED" "${REL}" \
    || fail "macOS certs-missing skip banner missing"
grep -q "Windows code-signing SKIPPED" "${REL}" \
    || fail "Windows certs-missing skip banner missing"
grep -q '::warning::macOS code-signing SKIPPED' "${REL}" \
    || fail "macOS skip must emit a ::warning:: annotation"
grep -q '::warning::Windows code-signing SKIPPED' "${REL}" \
    || fail "Windows skip must emit a ::warning:: annotation"

# --- 6. signing status flows into Release notes ---------------------------
grep -q 'signing-status-' "${REL}" \
    || fail "signing-status JSON not produced/consumed"
grep -q 'Code signing' "${REL}" \
    || fail "Release notes do not surface a Code signing section"

echo "PASS: release-pipeline OS code-signing contract holds"
