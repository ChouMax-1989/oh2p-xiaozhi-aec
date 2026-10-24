#!/bin/sh
set -eu
cd /work
mkdir -p build/out
export CARGO_TARGET_DIR=/work/build/target
rustc --version
cargo --version
zig version
cargo test --locked --offline --manifest-path aec/reader/Cargo.toml
cargo test --locked --offline --manifest-path aec/preserved/vpm-owner/Cargo.toml
cargo zigbuild --locked --offline --release --manifest-path aec/preserved/vpm-owner/Cargo.toml --target armv7-unknown-linux-gnueabihf.2.25
cargo zigbuild --locked --offline --release --manifest-path aec/reader/Cargo.toml --target armv7-unknown-linux-gnueabihf.2.25
cp build/target/armv7-unknown-linux-gnueabihf/release/vpm-owner build/out/vpm-owner
cp build/target/armv7-unknown-linux-gnueabihf/release/fvsh-reader build/out/arecord
if [ "${1:-}" = --client ]; then
    sh tools/fetch-client-source.sh
    patch --forward -p1 -d build/client-source <tools/client-fvsh-s16.patch
    cargo zigbuild --locked --release --manifest-path build/client-source/packages/client-rust/Cargo.toml --bin client --target armv7-unknown-linux-gnueabihf.2.25
    cp build/target/armv7-unknown-linux-gnueabihf/release/client build/out/client-candidate
fi
for binary in build/out/vpm-owner build/out/arecord build/out/client-candidate; do
    [ -f "$binary" ] || continue
    file "$binary"
    readelf -l "$binary" | grep interpreter
    maximum=$(readelf --version-info "$binary" | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1)
    [ -n "$maximum" ] || { echo "Cannot establish GLIBC requirements" >&2; exit 1; }
    [ "$(printf '%s\n' GLIBC_2.25 "$maximum" | sort -Vu | tail -1)" = GLIBC_2.25 ] || { echo "GLIBC requirement exceeds 2.25" >&2; exit 1; }
done
(cd build/out && sha256sum arecord vpm-owner >SHA256SUMS)
echo 'Build completed. Rebuilt files are candidates; prebuilt files are unchanged.'
