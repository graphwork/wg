# Agent 949 reopen/owner-release evidence capture

Captured read-only on 2026-07-31 before any reconciliation of
`remove-distracting-plumbing-control`.

The graph projected the task as `Abandoned`, generation `0`, attempt
`attempt-0-1`, fence/worktree lease `1`, with attempt disposition `None` and an
`Active` Pi watchdog. Kernel process identities 2685999/2686015 were no longer
present. The retained source tuple was:

- task: `remove-distracting-plumbing-control`
- owner: `agent-949`
- generation: `0`
- attempt: `attempt-0-1`
- attempt fence / worktree lease: `1`
- process epoch: `1`
- session: `019fb890-bdef-7631-a062-36c37310464f`
- runtime namespace: `c54213b9ec50b2e130a5ab1d2d97d9066e5dafce3dee19c920fad581b3189c97`
- worktree: `/home/bot/wg/.wg-worktrees/agent-949`

SHA-256 inventory (path relative to `/home/bot/wg/.wg`):

| Path | Bytes | SHA-256 |
|---|---:|---|
| `agents/agent-949/metadata.json` | 1301 | `dcaa0cdbacdc81b573e00e98cfb258ecc700c15c4ed8f1f5d57c1ed5c11e52d6` |
| `agents/agent-949/raw_stream.jsonl` | 1405709 | `b4a9f21e719249bfd5d4ab2add2c1e7127cea183ac06e4428aa1c4c6dcb0817f` |
| `agents/agent-949/output.log` | 1405828 | `466f29e84f136cb1312c359f693e7c9eeca953aadf292ca90f57210216c36ee6` |
| `agents/agent-949/pi-session-plan.json` | 467 | `12768182f29421b4a8e72b08ce090f38a85c1e98eec2b9a61bb4412b81b7b652` |
| `agents/agent-949/pi-session/2026-07-31T14-25-22-071Z_019fb890-bdef-7631-a062-36c37310464f.jsonl` | 197568 | `6077c70ab6bcecf6a71d9d7946a0baaac6fdc5d1b3abb18dfb3207ba23081665` |
| `attempts/by-source-tuple/c54213b9ec50b2e130a5ab1d2d97d9066e5dafce3dee19c920fad581b3189c97/pi/state.json` | 4804 | `93209e982b770d541463c5c402934d672cdd032bebdba33a5a8d442801e92bad` |
| `attempts/by-source-tuple/c54213b9ec50b2e130a5ab1d2d97d9066e5dafce3dee19c920fad581b3189c97/pi/progress.jsonl` | 63547 | `d29cd42f6fbe757ede0c1a82c087c67c83191c87ce6ab4e9b3623396bce2ad69` |
| `attempts/by-source-tuple/c54213b9ec50b2e130a5ab1d2d97d9066e5dafce3dee19c920fad581b3189c97/worktree-observer/state.json` | 739781 | `086e00b269e8185dc9d25c3d0477b9db1d144db27efefd228f53e66f435f4bf0` |
| `messages/remove-distracting-plumbing-control.jsonl` | 447 | `5d91d503a1e875b61eabc4dd82c6d6ef28f1b414c31958e27a50d6064f3b0a7f` |

No evidence file or retained worktree was mutated or removed during capture.
The regression scenario `reopen_waits_for_pi_owner_release.sh` exercises the
safe reconciliation algorithm on an isolated copy-shaped fixture: intent is
persisted first, exact process exit/reap is proven, old ownership is released,
and only then is one new generation enabled. The real tuple must not be
reconciled until this capture and the candidate validation are complete.

## Post-validation custody check

After the candidate smoke passed, all nine hashes above were re-read and still
matched byte-for-byte; PIDs 2685999 and 2686015 remained absent. No command was
run against the real task, registry row, runtime namespace, session directory,
or retained worktree. This keeps the production-shaped evidence available for
an explicit operator reconciliation after the fix is merged/installed rather
than turning validation itself into an unreviewed lifecycle mutation.
