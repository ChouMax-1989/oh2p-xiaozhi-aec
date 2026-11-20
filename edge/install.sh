#!/bin/sh
# SPDX-License-Identifier: MIT
# Installs locally on an already prepared OH2P. Does not connect to the NAS.
set -eu
umask 077
die() { echo "$*" >&2; exit 1; }
usage() {
    echo 'New install: sh install.sh ws://NAS_HOST:PORT [--alsa-config /etc/asound.conf] [--configure-alsa]'
    echo 'Upgrade: sh install.sh --upgrade'
}
[ "$#" -ge 1 ] || { usage; exit 2; }
ALSA=/etc/asound.conf
MODE=check
if [ "$1" = --upgrade ]; then
    [ "$#" -eq 1 ] || { usage; exit 2; }
    MODE=upgrade
else
    URL=$1
fi
shift
while [ "$#" -gt 0 ]; do
    case "$1" in
        --alsa-config) [ "$#" -ge 2 ] || die 'Missing ALSA config path'; ALSA=$2; shift 2;;
        --configure-alsa) MODE=configure; shift;;
        *) usage; die "Unknown option: $1";;
    esac
done
[ "$(id -u)" = 0 ] || die 'Run as root on the speaker'
for command in awk sha256sum readlink flock cmp cp mv chmod tr; do command -v "$command" >/dev/null || die "Missing command: $command"; done
[ -f /usr/lib/libvpm.so ] && [ -x /usr/bin/arecord ] || die 'Expected the OH2P firmware VPM library and native arecord'
EXPECTED_VPM=f5d76a34dc40eb1b7c9045614da636fe5558344c3334a44e152d22fbfaed56d9
[ "$(sha256sum /usr/lib/libvpm.so | awk '{print $1}')" = "$EXPECTED_VPM" ] || die 'VPM differs from the validated OH2P 1.62.2 ABI; inspect compatibility first'
[ -f /etc/rc.local ] && grep -q '/data/init.sh' /etc/rc.local || die 'Existing /data/init.sh boot hook was not found in /etc/rc.local'
PACKAGE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
WORK=/data/open-xiaoai
RUNTIME=/tmp/open-xiaoai
[ ! -L "$WORK" ] && [ ! -L /data/init.sh ] && [ ! -L "$RUNTIME" ] || die 'Refusing symlinked install targets'
[ ! -e "$WORK" ] || [ -d "$WORK" ] || die 'Install target must be a directory'
[ ! -e /data/init.sh ] || [ -f /data/init.sh ] || die 'Boot hook must be a regular file'

if [ "$MODE" = upgrade ]; then
    for file in boot.sh server.txt backup-path; do
        [ -f "$WORK/$file" ] && [ ! -L "$WORK/$file" ] || die "Previous installation is missing a regular $file"
    done
    cmp -s /data/init.sh "$WORK/boot.sh" || die 'Boot hook differs from the existing installation; reconcile changes before upgrading'
    PREVIOUS_BACKUP=$(cat "$WORK/backup-path")
    case "$PREVIOUS_BACKUP" in
        /data/oh2p-xiaozhi-backup-*/*) die 'Unexpected previous backup path';;
        /data/oh2p-xiaozhi-backup-?*) ;;
        *) die 'Unexpected previous backup path';;
    esac
    [ -d "$PREVIOUS_BACKUP" ] && [ ! -L "$PREVIOUS_BACKUP" ] || die 'Previous installation backup is missing'
    [ -f "$PREVIOUS_BACKUP/alsa-path" ] && [ ! -L "$PREVIOUS_BACKUP/alsa-path" ] || die 'Previous ALSA path is missing'
    ALSA=$(cat "$PREVIOUS_BACKUP/alsa-path")
    URL=$(cat "$WORK/server.txt")
fi
case "$URL" in ws://?*) ;; *) die 'Expected a ws:// WebSocket URL for the trusted LAN NAS';; esac
case "$URL" in *[[:space:]]*) die 'URL must be a single whitespace-free value';; esac
case "$ALSA" in /*) ;; *) die 'ALSA config path must be absolute';; esac
[ -f "$ALSA" ] && [ ! -L "$ALSA" ] || die 'ALSA config must be an existing regular file, not a symlink'
for file in SHA256SUMS aplay arecord client vpm-owner; do
    [ -f "$PACKAGE/prebuilt/$file" ] && [ ! -L "$PACKAGE/prebuilt/$file" ] || die "Missing or symlinked package file: $file"
done
awk '
    NF != 2 || length($1) != 64 || $1 !~ /^[a-f0-9]+$/ ||
    $2 !~ /^(aplay|arecord|client|vpm-owner)$/ || seen[$2]++ { invalid=1 }
    END { exit(invalid || NR != 4) }
' "$PACKAGE/prebuilt/SHA256SUMS" || die 'Expected four distinct binary checksums'
(cd "$PACKAGE/prebuilt" && sha256sum -c SHA256SUMS)

verify_stopped() {
    for exe in /proc/[0-9]*/exe; do
        target=$(readlink "$exe" 2>/dev/null || true)
        case "${target%" (deleted)"}" in
            "$WORK/client"|"$WORK/aec/vpm-owner"|"$WORK/aec/bin/arecord"|"$WORK/aec/bin/aplay")
                die 'Existing project processes are active; no binaries were replaced';;
        esac
    done
}
mkdir -p "$RUNTIME"
chmod 700 "$RUNTIME"
[ ! -L "$RUNTIME/aec-supervisor.lock" ] || die 'Refusing symlinked supervisor lock'
exec 9>"$RUNTIME/aec-supervisor.lock"
if [ "$MODE" != upgrade ]; then
    flock -n 9 || die 'Existing project processes are active; use --upgrade for an existing installation'
    verify_stopped
