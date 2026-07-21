# TUI graph-search navigation — design menu

## Scope and recommendation

This document is **design-only**. It records the current bug from source,
compares viable behavior menus, and recommends a future implementation. No
production code or test change is part of this round.

**Recommendation: Option B, transactional exact-task search.** `/` remains the
authoritative keyboard entry point for search. The contextual row also exposes
a labelled pointer affordance:

- graph/task/workspace contexts: **`/ Search`** (task search),
- Chat context: **`/ History`** (search only the loaded chat-history projection).

The label may contract to `/` only when the terminal cannot fit the full text;
it is never implemented as a printable-key interception while an embedded PTY
or editor owns input. In Chat PTY focus, `/` continues to belong to the child;
the user enters command mode first (`Ctrl+O`) or uses the contextual pointer
affordance. This contract is therefore the same through a direct terminal,
tmux, mosh, and Termux.

Graph/task search and Chat history search are deliberately separate features.
They have separate state, result sets, rendering labels, and acceptance rules.
Starting one clears the other.

## Source-confirmed current behavior and failure

The current behavior is not merely a styling glitch; the persistent result is
encoded as application state:

1. `event.rs::handle_graph_key` opens graph search on `/` by setting
   `search_active`, clearing the query/results, and entering
   `InputMode::Search`. The same behavior is hidden behind
   `open_task_context_picker`, which is reached by clicking the contextual
   `Task ▾` label; the label does not identify itself as search.
2. `event.rs::handle_search_input` sends `Enter` to
   `VizApp::accept_search_and_jump`. Up/Down instead call graph-line scroll,
   while the generic `Char(c)` arm consumes `j` and `k` as query text. Only
   Tab/BackTab currently change the match index, and candidates are rendered
   lines rather than an enforced set of exact task results.
3. `state.rs::accept_search` deliberately sets only `search_active = false`,
   restores the unfiltered line set, and **keeps** `search_input`,
   `fuzzy_matches`, and `current_match`. `accept_search_and_jump` then stores a
   line-number target.
4. `state.rs::has_active_search` ignores `search_active`; any non-empty query
   with matches remains “active.” `render.rs::draw_viz_content` uses that
   predicate to color every accepted match, and the legacy status renderer has
   a dedicated “Filter locked” branch. This is the direct cause of the
   permanent/stuck highlighting after `Enter`.
5. Normal graph `n/N` inspect the retained `fuzzy_matches`, so acceptance also
   changes later command-mode semantics (`n` no longer means New chat). The
   retained state is therefore behavioral, not cosmetic.
6. Chat search is separate in storage (`ChatSearchState`) and routing
   (`InputMode::ChatSearch`). Its `Enter` intentionally keeps query/highlights,
   with `n/N` navigating text occurrences. That behavior should not be changed
   as a side effect of fixing graph search.
7. Graph snapshot derivation is already asynchronous after bootstrap:
   `update_search` requests a latest-wins graph snapshot, `GraphViewKey` carries
   query/active intent, and `derive_graph_snapshot` reruns matching off the UI
   thread. However, snapshot install preserves `current_match` by vector index,
   not stable task ID, and accepted feedback stores an original line number.
   A reorder, refresh, deletion, or archive can therefore retarget those
   transient coordinates.
8. `dispatch_event(Event::Resize)` cancels layout drag and clears coordinator
   picker/dialog hits, but it does not invalidate graph/context search hit
   rectangles. `handle_mouse` also uses normal graph click handling while
   `InputMode::Search` is active; selecting a row does not transactionally clear
   graph-search state.
9. `render_context_row` adds search status only after identity/optional controls
   and only if it fits. The active minimal contextual-row path does not render
   the old bottom action-hint bar, so discoverability cannot be solved by
   editing legacy `draw_status_bar`/`draw_action_hints` text alone.

These findings were confirmed in
`src/tui/viz_viewer/{event.rs,state.rs,render.rs}` at the named symbols. They
also explain why a local fix such as “hide the yellow style after Enter” is
insufficient: retained matches would still alter `n`, refresh, and hit-test
semantics.

## Alternatives

### Option A — keep a persistent vim-style accepted search

`Enter` would remove the filter but keep query/matches; `n/N` would navigate
accepted matches and `Esc` would clear.

**Advantages:** smallest code delta; familiar to strict vim users; preserves
current `n/N` implementation. **Costs:** preserves the state users report as
stuck, overloads `n` (New chat versus next match), requires a permanent status
indicator at every width, and increases stale-ID/hit-region handling. It does
not satisfy the requested short-lived accepted feedback, so it is rejected.

### Option B — transactional exact-task search (recommended)

