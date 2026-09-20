# First npm release — run report (`publish-the-first`)

**Status: STOPPED by a stop-class publish failure.** The real release ran and
everything up to the npm publish succeeded, but the **operator-gated npm publish
step failed before any package was published** because of a defect in the
release workflow itself (a shell/Node path bug — *not* a provenance or
permission rejection). Per the task's `STOP CONDITIONS (report, do not work
around)`, the failure is reported verbatim below with the exact operator action
needed. No version was bumped, nothing was published from a local machine, and
provenance was not disabled.

- Task: `publish-the-first`
- Real release run (failed): **35538576933** — https://github.com/graphwork/wg/actions/runs/35538576933
- Prior green **dry run** reference: **35457631040** — https://github.com/graphwork/wg/actions/runs/35457631040 (`workflow_dispatch`, branch `wg/agent-148/fix-cross-platform`, conclusion `success`)

---

## 1. Claim-check (passed)

`@worksgood` scope resolves and `0.1.0` is **not** already published:

```text
$ npm view @worksgood/cli --json
npm error code E404
npm error 404 Not Found - GET https://registry.npmjs.org/@worksgood%2fcli - Not found

$ npm view @worksgood/cli version        # @worksgood/cli
npm error code E404
$ npm view @worksgood/linux-x64-gnu version
npm error code E404
$ npm view @worksgood/darwin-arm64 version
npm error code E404
```

Scope existence (the registry returns 404 for a nonexistent org and 200 for an
existing one):

```text
$ curl -s -o /dev/null -w '%{http_code}' https://registry.npmjs.org/-/org/worksgood/user
200        # org @worksgood exists
$ curl -s -o /dev/null -w '%{http_code}' https://registry.npmjs.org/-/org/zzzznotrealorg999/user
404        # control
```

So: scope resolves, `0.1.0` is free. No overwrite risk.

## 2. Preconditions found missing before triggering

The task statement said "everything is staged … and the NPM_TOKEN secret exists
in the repo." Two prerequisites the workflow actually requires were **not**
present, and a third changed the trigger path:

1. **No `v0.1.0` git tag existed** (`git ls-remote --tags origin` showed no
   `v0.1.0`; 7 tags total). The `assemble` job publishes the GitHub Release with
   `gh release create "${TAG}" … --verify-tag`, which **aborts if the remote tag
   does not already exist**. A `workflow_dispatch -f tag=v0.1.0` would therefore
   have failed at the assemble step.
2. **`NPM_PUBLISH_ENABLED` was not set** as a repository variable
   (`gh api repos/graphwork/wg/actions/variables` → `{"variables":[],"total_count":0}`).
   The publish steps are gated on
   `needs.plan.outputs.publish == 'true' && vars.NPM_PUBLISH_ENABLED == 'true'`;
   without it the job only runs the dry-run validation and prints the "publish
   not armed this run" checklist. (An org-level variable could not be read with
   the available token scopes — `gh api orgs/graphwork/actions/variables` → 403,
   `admin:org` required — so the repo-level value was set explicitly.)
3. The release workflow triggers on **`push: tags: v*.*.*`** as well as
   `workflow_dispatch`. Pushing the required tag therefore auto-triggers the real
   release; additionally dispatching `workflow_dispatch -f tag=v0.1.0` would have
   started a **second, racing run** that would collide on the same npm version
   and the same `gh release create`. Only the tag-push-triggered run was used.

Actions taken (all reversible and logged; no version bump, no local publish, no
provenance disable):

```text
$ gh variable set NPM_PUBLISH_ENABLED --repo graphwork/wg --body true
$ gh variable list --repo graphwork/wg
NPM_PUBLISH_ENABLED	true	2026-09-20T21:23:54Z

$ git tag -a v0.1.0 -m "WorksGood v0.1.0 — first npm release" 7f7e600cd0987e6fbb7613808ede56fc694a26c5
$ git push origin v0.1.0
 * [new tag]           v0.1.0 -> v0.1.0
