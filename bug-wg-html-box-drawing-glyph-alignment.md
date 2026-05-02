# Bug: wg html dependency graph does not preserve terminal cell alignment

## Summary

The `wg html` dependency graph view visually breaks long-range edges because
Unicode box-drawing connector glyphs do not render on a stable monospace cell
grid in the browser. The underlying ASCII/Unicode graph layout depends on every
rendered character occupying exactly one terminal cell. In the HTML view, some
horizontal connector runs appear to have uneven lengths, which makes long-range
connections hard or impossible to follow.

## Affected view

- Live example: `https://ulivo.poietic.life/wg/feeds/workgraph-itself/`
- Generated page title: `Workgraph live mirror — Workgraph`
- Main graph element: `<pre class="viz-pre viz-substantive">`
- Connector markup pattern: each connector character is wrapped as an edge span,
  for example:

```html
<span class="edge" data-edges="parent>child">─</span>
<span class="edge" data-edges="parent>child">─</span>
<span class="edge" data-edges="parent>child">┐</span>
```

## Observed behavior

In the browser-rendered graph:

- horizontal edge segments made from `─` do not appear to advance by exactly the
  same width as task-label characters and spaces;
- long connector runs visually drift, so their endpoints no longer appear
  aligned with the vertical `│`, corner `┐`/`┘`, or return-arrow `←` glyphs;
- the problem is most visible on long cross-graph dependencies, where a line of
  `─` spans must land on a precise column many characters away;
- embedding the page in another site makes the problem easier to notice, but the
  root issue is in the generated WG HTML rendering itself, not in the embedding
  page.

## Expected behavior

The HTML graph should render with terminal-like cell invariants:

- every visible graph character, including spaces and all connector glyphs,
  occupies exactly one fixed-width cell;
- wrapping connector characters in spans must not alter layout metrics;
- task labels, decorators, timestamps, edge spans, and plain spaces must all
  share the same advance width grid;
- long-range connectors should line up the same way they do in `wg viz` in a
  terminal.

## Likely causes to investigate

This may be caused by one or more of:

- browser font fallback for box-drawing glyphs despite `font-family:
  'JetBrains Mono', ui-monospace, 'Cascadia Code', 'Source Code Pro', Menlo,
  Consolas, monospace`;
- a chosen webfont whose box-drawing glyphs have different visual or advance
  behavior than ASCII characters at the configured size;
- browser anti-aliasing/subpixel positioning making thin box-drawing glyphs look
  shorter or longer even if their advance width is technically fixed;
- per-character `<span class="edge">` wrappers interacting with font fallback,
  font weight, line-height, or inherited styles differently from the surrounding
  text;
- mixed glyph classes (`─`, `│`, `┐`, `┘`, `←`, `→`, `├`, `└`) coming from
  different fallback fonts.

## Suggested fixes

1. Add an HTML regression fixture with long cross-graph edges and compare the
   rendered column positions of connector glyph bounding boxes in a browser.
2. Force a known-good terminal font stack for `.viz-pre`, `.viz-pre *`, and
   `.viz-pre .edge`, and verify box-drawing glyph coverage in the first chosen
   font.
3. Consider bundling or recommending a terminal font with complete box-drawing
   coverage for the generated static HTML.
4. Disable typography features that can perturb terminal-cell rendering:

```css
.viz-pre,
.viz-pre * {
  font-variant-ligatures: none;
  font-feature-settings: "liga" 0, "calt" 0;
  font-kerning: none;
  font-synthesis: none;
}
```

5. If browser text rendering remains unreliable, consider rendering connector
   cells as a CSS grid of fixed-width cells, or render the graph in a canvas/SVG
   layer while preserving clickable task labels separately.

## Acceptance criteria

- A browser screenshot of the generated `wg html` page shows long connector runs
  lining up with the same columns as `wg viz`.
- Selecting a node still highlights upstream/downstream/cycle edges correctly.
- The fix works without requiring downstream iframe scaling hacks.
- A smoke or visual regression test protects at least one long-range connector
  case with many repeated `─` spans and a final corner/return glyph.