fi

STAMP=$(date +%Y%m%d-%H%M%S)-$$
BACKUP=/data/oh2p-xiaozhi-backup-$STAMP
STAGE=/data/oh2p-xiaozhi-stage-$STAMP
[ ! -e "$BACKUP" ] && [ ! -e "$STAGE" ] || die 'Backup/stage collision'
mkdir -m 700 "$BACKUP" "$STAGE"
if [ "$MODE" = configure ]; then
    awk -v mode=configure -f "$PACKAGE/tools/alsa-routes.awk" "$ALSA" >"$STAGE/asound.conf"
else
    cp -p "$ALSA" "$STAGE/asound.conf"
fi
awk -v mode=check -f "$PACKAGE/tools/alsa-routes.awk" "$STAGE/asound.conf" >/dev/null
cp -p "$ALSA" "$BACKUP/asound.conf"
printf '%s\n' "$ALSA" >"$BACKUP/alsa-path"
if [ -e /data/init.sh ]; then cp -p /data/init.sh "$BACKUP/init.sh"; else touch "$BACKUP/init.absent"; fi
if [ -d "$WORK" ]; then touch "$BACKUP/work.existed"; fi
mkdir -p "$STAGE/work/aec/bin"
cp "$PACKAGE/prebuilt/client" "$STAGE/work/client"
cp "$PACKAGE/prebuilt/vpm-owner" "$STAGE/work/aec/vpm-owner"
cp "$PACKAGE/prebuilt/arecord" "$STAGE/work/aec/bin/arecord"
cp "$PACKAGE/prebuilt/aplay" "$STAGE/work/aec/bin/aplay"
cp "$PACKAGE/boot-aec.sh" "$STAGE/work/boot.sh"
cp "$PACKAGE/rollback.sh" "$STAGE/work/rollback.sh"
if [ "$MODE" = upgrade ]; then
    cp -p "$WORK/server.txt" "$STAGE/work/server.txt"
else
    printf '%s\n' "$URL" >"$STAGE/work/server.txt"
fi
printf '%s\n' "$BACKUP" >"$STAGE/work/backup-path"
if [ "$MODE" != upgrade ] || [ -f "$WORK/enabled" ]; then
    touch "$STAGE/work/enabled"
fi
(cd "$STAGE/work/aec" && sha256sum vpm-owner bin/arecord bin/aplay >SHA256SUMS)
chmod 755 "$STAGE/work/client" "$STAGE/work/aec/vpm-owner" "$STAGE/work/aec/bin/arecord" "$STAGE/work/aec/bin/aplay" "$STAGE/work/boot.sh" "$STAGE/work/rollback.sh"
cp -p "$STAGE/work/boot.sh" "$STAGE/init.sh"
sha256sum "$STAGE/work/boot.sh" | awk '{print $1}' >"$BACKUP/installed-init.sha256"
sha256sum "$STAGE/asound.conf" | awk '{print $1}' >"$BACKUP/installed-alsa.sha256"

