#!/bin/sh
# SPDX-License-Identifier: MIT
set -eu
umask 077
WORK=/data/open-xiaoai
die() { echo "$*" >&2; exit 1; }
[ "$(id -u)" = 0 ] || die 'Run as root on the speaker'
[ -r "$WORK/backup-path" ] || die 'No installation backup recorded'
BACKUP=$(cat "$WORK/backup-path")
case "$BACKUP" in /data/oh2p-xiaozhi-backup-*) ;; *) die 'Unexpected backup path';; esac
[ -d "$BACKUP" ] && [ ! -L "$BACKUP" ] || die 'Backup missing or symlinked'
DISABLED="$BACKUP/disabled-install"
[ ! -e "$DISABLED" ] || die 'Rollback has already been attempted; inspect backup'
ALSA=$(cat "$BACKUP/alsa-path")
[ -f "$ALSA" ] && [ ! -L "$ALSA" ] && [ ! -L /data/init.sh ] || die 'Configuration targets changed; inspect manually'
[ -f /data/init.sh ] && [ "$(sha256sum /data/init.sh | awk '{print $1}')" = "$(cat "$BACKUP/installed-init.sha256")" ] || die 'init.sh changed since installation; preserve and reconcile it manually'
[ "$(sha256sum "$ALSA" | awk '{print $1}')" = "$(cat "$BACKUP/installed-alsa.sha256")" ] || die 'ALSA config changed since installation; preserve and reconcile it manually'
# Supervisor holds this lock; verify its command before terminating it.
if [ -r /tmp/open-xiaoai/aec-pids ]; then
    supervisor=$(awk '{print $1}' /tmp/open-xiaoai/aec-pids)
    case "$supervisor" in ''|*[!0-9]*) die 'Invalid supervisor PID';; esac
    command=$(tr '\000' ' ' </proc/"$supervisor"/cmdline 2>/dev/null || true)
    case "$command" in *'sh /data/init.sh'*|*'sh /data/open-xiaoai/boot.sh'*) kill "$supervisor" 2>/dev/null || true;; '') ;; *) die 'Supervisor PID has been reused; inspect manually';; esac
fi
sleep 2
for exe in /proc/[0-9]*/exe; do
    target=$(readlink "$exe" 2>/dev/null || true)
    case "$target" in "$WORK/client"|"$WORK/aec/vpm-owner"|"$WORK/aec/bin/arecord") die 'Project processes still active; no configuration restored';; esac
done
mkdir -p /tmp/open-xiaoai
chmod 700 /tmp/open-xiaoai
exec 9>/tmp/open-xiaoai/aec-supervisor.lock
flock -n 9 || die 'Supervisor still owns its lock; no configuration restored'
cp -p "$BACKUP/asound.conf" "$ALSA"
if [ -f "$BACKUP/init.sh" ]; then cp -p "$BACKUP/init.sh" /data/init.sh; else rm -f /data/init.sh; fi
rm -f "$WORK/enabled"
mv "$WORK" "$DISABLED"
if [ -d "$BACKUP/open-xiaoai" ]; then mv "$BACKUP/open-xiaoai" "$WORK"; fi
echo "Previous files and ALSA restored. No previous process was started. Backup: $BACKUP"
