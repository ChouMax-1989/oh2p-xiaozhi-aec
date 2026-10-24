#!/bin/sh
set -eu
WORK=/data/open-xiaoai
[ -f "$WORK/enabled" ] || exit 0
cd "$WORK/aec"
sha256sum -c SHA256SUMS >/dev/null || exit 1
[ "$(sha256sum "$WORK/client" | cut -d ' ' -f 1)" = 7c19a777bf4bbadb305dd44beb29b344708a35a70923f803ba896542ba5fea20 ] || exit 1
mkdir -p /tmp/open-xiaoai
chmod 700 /tmp/open-xiaoai
exec 9>/tmp/open-xiaoai/aec-supervisor.lock
flock -n 9 || exit 0
# Refuse to start a second custom consumer. A routing change takes effect after
# the firmware capture has reopened (normally on the next speaker boot).
[ -r /proc/asound/card0/pcm3c/sub0/status ] && grep -q '^closed' /proc/asound/card0/pcm3c/sub0/status || exit 1
owner_pid=
client_pid=
cleanup() {
    if [ -n "$client_pid" ]; then
        for status in /proc/[0-9]*/status; do
            parent=$(awk '/^PPid:/{print $2}' "$status" 2>/dev/null || true)
            if [ "$parent" = "$client_pid" ]; then
                child=${status#/proc/}; child=${child%/status}
                kill "$child" 2>/dev/null || true
            fi
        done
        kill "$client_pid" 2>/dev/null || true
    fi
    [ -z "$owner_pid" ] || kill "$owner_pid" 2>/dev/null || true
    [ -z "$client_pid" ] || wait "$client_pid" 2>/dev/null || true
    [ -z "$owner_pid" ] || wait "$owner_pid" 2>/dev/null || true
}
trap 'cleanup' EXIT
trap 'exit 0' INT TERM HUP
# This mode requests the already implemented continuous VPM stream; no Sherpa is run.
"$WORK/aec/vpm-owner" --wake-mode sherpa </dev/null >/dev/null 2>/tmp/open-xiaoai/aec-owner.log &
owner_pid=$!
attempt=0
while [ ! -S /tmp/family-voice-vpm-audio.sock ]; do
    kill -0 "$owner_pid" 2>/dev/null || exit 1
    attempt=$((attempt + 1))
    [ "$attempt" -lt 30 ] || exit 1
    sleep 1
done
PATH="$WORK/aec/bin:/usr/bin:/bin:/usr/sbin:/sbin" "$WORK/client" "$(cat "$WORK/server.txt")" </dev/null >/dev/null 2>&1 &
client_pid=$!
printf '%s %s %s\n' "$$" "$owner_pid" "$client_pid" >/tmp/open-xiaoai/aec-pids
while kill -0 "$owner_pid" 2>/dev/null && kill -0 "$client_pid" 2>/dev/null; do sleep 1; done
exit 1
