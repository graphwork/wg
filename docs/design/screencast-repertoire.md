# Screencast Repertoire: Terminal-Native Demo Showcase

*A collection of screencasts demonstrating whimsical/beautiful CLI projects built entirely with wg.*

**Produced by:** design-screencast-repertoire  
**Date:** 2026-03-24  
**Status:** Design complete, ready for implementation  
**Related:** [demo-medley-catalog.md](demo-medley-catalog.md) (capability demos), [screencast-interaction-flow.md](screencast-interaction-flow.md) (hero interaction design), [screencast-freshness-pipeline.md](screencast-freshness-pipeline.md) (auto-regeneration)

---

## Motivation

The existing screencasts (haiku, heist, pancakes) and the demo medley catalog focus on demonstrating **wg features** — chat decomposition, validation gates, cycles, etc. These are compelling for users who already know they want a task coordination tool.

But for first impressions — README hero GIFs, social media, conference talks — we need screencasts that answer a different question: *"Why would I want to build things in the terminal?"*

This repertoire flips the emphasis. Each screencast features a **visually striking terminal project** as the main attraction, with wg orchestration visible but secondary. The viewer thinks "that's cool, I want to make that" first, and "oh, it was built with wg agents" second.

### Dual Goals

1. **Show the output:** A beautiful/whimsical CLI program that gives a "cool" vibe
2. **Show the process:** wg agents spawning, coordinating, and building it in parallel

---

## CLI Toy Research

Existing Unix CLI toys that can be hacked, remixed, or used as building blocks:

| Tool | What It Does | Visual Appeal | Hackability | Install |
|------|-------------|---------------|-------------|---------|
| **nyancat** | Animated rainbow cat flying across terminal | ★★★★★ | Medium (C source) | `apt install nyancat` |
| **lolcat** | Pipes text through rainbow gradient coloring | ★★★★ | High (just a pipe) | `gem install lolcat` or `snap install lolcat` |
| **figlet** | Renders text as large ASCII art banners | ★★★★ | High (font library) | `apt install figlet` |
| **toilet** | Like figlet but with color and Unicode effects | ★★★★★ | High (filter modes) | `apt install toilet` |
| **cowsay** | ASCII cow (or other animal) with speech bubble | ★★★ | High (custom cowfiles) | `apt install cowsay` |
| **cmatrix** | "The Matrix" falling green characters | ★★★★★ | Low (standalone) | `apt install cmatrix` |
| **pipes.sh** | Animated colored pipes drawing across terminal | ★★★★ | Medium (bash script) | GitHub clone |
| **cbonsai** | Procedurally grown ASCII bonsai tree | ★★★★★ | Medium (C source) | `apt install cbonsai` |
| **asciiquarium** | Animated aquarium with fish | ★★★★ | Medium (Perl) | `apt install asciiquarium` |
| **sl** | Steam locomotive animation | ★★★ | Low (joke tool) | `apt install sl` |
| **fortune** | Random quotes/aphorisms | ★★ | High (custom databases) | `apt install fortune-mod` |
| **boxes** | Wraps text in decorative ASCII boxes | ★★★ | High (custom designs) | `apt install boxes` |
| **spark** | Draws sparkline bar charts from numbers | ★★★ | High (just a pipe) | GitHub/brew |
| **rich-cli** | Beautiful terminal rendering (tables, panels, markdown) | ★★★★★ | High (Python library) | `pip install rich-cli` |
| **glow** | Renders Markdown beautifully in terminal | ★★★★ | High (just a pipe) | `brew install glow` |
| **tty-clock** | Stylized terminal clock | ★★★ | Low | `apt install tty-clock` |
| **genact** | Fake activity generator (compiler, download bars) | ★★★ | Medium (Rust) | `cargo install genact` |
| **termsvg** | Records terminal as SVG animations | ★★★★ | N/A (meta-tool) | npm |

### Key Combinations for Mashups

- **figlet + lolcat** = Rainbow ASCII art banners (trivial to pipe)
- **fortune + cowsay + lolcat** = Colorful wisdom from talking animals
- **nyancat frame + custom text overlay** = Nyan cat delivering content
- **cbonsai + figlet** = Growing trees with text labels
- **rich-cli + any data** = Beautiful dashboards from raw data
- **cmatrix style + custom content** = Themed "falling text" displays

---

## Screencast Concepts

### Concept 1: Nyan Cat News Haiku Generator

