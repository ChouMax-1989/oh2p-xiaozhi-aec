#!/bin/sh
set -eu
base=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
docker build --target runtime -t oh2p-xiaozhi-public:local "$base"
