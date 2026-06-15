# Evaluate Pi as a WG Executor

Task: `evaluate-pi-as`
Date: 2026-06-15

## Identified Project

The relevant `pi` is Pi Coding Agent, not Inflection's consumer Pi assistant.

- Upstream: https://github.com/earendil-works/pi
- Website/docs: https://pi.dev/ and https://pi.dev/docs/latest
- npm package: `@earendil-works/pi-coding-agent`
- CLI binary: `pi` via package bin `dist/cli.js`
- Current package checked locally: `0.79.4`
- License: MIT
- Maintenance status: active. GitHub showed 4,552 commits and latest release `v0.79.4` on 2026-06-15; npm `latest` is also `0.79.4`.
- Legacy package note: `@mariozechner/pi-coding-agent` still exists at `0.73.1`, but the current package/repo branding is `@earendil-works/pi-coding-agent`.

Install/run commands used:

```bash
npm view @earendil-works/pi-coding-agent version license bin dist-tags homepage description --json
npx --yes @earendil-works/pi-coding-agent@0.79.4 --help
npx --yes @earendil-works/pi-coding-agent@0.79.4 --list-models
```

Official install command from the docs:

```bash
npm install -g --ignore-scripts @earendil-works/pi-coding-agent
```

## Configuration Surface

Pi supports the basic knobs WG would need:

- CLI provider/model/API key: `--provider <name>`, `--model <pattern>`, `--api-key <key>`.
- Environment keys: `OPENROUTER_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, and many others are listed in `pi --help`.
- Session controls: `--continue`, `--resume`, `--session <path|id>`, `--session-id <id>`, `--session-dir <dir>`, `--no-session`, `--name`.
- Non-interactive modes: `-p/--print`, `--mode json`, and `--mode rpc`.
- Local/custom endpoints: `~/.pi/agent/models.json` supports provider `baseUrl`, `api`, `apiKey`, `headers`, `authHeader`, and model entries. Supported API shims include `openai-completions`, `openai-responses`, `anthropic-messages`, and `google-generative-ai`.

OpenRouter-style model IDs work. With a dummy `OPENROUTER_API_KEY`, model discovery listed models such as `openrouter anthropic/claude-3.5-haiku`. A custom `models.json` provider also accepted `anthropic/claude-3.5-haiku` as a model ID.

Commands used:

```bash
OPENROUTER_API_KEY=dummy \
  PI_CODING_AGENT_DIR=$(mktemp -d) \
  PI_CODING_AGENT_SESSION_DIR=$(mktemp -d) \
  npx --yes @earendil-works/pi-coding-agent@0.79.4 \
    --list-models openrouter --offline --no-context-files --no-extensions \
    --no-skills --no-prompt-templates --no-themes --no-approve
```

```bash
tmp=$(mktemp -d)
mkdir -p "$tmp"
printf '%s\n' \
  '{"providers":{"wg-test":{"baseUrl":"http://127.0.0.1:9/v1","api":"openai-completions","apiKey":"$WG_TEST_API_KEY","models":[{"id":"anthropic/claude-3.5-haiku","name":"OpenRouter-style test"}]}}}' \
  > "$tmp/models.json"
WG_TEST_API_KEY=dummy \
  PI_CODING_AGENT_DIR=$tmp \
  PI_CODING_AGENT_SESSION_DIR=$(mktemp -d) \
  npx --yes @earendil-works/pi-coding-agent@0.79.4 \
    --list-models "anthropic/claude-3.5-haiku" --offline --no-context-files \
    --no-extensions --no-skills --no-prompt-templates --no-themes --no-approve
```

## Local Smoke Tests

This machine has no real Pi provider credentials configured, so I did not run a live LLM turn. I did run local launch tests through the same process shapes WG would use.

### RPC Handler Shape

Command:

```bash
timeout 20s sh -c 'printf "%s\n%s\n" \
  "{\"id\":\"state-1\",\"type\":\"get_state\"}" \
  "{\"id\":\"abort-1\",\"type\":\"abort\"}" |
  PI_CODING_AGENT_DIR=$(mktemp -d) \
  PI_CODING_AGENT_SESSION_DIR=$(mktemp -d) \
  npx --yes @earendil-works/pi-coding-agent@0.79.4 \
    --mode rpc --no-session --offline --no-context-files --no-extensions \
    --no-skills --no-prompt-templates --no-themes --no-tools --no-approve'
```

Result:

- Exit code: `0`
- Output framing: JSONL on stdout
- `get_state` returned `success: true`
- `abort` returned `success: true`
- Session identity exists even with `--no-session` for process-local state

This is the strongest evidence that Pi could be wrapped as a non-interactive WG handler if WG ever needed it.

### Print Mode Error Shape

Command:

```bash
timeout 20s sh -c 'PI_CODING_AGENT_DIR=$(mktemp -d) \
  PI_CODING_AGENT_SESSION_DIR=$(mktemp -d) \
  npx --yes @earendil-works/pi-coding-agent@0.79.4 \
    -p --no-session --offline --no-context-files --no-extensions \
    --no-skills --no-prompt-templates --no-themes --no-tools --no-approve \
    --provider openrouter --model openrouter/anthropic/claude-3.5-haiku \
    "Reply with ok"'