**Tagline:** *"Nyan cat delivers the news — in haiku form."*

#### What Gets Built

A CLI tool (`haiku-news`) that:
1. Scrapes current news headlines from RSS feeds
2. Analyzes the mood/sentiment of each headline
3. Converts each headline into a 5-7-5 haiku
4. Displays the haiku scrolling behind an animated nyan cat, with lolcat rainbow coloring
5. Optionally: a "roast mode" that generates snarky haiku variants

The final output looks like nyan cat flying across the terminal, leaving a trail of rainbow-colored haiku about today's news instead of the usual rainbow trail.

#### wg Features Highlighted

- **Coordinator chat:** User types "Build a nyan cat news haiku generator" → coordinator creates the task graph
- **Fan-out:** Multiple agents work in parallel (scraper, syllable engine, mood analyzer, renderer)
- **Pipeline:** scrape → analyze → generate → render
- **Coordinator iteration:** "Add a roast mode" triggers a second wave of tasks
- **Live detail view:** Firehose tab shows agents writing haiku in real time

#### Script Outline

| # | Scene | What Happens | Time (compressed) |
|---|-------|-------------|------------------|
| 1 | Launch | `wg tui` — empty graph, chat visible | 3s |
| 2 | Prompt | User types request, coordinator decomposes into 6-8 tasks | 8s |
| 3 | Build | Agents work in parallel — scraper, syllable engine, haiku generator. Graph fills with activity. | 12s |
| 4 | Inspect | Switch to Firehose tab — watch agents generating haiku in real time | 8s |
| 5 | Roast mode | "Add roast mode" → 3 new tasks appear, agents start immediately | 6s |
| 6 | Showcase | Exit TUI, run `./haiku-news` — nyan cat + rainbow haiku scroll across terminal | 8s |

**Total: ~45s compressed**

#### Visual Appeal

- **The payoff shot:** Full-width terminal showing nyan cat animation with rainbow-gradient haiku trailing behind it
- **Colors:** Full 256-color rainbow gradient (lolcat), nyan cat's signature pop-tart body and rainbow trail
- **Animation:** Smooth horizontal scroll of nyan cat + text
- **Layout:** 80×24 terminal, nyan cat top-center, haiku lines appearing below/behind with rainbow fade
- **Screenshot-friendly:** Even a still frame captures the rainbow gradient + ASCII cat + haiku text

#### Implementation Notes

- **Feasibility: HIGH.** The "haiku news" scenario already exists in the recording infrastructure. The visual output layer (nyan cat + lolcat) is the new part and can be built as a Python/bash script that:
  1. Generates haiku (reuse existing logic)
  2. Pipes through `figlet` for large text, then through `lolcat` for color
  3. Renders nyan cat frames alongside (ASCII animation loop from nyancat source or custom)
- **Risk:** Nyan cat animation needs to be synced with haiku scroll. Fallback: static nyan cat ASCII art frame + scrolling haiku below it.
- **Effort: 2-3 days** (mostly the renderer script + recording/compression)

---

### Concept 2: Terminal Art Gallery

**Tagline:** *"Agents curate an ASCII art exhibition."*

#### What Gets Built

A CLI art gallery (`ascii-gallery`) where AI agents each create a different piece of ASCII art, and a curator agent selects and arranges them into a tiled gallery display. The final output is a museum-style terminal layout with framed art pieces, artist attribution, and title cards.

Art pieces are generated using combinations of:
- `figlet` / `toilet` with different fonts (banner, slant, shadow, smblock, etc.)
- `cowsay` with exotic animals (-f dragon, -f stegosaurus, -f vader)
- `boxes` with decorative borders
- Custom ASCII art generated by LLM agents
- Color applied via `lolcat`, `toilet --filter gay`, ANSI escape codes

#### wg Features Highlighted

- **Scatter-gather:** 4-5 parallel "artist" agents, each creating a different piece
- **Synthesis:** A "curator" agent evaluates and arranges the pieces
- **Validation gates:** `--verify "art piece fits within 40x15 characters"` on each artist task
- **Agency system:** Different artist agents have different "styles" (tradeoffs: bold vs. minimal, colorful vs. monochrome)

#### Script Outline

