;;; Guix development environment for building WG from this checkout.
;;;
;;; Usage:
;;;   guix shell -m manifest.scm
;;;   cargo build --release --locked --bins

(use-modules (guix profiles)
             (gnu packages base)
             (gnu packages commencement)
             (gnu packages cmake)
             (gnu packages compression)
             (gnu packages llvm)
             (gnu packages pkg-config)
             (gnu packages tls))

(packages->manifest
 (list gcc-toolchain
       gnu-make
       pkg-config
       openssl
       zlib
       zstd
       clang
       cmake-minimal))
