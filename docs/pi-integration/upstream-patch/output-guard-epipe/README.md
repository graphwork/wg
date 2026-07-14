# Pi output-guard closed-consumer fix

This is a ready-to-submit upstream patch for
[`earendil-works/pi`](https://github.com/earendil-works/pi). It fixes Pi 0.80.6
crashing after an NDJSON consumer closes stdout early (for example,
`pi --mode json … | head -n1`).

## Contents

- `OUTPUT_GUARD_EPIPE.patch` changes Pi's
  `packages/coding-agent/src/core/output-guard.ts` and adds the focused upstream
  Vitest suite `packages/coding-agent/test/output-guard.test.ts`.
- `output-guard.ts` is the patched source snapshot used by WG's credential-free
  terminal smoke. It is identical to the source post-image in the patch.

## Behavior

The output guard now installs an `error` listener while stdout is taken over.
An `EPIPE` marks raw stdout closed, resolves the ordered write queue, and drops
later events instead of turning an expected closed reader into exit 1. Other
write errors keep their prior fatal behavior. Transient `ENOBUFS`, `EAGAIN`, and
`EWOULDBLOCK` writes still retry every 10 ms, but stop after 100 retries (about
one second) rather than spinning forever. The listener is removed by
`restoreStdout()`.

The tests cover:

1. both the Socket `error` event and write callback reporting `EPIPE`;
2. complete ordered NDJSON delivery, including `turn_end.message.usage`;
3. transient retry success with ordering preserved; and
4. the 101-attempt bound when transient pressure never clears.

## Provenance and application

The patch was cut against upstream commit
`b084d2fb395f0f1aa924cb07b14e5d0edab115e2`
(`@earendil-works/pi-coding-agent` 0.80.6).

```bash
git clone https://github.com/earendil-works/pi.git
cd pi
git checkout b084d2fb395f0f1aa924cb07b14e5d0edab115e2
git apply /path/to/OUTPUT_GUARD_EPIPE.patch
npm install --ignore-scripts
npm run build
npm --workspace @earendil-works/pi-coding-agent test
```

## Validation performed

The closed-reader regression was written and run against the unmodified 0.80.6
source first. Vitest did not complete before the eight-second test timeout
because the raw write never settled after the simulated Socket `EPIPE`. The
equivalent real Pi pipeline exited 1 with an unhandled `write EPIPE`. After the
patch:

- the focused suite passed 4/4;
- the coding-agent suite passed 1541 tests (47 skipped);
- the full Pi monorepo build passed;
- a real patched Pi invocation piped to `head -n1` returned pipeline statuses
  `0 0 0` with an empty stderr (no `EPIPE` / unhandled exception); and
- a normal full JSON invocation returned 13 valid NDJSON events, including a
  non-zero `turn_end.message.usage.totalTokens` value.

WG does not need a wrapper change: its Pi worker consumes the complete stream.
The defect is in Pi's raw stdout guard, so the durable fix belongs upstream.

## WG runtime delivery

As of 2026-07-14, npm still publishes 0.80.6 as the latest
`@earendil-works/pi-coding-agent` release (registry integrity
`sha512-vcfD6tOk402isLl3Cm/qbn2O10TvgroMp1+/fEGM24ZdvETFCdOYv5VZ7m59EI5fPsjfSJh+CpQ5bhBrhfOg7g==`),
and those published bytes contain the vulnerable guard. A green source-snapshot
test therefore does not fix the runtime on `PATH`.

WG's supported development delivery workflow is:

```bash
make install-patched-pi
wg doctor
```

The make target runs `scripts/install-patched-pi.sh`. It clones the canonical Pi
package repository at the exact commit above, applies this patch to
`packages/coding-agent`, installs dependencies, builds the monorepo, runs the
focused upstream tests, creates an npm tarball, and installs that tarball. It
never edits an existing global `node_modules` tree in place. The resulting
package keeps upstream's version `0.80.6`, so `wg doctor` inspects the resolved
`dist/core/output-guard.js` bytes and reports whether both EPIPE handling and
the retry bound are present; a version-only check would misclassify a patched
development install.

Human `pi`, WG's JSON worker, and `wg pi-handler` all resolve the Pi executable
from `PATH`. The doctor detail prints that exact executable and guard path, so
one diagnostic covers all three launch surfaces without adding a warning to
the daemon loop.

Delivery evidence from the 2026-07-14 validation host:

- resolved CLI: `/home/bot/.nvm/versions/node/v25.4.0/lib/node_modules/@earendil-works/pi-coding-agent/dist/cli.js`;
- runtime version: `0.80.6` built from commit
  `b084d2fb395f0f1aa924cb07b14e5d0edab115e2`;
- installed compiled guard SHA-256:
  `96d0b1c5dd9832204c4a9ef92babf258f71ca7bea6b8876d0e94e7dd54161cc4`;
- focused upstream output-guard tests: 4 passed; and
- live full consumer: 13 valid NDJSON events, one `turn_end` with non-zero
  `usage.totalTokens`, and empty stderr.
