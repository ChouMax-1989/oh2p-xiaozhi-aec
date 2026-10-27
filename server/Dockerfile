# syntax=docker/dockerfile:1
# Public toolchains only; no private image or deployed filesystem is required.
FROM ghcr.io/astral-sh/uv:0.7.22 AS uv
FROM rust:1.93.0-bookworm AS rust
FROM python:3.12.12-slim-bookworm AS builder
COPY --from=uv /uv /usr/local/bin/uv
COPY --from=rust /usr/local/cargo /usr/local/cargo
COPY --from=rust /usr/local/rustup /usr/local/rustup
ENV PATH=/usr/local/cargo/bin:$PATH RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo CARGO_BUILD_JOBS=2
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl libopus-dev portaudio19-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# Both archive content and the four patched source files are verified.
RUN curl -fL --retry 3 https://codeload.github.com/idootop/open-xiaoai/zip/bc3396c64e2a435f354eb5cb12a203981f1fe422 -o /tmp/upstream.zip \
    && echo '1d3d38eb3e099faef31bf8ae13929d89c8cdda64e9fa2eade649ca36c667c88c  /tmp/upstream.zip' | sha256sum -c - \
    && python -m zipfile -e /tmp/upstream.zip /src \
    && mv /src/open-xiaoai-bc3396c64e2a435f354eb5cb12a203981f1fe422 /src/upstream \
    && rm /tmp/upstream.zip
COPY prepare_source.py /overlay/prepare_source.py
COPY patches /overlay/patches
COPY app /overlay/app
RUN python /overlay/prepare_source.py /src/upstream
RUN cargo test --locked --manifest-path /src/upstream/packages/client-rust/Cargo.toml pending_order_tests -- --nocapture
WORKDIR /src/upstream/examples/xiaozhi
# Keep upstream lockfiles intact, including PyAudio; compile its required headers.
RUN uv sync --frozen --no-dev --no-install-project \
    && .venv/bin/maturin build --release --locked --out /tmp/wheels \
    && uv pip install --python .venv/bin/python --no-deps /tmp/wheels/*.whl
# Models are public upstream release assets, not extracted from a running image.
RUN curl -fL --retry 3 https://github.com/idootop/open-xiaoai/releases/download/vad-kws-models/models.zip -o /tmp/models.zip \
    && echo '97272c44184a0f8222726a164f35e0a504177240181753ae13230790c29eb566  /tmp/models.zip' | sha256sum -c - \
    && python -m zipfile -e /tmp/models.zip xiaozhi \
    && rm /tmp/models.zip

FROM python:3.12.12-slim-bookworm AS runtime
LABEL org.opencontainers.image.source="https://github.com/idootop/open-xiaoai" \
      org.opencontainers.image.revision="bc3396c64e2a435f354eb5cb12a203981f1fe422+oh2p-realtime-cues"
RUN apt-get update && apt-get install -y --no-install-recommends libopus0 libportaudio2 libgomp1 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /src/upstream/examples/xiaozhi/.venv /app/.venv
COPY --from=builder /src/upstream/examples/xiaozhi/main.py /app/main.py
COPY --from=builder /src/upstream/examples/xiaozhi/xiaozhi /app/xiaozhi
COPY config.example.py /app/config.py
ENV CLI=true PYTHONDONTWRITEBYTECODE=1 PYTHONUNBUFFERED=1 \
    PATH=/app/.venv/bin:$PATH \
    LD_LIBRARY_PATH=/usr/local/lib:/app/.venv/lib/python3.12/site-packages/onnxruntime/capi \
    OH2P_REALTIME=1 OH2P_AEC_READY=1 OH2P_CANCEL_READY=1 OH2P_QUIET_CONNECT=1
# Sherpa may dlopen the unversioned C API name; use the locked ONNX wheel's library.
RUN python -c "import pathlib; p=pathlib.Path('/app/.venv/lib/python3.12/site-packages/onnxruntime/capi'); library=p/'libonnxruntime.so.1.23.2'; assert library.is_file(), library; pathlib.Path('/usr/local/lib/libonnxruntime.so').symlink_to(library)" \
    && python -c "import open_xiaoai_server as s, onnxruntime, sherpa_onnx; assert hasattr(s, 'playback_control')"
EXPOSE 4399
CMD ["/bin/sh", "-c", "python xiaozhi/services/audio/kws/keywords.py && exec python main.py"]

# No cloud, hardware, credentials or real speech: synthetic integration and loopback.
# Run: docker build --target test -t oh2p-xiaozhi-tests ./server
#      docker run --rm --network none oh2p-xiaozhi-tests
FROM runtime AS test
COPY . /release
ENV PYTHONPATH=/release/app:/app
WORKDIR /release/tests
CMD ["python", "-B", "-m", "unittest", "-v", "test_realtime", "test_release", "test_wire", "test_overlay", "test_extension", "test_wakeup_pending"]
