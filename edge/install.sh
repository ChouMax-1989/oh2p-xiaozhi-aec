#!/bin/sh
# SPDX-License-Identifier: MIT
# Installs locally on an already prepared OH2P. Does not connect to the NAS.
set -eu
umask 077
die() { echo "$*" >&2; exit 1; }
usage() { echo "Usage: sh install.sh ws://NAS_HOST:PORT [--alsa-config /etc/asound.conf] [--configure-alsa]"; }
[ "$#" -ge 1 ] || { usage; exit 2; }
URL=$1; shift
case "$URL" in ws://?*) ;; *) die 'Expected a ws:// WebSocket URL for the trusted LAN NAS';; esac
case "$URL" in *' '*|*'
'*|*''*) die 'URL must be a single whitespace-free value';; esac
ALSA=/etc/asound.conf
MODE=check
while [ "$#" -gt 0 ]; do
    case "$1" in
        --alsa-config) [ "$#" -ge 2 ] || die 'Missing ALSA config path'; ALSA=$2; shift 2;;
        --configure-alsa) MODE=configure; shift;;
        *) usage; die "Unknown option: $1";;
    esac
done
[ "$(id -u)" = 0 ] || die 'Run as root on the speaker'
case "$ALSA" in /*) ;; *) die 'ALSA config path must be absolute';; esac
[ -f "$ALSA" ] && [ ! -L "$ALSA" ] || die 'ALSA config must be an existing regular file, not a symlink'
for command in awk sha256sum readlink flock cmp tar cp mv chmod; do command -v "$command" >/dev/null || die "Missing command: $command"; done
[ -f /usr/lib/libvpm.so ] && [ -x /usr/bin/arecord ] || die 'Expected the OH2P firmware VPM library and native arecord'
EXPECTED_VPM=f5d76a34dc40eb1b7c9045614da636fe5558344c3334a44e152d22fbfaed56d9
[ "$(sha256sum /usr/lib/libvpm.so | awk '{print $1}')" = "$EXPECTED_VPM" ] || die 'VPM differs from the validated OH2P 1.62.2 ABI; inspect compatibility first'
[ -f /etc/rc.local ] && grep -q '/data/init.sh' /etc/rc.local || die 'Existing /data/init.sh boot hook was not found in /etc/rc.local'
PACKAGE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
WORK=/data/open-xiaoai
[ ! -L "$WORK" ] && [ ! -L /data/init.sh ] || die 'Refusing symlinked install targets'
for exe in /proc/[0-9]*/exe; do
    target=$(readlink "$exe" 2>/dev/null || true)
    case "$target" in "$WORK/client"|"$WORK/aec/vpm-owner"|"$WORK/aec/bin/arecord") die 'Existing project processes are active; stop the old supervisor before installation';; esac
done
(cd "$PACKAGE/prebuilt" && sha256sum -c SHA256SUMS)
STAMP=$(date +%Y%m%d-%H%M%S)-$$
BACKUP=/data/oh2p-xiaozhi-backup-$STAMP
STAGE=/data/oh2p-xiaozhi-stage-$STAMP
[ ! -e "$BACKUP" ] && [ ! -e "$STAGE" ] || die 'Backup/stage collision'
mkdir -m 700 "$BACKUP" "$STAGE"
awk -v mode="$MODE" -f "$PACKAGE/tools/alsa-routes.awk" "$ALSA" >"$STAGE/asound.conf"
awk -v mode=check -f "$PACKAGE/tools/alsa-routes.awk" "$STAGE/asound.conf" >/dev/null
cp -p "$ALSA" "$BACKUP/asound.conf"
printf '%s\n' "$ALSA" >"$BACKUP/alsa-path"
if [ -e /data/init.sh ]; then cp -p /data/init.sh "$BACKUP/init.sh"; else touch "$BACKUP/init.absent"; fi
if [ -d "$WORK" ]; then touch "$BACKUP/work.existed"; fi
mkdir -p "$STAGE/work/aec/bin"
cp "$PACKAGE/prebuilt/client" "$STAGE/work/client"
cp "$PACKAGE/prebuilt/vpm-owner" "$STAGE/work/aec/vpm-owner"
cp "$PACKAGE/prebuilt/arecord" "$STAGE/work/aec/bin/arecord"
cp "$PACKAGE/boot-aec.sh" "$STAGE/work/boot.sh"
cp "$PACKAGE/rollback.sh" "$STAGE/work/rollback.sh"
printf '%s\n' "$URL" >"$STAGE/work/server.txt"
printf '%s\n' "$BACKUP" >"$STAGE/work/backup-path"
touch "$STAGE/work/enabled"
(cd "$STAGE/work/aec" && sha256sum vpm-owner bin/arecord >SHA256SUMS)
chmod 755 "$STAGE/work/client" "$STAGE/work/aec/vpm-owner" "$STAGE/work/aec/bin/arecord" "$STAGE/work/boot.sh" "$STAGE/work/rollback.sh"
sha256sum "$STAGE/work/boot.sh" | awk '{print $1}' >"$BACKUP/installed-init.sha256"
sha256sum "$STAGE/asound.conf" | awk '{print $1}' >"$BACKUP/installed-alsa.sha256"
committed=0
restore_on_error() {
    result=$?
    if [ "$result" -ne 0 ] && [ "$committed" = 1 ]; then
        cp -p "$BACKUP/asound.conf" "$ALSA" || true
        if [ -f "$BACKUP/init.sh" ]; then cp -p "$BACKUP/init.sh" /data/init.sh; else rm -f /data/init.sh; fi
        if [ -d "$WORK" ]; then mv "$WORK" "$BACKUP/failed-install"; fi
        if [ -d "$BACKUP/open-xiaoai" ]; then mv "$BACKUP/open-xiaoai" "$WORK"; fi
    fi
    exit "$result"
}
trap restore_on_error EXIT
# All checks and copies finish before any active configuration is replaced.
if [ -d "$WORK" ]; then mv "$WORK" "$BACKUP/open-xiaoai"; fi
committed=1
mv "$STAGE/work" "$WORK"
ALSA_CHANGED=0
if ! cmp -s "$ALSA" "$STAGE/asound.conf"; then cat "$STAGE/asound.conf" >"$ALSA"; ALSA_CHANGED=1; fi
cp "$WORK/boot.sh" /data/init.sh
chmod 755 /data/init.sh
trap - EXIT
echo "Installed; no processes started. Backup: $BACKUP"
if [ "$ALSA_CHANGED" = 1 ]; then
    echo 'ALSA routing changed: reboot the speaker before starting. Existing vendor capture must reopen on hw:0,0.'
else
    echo 'Start when ready: sh /data/init.sh </dev/null >/dev/null 2>&1 &'
fi
echo 'Rollback: sh /data/open-xiaoai/rollback.sh'
