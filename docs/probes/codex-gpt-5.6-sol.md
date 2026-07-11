# Codex GPT 5.6 Sol route probe

Date: 2026-07-11

Task: `probe-codex-gpt-5-6`

## Result

Pass. The persistent WG dispatcher spawned this task through the Codex handler
using the explicit task route `codex:gpt-5.6-sol`. The installed Codex CLI
(`codex-cli 0.144.1`) accepted `--model gpt-5.6-sol`, started a thread, emitted
an assistant response, and successfully executed commands.

Runtime identity reported by `wg show probe-codex-gpt-5-6 --json`:

```json
{
  "model": "codex:gpt-5.6-sol",
  "resolved_reasoning": "high",
  "actual_executor": "codex",
  "actual_model": "gpt-5.6-sol"
}
```

The task log records the daemon dispatch as:

```text
Spawned by coordinator --executor codex --model gpt-5.6-sol [agent-114]
```

The live process ancestry was the persistent daemon (PID 4093484) spawning
`/home/bot/wg/.wg/agents/agent-114/run.sh`, which in turn launched Codex with
`--model gpt-5.6-sol`. The raw Codex JSON stream began with
`thread.started`, `turn.started`, and a completed `agent_message`, proving the
model route was accepted and produced a response rather than merely passing
WG's validation.

## Reasoning behavior

WG retained structured reasoning as metadata: `resolved_reasoning` is exactly
`high`. It did **not** propagate that value to Codex as a reasoning-effort
setting in this spawn.

The generated `run.sh` and the live process command line contained:

```text
codex exec --ignore-user-config --json --skip-git-repo-check
  --dangerously-bypass-approvals-and-sandbox
  -c model_verbosity="high"
  -c tool_output_token_limit=32000
  ...
  --model gpt-5.6-sol
```

There was no `model_reasoning_effort`, `reasoning_effort`, or equivalent
reasoning option. `model_verbosity="high"` is a separate Codex response-
verbosity setting and is not evidence that structured reasoning `high` was
propagated. Therefore the observed result is unambiguous: reasoning `high`
was WG metadata only for this Codex-handler execution.

## Scope

No source, configuration, profile, or service setting was changed. The daemon
was inspected but not restarted, paused, reloaded, or reconfigured. This report
is the sole repository artifact created for the probe.
