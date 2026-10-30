/**
 * fleet-bindings.ts — the `/wg-fleet` panel's **single source of truth** keymap.
 *
 * The operator standard for the fleet panel is *full TUI interaction parity*:
 * the panel must converge on the WG TUI's `viz_viewer` interaction model, and
 * the spec for that model is **the TUI's own help text** — the `binding(...)`
 * table in `src/tui/viz_viewer/render.rs::draw_help_overlay` (and the
 * `handle_help_key` / `handle_key` handlers in `src/tui/viz_viewer/event.rs`).
 * Every row below that also exists in the TUI copies that row's key column and
 * description VERBATIM (the cross-language parity assertion in
 * `tests/smoke/scenarios/fleet_clickable_strip.sh` re-reads the Rust table and
 * compares).
 *
 * This module is deliberately data-first so the help modal can never go stale:
 *
 *  - {@link FLEET_BINDINGS} IS the keymap: dispatch ({@link matchFleetBinding})
 *    and the `?` help modal ({@link helpLines}) both read the same rows, so a
 *    new binding is discoverable the moment it exists;
 *  - `FleetBindingId` is an explicit union and the panel implements its actions
 *    as a `Record<FleetBindingId, …>`, so a binding without a handler (or a
 *    handler without a binding) is a **compile error**, not silent drift;
 *  - {@link validateFleetBindings} rejects two different actions bound to the
 *    same key in the same scope;
 *  - {@link mouseBindings} carries the mouse gestures that mirror a keyboard
 *    path (pi's TUI contract: "design every interaction with a keyboard path"),
 *    so the help modal can list both.
 *
 * Nothing here touches the DOM, a terminal, or graph state.
 */

import { Key, matchesKey, type KeyId } from "@earendil-works/pi-tui";

/**
 * Where a binding applies. Dispatch resolves the ACTIVE scope first (help is a
 * modal; a search prompt or confirm prompt owns the keyboard while it is open)
 * and then falls back to {@link FleetBindingScope} `"any"`.
 */
export type FleetBindingScope =
  | "any"
  | "tree"
  | "detail"
  | "help"
  | "search"
  | "layout"
  | "confirm"
  | "input";

/** Every panel action. The panel implements a handler for EACH of these. */
export type FleetBindingId =
  // ── Navigation (TUI: heading "Navigation") ──
  | "nav.up"
  | "nav.down"
  | "nav.pageUp"
  | "nav.pageDown"
  | "nav.top"
  | "nav.bottom"
  | "nav.scrollDown"
  | "nav.scrollUp"
  // ── Panels / layout (TUI: heading "Panels") ──
  | "layout.third"
  | "layout.half"
  | "layout.twoThirds"
  | "layout.grow"
  | "layout.shrink"
  | "layout.vizGrow"
  | "layout.vizShrink"
  | "layout.cycle"
  | "layout.toggleDock"
  | "layout.row"
  | "layout.pendingSplit"
  | "layout.pendingFull"
  | "layout.pendingHidden"
  | "layout.dockGrow"
  | "layout.dockShrink"
  // ── Inspector views ──
  | "inspector.next"
  | "inspector.prev"
  | "inspector.rawJson"
  | "inspector.jsonMode"
  | "inspector.collapseSection"
  | "inspector.scrollUp"
  | "inspector.scrollDown"
  // ── Flow (Enter/Esc) ──
  | "flow.enter"
  | "flow.back"
  | "flow.expand"
  | "flow.close"
  // ── Task actions (TUI: heading "Quick Actions (graph panel)") ──
  | "action.create"
  | "action.done"
  | "action.fail"
  | "action.retry"
  | "action.edit"
  | "action.message"
  | "action.refresh"
  | "confirm.yes"
  | "confirm.no"
  // ── Search / sort / filter ──
  | "search.open"
  | "search.prev"
  | "search.next"
  | "sort.cycle"
  | "filter.cycle"
  // ── Help modal (TUI: `handle_help_key`) ──
  | "help.open"
  | "help.close"
  | "help.up"
  | "help.down"
  | "help.pageUp"
  | "help.pageDown"
  | "help.top"
  | "help.bottom";

