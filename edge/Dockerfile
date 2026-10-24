FROM rust:1.93.0-bookworm
ARG CARGO_ZIGBUILD_VERSION=0.22.0
RUN apt-get update && apt-get install -y --no-install-recommends python3-venv binutils file patch ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && python3 -m venv /opt/zig \
    && /opt/zig/bin/pip install --no-cache-dir ziglang==0.16.0 \
    && cargo install cargo-zigbuild --version "$CARGO_ZIGBUILD_VERSION" --locked \
    && rustup target add armv7-unknown-linux-gnueabihf
ENV PATH="/opt/zig/lib/python3.11/site-packages/ziglang:${PATH}"
WORKDIR /work
ENTRYPOINT ["sh", "/work/tools/build-in-container.sh"]