if [ "$MODE" = upgrade ]; then
    if ! flock -n 9; then
        [ -f "$RUNTIME/aec-pids" ] && [ ! -L "$RUNTIME/aec-pids" ] || die 'Cannot identify the running supervisor; no binaries were replaced'
        supervisor=$(awk 'NR==1 {print $1; exit}' "$RUNTIME/aec-pids")
        case "$supervisor" in ''|*[!0-9]*) die 'Invalid supervisor PID';; esac
        [ "$supervisor" -gt 1 ] || die 'Invalid supervisor PID'
        supervisor_command=$(tr '\000' ' ' 2>/dev/null <"/proc/$supervisor/cmdline" || true)
        case "$supervisor_command" in
            'sh /data/init.sh '|'/bin/sh /data/init.sh '|'sh /data/open-xiaoai/boot.sh '|'/bin/sh /data/open-xiaoai/boot.sh ') ;;
            *) die 'Cannot verify the running supervisor; no binaries were replaced';;
        esac
        [ "$(readlink "/proc/$supervisor/fd/9" 2>/dev/null || true)" = "$RUNTIME/aec-supervisor.lock" ] ||
            die 'Supervisor does not hold the expected lock; no binaries were replaced'
        kill -TERM "$supervisor" 2>/dev/null || die 'Could not stop the supervisor; no binaries were replaced'
        attempt=0
        until flock -n 9; do
            attempt=$((attempt + 1))
            [ "$attempt" -lt 15 ] || die 'Supervisor did not stop; no binaries were replaced'
            sleep 1
        done
    fi
    verify_stopped
    [ "$(cat "$WORK/backup-path")" = "$PREVIOUS_BACKUP" ] &&
        cmp -s /data/init.sh "$BACKUP/init.sh" && cmp -s "$WORK/server.txt" "$STAGE/work/server.txt" &&
        cmp -s "$ALSA" "$BACKUP/asound.conf" || die 'Configuration changed during the upgrade; no binaries were replaced'
fi

ALSA_CHANGED=0
restore_on_error() {
    result=$?
    if [ "$result" -ne 0 ]; then
        if [ "$ALSA_CHANGED" = 1 ]; then cp -p "$BACKUP/asound.conf" "$ALSA" || true; fi
        if [ ! -f "$STAGE/init.sh" ]; then
            if [ -f "$BACKUP/init.sh" ]; then cp -p "$BACKUP/init.sh" /data/init.sh; else rm -f /data/init.sh; fi
        fi
        if [ ! -d "$STAGE/work" ] && [ -d "$WORK" ]; then mv "$WORK" "$BACKUP/failed-install"; fi
        if [ -d "$BACKUP/open-xiaoai" ]; then mv "$BACKUP/open-xiaoai" "$WORK"; fi
    fi
    exit "$result"
}
trap restore_on_error EXIT
trap 'exit 1' INT TERM HUP
# All checks and copies finish before any active configuration is replaced.
if [ -d "$WORK" ]; then mv "$WORK" "$BACKUP/open-xiaoai"; fi
mv "$STAGE/work" "$WORK"
if [ "$MODE" = configure ] && ! cmp -s "$ALSA" "$STAGE/asound.conf"; then
    ALSA_CHANGED=1
    cat "$STAGE/asound.conf" >"$ALSA"
fi
mv "$STAGE/init.sh" /data/init.sh
trap - EXIT INT TERM HUP
rm -f "$STAGE/asound.conf"
rmdir "$STAGE"
echo "Installed; no processes started. Backup: $BACKUP"
if [ ! -f "$WORK/enabled" ]; then
    echo 'The previous installation was disabled; it remains disabled.'
elif [ "$ALSA_CHANGED" = 1 ]; then
    echo 'ALSA routing changed: reboot the speaker before starting. Existing vendor capture must reopen on hw:0,0.'
else
    echo 'Reboot the speaker to start, or run:'
    echo '(trap "" HUP; exec /bin/sh /data/init.sh) </dev/null >/dev/null 2>&1 &'
fi
echo 'Rollback: sh /data/open-xiaoai/rollback.sh'
