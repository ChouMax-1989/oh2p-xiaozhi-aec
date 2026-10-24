#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
REV=bc3396c64e2a435f354eb5cb12a203981f1fe422
EXPECTED=26f26b367746ce438721e23ca87f97f508431e93dfe04a0fc6db616e276d584e
mkdir -p build
if [ -e build/client-source ]; then
    echo 'build/client-source already exists; preserve or remove it explicitly before refetching' >&2
    exit 1
fi
curl --fail --location --proto '=https' --tlsv1.2 "https://codeload.github.com/idootop/open-xiaoai/tar.gz/$REV" -o build/client-source.tar.gz
printf '%s  %s\n' "$EXPECTED" build/client-source.tar.gz | sha256sum -c -
mkdir build/client-source
tar -xzf build/client-source.tar.gz -C build/client-source --strip-components=1
