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
