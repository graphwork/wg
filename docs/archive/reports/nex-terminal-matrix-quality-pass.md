# Nex Terminal-Bench Matrix Quality Pass

Task: `quality-nex-terminal-matrix`
Date: 2026-06-02

## Scope

Reviewed the WG metadata for the Nex Terminal-Bench follow-up matrix before
the benchmark tasks dispatch:

- `bench-nex-v4flash-terminal`
- `bench-nex-minimax-terminal`
- `synth-nex-terminal-matrix`

The completed baseline task `bench-nex-deepseek-terminal` was also checked as
the required comparison anchor for synthesis.

## Metadata Changes

`bench-nex-v4flash-terminal` now names both model forms explicitly:
`openrouter:deepseek/deepseek-v4-flash` for WG/Nex routing and raw API model
ID `deepseek/deepseek-v4-flash`. Its validation now requires a Nex eval-mode
or closest non-interactive Nex run, not a Codex benchmark run. It also requires
an external per-attempt timeout of 180 seconds or less, `--max-turns <= 10`,
and no more than two Nex attempts total.

`bench-nex-minimax-terminal` now names both model forms explicitly:
`openrouter:minimax/minimax-m2.7` for WG/Nex routing and raw API model ID
`minimax/minimax-m2.7`. Its validation now requires a Nex eval-mode or closest
non-interactive Nex run, not a Codex benchmark run. It also requires an
external per-attempt timeout of 180 seconds or less, `--max-turns <= 8`, and no
more than two Nex attempts total.

Both benchmark tasks now explicitly forbid secret-bearing diagnostics such as
`set -x`, `env`, `printenv`, `ps e`, and `/proc/*/environ`, and they require
post-run process checks that do not print process environments.

`synth-nex-terminal-matrix` now states that it is a document synthesis task
only: it must not run new OpenRouter calls or new benchmarks. Its validation
requires references to all three benchmark reports, treats
`bench-nex-deepseek-terminal` as the completed baseline comparison, and requires
no lingering helper process.

## Dependency Check

The synthesis task depends on all required inputs:

- `bench-nex-deepseek-terminal` (done)
- `bench-nex-v4flash-terminal` (open, blocked by this quality pass)
- `bench-nex-minimax-terminal` (open, blocked by this quality pass)
- `quality-nex-terminal-matrix` (this gate)

Both follow-up benchmark tasks are blocked by `quality-nex-terminal-matrix`.

## Validation Notes

The benchmark task runtime model remains `codex:gpt-5.5` because Codex is the
WG worker orchestrating the metadata task. The benchmark descriptions now make
the intended boundary explicit: Codex may orchestrate, but the measured
benchmark harness must be Nex.

No source code was changed. This pass validated WG task metadata and recorded
the result here for review.
