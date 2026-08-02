# Automatic task archival is opt-in

Task archival moves terminal task records out of the active `graph.jsonl` view and
preserves their full records in `archive.jsonl`. It is an **organization feature**,
not backup, garbage collection, or evidence destruction.

## Safe default

```toml
[dispatcher]
archive_retention_days = 0
```

`0` disables automatic task archival. It is the compiled default for a missing
key, new graphs, `wg setup`, serialized default config, and built-in starter
profiles. A daemon start, restart, reload, or binary upgrade therefore cannot
make old terminal tasks disappear merely because they crossed a wall-clock age.
`wg service status` reports:

```text
Automatic archival: disabled (retention=0; visible history preserved)
```

Built-in profiles deliberately inherit this default instead of writing an
authoritative retention value. That lets an operator's explicit non-zero
project, global, or custom-profile setting survive setup and profile changes.
An old config that explicitly contains a non-zero value remains an opt-in; WG
does not silently rewrite it during load or reload.

Inspect the effective value and its source before an upgrade:

```bash
wg config get dispatcher.archive_retention_days
wg service status
wg archive auto --dry-run
```

At retention `0`, the last command reports that automatic archival is disabled.
It also cancels any stale held automatic plan without changing `graph.jsonl` or
writing archive records. The daemon performs the same cancellation after a
config reload/tick. `wg archive auto --confirm` refuses while disabled.

## Risk model

Completed and abandoned development tasks can still carry live operational
value: dependency boundaries, artifact and receipt references, session and cost
attribution, audit trails, incident timelines, recovery breadcrumbs, and the
reason a change was accepted or reverted. Wall-clock age alone does not prove
that this evidence is no longer needed. Hiding a large backlog immediately after
a restart or upgrade also makes an archival action easy to misdiagnose as data
loss.

For evidence-bearing development graphs, keep automatic retention disabled and
review batches manually. Before moving records, back up the whole WG directory
and repository according to local policy. `archive.jsonl` is useful preserved
organization state, but it is in the same project directory and is **not** an
independent backup.

## Deliberate opt-in workflow

Choose a retention only after deciding that unattended incremental movement is
acceptable for this graph:

```bash
wg config set dispatcher.archive_retention_days 30
wg archive auto --dry-run
wg archive auto --confirm
```

The first eligible/backlogged batch is held rather than archived. The dry-run
persists an exact, sorted batch with task digests and prints its task IDs.
Confirmation accepts that batch only if the candidate binary/build, retention,
task identities, and task bytes still match. A changed task or changed plan fails closed. Use
`wg archive --undo` to restore the most recently archived batch.

After confirmation, only small, recent increments beyond the acknowledged
watermark may move automatically. A build change, retention change, overdue
watermark, or oversized increment creates another visible hold requiring review.
Disable at any time with:

```bash
wg config set dispatcher.archive_retention_days 0
wg archive auto --dry-run
```

Disabling clears/neutralizes a pending batch and never confirms it.

## Manual archival remains available

Automatic retention is independent of explicit organization:

```bash
# Review without mutation.
wg archive --older 30d --dry-run

# Archive named, reviewed records.
wg archive task-a task-b

# Bulk archival requires an explicit confirmation flag.
wg archive --older 30d --yes

# Reverse the last batch.
wg archive --undo
```

Manual archival does not enable automatic retention. Existing archive files are
never deleted or rewritten merely because retention becomes `0`.

## Not stream/cache cleanup

`dispatcher.archive_retention_days` applies only to task-graph archival.
Bounded raw stream compression, terminal-output tails, owned build-cache cleanup,
chat-file retention, worktree retention, and `wg gc` have separate policies and
risk boundaries. Disabling automatic task archival must not disable those bounded
storage safeguards, and those safeguards must not be used as authority to move or
delete task history.