```

The tag points at `main` HEAD `7f7e600cd0987e6fbb7613808ede56fc694a26c5`
(`feat: recoverable review rejection resumes the same node in place`).

## 3. Real release run 35538576933 — outcome

`push` on tag `v0.1.0` auto-triggered the workflow at 2026-09-20T21:24:00Z.

| Job | Conclusion |
| --- | --- |
| Plan release | success |
| Build aarch64-unknown-linux-gnu | success |
| Build x86_64-unknown-linux-gnu | success |
| Build x86_64-pc-windows-msvc | success |
| Build x86_64-apple-darwin | success |
| Build aarch64-apple-darwin | success |
| **npm packages** | **failure** |
| Assemble release | success |

All five target builds, the npm resolver unit tests, `make-packages.sh --pack`,
and the `npm publish --dry-run` structural validation **passed**. The failure is
isolated to the operator-gated publish step.

The `assemble` job **did** create the GitHub Release `v0.1.0` ("WG 0.1.0") with
all 23 native release assets (checksums, signatures, manifest, attestations).
It contains **no** npm packages.

## 4. Verbatim failure (the stop)

From `gh run view 35538576933 --log-failed`, step
**`Publish npm platform packages (operator-gated)`**:

```text
Run set -euo pipefail
for dir in npm-dist/@worksgood/*/; do
  pkg_name="$(node -p "require(process.argv[1]).name" "${dir}package.json")"
  if [[ "${pkg_name}" == "@worksgood/cli" ]]; then continue; fi
  (cd "${dir}" && npm publish --provenance --access public)
  echo "published ${pkg_name}"
done
...
Error: Cannot find module 'npm-dist/@worksgood/cli/package.json'
Require stack:
- /home/runner/work/wg/wg/[eval]
    ...
  code: 'MODULE_NOT_FOUND',
  requireStack: [ '/home/runner/work/wg/wg/[eval]' ]
}
Node.js v22.23.2
##[error]Process completed with exit code 1.
```

The `.npmrc` registry auth was configured (`NODE_AUTH_TOKEN` present,
`NPM_CONFIG_USERCONFIG=/home/runner/work/_temp/.npmrc`), and `npm publish` was
never reached — the failure is the `node -p` name-extraction **before** the first
publish. This is therefore **not** a provenance or permissions rejection; it is a
workflow defect. No health-of-credentials conclusion can be drawn from this run.

## 5. Root cause and the exact fix (operator action required)

`.github/workflows/release.yml:655`:

```bash
pkg_name="$(node -p "require(process.argv[1]).name" "${dir}package.json")"
```

`require()` treats a bare relative path (`npm-dist/…`) as a **module id**, not a
file path, so it can never resolve. This line was never exercised by the green
dry run, because dry runs set `publish=false` and skip the gated publish step —
which is exactly why `35457631040` was green while the real publish fails.

Minimal fix (read the file explicitly instead of `require`-ing a relative path):

```bash
pkg_name="$(node -p "JSON.parse(require('fs').readFileSync(process.argv[1], 'utf8')).name" "${dir}package.json")"
```

**Operator steps to complete the first release:**

1. Apply the one-line fix above to `.github/workflows/release.yml` on `main`
   (merge to `main` through the normal landing flow).
2. Re-run the real release. Because a GitHub Release `v0.1.0` already exists,
   either delete it first (the tag can stay — the native assets are regenerated)
   or make the assemble step idempotent:
   `gh release delete v0.1.0 --repo graphwork/wg --yes`.
   The tag `v0.1.0` already exists and already matches `Cargo.toml` `0.1.0`, so
   no version bump is required.
3. Trigger again, e.g. `gh workflow run release.yml --repo graphwork/wg --ref main
   -f tag=v0.1.0 -f dry_run=false` (or re-push a `release-test-*` rehearsal
   first). Note `NPM_PUBLISH_ENABLED=true` is already set.
4. Watch for a genuine npm failure this time. If npm rejects the publish on
   **provenance or permissions** (token scope, 2FA/OTP, trusted-publishing
   linkage, scope rights), that IS the declared stop condition and should be
   reported, not bypassed.

If the token/OIDC is misconfigured, the likely error will be an `EOTP`/`E403`
from npm on the first `npm publish`; that has not been observed yet.

## 6. Published npm versions

**None.** Post-run registry state (the packages are still unresolvable):

```text
@worksgood/cli            => npm error code E404
@worksgood/linux-x64-gnu  => npm error code E404
@worksgood/darwin-arm64   => npm error code E404
```

Local package assembly reports version `0.1.0` in every generated `package.json`
(from `Cargo.toml`, the single source of truth), and the metapackage declares
`@earendil-works/pi-coding-agent` as a dependency and exact-pins the two slice
platform packages at `__WG_VERSION__` → `0.1.0`.

## 7. Clean-prefix install smoke test

The task's step (4) — install the **published** metapackage from the registry —
could not be performed because nothing was published. As the closest available
end-to-end proof, the checked-in `scripts/npm/verify-install.sh` was run against
locally assembled tarballs built from the installed `0.1.0` binaries. It packs
the metapackage + `@worksgood/linux-x64-gnu`, installs into a clean prefix
(pulling `@earendil-works/pi-coding-agent` from the live registry), and execs the
real binaries from the npm-installed layout:

```text
$ bash scripts/npm/verify-install.sh --bin-dir "$HOME/.cargo/bin" \
      --out-dir /tmp/wg-npm-verify-publish-the-first --keep
== verify-install: version=0.1.0 platform=linux-x64-gnu bins=/home/bot/.cargo/bin
assembled .../@worksgood/cli (version 0.1.0)
assembled .../@worksgood/linux-x64-gnu (from binaries in /home/bot/.cargo/bin)
PASS: tarballs packed: worksgood-cli-0.1.0.tgz, worksgood-linux-x64-gnu-0.1.0.tgz
PASS: zero install scripts in all package.json files (structural, non-negotiable)
PASS: clean-prefix install pulled @worksgood/cli + @worksgood/linux-x64-gnu + @earendil-works/pi-coding-agent
PASS: shim execs real binaries from npm-installed layout: worksgood 0.1.0 / wg 0.1.0 / nex 0.1.0
PASS: --ignore-scripts install fully functional (zero-scripts design): worksgood 0.1.0
PASS: WG_BINARY_PATH override honored (exec + loud missing-file error)
PASS: missing optional dep -> friendly cargo-install fallback (not a stack trace)
PASS: npm publish --dry-run validates every package (registry-independent)

== verify-install: ALL CHECKS PASSED (dry-run only; nothing was published) ==
```

This proves the packaging tooling and the npm install layout are sound. It does
**not** substitute for the required registry install of the published
metapackage, which remains outstanding until the publish succeeds.

Full transcript retained at
`/tmp/wg-npm-verify-publish-the-first/` and `/tmp/verify-install-publish-the-first.log`.

## 8. Residual state left behind (for the operator)

| Item | State |
| --- | --- |
| git tag `v0.1.0` | pushed, annotated, points at `7f7e600` (main HEAD) |
| GitHub Release `v0.1.0` "WG 0.1.0" | exists, 23 native assets, **no npm packages** |
| repo variable `NPM_PUBLISH_ENABLED` | `true` (armed; set during this task) |
| npm packages `@worksgood/{cli,linux-x64-gnu,darwin-arm64}` | **not published** (404) |
| release workflow `release.yml` | unchanged; publish step still defective |
| version | not bumped (`Cargo.toml` 0.1.0, tag 0.1.0) |
| provenance | never disabled |

## 9. Honest bottom line

The first npm release did **not** ship. The publishing pipeline is one one-line
workflow fix away from working, and the local install path is proven. The exact
failing command, run id, and required operator action are recorded above. This
report intentionally does not smooth over the non-convergence: the real run is
red on `npm packages` and the registry has no `@worksgood` packages.

---

## Resolution (operator, 2026-09-21) — **PUBLISHED**

The stop condition above was resolved and the first release shipped:

- **Root cause fixed:** the publish step's `node -p "require(<relative path>)"` was replaced with
  `JSON.parse(require('fs').readFileSync(process.argv[1],'utf8')).name` (commit `6da00f55`,
  cherry-picked to `main` as `e6c06501` after attempt-loss killed the task that would have
  landed it through review — an explicit operator recovery, noted as such). A regression test
  ships with it (`scripts/npm/test/publish-step.test.js`, 2 passed / 0 failed).
- **Published:** run `35540905543` (dispatched from `wg/agent-163/complete-the-first`)
  completed **success**; `@worksgood/cli`, `@worksgood/linux-x64-gnu`, and
  `@worksgood/darwin-arm64` all resolve at **0.1.0**, and the metapackage declares
  `@earendil-works/pi-coding-agent@^0.85.1` as a dependency.
- **Clean-prefix install verified by the operator:** `npm install @worksgood/cli` into an
  empty prefix yields `nex`, `pi`, `wg`, `worksgood` on `.bin`; `wg --version` and
  `worksgood --version` both report `0.1.0`; `@earendil-works/pi-coding-agent` is present in
  `node_modules`. Zero postinstall scripts, 0 vulnerabilities.
- **Known gap, disclosed:** macOS binaries ship **unsigned and un-notarized** (Apple
  Developer ID secrets are not configured), so macOS users will hit Gatekeeper. Install docs
  should say so until the secrets are configured.
