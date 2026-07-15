# B-10: multi-node devnet image. Replaces the old boilerplate that just
# volume-mounted the repo into paritytech/ci-linux and ran `cargo build`
# inside a single throwaway container -- this builds a real, standalone image
# from a real multi-stage Dockerfile, matching what docker-compose.yml's 3
# services actually need (an image they can all share, not rebuild each).
#
# NOTE: this is a native `cargo build --release` of the full node (runtime
# wasm blob + rocksdb + the whole polkadot-sdk dependency graph) -- expect
# 20-40+ minutes on the first build, per the spec's own estimate
# (docs/IMPLEMENTATION_D_SPEC.md-style warning applies here too). Subsequent
# builds reuse Docker's layer cache for unchanged dependencies.

# ---- builder ----------------------------------------------------------
# Pinned to the exact toolchain version this repo is actually built/tested
# with on the host (rustc 1.94.0) rather than the floating `rust:1-bookworm`
# tag. Found the hard way (2026-07-14): `rust:1-bookworm` resolved to 1.97.0,
# whose newer rust-lld treats every sp_io/custom host-function extern
# (ext_storage_get_version_1, ext_falcon_crypto_verify_version_1, ...) as an
# unresolved symbol instead of a wasm import, failing the runtime wasm link
# with "undefined symbol" for every single host function -- even though this
# exact runtime links fine on the host with 1.94.0. Bump this only in lockstep
# with a verified host-toolchain bump, not independently.
FROM rust:1.94.0-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    clang \
    libclang-dev \
    protobuf-compiler \
    pkg-config \
    libssl-dev \
    git \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add wasm32-unknown-unknown && rustup component add rust-src

WORKDIR /argonaut
COPY . .

# -j4: same OOM lesson as this repo's .cargo/config.toml (gitignored, so it
# doesn't apply inside this fresh container) -- keep container builds from
# spiking memory the same way the host build did on 2026-07-09.
RUN cargo build --release -p argonaut-node --config 'build.jobs=4'

# ---- runtime ------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -u 1000 -U -s /bin/bash argonaut
COPY --from=builder /argonaut/target/release/argonaut-node /usr/local/bin/argonaut-node

# /data must exist and be owned by argonaut *before* a volume is ever mounted
# there: Docker seeds a fresh named volume's initial content/ownership from
# whatever is already at that path in the image, so without this RocksDB's
# --base-path hits a root-owned directory as uid 1000 and fails with
# "Permission denied" on first `docker compose up` (found live, 2026-07-14).
RUN mkdir -p /data && chown -R argonaut:argonaut /data

USER argonaut
WORKDIR /home/argonaut

EXPOSE 30333 9944 9615
VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/argonaut-node"]
CMD ["--help"]