Search is a temporary task chooser. `Enter` or an exact-row click commits one
stable task ID, clears all query/filter/match semantics, restores ordinary
navigation, and leaves only short feedback. A new `/` starts a new transaction.

**Advantages:** matches the user’s “find then go” model; removes the `n`
conflict; gives Enter/Esc crisp invariants; makes stale state fail closed; reuses
the current async projection. **Costs:** users cannot continue an accepted
search with `n/N`; they press `/` again. This is the recommended tradeoff.

### Option C — replace inline filtering with a modal task palette

`/` would open a separate ranked list overlay, leaving the graph unchanged
until commit.

**Advantages:** clearest exact-result hit testing and room for metadata;
query/editor and list navigation can be visually separated. **Costs:** largest
change, duplicates existing coordinator/launcher palette mechanics, introduces
another modal and responsive layout, and discards useful ancestor context from
the existing filtered graph. It is disproportionate to this bug and deferred.

### Sub-decision menu within Option B

| Question | Alternatives | Recommendation |
|---|---|---|
| Candidate granularity | every rendered line; nearest owning task; exact task-node rows only | exact task-node rows only, so Enter/click cannot ambiguously retarget |
| Accepted feedback identity | original line; match index; stable task ID | stable task ID resolved to a line each frame, disappearing if removed |
| `n/N` while editing | navigate; literal input | literal query input; no accepted graph-search state |
| `Tab/BackTab` | panel/layout actions; result navigation | next/previous result only while Editing; normal semantics outside |
| `j/k` while editing | literal input; result navigation | result navigation, as requested; document the exception and permit pasted query text |
| Context switch | retain per-context graph query; clear | clear graph search/feedback on Chat/Task/Workspace change |
| Resize | clear query; preserve semantic state and invalidate coordinates | preserve Editing intent, immediately invalidate frame-derived hits, rebuild next frame |
| Refresh/delete | preserve numeric index; stable-ID remap | stable-ID remap, first remaining match fallback, empty if none |

## Recommended graph-search state machine

The future graph state should be explicit (`GraphSearchPhase` or an equivalent
single authoritative enum):

| State | Stored/UI state | Entry | Exit |
|---|---|---|---|
| **Inactive** | no query, filter, matches, or target feedback | startup or expiry of feedback | `/` or contextual `/ Search` -> Editing |
| **Editing/filtering** | task query plus an asynchronously-derived, filtered graph and one selected exact task match | `/` | `Enter` -> Accepted feedback; `Esc` or a context change -> Cleared |
| **Accepted/jump feedback** | query/filter/match coloring is gone; the selected task ID has a two-second target indication | `Enter` or clicking an exact result | timer -> Inactive; context change -> Cleared; `/` -> Editing |
| **Cleared** | no query, filter, coloring, selected match, or target indication | `Esc`, context change, or removal of the last/accepted target | `/` -> Editing (otherwise behavior is identical to inactive) |

`Enter` is a commit, not a vim-style persistent-search lock. It resolves the
selected match to its exact stable task ID, selects that task, restores the full
graph, keeps the target visible, and shows only a short target indication. It
never leaves accepted match coloring behind. An ordinary task opens the task
context/detail; a chat or user-board task follows its normal exact-task context.
Graph keyboard focus remains on the committed task so ordinary arrows work
immediately.

`Esc` in Editing always clears the graph query, filter, selected result, match
coloring, and target feedback and returns to normal navigation. It does **not**
quit. A later `Esc` in normal graph navigation retains the existing quit/error
behavior.

## Bindings

### While graph search is Editing

- `Up` or `k`: previous exact visible task match (wraps).
- `Down` or `j`: next exact visible task match (wraps).
- `Tab`: next exact visible task match; `BackTab`: previous.
- `Enter`: select/focus the exact match and clear search presentation.
- `Esc`: clear search and return to normal navigation.
- `Left`/`Right`: horizontal graph scroll.
- `n` and `N`: literal query input. They are **not** match-navigation aliases
  while the query editor owns printable input.
- Other printable characters edit the query. Plain `j`/`k` are the documented
  navigation exception; a pasted query may still contain them.

This gives arrows, touch keyboards, and vim users one coherent match-selection
axis. It avoids a second `n/N` axis that would conflict with query entry and
with `n`'s normal command-mode meaning.

### Outside graph search

- `Up`/`Down` retain ordinary previous/next task selection.
- `j`/`k` retain ordinary graph-line scrolling.
- `n` retains **New chat**; `N` has no graph-search meaning.
- `Tab` retains panel focus and `BackTab` retains layout cycling.

There is no persistent accepted-search navigation state. Start a new search
with `/` to choose another task.

### Chat history search

