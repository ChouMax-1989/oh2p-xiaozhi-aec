#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
TEMP=$(mktemp -d)
trap 'rm -f "$TEMP/configured" "$TEMP/expected" "$TEMP/second" "$TEMP/unknown" "$TEMP/stock" "$TEMP/stock-fixed"; rmdir "$TEMP"' EXIT
if awk -v mode=check -f tools/alsa-routes.awk tests/alsa.fixture.conf >"$TEMP/configured" 2>/dev/null; then echo 'Unexpected acceptance of native capture on device 3' >&2; exit 1; fi
awk -v mode=configure -f tools/alsa-routes.awk tests/alsa.fixture.conf >"$TEMP/configured"
sed 's/device 3/device 0/' tests/alsa.fixture.conf >"$TEMP/expected"
cmp "$TEMP/configured" "$TEMP/expected"
awk -v mode=check -f tools/alsa-routes.awk "$TEMP/configured" >"$TEMP/second"
cmp "$TEMP/configured" "$TEMP/second"
# Exact original OTA form: mico_record and noop both reference Capture.
awk '
    /^pcm.mico_record [{]/ { print; print "    type plug"; print "    slave.pcm Capture"; print "}"; skip=1; next }
    skip && /^}/ { skip=0; next }
    !skip { print }
' tests/alsa.fixture.conf >"$TEMP/stock"
if awk -v mode=check -f tools/alsa-routes.awk "$TEMP/stock" >/dev/null 2>&1; then echo 'Unexpected acceptance of stock shared microphone route' >&2; exit 1; fi
awk -v mode=configure -f tools/alsa-routes.awk "$TEMP/stock" >"$TEMP/stock-fixed"
cmp "$TEMP/stock-fixed" "$TEMP/expected"
awk -v mode=check -f tools/alsa-routes.awk "$TEMP/stock-fixed" >/dev/null
sed 's/type dsnoop/type hw/' "$TEMP/configured" >"$TEMP/unknown"
if awk -v mode=configure -f tools/alsa-routes.awk "$TEMP/unknown" >/dev/null 2>&1; then echo 'Unexpected acceptance of unknown ALSA layout' >&2; exit 1; fi
echo 'ALSA route transformation, validation, preservation and unknown-layout rejection passed.'
