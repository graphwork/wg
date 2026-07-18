# TUI Pi chat launch: `Failed to run wg: ENOENT`

## Symptom

A long-running installed `wg tui` opened **New chat**, the user explicitly
selected **Pi**, and confirmation left the chooser open with:

```text
Failed to create chat: Failed to run wg: No such file or directory (os error 2)
```

No `.chat-N` row was committed and Pi never ran.

## Reproduction and process trace

The failure was reproduced without touching the live TUI or `.chat-8`:

1. Copy the installed `wg` to an isolated temp path and enter it through a
   symlink.
2. Start the real TUI in a private tmux server with isolated `HOME`, graph, and
   a minimal non-login `PATH` containing a credential-free fake `pi`.
3. Open command-mode **New chat**, highlight **Pi**.
4. Atomically replace the installed-copy target, matching `cargo install` and
   package-manager rename behavior while the TUI remains open.
5. Confirm twice.

Before replacement, `/proc/<tui-pid>/exe` resolved to the installed-copy
path. After replacement it resolved to:

```text
/tmp/.../wg-image (deleted)
```

`std::env::current_exe()` returned that display pathname. The launcher's
background command then attempted the equivalent of:

```text
execve("/tmp/.../wg-image (deleted)", ["wg", "chat", "create", ...])
    = -1 ENOENT
```

Thus **neither `wg` generally nor Pi was absent**. The missing path was the
stale name of the old, still-running WG image used for the TUI's internal
recursive `wg chat create` invocation. The generic error hid that distinction.

The equivalent public invocation through the replacement binary succeeded and
created exactly one dormant `.chat-0`, proving the defect was in the TUI-only
recursive self-launch edge rather than the shared chat persistence core.

### Environment matrix

| Case | Before fix | After fix |
|---|---|---|
| Normal installed path, unchanged inode | works | works |
| Installed/symlinked path atomically replaced | internal `execve` gets ENOENT | runs exact mapped WG image via kernel-owned executable link |
| Minimal PATH with no `wg` | fails after replacement despite a live WG image | works; no PATH lookup |
| Foreign WireGuard-style `wg` first on PATH | risk of executing the wrong system if PATH fallback is added | never executed; kernel-owned self handle wins |
| Non-login private tmux server | same ENOENT | works |
| Mosh-like env (`MOSH_IP`, no login-shell repair) | same ENOENT | works |
| Public `wg chat create --exec pi` | works if Pi exists | works; missing Pi is preflighted |
| Pi absent | row could be persisted before pane died ambiguously | explicit ``pi` was not found on PATH`; zero row/session, no fallback |

## Fix

`worksgood::self_exe` separates executable identity from executable handles:

- Immediate recursive `Command` calls use `/proc/self/exe` on Linux. The kernel
  keeps this handle executable for the lifetime of the process even when the
  installed pathname has been replaced.
- Commands handed to tmux use `/proc/<tui-pid>/exe`; plain `/proc/self/exe`
  would incorrectly mean the tmux process at execution time.
- Other platforms retain the absolute `current_exe()` path.
- Recursive-launch diagnostics now name the exact handle and running identity
  instead of claiming merely that `wg` failed.

Interactive Pi is also validated before `wg chat create` crosses the IPC/graph
mutation boundary. The PTY launch resolves Pi to its discovered absolute path.
This is deliberately the **standalone interactive Pi console** contract. It
does not call `wg pi-handler`, does not select a Node/plugin backend, and does
not add `--mode rpc`, `-e`, or `-ne`; the hermetic managed worker/plugin path
remains separate.

## Permanent regression

`tests/smoke/scenarios/tui_pi_hot_upgrade_self_exec.sh` drives the actual human
flow against an installed-binary copy under minimal PATH and asserts:

- the running image really becomes ` (deleted)`;
- double confirmation creates one canonical row and one path-owned tmux
  session;
- standalone Pi receives `.chat-0` / `chat-0`, no RPC/plugin flags;
- restarting the TUI reattaches the exact Pi PID without duplication;
- public and TUI missing-Pi confirmations name Pi and leave zero rows/sessions;
- a foreign WireGuard-style `wg` first on PATH is never executed;
- no unrelated provider is attempted.
