#!/bin/sh
# SPDX-License-Identifier: MIT
# Exercise the packaged scripts against a disposable speaker filesystem.
set -eu
EDGE=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TMP_BASE=$(CDPATH= cd -- "${TMPDIR:-/tmp}" && pwd -P)
TEST_ROOT=$(mktemp -d "$TMP_BASE/oh2p-install-test.XXXXXX")
case "$TEST_ROOT" in "$TMP_BASE"/oh2p-install-test.*) ;; *) exit 1;; esac
active_pid=
cleanup() {
    if [ -n "$active_pid" ]; then
        kill "$active_pid" 2>/dev/null || true
        wait "$active_pid" 2>/dev/null || true
    fi
    rm -rf "$TEST_ROOT"
}
trap cleanup EXIT
mkdir -p "$TEST_ROOT/data" "$TEST_ROOT/etc" "$TEST_ROOT/usr/lib" "$TEST_ROOT/usr/bin" \
    "$TEST_ROOT/package" "$TEST_ROOT/shims"
export TEST_ROOT
WORK="$TEST_ROOT/data/open-xiaoai"
PACKAGE="$TEST_ROOT/package"
ALSA="$TEST_ROOT/etc/custom-asound.conf"
cp -R "$EDGE/prebuilt" "$EDGE/tools" "$PACKAGE/"
cp "$EDGE/boot-aec.sh" "$PACKAGE/boot-aec.sh"
for script in install.sh rollback.sh; do
    # Redirect absolute filesystem paths only. On Linux, /proc and flock are real.
    sed -e "s|/data/|$TEST_ROOT/data/|g" \
        -e "s|/etc/asound.conf|$TEST_ROOT/etc/asound.conf|g" \
        -e "s|/etc/rc.local|$TEST_ROOT/etc/rc.local|g" \
        -e "s|/usr/lib/libvpm.so|$TEST_ROOT/usr/lib/libvpm.so|g" \
        -e "s|/usr/bin/arecord|$TEST_ROOT/usr/bin/arecord|g" \
        -e "s|/tmp/open-xiaoai|$TEST_ROOT/run|g" \
        "$EDGE/$script" >"$PACKAGE/$script"
done

printf '/bin/sh %s/data/init.sh\n' "$TEST_ROOT" >"$TEST_ROOT/etc/rc.local"
printf 'echo Initial vendor boot script\n' >"$TEST_ROOT/data/init.sh"
cp "$TEST_ROOT/data/init.sh" "$TEST_ROOT/vendor-init.sh"
touch "$TEST_ROOT/usr/lib/libvpm.so"
printf '#!/bin/sh\nexit 0\n' >"$TEST_ROOT/usr/bin/arecord"
chmod +x "$TEST_ROOT/usr/bin/arecord"
cp "$EDGE/tests/alsa.fixture.conf" "$ALSA"

REAL_SHA256SUM=$(command -v sha256sum)
export REAL_SHA256SUM
cat >"$TEST_ROOT/shims/id" <<'EOF'
#!/bin/sh
echo 0
EOF
cat >"$TEST_ROOT/shims/sha256sum" <<'EOF'
#!/bin/sh
if [ "$1" = "$TEST_ROOT/usr/lib/libvpm.so" ]; then
    echo 'f5d76a34dc40eb1b7c9045614da636fe5558344c3334a44e152d22fbfaed56d9  libvpm.so'
else
    exec "$REAL_SHA256SUM" "$@"
fi
EOF
real_process_checks=0
if [ "$(uname -s)" = Linux ] && command -v flock >/dev/null 2>&1; then
    real_process_checks=1
else
    printf '#!/bin/sh\nexit 0\n' >"$TEST_ROOT/shims/flock"
    echo 'Skipping real flock and /proc checks on this host.'
