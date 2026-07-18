# WG developer convenience targets.
#
# The canonical build/install is `cargo install --path . --locked` (see
# CLAUDE.md "Development"); these targets cover the few multi-step chores.

.PHONY: embed-worksgood-pi embed-worksgood-pi-check embed-pi-plugin embed-pi-plugin-check install-patched-pi

# Regenerate the committed, version-locked plugin bundle the wg binary embeds.
# Run this after editing anything under worksgood-pi/src/** or bumping the
# WG_PI_PLUGIN_COMPAT_VERSION const. Requires node + npm (NOT needed for a plain
# `cargo install` — the bytes are committed).
embed-worksgood-pi:
	scripts/embed-worksgood-pi.sh

# Compatibility aliases: `wg pi-plugin` remains the operational CLI surface.
embed-pi-plugin: embed-worksgood-pi

# Anti-drift gate (used by CI): re-embed, then fail if the committed bundle
# differs from a fresh build. A source edit without a re-embed is caught here.
embed-worksgood-pi-check: embed-worksgood-pi
	git diff --exit-code worksgood-pi/embedded worksgood-pi/src/version.ts

embed-pi-plugin-check: embed-worksgood-pi-check

# Build and install the pinned Pi 0.80.6 source with WG's maintained EPIPE
# patch. This is an explicit source-package install, never an in-place edit of
# an arbitrary global node_modules tree.
install-patched-pi:
	scripts/install-patched-pi.sh
