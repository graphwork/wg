# Executor Arena Smoke Report

- Task: `exec-smoke-arena`
- Run time: 2026-06-01T05:54:39Z
- Smoke model: `deepseek/deepseek-v4-flash`
- OpenRouter endpoint: `https://openrouter.ai/api/v1`

## Key Handling

- Read `~/.openrouter.key` only into shell environment variables for live calls.
- Exported `OPENROUTER_API_KEY`; exported `OPENAI_API_KEY` only for OpenAI-compatible CLIs that require that convention.
- Did not print or persist the secret value. WG endpoint output only showed the built-in masked key status.
- Scratch configs used `api_key_env = "OPENROUTER_API_KEY"` where a WG endpoint entry was needed; no scratch config contained the key value.

## Summary Matrix

| Surface | Installed | Result | Evidence | Blocker / Notes |
| --- | --- | --- | --- | --- |
| OpenRouter `chat/completions` | n/a | PASS | Direct POST to `/chat/completions` with `deepseek/deepseek-v4-flash` returned `ok`; response resolved to `deepseek/deepseek-v4-flash-20260423`; usage was 37 total tokens, reported cost `$0.0000089199`. | Meets the required direct OpenRouter smoke. |
| `wg endpoints test openrouter` | yes | PASS | Reported `Status: 200 OK`, `Connectivity: OK`, `Models: OK`, `Authentication: OK`, and `Generation: OK`. | This exercised configured endpoint health, not the required v4-flash model specifically. |
| `native` / `wg nex` | yes | FAIL | `wg nex --eval-mode --minimal-tools --model openrouter:deepseek/deepseek-v4-flash --endpoint openrouter ...` exited 1 with OpenRouter 401: `No cookie auth credentials found`. Reproduced in a fresh scratch WG project with endpoint `api_key_env = "OPENROUTER_API_KEY"`. | Env-only OpenRouter endpoint credentials are not reaching the native/Nex request path in this smoke. Existing tests cover `api_key_file`; this report intentionally did not point Nex at `~/.openrouter.key` directly. |
| standalone `nex --wg` | yes | FAIL | Same scratch project and env-only endpoint shape as `wg nex`; exited 1 with OpenRouter 401: `No cookie auth credentials found`. | Same env-key propagation gap as `wg nex`. |
| standalone `nex` | yes | FAIL | `nex --eval-mode --nex-dir <scratch> --model openrouter:deepseek/deepseek-v4-flash ...` exited 1 with OpenRouter 401: `No cookie auth credentials found`. | Standalone env-only OpenRouter auth is not sufficient for this path. |
| `codex` CLI | yes | PASS | `codex-cli 0.135.0`; non-interactive `codex exec --json --ephemeral --sandbox read-only` with OpenRouter provider overrides, `wire_api = "responses"`, and `deepseek/deepseek-v4-flash` returned last message `ok` in 4 JSONL lines. | Used `OPENAI_API_KEY` from the OpenRouter key and CLI-local `--config` overrides; no persistent Codex config was changed. |
| `claude` CLI | yes | BLOCKED | `claude --version` returned `2.1.152 (Claude Code)` and `wg executors --all` found `/home/bot/.local/bin/claude`. | The WG Claude adapter is Anthropic/Claude-CLI based and has no cheap OpenRouter model route in the discovered command surface. Only dry/version smoke was run to avoid non-OpenRouter spend. |
| `gemini` CLI | no | BLOCKED | `wg executors --all` and `command -v gemini` reported missing. | Install and configure Google Gemini CLI before smoke. No OpenRouter route is documented for this adapter in the current arena guide. |
| `opencode` CLI | no | BLOCKED | `wg executors --all` and `command -v opencode` reported missing. | Install OpenCode and configure OpenRouter with `opencode auth login` or `/connect`; WG model arg shape is `--model openrouter/<provider>/<model>`. |
| `aider` CLI | no | BLOCKED | `wg executors --all` and `command -v aider` reported missing. | Install Aider; `OPENROUTER_API_KEY` should work directly with `--model openrouter/<provider>/<model>`. |
| `goose` CLI | no | BLOCKED | `wg executors --all` and `command -v goose` reported missing. | Install Goose and configure OpenRouter provider; WG appends `--provider openrouter --model <provider>/<model>`. |
| `qwen` / `qwen-code` CLI | no | BLOCKED | `wg executors --all`, `command -v qwen`, and `command -v qwen-code` reported missing. | Install Qwen Code; configure OpenAI-compatible env with `OPENAI_API_KEY` and `OPENAI_BASE_URL=https://openrouter.ai/api/v1`; WG appends `--model <provider>/<model>`. |
| `cline` CLI | no | BLOCKED | `wg executors --all` and `command -v cline` reported missing. | Install Cline and authenticate OpenRouter with `cline auth -p openrouter ...`; WG appends `--provider openrouter --model <provider>/<model>`. |
| `crush` CLI | no | BLOCKED | `wg executors --all` and `command -v crush` reported missing. | Install Crush, configure provider interactively, and verify `crush run --help`; adapter is experimental. |
| `amplifier` CLI | no | BLOCKED | `wg executors --all` and `command -v amplifier` reported missing. | Install Amplifier and configure its provider/bundle surface; adapter is experimental and has no default OpenRouter model argument in WG templates. |

## Commands Run

All commands below were run with the OpenRouter key loaded from `~/.openrouter.key` into env only, when a live OpenRouter call was required.

```bash
curl -sS https://openrouter.ai/api/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -H 'Authorization: Bearer $OPENROUTER_API_KEY' \
  --data '{"model":"deepseek/deepseek-v4-flash","messages":[{"role":"user","content":"Reply with exactly: ok"}],"max_tokens":3,"temperature":0}'

wg endpoints test openrouter

wg nex --eval-mode --minimal-tools \
  --model openrouter:deepseek/deepseek-v4-flash \
  --endpoint openrouter \
  --max-turns 2 \
  'Reply exactly: ok. Do not use tools.'

nex --wg --eval-mode --minimal-tools \
  --model openrouter:deepseek/deepseek-v4-flash \
  --endpoint openrouter \
  --max-turns 2 \
  'Reply exactly: ok. Do not use tools.'

nex --eval-mode --nex-dir <scratch> --minimal-tools \
  --model openrouter:deepseek/deepseek-v4-flash \
  --max-turns 2 \
  'Reply exactly: ok. Do not use tools.'

codex exec --json --ephemeral --ignore-rules --skip-git-repo-check --sandbox read-only \
  --config 'model_provider="wg"' \
  --config 'model_providers.wg.name="wg"' \
  --config 'model_providers.wg.base_url="https://openrouter.ai/api/v1"' \
  --config 'model_providers.wg.env_key="OPENAI_API_KEY"' \
  --config 'model_providers.wg.wire_api="responses"' \
  --model deepseek/deepseek-v4-flash \
  --output-last-message <scratch> \
  'Reply exactly: ok'

wg executors --all
claude --version
codex --version
wg --version
nex --version
```

## Follow-Up

The only unexpected hard failure is the Nex/native env-key path: direct OpenRouter, endpoint health checks, and Codex-over-OpenRouter all authenticate, but `wg nex` and `nex` do not attach credentials when the endpoint references `OPENROUTER_API_KEY`. A follow-up fix should add coverage for `api_key_env` on Nex/native OpenRouter endpoint resolution, not only `api_key_file`.