fi
chmod +x "$TEST_ROOT"/shims/*
PATH="$TEST_ROOT/shims:$PATH"
export PATH

reject_upgrade() {
    if sh "$PACKAGE/install.sh" --upgrade >"$TEST_ROOT/rejected" 2>&1; then
        echo 'Unsafe upgrade was accepted' >&2
        exit 1
    fi
    if ! grep -q "$1" "$TEST_ROOT/rejected"; then
        cat "$TEST_ROOT/rejected" >&2
        exit 1
    fi
}
verify_installed_binaries() {
    cmp "$PACKAGE/prebuilt/client" "$WORK/client"
    cmp "$PACKAGE/prebuilt/vpm-owner" "$WORK/aec/vpm-owner"
    cmp "$PACKAGE/prebuilt/arecord" "$WORK/aec/bin/arecord"
    cmp "$PACKAGE/prebuilt/aplay" "$WORK/aec/bin/aplay"
    if [ "$real_process_checks" = 1 ]; then
        [ -x "$WORK/aec/bin/aplay" ]
    fi
    (cd "$WORK/aec" && sha256sum -c SHA256SUMS)
}

sh "$PACKAGE/install.sh" ws://NAS_HOST:4399 --alsa-config "$ALSA" --configure-alsa
verify_installed_binaries
first_backup=$(cat "$WORK/backup-path")
cmp "$TEST_ROOT/vendor-init.sh" "$first_backup/init.sh"
cmp "$EDGE/tests/alsa.fixture.conf" "$first_backup/asound.conf"
awk -v mode=check -f "$EDGE/tools/alsa-routes.awk" "$ALSA" >/dev/null
cmp "$WORK/boot.sh" "$TEST_ROOT/data/init.sh"

# Model the old release: there is no private aplay, and the audio owner is older.
rm -f "$WORK/aec/bin/aplay"
printf '%s\n' 'Legacy owner' >"$WORK/aec/vpm-owner"
(cd "$WORK/aec" && sha256sum vpm-owner bin/arecord >SHA256SUMS)
printf '%s' 'ws://CUSTOM_NAS:54321' >"$WORK/server.txt"
printf '\n# Custom ALSA setting without a final newline' >>"$ALSA"
cp -p "$WORK/server.txt" "$TEST_ROOT/original-server"
cp -p "$ALSA" "$TEST_ROOT/original-alsa"
cat >"$WORK/boot.sh" <<'EOF'
#!/bin/sh
set -eu
exec 9>"$TEST_ROOT/run/aec-supervisor.lock"
flock -n 9
trap 'rm -f "$TEST_ROOT/run/aec-pids"; exit 0' TERM
printf '%s\n' "$$" >"$TEST_ROOT/run/aec-pids"
while :; do sleep 1; done
EOF
cp "$WORK/boot.sh" "$TEST_ROOT/data/init.sh"

# Reject an untracked modification to the boot hook and an incomplete package.
echo '# Local boot hook change' >>"$TEST_ROOT/data/init.sh"
reject_upgrade 'Boot hook differs'
cp "$WORK/boot.sh" "$TEST_ROOT/data/init.sh"
cp "$PACKAGE/prebuilt/SHA256SUMS" "$TEST_ROOT/original-sums"
sed '/  aplay$/d' "$TEST_ROOT/original-sums" >"$PACKAGE/prebuilt/SHA256SUMS"
reject_upgrade 'Expected four distinct binary checksums'
cp "$TEST_ROOT/original-sums" "$PACKAGE/prebuilt/SHA256SUMS"
[ ! -e "$WORK/aec/bin/aplay" ]
[ "$(cat "$WORK/backup-path")" = "$first_backup" ]

if [ "$real_process_checks" = 1 ]; then
    # The normal upgrade must refuse an active private process with no supervisor.
    cp "$(command -v sleep)" "$WORK/aec/bin/aplay"
    # Multicall coreutils selects sleep by argv[0], while /proc/exe records the copied path.
    bash -c 'exec -a sleep "$1" 60' bash "$WORK/aec/bin/aplay" &
    active_pid=$!
    attempts=0
    while [ "$(readlink "/proc/$active_pid/exe" 2>/dev/null || true)" != "$WORK/aec/bin/aplay" ]; do
        attempts=$((attempts + 1))
        [ "$attempts" -lt 10 ] || exit 1
        sleep 1
    done
    reject_upgrade 'Existing project processes are active'
    kill -0 "$active_pid"
    kill "$active_pid"
    wait "$active_pid" 2>/dev/null || true
    active_pid=
    rm -f "$WORK/aec/bin/aplay"

    sh "$TEST_ROOT/data/init.sh" &
    active_pid=$!
    attempts=0
    while [ ! -s "$TEST_ROOT/run/aec-pids" ]; do
        attempts=$((attempts + 1))
        [ "$attempts" -lt 10 ] || exit 1
        sleep 1
    done
fi

sh "$PACKAGE/install.sh" --upgrade
if [ -n "$active_pid" ]; then
    wait "$active_pid"
    active_pid=
fi
verify_installed_binaries
cmp "$WORK/server.txt" "$TEST_ROOT/original-server"
cmp "$ALSA" "$TEST_ROOT/original-alsa"
cmp "$WORK/boot.sh" "$TEST_ROOT/data/init.sh"
upgrade_backup=$(cat "$WORK/backup-path")
[ "$upgrade_backup" != "$first_backup" ]
cmp "$upgrade_backup/open-xiaoai/boot.sh" "$upgrade_backup/init.sh"
[ ! -e "$upgrade_backup/open-xiaoai/aec/bin/aplay" ]

# Run a copy outside the installation so Windows can rename the directory.
cp "$WORK/rollback.sh" "$TEST_ROOT/rollback.sh"
sh "$TEST_ROOT/rollback.sh"
grep -q 'Legacy owner' "$WORK/aec/vpm-owner"
[ ! -e "$WORK/aec/bin/aplay" ]
[ "$(cat "$WORK/backup-path")" = "$first_backup" ]
cmp "$WORK/server.txt" "$TEST_ROOT/original-server"
cmp "$ALSA" "$TEST_ROOT/original-alsa"
cmp "$WORK/boot.sh" "$TEST_ROOT/data/init.sh"
echo 'Speaker installs, upgrades and rolls back in the temporary fixture.'