/** One keymap row: keys + the help text the operator reads. */
export interface FleetBinding {
  /** The action this row dispatches. */
  id: FleetBindingId;
  /**
   * pi-tui key identifiers (`Key.ctrl("c")`, `Key.alt("left")`, `"a"`, …).
   * A row may carry several equivalent keys (e.g. `Esc` and `q`).
   */
  keys: readonly KeyId[];
  /** Help key column — VERBATIM from the TUI's binding table when it exists. */
  label: string;
  /** Help description — VERBATIM from the TUI's binding table when it exists. */
  desc: string;
  /** Help section heading (TUI headings are reused verbatim). */
  section: string;
  /** Scope in which this row dispatches (see {@link FleetBindingScope}). */
  scope: FleetBindingScope;
  /**
   * `true` ⇒ the row is hidden (dormant read-only view) and never dispatches:
   * a dormant session has no cockpit, so task actions are *hidden*, not
   * failing or silently ignored.
   */
  activeOnly?: boolean;
  /**
   * Alias row: it must DISPATCH (the same action in another scope) but it is
   * not a separate line in the help modal, so the operator reads one row per
   * real binding instead of one per scope.
   */
  hideInHelp?: boolean;
}

// ── help section headings (TUI order first, panel-only sections after) ───────

export const FLEET_HELP_SECTIONS = [
  "Navigation",
  "Panels",
  "Inspector",
  "Quick Actions (graph panel)",
  "Graph search transaction",
  "General",
  "Help scrolling",
] as const;

const S_NAV = "Navigation";
const S_PANELS = "Panels";
const S_INSPECTOR = "Inspector";
const S_ACTIONS = "Quick Actions (graph panel)";
const S_SEARCH = "Graph search transaction";
const S_GENERAL = "General";
const S_HELP = "Help scrolling";

const ESC: KeyId = Key.escape;
const ENTER: KeyId = Key.enter;

/**
 * THE keymap. TUI-verbatim rows carry the Rust table's exact strings; panel-only
 * rows (the inspector dock, search prompt, confirms) extend the same vocabulary
 * rather than inventing a second one.
 */
