#!/bin/sh
set -eu
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
case "${1:-}" in ''|--client) ;; *) echo "Usage: sh edge/build.sh [--client]" >&2; exit 2;; esac
docker build -t oh2p-xiaozhi-edge-builder "$ROOT"
docker run --rm --cpus=2 --memory=2g -v "$ROOT:/work" oh2p-xiaozhi-edge-builder "${1:-}"