| # | Scene | What Happens | Time (compressed) |
|---|-------|-------------|------------------|
| 1 | Launch | `wg tui` — user asks coordinator for "an ASCII art gallery" | 8s |
| 2 | Artists spawn | 4 parallel artist tasks appear, agents claim them | 6s |
| 3 | Creation | Watch artists work — Firehose shows them experimenting with fonts/layouts | 10s |
| 4 | Curation | Curator task activates after all artists finish, selects and arranges | 6s |
| 5 | Showcase | Exit TUI, run `./ascii-gallery` — tiled display of framed art pieces | 10s |

**Total: ~40s compressed**

#### Visual Appeal

- **The payoff shot:** 4-6 framed ASCII art pieces in a grid layout, each in a decorative `boxes` border, with title cards below each
- **Colors:** Each piece has its own color scheme (one rainbow, one green-on-black, one bold white, one amber)
- **Layout:** 120×40 terminal, 2×2 or 2×3 grid of art pieces, museum-style spacing
- **Variety:** Mix of figlet text art, cowsay characters, geometric patterns, and freeform ASCII drawings
- **Screenshot-friendly:** Excellent — the static grid layout is inherently photogenic

#### Example Gallery Output

```
┌──────────────────────┐  ┌──────────────────────┐
│  ____  _   _ ____ ___│  │        ^__^           │
│ |  _ \| | | / ___|_  │  │        (oo)\_______   │
│ | |_) | | | \___ \ | │  │        (__)\       )\ │
│ |  _ <| |_| |___) || │  │            ||----w |  │
│ |_| \_\\___/|____/ | │  │            ||     ||  │
│  "Rust" — figlet/slant│  │  "Moo" — cowsay      │
└──────────────────────┘  └──────────────────────┘
┌──────────────────────┐  ┌──────────────────────┐
│   ╔══╗ ╔══╗ ╔══╗    │  │  ░▒▓█ TERMINAL █▓▒░  │
│   ║▓▓║ ║░░║ ║▒▒║    │  │  ░▒▓█  POWER   █▓▒░  │
│   ╚══╝ ╚══╝ ╚══╝    │  │  ░▒▓█  HOUR    █▓▒░  │
│  "Blocks" — custom   │  │  "Glow" — toilet/gay  │
└──────────────────────┘  └──────────────────────┘
```

#### Implementation Notes

- **Feasibility: HIGH.** Each "artist" agent runs simple commands (figlet + cowsay + boxes) and writes output to a file. The gallery renderer is a Python/bash script that reads the files and tiles them.
- **Risk:** Art pieces must fit consistent dimensions. Enforce with `--verify` validation.
- **Effort: 2-3 days**

---

### Concept 3: The Matrix Wisdom Wall

**Tagline:** *"cmatrix meets fortune — wisdom rains from the sky."*

#### What Gets Built

A `cmatrix`-inspired terminal animation (`wisdom-rain`) where instead of random characters falling, **meaningful text** rains down — fortune cookie wisdom, programming quotes, poetry fragments, news headlines. Different columns carry different content themes, color-coded by category (green for code wisdom, cyan for poetry, yellow for news, magenta for jokes).

#### wg Features Highlighted

- **Fan-out:** Parallel agents scrape different content sources (quotes API, poetry API, news RSS, joke API)
- **Pipeline:** Source → filter/format → feed into renderer
- **Cycles:** A `refresh-content` cycle task that periodically fetches new content and feeds it into the running display
- **Coordinator chat:** "Make it more philosophical" → coordinator adds a philosophy quote source

#### Script Outline

| # | Scene | What Happens | Time (compressed) |
|---|-------|-------------|------------------|
| 1 | Launch | `wg tui` — user asks for "a matrix-style wisdom wall" | 6s |
| 2 | Sources | 4 parallel content-source tasks spawn (quotes, poetry, news, jokes) | 6s |
| 3 | Build | Agents fetch content and build the renderer. Watch progress in Firehose. | 10s |
| 4 | Iterate | "Make it more philosophical" → new source task added | 5s |
| 5 | Showcase | Exit TUI, run `./wisdom-rain` — themed text rains down the terminal in multiple colors | 10s |

**Total: ~37s compressed**

#### Visual Appeal

- **The payoff shot:** Full terminal filled with falling text streams in multiple colors, like cmatrix but with readable words/phrases
- **Colors:** Column-coded — green (code quotes), cyan (poetry), yellow (news), magenta (jokes), white (philosophy)
- **Animation:** Continuous downward scroll at varying speeds per column, characters fading from bright to dim as they age
- **Layout:** Full terminal width, each column 15-20 chars wide, staggered start positions
- **Screenshot-friendly:** Moderate — animation is the main appeal, but a single frame still shows the colored columns with text fragments