export const FLEET_BINDINGS: readonly FleetBinding[] = [
  // ── Navigation ────────────────────────────────────────────────────────────
  {
    id: "nav.up",
    keys: [Key.up],
    label: "↑ / ↓",
    desc: "Select prev / next task",
    section: S_NAV,
    scope: "tree",
  },
  {
    id: "nav.down",
    keys: [Key.down],
    label: "↑ / ↓",
    desc: "Select prev / next task",
    section: S_NAV,
    scope: "tree",
  },
  {
    id: "nav.scrollDown",
    keys: ["j"],
    label: "j / k",
    desc: "Scroll down / up",
    section: S_NAV,
    scope: "tree",
  },
  {
    id: "nav.scrollUp",
    keys: ["k"],
    label: "j / k",
    desc: "Scroll down / up",
    section: S_NAV,
    scope: "tree",
  },
  {
    id: "nav.pageDown",
    keys: [Key.pageDown, Key.ctrl("d")],
    label: "PgUp / PgDn, Ctrl-d / u",
    desc: "Page down / up",
    section: S_NAV,
    scope: "tree",
  },
  {
    id: "nav.pageUp",
    keys: [Key.pageUp, Key.ctrl("u")],
    label: "PgUp / PgDn, Ctrl-d / u",
    desc: "Page down / up",
    section: S_NAV,
    scope: "tree",
  },
  // Detail-pane aliases: the same navigation actions, in the inspector scope
  // (the existing detail view scrolled with ↑/↓/PgUp/PgDn/Home/End; `j`/`k`
  // are the TUI's scroll keys). Aliases, so the help lists them once.
  {
    id: "nav.up",
    keys: [Key.up, "k"],
    label: "↑ / ↓",
    desc: "Line",
    section: S_INSPECTOR,
    scope: "detail",
    hideInHelp: true,
  },
  {
    id: "nav.down",
    keys: [Key.down, "j"],
    label: "↑ / ↓",
    desc: "Line",
    section: S_INSPECTOR,
    scope: "detail",
    hideInHelp: true,
  },
  {
    id: "nav.pageUp",
    keys: [Key.pageUp, Key.ctrl("u")],
    label: "PgUp / PgDn",
    desc: "Page",
    section: S_INSPECTOR,
    scope: "detail",
    hideInHelp: true,
  },
  {
    id: "nav.pageDown",
    keys: [Key.pageDown, Key.ctrl("d")],
    label: "PgUp / PgDn",
    desc: "Page",
    section: S_INSPECTOR,
    scope: "detail",
    hideInHelp: true,
  },
  {
    id: "nav.top",
    keys: [Key.home, "g"],
    label: "Home / End",
    desc: "Top / bottom",
    section: S_INSPECTOR,
    scope: "detail",
    hideInHelp: true,
  },
  {
    id: "nav.bottom",
    keys: [Key.end, Key.shift("g")],
    label: "Home / End",
    desc: "Top / bottom",
    section: S_INSPECTOR,
    scope: "detail",
    hideInHelp: true,
  },
  {
    id: "nav.top",
    keys: ["g", Key.home],
    label: "g / G",
    desc: "Jump to top / bottom",
    section: S_NAV,
    scope: "tree",
  },
  {
    id: "nav.bottom",
    keys: [Key.shift("g"), Key.end],
    label: "g / G",
    desc: "Jump to top / bottom",
    section: S_NAV,
    scope: "tree",
  },

  // ── Panels / layout (TUI-verbatim rows) ───────────────────────────────────
  {
    id: "inspector.prev",
    keys: [Key.alt("left")],
    label: "Alt-←/→",
    desc: "Cycle inspector views (with slide animation)",
    section: S_PANELS,
    scope: "detail",
    activeOnly: true,
  },
  {
    id: "inspector.next",
    keys: [Key.alt("right")],
    label: "Alt-←/→",
    desc: "Cycle inspector views (with slide animation)",
    section: S_PANELS,
    scope: "detail",
    activeOnly: true,
  },
  {
    id: "layout.third",
    keys: ["1"],
    label: "1 / 2 / 3",
    desc: "Set panel to 1/3 / 1/2 / 2/3",
    section: S_PANELS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "layout.half",
    keys: ["2"],
    label: "1 / 2 / 3",
    desc: "Set panel to 1/3 / 1/2 / 2/3",
    section: S_PANELS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "layout.twoThirds",
    keys: ["3"],
    label: "1 / 2 / 3",
    desc: "Set panel to 1/3 / 1/2 / 2/3",
    section: S_PANELS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "layout.grow",
    keys: ["+", Key.rightbracket],
    label: "+ / -",
    desc: "Resize the fleet view by 5% (keyboard path for the top-edge drag)",
    section: S_PANELS,
    scope: "any",
  },
  {
    id: "layout.shrink",
    keys: ["-", Key.leftbracket],
    label: "+ / -",
    desc: "Resize the fleet view by 5% (keyboard path for the top-edge drag)",
    section: S_PANELS,
    scope: "any",
  },
  {
    id: "layout.pendingHidden",
    keys: ["0"],
    label: "r / f / 0",
    desc: "Pending Split / Full / Hidden (layout row)",
    section: S_PANELS,
    scope: "layout",
  },
  {
    id: "layout.pendingSplit",
    keys: ["r"],
    label: "r / f / 0",
    desc: "Pending Split / Full / Hidden (layout row)",
    section: S_PANELS,
    scope: "layout",
  },
  {
    id: "layout.pendingFull",
    keys: ["f"],
    label: "r / f / 0",
    desc: "Pending Split / Full / Hidden (layout row)",
    section: S_PANELS,
    scope: "layout",
  },
  {
    id: "layout.dockGrow",
    keys: ["+"],
    label: "+ / -",
    desc: "Resize pending panel by 5% (layout row only)",
    section: S_PANELS,
    scope: "layout",
    activeOnly: true,
  },
  {
    id: "layout.dockShrink",
    keys: ["-"],
    label: "+ / -",
    desc: "Resize pending panel by 5% (layout row only)",
    section: S_PANELS,
    scope: "layout",
    activeOnly: true,
  },
  {
    id: "layout.row",
    keys: ["p"],
    label: "p",
    desc: "Open the Panel/Layout row (dock presets, pending split/full/hidden)",
    section: S_PANELS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "layout.toggleDock",
    keys: ["\\"],
    label: "\\",
    desc: "Toggle right panel visible",
    section: S_PANELS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "layout.vizGrow",
    keys: ["i"],
    label: "i",
    desc: "Grow viz pane (10% per press, wraps)",
    section: S_PANELS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "layout.vizShrink",
    keys: ["v"],
    label: "v",
    desc: "Shrink viz pane (10% per press, wraps)",
    section: S_PANELS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "layout.cycle",
    keys: ["="],
    label: "=",
    desc: "Cycle layout: split/panel/graph",
    section: S_PANELS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "inspector.rawJson",
    keys: [Key.shift("r")],
    label: "R",
    desc: "Toggle raw JSON in Detail tab",
    section: S_PANELS,
    scope: "detail",
    activeOnly: true,
  },
  {
    id: "inspector.collapseSection",
    keys: [Key.space],
    label: "Space",
    desc: "Toggle section collapse in Detail",
    section: S_PANELS,
    scope: "detail",
    activeOnly: true,
  },

  // ── Inspector (panel-only additions) ──────────────────────────────────────
  {
    id: "inspector.jsonMode",
    keys: [Key.shift("j")],
    label: "J",
    desc: "Toggle raw JSON mode",
    section: S_INSPECTOR,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "inspector.scrollUp",
    keys: [Key.shift("up")],
    label: "Shift-↑/↓",
    desc: "Scroll detail panel",
    section: S_INSPECTOR,
    scope: "detail",
    activeOnly: true,
  },
  {
    id: "inspector.scrollDown",
    keys: [Key.shift("down")],
    label: "Shift-↑/↓",
    desc: "Scroll detail panel",
    section: S_INSPECTOR,
    scope: "detail",
    activeOnly: true,
  },

  // ── Flow ──────────────────────────────────────────────────────────────────
  {
    id: "flow.enter",
    keys: [ENTER, "l", Key.right],
    label: "Enter",
    desc: "Activate / apply pending layout",
    section: S_NAV,
    scope: "tree",
  },
  {
    id: "flow.enter",
    keys: [ENTER],
    label: "Enter",
    desc: "Return to the tree",
    section: S_NAV,
    scope: "detail",
    hideInHelp: true,
  },
  {
    id: "flow.enter",
    keys: [ENTER],
    label: "Enter / Esc",
    desc: "Apply / restore complete prior layout",
    section: S_PANELS,
    scope: "layout",
    activeOnly: true,
  },
  {
    id: "flow.back",
    keys: [ESC, "q", "h", "l", Key.backspace, Key.left],
    label: "Esc / q / ← / Bksp",
    desc: "Back to the tree; from the tree, restore the prior layout or close",
    section: S_NAV,
    scope: "detail",
  },
  {
    id: "flow.back",
    keys: [ESC],
    label: "Enter / Esc",
    desc: "Apply / restore complete prior layout",
    section: S_PANELS,
    scope: "layout",
  },
  {
    id: "flow.enter",
    keys: [ENTER],
    label: "Enter",
    desc: "Submit the typed input",
    section: S_ACTIONS,
    scope: "input",
    activeOnly: true,
    hideInHelp: true,
  },
  {
    id: "flow.back",
    keys: [ESC],
    label: "Esc",
    desc: "Cancel the typed input",
    section: S_ACTIONS,
    scope: "input",
    activeOnly: true,
    hideInHelp: true,
  },
  {
    id: "flow.expand",
    keys: ["o", Key.space],
    label: "o / Space",
    desc: "Expand / collapse the selected subtree",
    section: S_NAV,
    scope: "tree",
  },
  {
    id: "flow.close",
    keys: ["q"],
    label: "q",
    desc: "Quit",
    section: S_GENERAL,
    scope: "tree",
  },
  {
    id: "flow.back",
    keys: [ESC],
    label: "Esc",
    desc: "Restore the prior layout (or close when there is nothing to restore)",
    section: S_GENERAL,
    scope: "tree",
  },

  // ── Task actions (TUI: "Quick Actions (graph panel)" verbatim) ────────────
  {
    id: "action.create",
    keys: ["a"],
    label: "a",
    desc: "Create new task",
    section: S_ACTIONS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "action.done",
    keys: [Key.shift("d")],
    label: "D",
    desc: "Mark selected task done",
    section: S_ACTIONS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "action.fail",
    keys: ["f"],
    label: "f",
    desc: "Mark selected task failed",
    section: S_ACTIONS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "action.retry",
    keys: ["x"],
    label: "x",
    desc: "Retry selected task",
    section: S_ACTIONS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "action.edit",
    keys: ["e"],
    label: "e",
    desc: "Edit task description",
    section: S_ACTIONS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "action.message",
    keys: [Key.shift("m")],
    label: "M",
    desc: "Send message to task agent",
    section: S_ACTIONS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "action.refresh",
    keys: ["r"],
    label: "r",
    desc: "Force refresh",
    section: S_ACTIONS,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "confirm.yes",
    keys: ["y", ENTER],
    label: "y / Enter",
    desc: "Confirm the pending action",
    section: S_ACTIONS,
    scope: "confirm",
    activeOnly: true,
  },
  {
    id: "confirm.no",
    keys: ["n", ESC],
    label: "n / Esc",
    desc: "Cancel the pending action",
    section: S_ACTIONS,
    scope: "confirm",
    activeOnly: true,
  },

  // ── Search transaction (TUI: "Graph search transaction" verbatim) ─────────
  {
    id: "search.open",
    keys: ["/"],
    label: "/",
    desc: "Start exact-task search in command mode",
    section: S_SEARCH,
    scope: "any",
    activeOnly: true,
  },
  {
    id: "search.prev",
    keys: [Key.up, "k", Key.shift("tab")],
    label: "↑/k/BackTab",
    desc: "Previous result",
    section: S_SEARCH,
    scope: "search",
  },
  {
    id: "search.next",
    keys: [Key.down, "j", Key.tab],
    label: "↓/j/Tab",
    desc: "Next result",
    section: S_SEARCH,
    scope: "search",
  },
  {
    id: "flow.enter",
    keys: [ENTER],
    label: "Enter/tap",
    desc: "Commit stable task ID; clear all match state",
    section: S_SEARCH,
    scope: "search",
  },
  {
    id: "flow.back",
    keys: [ESC],
    label: "Esc",
    desc: "Clear totally without quitting",
    section: S_SEARCH,
    scope: "search",
  },

  // ── General ───────────────────────────────────────────────────────────────
  {
    id: "sort.cycle",
    keys: ["s"],
    label: "s",
    desc: "Cycle sort: Chrono↓/↑/Status",
    section: S_GENERAL,
    scope: "any",
  },
  {
    id: "filter.cycle",
    keys: [".", Key.lessthan],
    label: ". / <",
    desc: "Cycle plumbing: hidden → running only → all",
    section: S_GENERAL,
    scope: "any",
  },
  {
    id: "help.open",
    keys: ["?"],
    label: "?",
    desc: "Toggle this help",
    section: S_GENERAL,
    scope: "any",
  },

  // ── Help modal (TUI: handle_help_key) ─────────────────────────────────────
  {
    id: "help.close",
    keys: ["?", ESC, "q"],
    label: "? / Esc / q",
    desc: "Close Help; restore view",
    section: S_HELP,
    scope: "help",
  },
  {
    id: "help.up",
    keys: [Key.up, "k"],
    label: "↑ / ↓, j / k",
    desc: "Line",
    section: S_HELP,
    scope: "help",
  },
  {
    id: "help.down",
    keys: [Key.down, "j"],
    label: "↑ / ↓, j / k",
    desc: "Line",
    section: S_HELP,
    scope: "help",
  },
  {
    id: "help.pageUp",
    keys: [Key.pageUp],
    label: "PgUp / PgDn",
    desc: "Page",
    section: S_HELP,
    scope: "help",
  },
  {
    id: "help.pageDown",
    keys: [Key.pageDown],
    label: "PgUp / PgDn",
    desc: "Page",
    section: S_HELP,
    scope: "help",
  },
  {
    id: "help.top",
    keys: [Key.home, "g"],
    label: "Home / End",
    desc: "Top / bottom",
    section: S_HELP,
    scope: "help",
  },
  {
    id: "help.bottom",
    keys: [Key.end, Key.shift("g")],
    label: "Home / End",
    desc: "Top / bottom",
    section: S_HELP,
    scope: "help",
  },
];

