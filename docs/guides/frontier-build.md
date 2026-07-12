# Frontier build environment

Frontier login/build nodes do not provide the Guix profile used by `env.sh`.
Use the tracked Frontier environment script instead:

```bash
source ./env.frontier.sh
cargo install --path . --force --locked --root /autofs/nccs-svm1_home1/erikgarrison/.cargo
```

The `boring-sys2` build runs bindgen and needs `libclang.so` discoverable.
On Frontier the resolved library directory is:

```bash
LIBCLANG_PATH=/opt/rocm-7.13.0/lib/llvm/lib
```

`env.frontier.sh` also prepends that directory to `LD_LIBRARY_PATH` and exports
`BINDGEN_EXTRA_CLANG_ARGS` with the local compiler include roots, for example:

```bash
BINDGEN_EXTRA_CLANG_ARGS="-I/usr/lib64/gcc/x86_64-suse-linux/7/include -I/usr/lib64/gcc/x86_64-suse-linux/7/include-fixed -I/usr/include"
```

This avoids the original bindgen failure where `boring-sys2` could not find a
loadable `libclang.so`; the compiler include roots avoid a follow-on
`stddef.h` lookup failure after libclang is found.