#### Example Output (single frame)

```
  rse is     │ Shall I │   BREAKING:   │ Why do
 worse th    │ compare │   Fed holds   │ program
an a dise    │ thee to │   rates ste   │mers pre
ase — Dij    │ a summe │   ady amid    │fer dark
kstra       │r's day? │   inflation   │ mode?
             │ Thou ar │   concerns    │ Because
 Simplici    │t more l │              │ light a
ty is pre    │ovely an │   Tech lead   │ttracts
requisite    │d more t │   ers call    │ bugs.
 for reli    │emperate │   for open    │
ability      │         │   source      │
```

#### Implementation Notes

- **Feasibility: MEDIUM.** The content sourcing is easy (curl + jq or Python requests). The renderer is the challenge — need to write a custom terminal animation (Python with `curses` or a small Rust program). Could adapt cmatrix source code.
- **Risk:** Custom renderer is the bottleneck. Fallback: use `cmatrix` with a custom character set (cmatrix accepts `-u` for update delay but not custom content). More practical: Python curses script (50-100 lines).
- **Effort: 3-4 days** (renderer is the big piece)

---

### Concept 4: Bonsai Haiku Garden

**Tagline:** *"Grow a zen garden in your terminal."*

#### What Gets Built

A terminal zen garden (`zen-garden`) that combines procedurally grown ASCII bonsai trees (via `cbonsai`) with haiku poetry generated for each tree. The final display shows 3-4 bonsai trees growing side by side, each with a haiku inscription beneath it, surrounded by a minimalist rock garden pattern. Trees grow in real-time animation.

#### wg Features Highlighted

- **Diamond pattern:** 3 parallel "grow tree" tasks → 1 "compose garden" synthesis task
- **Pipeline per tree:** grow → describe → write haiku (3-step pipeline, parallelized across trees)
- **Agency:** Different "gardener" agents with different aesthetic tradeoffs (wild vs. manicured, sparse vs. dense)
- **`--verify`:** "Haiku must be valid 5-7-5 syllable structure"

#### Script Outline

| # | Scene | What Happens | Time (compressed) |
|---|-------|-------------|------------------|
| 1 | Launch | `wg tui` — user asks for "a zen garden with bonsai and haiku" | 6s |
| 2 | Garden plan | Coordinator creates tree tasks (3 parallel) + haiku tasks + compose task | 5s |
| 3 | Growing | Agents grow trees and write haiku. Firehose shows haiku drafts. | 10s |
| 4 | Compose | Garden composer arranges trees + haiku into final layout | 5s |
| 5 | Showcase | Exit TUI, run `./zen-garden` — animated bonsai trees growing with haiku | 12s |

**Total: ~38s compressed**

#### Visual Appeal

