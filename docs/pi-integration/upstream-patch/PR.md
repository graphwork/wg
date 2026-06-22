# PR: `resolveAppMode`: add a default-off `PI_NO_TUI` escape hatch

**Target repo:** `earendil-works/pi`
**File:** `packages/coding-agent/src/main.ts` (`resolveAppMode`)
**Size:** +7 lines (3 lines of logic, 4 of comment), one new env var, one function.

> Suggested commit message
>
> ```
> feat(main): add default-off PI_NO_TUI escape hatch to resolveAppMode
>
> Let a supervising harness force non-interactive even when pi is launched
> under a PTY (both fds are TTYs). Inert unless PI_NO_TUI is truthy; an
> explicit --mode rpc/json or -p/--print still wins. Reuses the existing
> isTruthyEnvFlag() helper for consistency with PI_OFFLINE /
> PI_STARTUP_BENCHMARK.
> ```

## Summary

Add an optional, **default-off** environment switch, `PI_NO_TUI`, at the top of
`resolveAppMode`. When set to a truthy value it forces the non-interactive
`print` mode **even when both stdin and stdout are TTYs** — the one situation in
which pi currently always enters the full-screen interactive TUI. It changes
nothing unless the variable is set, and an explicit `--mode rpc`/`--mode json`
or `-p`/`--print` still wins.

```ts
function resolveAppMode(parsed: Args, stdinIsTTY: boolean, stdoutIsTTY: boolean): AppMode {
	// Default-off escape hatch: let a supervising harness force non-interactive
	// even when pi is launched under a PTY (both fds are TTYs). Inert unless
	// PI_NO_TUI is set to a truthy value; an explicit --mode rpc/json or
	// -p/--print still wins (the guard only fires when no mode was requested).
	if (isTruthyEnvFlag(process.env.PI_NO_TUI) && parsed.mode === undefined && !parsed.print) {
		return "print";
	}
	if (parsed.mode === "rpc") {
		return "rpc";
	}
	// ... unchanged ...
}
```

## Motivation — the harness use case

We embed `pi` as a non-interactive child process inside a supervising harness.
For headless work we already drive pi the documented way — `--mode rpc` for a
long-lived chat protocol, `-p`/`--mode json` for one-shot workers — over piped
stdio, and that is fully sufficient today. We do **not** need this patch for the
common path and we don't block on it.

The gap it closes is narrow but real: a launcher that runs pi under a **PTY on
both fds** (so a human can attach to and watch pi's *real* TUI) but that cannot
guarantee the right CLI flags reach pi — e.g. a shared/opaque spawn wrapper, or
a layer that forwards an existing argv it doesn't own. In that both-TTY case
`resolveAppMode` unconditionally returns `interactive`, and a raw-mode terminal
grab is exactly what a supervisor wants to be able to veto from the outside.
`PI_NO_TUI` gives the supervisor a single environment-level kill switch for the
grab without having to rewrite argv. It is a convenience over "always pass
`-p`", not a new capability — it just lets *intent* ("a harness is driving me")
travel through the environment, which a wrapper can always set even when it
can't edit flags.

## Why this is a small, safe change to carry

- **Default-off.** The guard only fires when `PI_NO_TUI` is truthy
  (`1`/`true`/`yes`, via the existing `isTruthyEnvFlag`). Unset/`0`/`false`
  reproduce today's behavior byte-for-byte. Zero behavior change for existing
  users.
- **Minimal surface.** ~3 lines of logic, one env var, one function touched. No
  new dependency, no new flag, no config-schema change.
- **Composes, does not override.** The guard's own condition
  (`parsed.mode === undefined && !parsed.print`) means an explicit
  `--mode rpc`/`--mode json` or `-p`/`--print` bypasses it entirely. Existing
  mode selection wins; this only decides the otherwise-`interactive` fall-through.
- **Idiomatic.** Reuses the repo's own `isTruthyEnvFlag()` helper, matching how
  `PI_OFFLINE` and `PI_STARTUP_BENCHMARK` are already read in `main.ts`.
- **Discoverable & symmetric.** It reads as the env-level twin of `-p`: same
  outcome (`print`), expressed where a supervisor naturally controls a child.

## Behavior matrix

| `PI_NO_TUI` | flags        | stdin TTY | stdout TTY | result        | vs. today |
|-------------|--------------|-----------|------------|---------------|-----------|
| unset       | —            | yes       | yes        | `interactive` | unchanged |
| unset       | —            | no        | yes        | `print`       | unchanged |
| `0`/`false` | —            | yes       | yes        | `interactive` | unchanged |
| `1`/`true`  | —            | yes       | yes        | **`print`**   | **new**   |
| `1`         | `--mode rpc` | yes       | yes        | `rpc`         | unchanged (explicit wins) |
| `1`         | `--mode json`| yes       | yes        | `json`        | unchanged (explicit wins) |
| `1`         | `-p`         | yes       | yes        | `print`       | unchanged |

Only one row changes, and only when the operator opts in via the env var.

## Testing

A standalone truth-table check of the patched logic (and the existing
`isTruthyEnvFlag` helper) is included alongside the patch as `tui_guard_test.mjs`
— `node tui_guard_test.mjs`, 11/11 PASS. Happy to fold these into the repo's
test suite in whatever form you prefer (e.g. a unit test next to
`resolveAppMode`).
