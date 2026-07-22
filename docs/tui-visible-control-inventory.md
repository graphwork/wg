# TUI visible-control inventory

This inventory records the pointer contract after the minimal-chrome pass. The
rule is deliberately narrow: text styled as a control owns a rectangle derived
from the same frame that rendered it. Muted keyboard legends and state prose are
not pointer controls. Arbitrary task, message, log, and terminal content never
becomes a link merely because it contains brackets or an arrow.

| Surface | Visible action-shaped affordance | Pointer ownership |
|---|---|---|
| Context row, full and compact | Chat/Task/Workspace lanes, search, Controls, Help, previous/next, context ellipsis, pulse, New chat | `draw_symbolic_context_bar` records `last_context_*_area`; `handle_mouse` routes these before PTY, divider, scroll, and graph. The compact and full labels use the same rectangles. |
| Chat | previous/next, Choose chat, Close, chat tabs, tab overflow arrows, New chat | `draw_chat_content_header` and `draw_coordinator_bar` record exact `last_chat_*`/`coordinator_*_hit` regions. Terminal chat tasks still route to Detail; child PTY content itself is opaque. |
| Task Detail | section headers and iteration previous/next | Detail rows toggle through `toggle_detail_section_at_screen_row`; `render_detail_iteration_bar` records separate previous/next zones. Its center iteration label is informational. |
| Session Log | `view=[Mode]`, summary, JSON, older/newer attempt | `draw_log_tab` creates `LogHeaderHit` from each complete rendered span. `handle_mouse` invokes `cycle_log_view`, `toggle_log_summary`, `toggle_log_json`, or `log_pane_cycle_attempt`, respectively. Compact labels retain the same actions. Optional task/agent/tail/mode-state text is muted provenance prose. |
| Agency | lifecycle phase annotations such as assigning/evaluating/validating | Graph rendering records `AnnotationHitRegion`; clicking drills into the exact agency task. Status arrows inside ordinary event prose are not controls. |
| Config | selectable configuration rows | A row click selects the row; keyboard Enter/Space performs the selected edit. Former dead disclosure triangles were replaced with muted `expanded`/`collapsed` state prose. Editing brackets are field contents, not buttons. |
| Settings | Scope, Run setup, Run lint | `draw_settings_actions` records exact `last_settings_*_area` rectangles; clicks share the `s`, `W`, and `L` methods. Source labels such as `[global]` are provenance and use source colors, not button backgrounds. |
| Dashboard / service | packed service pulse and health badge | The pulse opens Dashboard and the health badge opens/closes service detail/control. Rows inside the service control panel are a keyboard selection list; bracketed keys there are muted key legends, not independent buttons. Status dots and direction arrows are state. |
| Dialogs | launcher fields/buttons, Chat/Task picker rows and footers, choice rows | Same-frame `last_launcher_*`, picker row/footer, and `choice_dialog_row_hits` rectangles are routed before click-outside or underlying content. Confirm-dialog `[y]`/`[n/Esc]` text is a muted keyboard legend, not a pointer button. |
| Files | disclosure triangles and tree rows | `TreeState::click_at` owns the rendered tree row, including its triangle. Preview truncation `[truncated]` is metadata. |
| Empty/loading states | parenthetical instructions and loading ellipses | Intentionally informational, rendered in `DarkGray`; no hit region. The actual New-chat affordance is separately styled and mapped. |

## Non-action glyph policy

* `…` produced by `truncate` and message/log wrapping means omitted content; it
  is prose. The contextual `⋮` is different and owns `last_context_menu_area`.
* `←`, `→`, status dots, checkmarks, and timestamps describe direction/state.
  They do not imply navigation.
* Bracketed hotkeys in `DarkGray` are keyboard documentation. Bracketed text
  with button/background/action styling must have a same-frame hit rectangle.
* A control that does not fit completely is not rendered and receives no hit
  rectangle. Resize clears the frame maps; Session Log hits additionally bind
  task, attempt, mode, summary, and JSON ownership so queued stale taps are
  swallowed rather than reinterpreted.
