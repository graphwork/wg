# Genuine FLIP v2 seed — 2026-09-05

This minimal seed exercises the installed completion controller built from the
post-`88e79dc9` source baseline
`88e79dc94d8c89ab70d3c7407d36b47a013b8ea1`.

## Installed-code identity

The installed CLI image at `/home/bot/.cargo/bin/wg` had SHA-256
`d6d416c870e5ae7714b518cc56a7025b9359985fdc9eb5f58c0393b26674c806`.
The live daemon was PID `2203209`; `/proc/2203209/exe` and the installed CLI
path were the same file, inode `4723454`. Thus the CLI submitting this seed and
the live daemon controlling its completion were byte-identical installed code.

## Why this seed is independent

A report cannot include the CID of its own completion receipt without changing
its bytes, and changing those bytes would change the receipt it asks the
controller to bind. This deliberately small report breaks that causal
self-reference: its real installed `wg done` completion can independently
produce a receipt-version-2 genuine two-phase FLIP Pass followed by the exact
Eval Pass. A later canary may cite that immutable receipt; this seed does not
require itself to predict or embed it.
