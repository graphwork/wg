# WG developer convenience targets.
#
# The canonical build/install is `cargo install --path . --locked` (see
# CLAUDE.md "Development"); these targets cover the few multi-step chores.

.PHONY: embed-pi-plugin embed-pi-plugin-check

# Regenerate the committed, version-locked plugin bundle the wg binary embeds.
# Run this after editing anything under pi-plugin/src/** or bumping the
# WG_PI_PLUGIN_COMPAT_VERSION const. Requires node + npm (NOT needed for a plain
# `cargo install` — the bytes are committed).
embed-pi-plugin:
	scripts/embed-pi-plugin.sh

# Anti-drift gate (used by CI): re-embed, then fail if the committed bundle
# differs from a fresh build. A source edit without a re-embed is caught here.
embed-pi-plugin-check: embed-pi-plugin
	git diff --exit-code pi-plugin/embedded pi-plugin/src/version.ts
