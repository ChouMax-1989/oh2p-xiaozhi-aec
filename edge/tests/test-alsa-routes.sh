#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
TEMP=$(mktemp -d)
trap 'rm -f "$TEMP/configured" "$TEMP/expected" "$TEMP/second" "$TEMP/unknown"; rmdir "$TEMP"' EXIT
if awk -v mode=check -f tools/alsa-routes.awk tests/alsa.fixture.conf >"$TEMP/configured" 2>/dev/null; then echo 'Unexpected acceptance of native capture on device 3' >&2; exit 1; fi
awk -v mode=configure -f tools/alsa-routes.awk tests/alsa.fixture.conf >"$TEMP/configured"
sed 's/device 3/device 0/' tests/alsa.fixture.conf >"$TEMP/expected"
cmp "$TEMP/configured" "$TEMP/expected"
awk -v mode=check -f tools/alsa-routes.awk "$TEMP/configured" >"$TEMP/second"
cmp "$TEMP/configured" "$TEMP/second"
sed 's/type dsnoop/type hw/' "$TEMP/configured" >"$TEMP/unknown"
if awk -v mode=configure -f tools/alsa-routes.awk "$TEMP/unknown" >/dev/null 2>&1; then echo 'Unexpected acceptance of unknown ALSA layout' >&2; exit 1; fi
echo 'ALSA route transformation, validation, preservation and unknown-layout rejection passed.'
