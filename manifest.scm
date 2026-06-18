;;; Guix development environment for building workgraph from this checkout.
;;;
;;; Usage from this directory:
;;;   ./env.sh cargo build --release --locked
;;;   ./env.sh cargo install --path . --locked
;;;
;;; Or enter a shell:
;;;   ./env.sh

(use-modules (guix profiles)
             (gnu packages bash)
             (gnu packages base)
             (gnu packages certs)
             (gnu packages cmake)
             (gnu packages commencement)
             (gnu packages compression)
             (gnu packages glib)
             (gnu packages llvm)
             (gnu packages nss)
             (gnu packages perl)
             (gnu packages pkg-config)
             (gnu packages rust)
             (gnu packages sqlite)
             (gnu packages tls)
             (gnu packages version-control))

(packages->manifest
 (list bash-minimal
       coreutils
       findutils
       git
       gnu-make
       nss-certs
       sed
       rust
       gcc-toolchain
       pkg-config
       cmake-minimal
       clang
       dbus
       openssl
       sqlite
       zlib
       zstd
       bzip2
       xz
       perl))