```

Result:

- Exit code: `1`
- Error was direct and actionable: `No API key found for openrouter.`
- This is acceptable for a batch executor: credential failure is predictable and nonzero.

### PTY/TUI Launch Shape

Command:

```bash
timeout 20s script -q -c 'PI_CODING_AGENT_DIR=$(mktemp -d) \
  PI_CODING_AGENT_SESSION_DIR=$(mktemp -d) \
  npx --yes @earendil-works/pi-coding-agent@0.79.4 \
    --no-session --offline --no-context-files --no-extensions \
    --no-skills --no-prompt-templates --no-themes --no-tools --no-approve \
    --provider openrouter --model openrouter/anthropic/claude-3.5-haiku \
    "test prompt"' /tmp/pi-pty-smoke.typescript
```

Result:

- Timed out and was killed by `timeout` with code `124`.
- Pi rendered a full-screen TUI, showed `Error: No API key found for openrouter`, and stayed open for user input.
- It emitted a tmux-specific warning: `tmux extended-keys is off. Modified Enter keys may not work.`
- Captured PTY transcript was 13,206 bytes for startup plus error, mostly ANSI screen-diff output.

This is good for human chat sessions, but not a clean worker process contract unless WG manages the PTY lifecycle explicitly.

## Terminal Behavior Findings

- Tmux scrolling: Pi is a full-screen TUI in interactive mode. The docs recommend tmux for background bash observability, and the local PTY run emitted a tmux extended-keys warning. That does not directly solve WG's existing tmux scrolling pain; it moves much of the UI/scrollback behavior inside Pi's TUI.
- Large output: Pi's RPC docs define streaming `tool_execution_update` events, final `tool_execution_end` events, and bash result truncation with `fullOutputPath`. That is a better structured contract than scraping a terminal. However, my no-model/no-session RPC attempt to call the documented `bash` command did not return a bash response before stdin close, so this needs a credentialed live test before relying on it.
- Stdin/stdout framing: RPC mode uses JSONL with strict LF delimiters. This is a good WG integration surface.
- Streaming: JSON event stream mode and RPC events are documented. Message updates include text deltas, tool-call deltas, tool execution progress, turn boundaries, and final agent events.
- Recovery after process death: Pi has persistent sessions, exact `--session-id`, `--session`, `--continue`, session directories, and JSONL session files. WG could map chat IDs to Pi session IDs/files. I did not verify crash recovery with a real LLM turn because credentials are absent.
- PTY exit behavior: interactive Pi does not exit after credential failure; it remains in the TUI. WG should not treat interactive mode as a worker handler without a supervisor timeout and explicit input protocol.

## WG Recommendation

Recommendation: keep Pi as an experimental/custom executor recipe for now; do not add it as a first-class WG executor yet.

Reasons:

- Positive: Pi is active, scriptable, MIT licensed, supports OpenRouter and custom endpoints, has explicit RPC/JSON modes, and has session controls that map well to WG chat identity.
- Negative: WG already has direct `codex`, `claude`, and in-process `nex` handlers. Pi is another coding-agent harness with overlapping tools and its own TUI/session/config model, so first-class integration would add meaningful maintenance surface.
- Negative: the human-facing PTY mode does not exit cleanly on credential failure and emits large ANSI screen diffs. That is not materially better than current terminal pain points.
- Unknown: credentialed RPC streaming, session resume after kill, and large tool-output truncation need live-provider validation before WG can depend on Pi for worker execution.

Minimal shape if WG later promotes this:

- Executor name/model prefix: `pi:<model>` with handler kind `pi`.
- CLI launch syntax for worker/chat RPC mode:
  ```bash
  pi --mode rpc --provider <provider> --model <model> \
     --session-id <wg-chat-or-task-id> --session-dir <wg-state-dir>/pi-sessions
  ```
- Print-mode fallback for one-shot workers:
  ```bash
  pi -p --provider <provider> --model <model> --session-id <task-id> "<prompt>"
  ```
- Endpoint/model handling: map WG `model` prefixes to Pi `--provider` and `--model`; write temporary `models.json` only for custom `baseUrl`/headers/key refs that Pi cannot receive through CLI flags.
- Secrets: prefer environment variables or WG-generated temp config with env interpolation; avoid passing secret values directly in process args.
- TUI new-chat option: only after RPC handler validation; interactive TUI can be exposed as an advanced manual launch, not the default handler.
- Smoke tests before first-class status:
  - CLI config lint rejects `pi:` without installable `pi` binary or model.
  - RPC launch responds to `get_state` and accepts a prompt with a real test provider.
  - TUI `[+]` chat launch starts Pi and maps chat ID to a stable session.
  - Process kill/restart resumes the same Pi session.
  - Large bash/tool output produces structured truncation or a stable `fullOutputPath`.

## Sources

- Pi GitHub repository: https://github.com/earendil-works/pi
- Pi docs: https://pi.dev/docs/latest
- Pi website feature overview: https://pi.dev/
- RPC mode docs: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/rpc.md
- Custom models docs: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md
- Providers docs: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/providers.md
- npm package metadata: `npm view @earendil-works/pi-coding-agent ...`
