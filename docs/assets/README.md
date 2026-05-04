# Assets

`wg-tui.gif` — animated capture of `wg tui` showing the task graph, agents,
claims, logs, and dependency view in motion. Referenced from the project
README opening as the visual front door.

To record:

```bash
# in a real workgraph project with active work
asciinema rec wg-tui.cast -c "wg tui"
agg wg-tui.cast wg-tui.gif        # asciinema-agg, or convert via gifski
```

Until the real capture lands, the README still references `wg-tui.gif` so
GitHub's renderer shows the alt text gracefully.