/**
 * Mouse gestures and the keyboard path each one mirrors. pi's TUI contract is
 * "design every interaction with a keyboard path", so this table is asserted
 * against the keymap (every mouse gesture names a keyboard binding id) and is
 * rendered in the help modal as the "Mouse" section header line.
 */
export interface FleetMouseBinding {
  gesture: string;
  desc: string;
  /** The keyboard binding id that reaches the same action. */
  keyboard: FleetBindingId;
}

export const FLEET_MOUSE_BINDINGS: readonly FleetMouseBinding[] = [
  { gesture: "wheel", desc: "Scroll the focused pane", keyboard: "nav.down" },
  { gesture: "click row", desc: "Select the task under the pointer", keyboard: "nav.down" },
  { gesture: "click again", desc: "Inspect the selected task", keyboard: "flow.enter" },
  { gesture: "drag content", desc: "Pan the pane", keyboard: "nav.scrollDown" },
  { gesture: "drag divider", desc: "Grow/shrink the inspector dock", keyboard: "layout.half" },
  { gesture: "drag top edge", desc: "Resize the fleet view", keyboard: "layout.grow" },
  { gesture: "click [< Back]", desc: "Return to the tree", keyboard: "flow.back" },
  { gesture: "wheel over Help", desc: "Scroll Help", keyboard: "help.down" },
];

