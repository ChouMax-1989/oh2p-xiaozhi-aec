#!/usr/bin/env bash
# SPDX-License-Identifier: MIT
# Clean OFFLINE-DEVICE build: no speaker connection, partition read or flashing.
set -euo pipefail
usage() { echo 'Usage: sudo bash firmware/build.sh (--public-default | --authorized-key /path/to/id_rsa.pub) [--out /absolute/new/directory]'; }
MODE=; KEY=; OUT=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --public-default) [ -z "$MODE" ] || { usage; exit 2; }; MODE=public; shift;;
        --authorized-key) [ "$#" -ge 2 ] && [ -z "$MODE" ] || { usage; exit 2; }; MODE=key; KEY=$2; shift 2;;
        --out) [ "$#" -ge 2 ] || { usage; exit 2; }; OUT=$2; shift 2;;
        *) usage; exit 2;;
    esac
done
[ -n "$MODE" ] || { usage; exit 2; }
[ "$(id -u)" = 0 ] || { echo 'Run in a disposable Linux builder as root (device nodes must be preserved).' >&2; exit 1; }
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT=${OUT:-"$ROOT/firmware/out"}
case "$OUT" in /*) ;; *) echo 'Output path must be absolute' >&2; exit 2;; esac
[ ! -e "$OUT" ] || { echo 'Output already exists; preserve it or choose a fresh --out path.' >&2; exit 1; }
for command in curl python3 openssl patch tar make cc sha256sum; do command -v "$command" >/dev/null || { echo "Missing dependency: $command" >&2; exit 1; }; done
if [ "$MODE" = key ]; then
    KEY=$(realpath "$KEY")
    python3 "$ROOT/firmware/prepare.py" check-key "$KEY"
fi
mkdir -p "$OUT/downloads" "$OUT/release" "$OUT/work"
download() {
    curl --fail --location --proto '=https' --tlsv1.2 "$1" -o "$OUT/downloads/$2"
    printf '%s  %s\n' "$3" "$OUT/downloads/$2" | sha256sum -c -
}
download https://cdn.cnbj1.fds.api.mi-img.com/xiaoqiang/rom/oh2p/mico_all_616cd9d93_1.62.2.bin mico_all_616cd9d93_1.62.2.bin 08c6c0c5e7dfd3de59a7e37b4709ad90ab84e8025c799a642730e97ef53c0eba
download https://codeload.github.com/stevenjoezhang/xiaoai-agent/tar.gz/9fc56179fbdde610e95755a843615669dece0d08 xiaoai-agent.tar.gz a2134c76436e879ed9e0244037cba886150bb0128aa810ee840cc24f71f05394
download https://codeload.github.com/plougher/squashfs-tools/tar.gz/refs/tags/4.7.5 squashfs-tools.tar.gz 547b7b7f4d2e44bf91b6fc554664850c69563701deab9fd9cd7e21f694c88ea6
mkdir "$OUT/work/upstream" "$OUT/work/tools" "$OUT/work/official"
tar -xzf "$OUT/downloads/xiaoai-agent.tar.gz" -C "$OUT/work/upstream" --strip-components=1
tar -xzf "$OUT/downloads/squashfs-tools.tar.gz" -C "$OUT/work/tools" --strip-components=1
python3 "$OUT/work/upstream/deploy/client-patch/src/extract.py" -e "$OUT/downloads/mico_all_616cd9d93_1.62.2.bin" -d "$OUT/work/official/extracted"
make -C "$OUT/work/tools/squashfs-tools" -j2
TOOLS="$OUT/work/tools/squashfs-tools"
"$TOOLS/unsquashfs" -d "$OUT/work/root" "$OUT/work/official/extracted/root.squashfs"
# Never use a device dump as an input. Copy clean matching official components.
cp "$OUT/work/official/extracted/boot.img.encrypt" "$OUT/release/boot.img.encrypt"
cp "$OUT/work/official/extracted/dtb.img.encrypt" "$OUT/release/dtb.img.encrypt"
cp "$OUT/work/official/extracted/root.squashfs" "$OUT/release/root-original.squashfs"
python3 "$ROOT/firmware/prepare.py" patch "$OUT/work/root" "$OUT/work/upstream/deploy/client-patch/patches" "$MODE" "$KEY"
"$TOOLS/mksquashfs" "$OUT/work/root" "$OUT/release/root-ssh-candidate.squashfs" \
    -noappend -comp xz -b 131072 -no-tailends -no-xattrs -all-root -exports \
    -p 'dev/console c 600 0 0 5 1'
[ "$(stat -c %s "$OUT/release/root-ssh-candidate.squashfs")" -lt 41943040 ] || { echo 'Candidate exceeds OH2P system partition limit' >&2; exit 1; }
"$TOOLS/unsquashfs" -d "$OUT/work/roundtrip" "$OUT/release/root-ssh-candidate.squashfs"
python3 "$ROOT/firmware/prepare.py" verify "$OUT/work/roundtrip" "$MODE"
python3 "$ROOT/firmware/prepare.py" manifest "$OUT/release" "$MODE"
(cd "$OUT/release" && sha256sum boot.img.encrypt dtb.img.encrypt root-original.squashfs root-ssh-candidate.squashfs firmware-manifest.json >SHA256SUMS)
echo "Prepared candidate at $OUT/release. No device was flashed or tested."
if [ "$MODE" = public ]; then echo 'PUBLIC DEFAULT: root password is open-xiaoai. It is shared and has no forced first-login rotation.'; fi
echo 'This is a set of partition inputs, not a signed full-device OTA or USB Burning Tool image.'