- **The payoff shot:** 3 bonsai trees at different growth stages, each with a haiku below, surrounded by a minimalist sand/rock pattern (dots and tildes)
- **Colors:** Muted earth tones — green for leaves, brown/yellow for trunks, white for haiku text, dim grey for rocks/sand. Restrained palette gives a calm, zen feeling.
- **Animation:** Trees grow from seed to full form (cbonsai's native animation), haiku fades in beneath each tree once it finishes growing
- **Layout:** 100×35 terminal, three trees evenly spaced, haiku centered below each
- **Screenshot-friendly:** Excellent — the static final frame is inherently beautiful and zen

#### Example Final Frame

```
                    ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░
          &&&               ###                    @@@
         &&&&&             #####                  @@@@@
        &&&&&&&           ### ###                @@@@@@@
          |||               |||                    |||
          |||               |||                    |||
    ~~~~~~|||~~~~~~~~~  ~~~~|||~~~~~~~~~~~~   ~~~~~|||~~~~~~~~~~
  
     morning dew falls      wind through pines     old pond — yes,
     the branch remembers   each needle hums a     a frog leaps in:
     last autumn's weight    different note         water's sound
  
    ○  .  ○    .    ○    .    ○  .    ○    .    ○    .  ○   .  ○
```

#### Implementation Notes

- **Feasibility: HIGH.** `cbonsai` already generates beautiful trees. Haiku generation reuses existing infrastructure. The garden compositor is a Python script that arranges pre-generated tree outputs + haiku text.
- **cbonsai features:** `-S` seed for reproducibility, `-t` for tree type, `-l` for leaf character, `-p` to print (no animation) for static layout. Use `-p` for the gallery frame, animated mode for the showcase.
- **Risk:** Low. All components are well-understood and exist.
- **Effort: 1-2 days** (simplest concept — leverages existing tools heavily)

---

### Concept 5: Fortune Cookie Factory

**Tagline:** *"Mass-produce wisdom, one cookie at a time."*

#### What Gets Built

A CLI fortune cookie factory (`cookie-factory`) where agents operate different "stations" on a production line:
1. **Quote sourcing:** Fetch fortunes from multiple databases (fortune-mod, custom, API)
2. **Lucky numbers:** Generate personalized lucky number sets
3. **Cookie art:** Generate unique ASCII cookie art for each fortune (cracked cookie, whole cookie, fancy cookie)
4. **Assembly:** Combine fortune + numbers + art into formatted cookies
5. **Display:** Render a batch of 4-6 fortune cookies using cowsay with different animals, piped through lolcat

The visual output is a colorful grid of cowsay animals each delivering a different fortune cookie, with lucky numbers and ASCII cookie art.

#### wg Features Highlighted

- **Pipeline:** source → format → illustrate → assemble → display (5-stage pipeline)
- **Fan-out at assembly:** Each cookie assembled in parallel
- **Coordinator chat:** Natural language request, coordinator creates the factory
- **Validation:** `--verify "fortune fits in 60 chars, haiku syllable count correct"`

#### Script Outline

| # | Scene | What Happens | Time (compressed) |
|---|-------|-------------|------------------|
| 1 | Launch | `wg tui` — user asks for "a fortune cookie factory" | 6s |
| 2 | Factory | Coordinator creates pipeline + parallel assembly tasks | 5s |
| 3 | Production | Agents work the production line. Firehose shows fortunes being crafted. | 8s |
| 4 | Showcase | Exit TUI, run `./cookie-factory` — grid of cowsay animals with fortunes, rainbow colored | 10s |

**Total: ~29s compressed**

#### Visual Appeal

- **The payoff shot:** 4-6 cowsay animals (cow, dragon, stegosaurus, tux, vader, sheep) each delivering a fortune, all rainbow-colored via lolcat
- **Colors:** Full rainbow gradient via lolcat — each animal in a different color band
- **Layout:** 120×40 terminal, 2×3 grid of cowsay outputs, each in a box
- **Animation:** Fortunes "print" sequentially (typewriter effect), then lolcat gradient sweeps across
- **Screenshot-friendly:** Very high — cowsay + rainbow is inherently shareable/memeable

#### Example Output (one cookie)

```
 ________________________________________
/ The best code is no code at all.       \
| Your lucky numbers: 7, 42, 256, 1337  |
|                                        |
|        ,───.                           |
|       ( 🥠  )  ← ASCII cookie         |
|        `───'                           |
\ Today's mood: contemplative            /
 ----------------------------------------
        \   ^__^
         \  (oo)\_______
            (__)\       )\/\
                ||----w |
                ||     ||
