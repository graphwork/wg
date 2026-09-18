#!/usr/bin/env node
'use strict';
// @worksgood/cli bin shim — resolves the prebuilt platform binary and execs it.
// Zero install scripts anywhere; see scripts/npm/README.md.
require('../lib/resolver.js').run('worksgood');
