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

## Canary boundary

The installed-code identity above is the complete candidate evidence. The
completion result is intentionally not presented as an input claim: it can
exist only after the controller reviews these committed bytes.

Running the installed `wg done` against this independent candidate is the
canary. The controller's immutable graph receipts—not text predicted by this
report—must establish receipt-version-2 genuine two-phase FLIP Pass followed by
the exact Eval Pass. Requiring this report to embed that receipt's CID would be
causally self-referential: adding the CID would change the bytes to which the
receipt binds. A later canary may cite the immutable receipt after this seed
completes.