Chat labels say **Chat history** (never just “Search”). Its existing text-history
semantics remain: typing searches message text in the bounded loaded projection,
`Tab`/`BackTab` (or `Ctrl+N`/`Ctrl+P`) move among text occurrences, `Ctrl+A`
requests the bounded all-history path, `Enter` accepts while retaining Chat text
highlights, and `n/N` navigate accepted Chat occurrences. `Esc` clears only Chat
history search. Graph match state never participates.

## Mouse/touch

During graph Editing, a press on a highlighted exact task row commits that
specific stable task ID, equivalent to selecting it and pressing `Enter`.
Presses on contextual ancestor rows or empty canvas do not silently retarget the
search. Normal graph hit testing resumes after acceptance/clear. Frame-derived
hit regions are invalidated immediately on resize and rebuilt by the next draw.

## Refresh, resize, and deletion rules

Graph-sized derivation and fuzzy matching stay on the existing latest-wins
snapshot worker. Keystrokes change only bounded UI intent and request a new
snapshot; rendering never scans the graph.

- Snapshot publication remaps the selected match by stable task ID, not by a
  stale vector index or line number. If that task was deleted/archived/hidden,
  selection falls back to the first remaining match; if none remain it becomes
  empty.
- Accepted feedback stores a stable task ID, not a line. A refresh resolves its
  current line; deletion simply makes the indication disappear.
- Resize preserves semantic Editing state but invalidates all old graph and
  contextual hit rectangles before the next draw.
- Switching Chat/Task/Workspace clears graph search/filter/feedback. Starting
  Chat history search clears graph search, and starting graph search clears Chat
  history search.
- The renderer gates graph match coloring on Editing plus a non-empty current
  query. Hidden/stale worker products can therefore never revive accepted
  coloring while a replacement snapshot is in flight.

## Future implementation plan (separately approved)

Keep the change focused in `src/tui/viz_viewer/`:

1. **State (`state.rs`).** Add one authoritative graph-search phase and helpers
   for begin, update intent, move selection, accept exact task, context-clear,
   and expiry. Gate coloring/filter semantics on Editing, not merely on a
   non-empty retained vector. Acceptance captures a stable task ID, clears the
   semantic query/filter immediately, selects that ID, and asks the existing
   snapshot engine for the unfiltered projection. Store accepted feedback by
   task ID + deadline. Preserve current off-thread graph matching; do not add a
   render/input scan of `lines`, `task_order`, or the graph.
2. **Candidate derivation (`state.rs` snapshot path).** Produce navigable
   results only for exact task-node rows. Carry/remap the current match’s task ID
   across `install_graph_snapshot`; if absent, select the first remaining hit.
   Resolve feedback through the new `node_line_map`; missing/deleted targets
   render nothing and clear on the next bounded cleanup.
3. **Keys (`event.rs`).** Route Up/k to previous and Down/j to next before the
   generic printable arm in `InputMode::Search`; keep Tab/BackTab as aliases.
   Make Enter call the exact-task transaction and Esc the total clear. Remove
   graph normal-mode `n/N` dependence on retained fuzzy vectors. Leave
   `handle_chat_search_input` acceptance and text navigation unchanged.
4. **Context transitions (`event.rs`).** Centralize `begin_graph_search`,
   `begin_chat_history_search`, and `clear_graph_search_for_context_change` so
   keyboard and pointer paths cannot drift. Apply context-clear before Chat,
   Task, Workspace, launcher, and tab transitions. During Editing, an exact
   highlighted graph-row click commits that stable task; non-result/empty rows
   do not retarget.
5. **Resize/hits (`event.rs`, `render.rs`).** Add a dedicated contextual-search
   hit rectangle rather than overloading the task/chat picker rectangle. Reset
   graph and contextual hit rectangles on `Event::Resize` before queued pointer
   events can use old geometry. Rebuild only from the next render.
6. **Rendering (`render.rs`).** In the active contextual row, reserve a labelled
   `/ Search` (graph/task/workspace) or `/ History` (Chat) before optional
   ambient detail; contract to `/` only at the hard width floor. While editing,
   render scope first (`Tasks /…` versus `Chat history /…`). Render graph match
   color only in Editing. Accepted feedback is one task line/identity for about
   two seconds. Update help text, but do not revive the removed legacy global
   footer/status bar.

## Exact future validation plan

The implementation follow-up should add failing tests first and use these
assertions/names (names may be adjusted to local test-module conventions, but
coverage may not be weakened).

### Source-level state/key regression tests

In `state.rs` and `event.rs` test modules:

- `graph_search_enter_is_transactional`: seed at least three exact task rows,
  search a query matching two, select the second, press Enter, and assert exact
  selected task ID, full visible line count, empty query, no current/fuzzy
  semantic matches, non-Editing phase, and only task-ID target feedback.