```

#### Implementation Notes

- **Feasibility: HIGH.** All components are trivial pipes: `fortune | cowsay -f dragon | lolcat`. The "factory" aspect is adding variety (different animals, different fortune databases, lucky number generation).
- **Risk:** Very low. This is the safest concept — every component is a one-liner.
- **Effort: 1-2 days**

---

## Ranking and Recommendation

### Visual Impact vs. Implementation Effort

| # | Concept | Visual Impact | Effort | WG Features | Feasibility | **Overall Rank** |
|---|---------|:------------:|:------:|:-----------:|:-----------:|:----------------:|
| 1 | Nyan Cat News Haiku | ★★★★★ | 2-3 days | Fan-out, pipeline, iteration | High | **1st** |
| 4 | Bonsai Haiku Garden | ★★★★★ | 1-2 days | Diamond, agency, verify | High | **2nd** |
| 2 | Terminal Art Gallery | ★★★★ | 2-3 days | Scatter-gather, synthesis, verify | High | **3rd** |
| 5 | Fortune Cookie Factory | ★★★★ | 1-2 days | Pipeline, fan-out, chat | High | **4th** |
| 3 | Matrix Wisdom Wall | ★★★★★ | 3-4 days | Fan-out, cycles, iteration | Medium | **5th** |

### Recommended Production Order

**Phase 1 — Hero pair (ship first):**
1. **Nyan Cat News Haiku** — The marquee demo. Highest recognition factor (nyan cat is iconic), strongest "wow" moment, and directly extends existing haiku-news infrastructure.
2. **Bonsai Haiku Garden** — The zen counterpoint. Calm, beautiful, and very different in feel from the nyan cat energy. Fastest to implement. Together they show range.

**Phase 2 — Depth demos (ship second):**
3. **Terminal Art Gallery** — Demonstrates scatter-gather and agency (different artist styles). The gallery format is inherently screenshot-friendly for social media.
4. **Fortune Cookie Factory** — The crowd-pleaser. Everyone knows cowsay. Low risk, high shareability. Good "intro" screencast for people who find the others too complex.

**Phase 3 — Stretch goal:**
5. **Matrix Wisdom Wall** — Most impressive animation but highest implementation effort due to the custom renderer. Worth doing if the others land well and we want one more showpiece.

---

## Cross-Cutting Design Notes

### Terminal Size

All concepts should target **120×35** for the showcase (output display) scenes. The TUI scenes use the existing **65×38** harness. The recording script switches terminal size between TUI and showcase phases:
- Scenes 1-4 (TUI): 65×38 (matches existing harness)
- Scene 5+ (showcase): 120×35 (wider for visual output)

Alternatively, record TUI and showcase as separate `.cast` files and splice them.

### Recording Pattern

Each screencast follows the same structural template:

```
[TUI Phase]                          [Showcase Phase]
┌─────────────────────┐              ┌─────────────────────┐
│ 1. Launch wg tui    │              │ 5. Exit TUI         │
│ 2. Chat → tasks     │──── build ──▶│ 6. Run the thing    │
│ 3. Watch agents     │              │ 7. Marvel at output  │
│ 4. Inspect output   │              └─────────────────────┘
└─────────────────────┘
```

The TUI phase shows *how it was built*. The showcase phase shows *what was built*. The transition (exiting TUI → running the output) is the bridge between "coordination tool" and "cool thing you made."

### Existing Infrastructure Reuse

| Component | Reuse From | Modification Needed |
|-----------|-----------|-------------------|
| Recording harness | `record-harness.py` | Add terminal resize support |
| Time compression | `compress-cast.py` | Scene-specific compression params per concept |
| Demo setup | `setup-demo.sh` | Per-concept CLAUDE.md with task templates |
| Fallback injection | `record-showcase.py` | Per-concept fallback tasks and chat history |
| TUI scenes | `record-interaction.py` | Reuse scene 1-4 pattern with different prompts |

### Real vs. Simulated

For each concept:
- **TUI phase (scenes 1-4):** Uses real coordinator + real agents when possible, with fallback injection (same pattern as existing `record-interaction.py`)
- **Showcase phase (scenes 5+):** The output scripts (`haiku-news`, `ascii-gallery`, etc.) are pre-built. Agents "build" them during the recording, but the showcase runs the pre-built version for deterministic visual output.

This is the same hybrid approach used in the existing showcase screencast — agents do real work that produces real artifacts, but the final "wow" output is a curated version to ensure visual quality.

### CLI Tool Dependencies

Tools needed across all concepts:

```bash
# Core (used by most concepts)
apt install figlet toilet cowsay boxes fortune-mod
gem install lolcat  # or: snap install lolcat
pip install rich-cli

# Concept-specific
apt install cbonsai        # Bonsai Garden
apt install nyancat        # Nyan Cat (or custom renderer)
```

All tools are available in standard package managers and have permissive licenses.

---

## Appendix: Additional Concept Ideas (Backlog)

Ideas that didn't make the top 5 but could be developed later:

- **ASCII Aquarium Composer** — Agents design fish, plants, and bubbles; compositor builds a custom asciiquarium scene
- **Terminal Music Visualizer** — Agents build a spectrum analyzer that visualizes audio (using `aplay` + custom FFT → sparklines)
- **Conway's Game of Life Evolver** — Agents use genetic algorithms to evolve interesting GoL patterns; display runs the winners with rainbow coloring
- **Starfield Text Crawler** — "Star Wars" opening crawl with animated starfield; agents write the story, build the starfield, build the scroller
- **Pipes Symphony** — Multiple agents each generate a "voice" in a pipes.sh-style animation, composed into a synchronized multi-color pipe drawing
- **Weather Dashboard** — Agents fetch weather data from multiple APIs, build sparkline charts, compose into a beautiful terminal dashboard with ASCII weather icons

---

*End of design document. Artifact of task design-screencast-repertoire.*