/**
 * True when `data` matches any of a row's keys.
 *
 * A single-character key is compared DIRECTLY (`data === key`) as well as
 * through `matchesKey`, because pi-tui parses `"+"` as a modifier separator
 * (`matchesKey("+", "+") === false`), and a bare `+`/`?`/`<` is exactly the
 * kind of key the TUI's keymap uses. The `matchesKey` call still runs so Kitty
 * CSI-u / modifyOtherKeys spellings of the same character are recognized.
 */
function matchesAny(data: string, keys: readonly KeyId[]): boolean {
  for (const key of keys) {
    if (key.length === 1 && data === key) return true;
    if (matchesKey(data, key)) return true;
  }
  return false;
}

/**
 * Resolve an input event to a binding id for the CURRENT scope.
 *
 * Scope order: the active modal/interactive scope first (so `q` closes Help but
 * quits from the tree, and a search prompt owns printable keys), then `"any"`.
 * `inactive` (the dormant read-only view) drops every `activeOnly` row, so task
 * actions cannot fire in a session that has no cockpit.
 */
export function matchFleetBinding(
  data: string,
  scope: FleetBindingScope,
  opts: { inactive?: boolean; table?: readonly FleetBinding[]; anyFallback?: boolean } = {},
): FleetBindingId | null {
  const table = opts.table ?? FLEET_BINDINGS;
  const inactive = opts.inactive === true;
  const usable = (row: FleetBinding): boolean => !(inactive && row.activeOnly);
  for (const row of table) {
    if (!usable(row) || row.scope !== scope) continue;
    if (matchesAny(data, row.keys)) return row.id;
  }
  if (opts.anyFallback === false) return null;
  for (const row of table) {
    if (!usable(row) || row.scope !== "any") continue;
    if (matchesAny(data, row.keys)) return row.id;
  }
  return null;
}

