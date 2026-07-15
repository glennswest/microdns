#!/usr/bin/env bash
#
# Reproducibly cross-build the MicroDNS ARM binaries (static musl) and produce
# .deb + .rpm packages and binary tarballs into ./dist.
#
# Requires: rustup + cross + rootless podman + nfpm on the build host.
#   dnf install -y nfpm ; cargo install cross ; rustup toolchain install stable
#
# Why isolated target dirs (CARGO_TARGET_DIR per arch):
#   cross uses a different container image per target, and those images ship
#   DIFFERENT glibc versions. Cargo compiles build scripts for the *host* into a
#   shared `target/release/build`, so if two arches share one target dir, the
#   build scripts compiled under one image's glibc fail to run under the other's
#   ("GLIBC_2.xx not found"). Giving each arch its own target dir removes the
#   sharing entirely, so the build is correct regardless of order or reruns.
#
set -euo pipefail

cd "$(dirname "$0")/.."
export CROSS_CONTAINER_ENGINE=podman
export PATH="$HOME/.cargo/bin:$PATH"

VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')
echo ">> MicroDNS v${VERSION} — ARM package build"
mkdir -p dist

# rust-triple  nfpm-arch  label
build_one() {
    local triple=$1 pkgarch=$2 label=$3
    local tdir="target-${label}"
    echo ">> [${label}] cross build (${triple})"
    CARGO_TARGET_DIR="${tdir}" cross build --release --target "${triple}"

    local bin="${tdir}/${triple}/release/microdns"
    file "${bin}"

    local cfg="/tmp/nfpm-${label}.yaml"
    sed -e "s|\${PKG_ARCH}|${pkgarch}|g" \
        -e "s|\${PKG_VERSION}|${VERSION}|g" \
        -e "s|\${BIN}|${bin}|g" \
        deploy/packaging/nfpm.yaml > "${cfg}"

    echo ">> [${label}] nfpm deb + rpm"
    nfpm package -f "${cfg}" -p deb -t dist/
    nfpm package -f "${cfg}" -p rpm -t dist/

    echo ">> [${label}] tarball"
    tar -C "${tdir}/${triple}/release" -czf "dist/microdns-${VERSION}-linux-${label}.tar.gz" microdns
}

build_one aarch64-unknown-linux-musl     arm64 arm64
build_one armv7-unknown-linux-musleabihf arm7  armv7

echo ">> done. dist/:"
ls -lh dist/
