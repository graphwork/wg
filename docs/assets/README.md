# Assets

README hero and console captures.

- `pi-fleet.gif` — animated capture of the `pi-worksgood` `/wg-fleet` cockpit
  inside a live Pi session: the counts header (in-progress/ready/blocked/done),
  the dependency tree, and the per-task detail pane (status, live agent
  activity, bounded stream tail). Referenced from the project README opening as
  the primary visual — pi sessions are the primary interface.
- `wg-tui.gif` — animated capture of `wg tui` showing the task graph, agents,
  claims, logs, and dependency view in motion. Referenced from the README's
  secondary TUI console section.

To record (`pi-fleet.gif`, from a real WG project with active work):

```bash
# enable the cockpit first:
#   {"fleetView": true} in ~/.pi/agent/extensions/pi-worksgood/config.json
asciinema rec pi-fleet.cast -c "pi"   # then run /wg-fleet in the session
agg pi-fleet.cast pi-fleet.gif         # asciinema-agg, or convert via gifski
```

To record (`wg-tui.gif`):

```bash
# in a real WG project with active work
asciinema rec wg-tui.cast -c "wg tui"
agg wg-tui.cast wg-tui.gif          # asciinema-agg, or convert via gifski
```

Until the real captures land, the README references both filenames so GitHub's
renderer shows the alt text gracefully.