- `graph_search_esc_is_total_clear_and_does_not_quit`: from Editing and from
  accepted feedback, press Esc; assert query/filter/matches/feedback empty,
  normal input mode, and `should_quit == false`.
- `graph_search_context_switch_and_refresh_cannot_restore_match_coloring`:
  accept and clear, then install a newer snapshot whose old numeric match index
  points at a different task; assert phase/query prevent coloring and no search
  status returns.
- `graph_search_refresh_remaps_selected_hit_by_task_id`: select result B,
  publish a reordered snapshot, and assert B remains current. Publish another
  without B (delete/archive/hide) and assert first remaining result or none,
  never the task now occupying B’s old line/index.
- `graph_search_arrows_and_vim_keys_choose_matches`: dispatch Up, Down, `k`,
  `j`, Tab, and BackTab in Editing and assert wrapped exact-task result order
  plus visibility. Dispatch Up/Down and j/k in Normal and assert ordinary task
  selection/graph scroll remains unchanged.
- `graph_search_n_is_query_but_normal_n_is_new_chat`: in Editing, `n/N` extend
  the query and do not launch/navigate; after accept/clear, `n` opens New chat
  and `N` does not revive search.
- `chat_history_search_is_scoped_and_does_not_leak_graph_state`: starting Chat
  search clears graph state, Enter retains only Chat text highlights, `n/N`
  move Chat occurrences, and returning to graph shows no graph query/filter.

### Mouse, resize, and render tests

In `event.rs`/`render.rs` test modules:

- `clicking_exact_search_result_commits_that_task`: render a filtered graph,
  click the first and last printable cells of a highlighted exact row, and
  assert the same stable task ID plus transactional clear. Click an ancestor or
  empty row and assert no commit/retarget.
- `resize_invalidates_search_hit_regions_before_redraw`: capture old graph and
  `/ Search` rectangles, dispatch `Resize`, click both stale coordinates before
  redraw, and assert no selection/context change; redraw at the new dimensions
  and assert new hits work.
- `accepted_feedback_tracks_task_across_refresh_and_disappears_on_delete`:
  render feedback, reorder rows, assert it follows the task, then remove the
  task and assert no unrelated row is highlighted.
- Context-row snapshots at widths **160** (wide), **96** (narrow/stacked),
  **55** (leave-compact hysteresis boundary), **49** (compact entry), and
  **32** (Termux-like hard case). Assert a visible task-search affordance and
  exact live hit region at every supported width, scoped `Tasks`/`Chat history`
  editing labels where they fit, and that the New-chat primary action and
  selected identity obey the contextual-row priority contract.
- Route `/` through the actual dispatcher under Graph command focus, Chat
  command focus, Chat PTY focus, ChatInput, MessageInput, and launcher/editor
  modes. Assert Graph/Chat command contexts open the correct scope; PTY and text
  editors receive printable `/` unchanged.

### Permanent human-flow smoke

Add `tests/smoke/scenarios/tui_search_navigation.sh` and a grow-only
`tests/smoke/manifest.toml` entry owned by the implementation task. The scenario
must:

1. create an isolated graph with three visually separated task IDs/titles, two
   sharing a search term;
2. launch real `wg tui --no-mouse --show-keys` in a fresh 120x30 tmux session;
3. send `/`, type the shared term, send Down (and a second pass with `j`), then
   Enter; capture the pane and prove the exact second task is selected while
   the other match is not persistently colored/filtered and ordinary Down now
   selects the ordinary next task;
4. repeat search then Esc and prove the full graph returns and a second Esc is
   the only one with normal exit behavior;
5. resize the pane through 160, 96, 49, and 32 columns, checking `/ Search` or
   its documented `/` contraction and that `/` still opens `Tasks` search;
6. open Chat command context and prove `/` labels/searches Chat history, then
   return to Graph and prove no Chat query appears there;
7. mutate/refresh the graph during Editing (reorder then delete the selected
   hit), assert stable-ID remap/fail-closed behavior, and keep key
   acknowledgement bounded (100 ms, matching existing TUI latency scenarios).

The scenario must be credential-free, trap tmux/session cleanup on success,
failure, INT, and TERM, and use one isolated bounded `CARGO_TARGET_DIR` only if
it builds locally. The follow-up validation sequence is:

```text
cargo fmt
cargo fmt --check
cargo clippy
cargo test <focused graph-search tests>
cargo test
CARGO_TARGET_DIR=<one temporary bounded path> <owned smoke scenario>
rm -rf <that temporary target>
```

Then run the WG-owned smoke gate. The temporary target must be removed even on
failure. This design-only round intentionally does not add that scenario or
modify the manifest.
