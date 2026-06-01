# Executor Arena

WG can dispatch workers through several installed CLI agents. The core
handlers (`claude`, `codex`, and `native`/`nex`) are live-chat capable. The
external executor arena below is worker-only: use it for task agents spawned by
`wg service`, `wg spawn`, or agent identities, not for persistent chat agents.

Run discovery before choosing a worker:

```bash
wg executors --all
```

`wg init` seeds `.wg/executors/*.toml.example` for every supported external
worker. Copy the example you want to customize, then edit the copied file:

```bash
cp .wg/executors/opencode.toml.example .wg/executors/opencode.toml
```

## Adapter Stability

Stable means WG has a tested prompt-delivery and model-argument shape for the
adapter. Experimental means the surface exists, but the upstream CLI has moved
recently enough that you should verify `--help` for your installed binary
before relying on it in unattended production runs.

| Executor | Status | WG default command | Prompt delivery | OpenRouter model argument |
|----------|--------|--------------------|-----------------|---------------------------|
| `opencode` | Stable | `opencode run --format json --dangerously-skip-permissions` | `--file prompt.txt` plus a short positional instruction | `--model openrouter/<provider>/<model>` |
| `aider` | Stable | `aider --yes-always` | `--message-file prompt.txt` | `--model openrouter/<provider>/<model>` |
| `goose` | Stable | `goose run --no-session --output-format json` | `-i prompt.txt` | `--provider openrouter --model <provider>/<model>` |
| `qwen` | Stable | `qwen --output-format json --yolo` | `--prompt ...` plus prompt.txt on stdin | `--model <provider>/<model>` |
| `cline` | Stable | `cline --json --auto-approve true` | positional prompt plus prompt.txt on stdin | `--provider openrouter --model <provider>/<model>` |
| `crush` | Experimental | `crush run --quiet` | prompt.txt on stdin | `--model openrouter/<provider>/<model>` |
| `amplifier` | Experimental | `amplifier run --mode single --output-format json --bundle wg` | prompt.txt bridged into final positional prompt | none by default |

## OpenRouter Environment

WG's native `openrouter:` route reads `OPENROUTER_API_KEY` first and falls back
to `OPENAI_API_KEY`. Spawned external workers also receive the resolved
OpenRouter key as both `OPENROUTER_API_KEY` and `OPENAI_API_KEY` when the key
comes from a WG endpoint config.

Some CLIs still need their own provider configuration. For OpenAI-compatible
CLIs, especially Qwen Code and custom profiles, use the OpenRouter base URL:

```bash
export OPENROUTER_API_KEY="sk-or-v1-..."
export OPENAI_API_KEY="$OPENROUTER_API_KEY"
export OPENAI_BASE_URL="https://openrouter.ai/api/v1"
```

Tool-specific notes:

| Executor | OpenRouter setup |
|----------|------------------|
| `opencode` | Configure OpenRouter with `opencode auth login` or `/connect`; OpenCode stores provider credentials and accepts `provider/model` model IDs. |
| `aider` | `OPENROUTER_API_KEY` works directly; Aider's OpenRouter model form is `openrouter/<provider>/<model>`. |
| `goose` | Configure a Goose provider or pass provider/model flags; WG appends `--provider openrouter` for `openrouter:` model specs. |
| `qwen` | Configure `~/.qwen/settings.json` for an OpenAI-compatible provider, or set `OPENAI_API_KEY` and `OPENAI_BASE_URL=https://openrouter.ai/api/v1`. |
| `cline` | Run `cline auth -p openrouter ...` first, or use Cline's provider/key flags for the run. |
| `crush` | Configure a provider interactively with `crush`; verify `crush run --help` for your installed version. |
| `amplifier` | Configure Amplifier providers/bundles first; this adapter is experimental and may require a local template override. |

Do not put API keys in `.wg/executors/*.toml`. Use WG endpoint secrets,
environment variables, or each tool's own credential store.

## Source Links

These upstream docs were used for the current command forms:

| Executor | Sources |
|----------|---------|
| OpenCode | [CLI](https://opencode.ai/docs/cli/), [providers](https://opencode.ai/docs/providers/) |
| Aider | [options](https://aider.chat/docs/config/options.html), [models and keys](https://aider.chat/docs/troubleshooting/models-and-keys.html), [API keys](https://aider.chat/docs/config/api-keys.html) |
| Goose | [CLI commands](https://block.github.io/goose/docs/guides/goose-cli-commands/) |
| Qwen Code | [settings](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/settings/), [authentication](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/auth/) |
| Cline | [CLI overview](https://docs.cline.bot/usage/cli-overview), [CLI reference](https://docs.cline.bot/cli/cli-reference) |
| Crush | [run command](https://www.mintlify.com/charmbracelet/crush/cli/run), [GitHub](https://github.com/charmbracelet/crush) |
| Amplifier | [project README](https://github.com/microsoft/amplifier), [CLI README](https://github.com/microsoft/amplifier-app-cli) |
| OpenRouter | [quickstart](https://openrouter.ai/docs/quickstart), [API authentication](https://openrouter.ai/docs/api-keys) |