/** Rows the help modal shows for the current session state (dormant hides actions). */
export function helpBindings(opts: { inactive?: boolean; table?: readonly FleetBinding[] } = {}): FleetBinding[] {
  const table = opts.table ?? FLEET_BINDINGS;
  return table.filter((row) => !(opts.inactive === true && row.activeOnly));
}

/**
 * The help modal body, auto-generated from the binding table: a heading line per
 * section (TUI cyan/bold) and `  <key padded 14><desc>` rows — the exact shape
 * `draw_help_overlay` renders, so the panel's help text IS the TUI's help text
 * for every shared binding.
 */
export function helpLines(opts: { inactive?: boolean; table?: readonly FleetBinding[] } = {}): string[] {
  const rows = helpBindings(opts).filter((row) => row.hideInHelp !== true);
  const out: string[] = [];
  for (const section of FLEET_HELP_SECTIONS) {
    const inSection = rows.filter((row) => row.section === section);
    if (inSection.length === 0) continue;
    if (out.length > 0) out.push("");
    out.push(section);
    const seen = new Set<string>();
    for (const row of inSection) {
      // Collapse rows that repeat the same label+description inside a section
      // (e.g. the two "↑ / ↓" navigation rows are one help line in the TUI).
      const dedupeKey = `${row.label}\u0000${row.desc}`;
      if (seen.has(dedupeKey)) continue;
      seen.add(dedupeKey);
      out.push(`  ${row.label.padEnd(14)}${row.desc}`);
    }
  }
  if (out.length > 0) out.push("");
  out.push("Mouse");
  for (const row of FLEET_MOUSE_BINDINGS) {
    out.push(`  ${row.gesture.padEnd(14)}${row.desc}`);
  }
  return out;
}

/**
 * Reject two DIFFERENT actions bound to the same key in the same scope. Rows
 * that repeat a key with the SAME id are the TUI's own duplication (one key
 * listed in two help sections), which is legitimate.
 */
export function validateFleetBindings(table: readonly FleetBinding[] = FLEET_BINDINGS): void {
  const seen = new Map<string, FleetBindingId>();
  for (const row of table) {
    for (const key of row.keys) {
      const slot = `${row.scope}\u0000${key}`;
      const prior = seen.get(slot);
      if (prior !== undefined && prior !== row.id) {
        throw new Error(
          `fleet keymap conflict: ${JSON.stringify(key)} in scope ${row.scope} is bound to both ${prior} and ${row.id}`,
        );
      }
      seen.set(slot, row.id);
    }
  }
}

/** Every binding id the keymap declares (used by the panel's handler record). */
export function bindingIds(table: readonly FleetBinding[] = FLEET_BINDINGS): FleetBindingId[] {
  return [...new Set(table.map((row) => row.id))];
}
