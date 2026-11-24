# packaging/Dockerfile
# Multi-stage build for the Kora community indexer.
#
# Stage 1 (builder): compiles the indexer binary with the full Rust toolchain.
# Stage 2 (runtime): copies only the binary and minimal runtime deps into a
#   slim Debian image, keeping the final image under ~100 MB.
#
# Build:
#   docker build -t kora-indexer:latest -f packaging/Dockerfile .
#
# Run (standalone, bring-your-own Postgres):
#   docker run --env-file .env kora-indexer:latest
#
# See docker-compose.yml for a batteries-included Postgres setup.

# ── Stage 1: builder ──────────────────────────────────────────────────────────
FROM rust:1.78-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates git \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependencies before copying source (layer-cache optimisation).
COPY Cargo.toml Cargo.lock ./
# Create stub lib files for every workspace member so `cargo fetch` succeeds
# without the full source tree.
RUN mkdir -p services/indexer/src \
 && echo 'fn main(){}' > services/indexer/src/main.rs

COPY services/indexer/Cargo.toml services/indexer/Cargo.toml

RUN cargo fetch --locked

# Now copy the real source and build.
COPY services/indexer ./services/indexer

RUN cargo build --release --locked -p kora-indexer \
 && strip target/release/kora-indexer

# ── Stage 2: runtime ──────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 wget \
 && rm -rf /var/lib/apt/lists/*

# Run as a non-root user matching the systemd unit's `kora` user convention.
RUN useradd --system --create-home --shell /bin/false kora

WORKDIR /opt/kora/indexer

COPY --from=builder /build/target/release/kora-indexer /usr/local/bin/kora-indexer

# Data directory for the SQLite database (override DATABASE_URL for Postgres).
RUN mkdir -p /opt/kora/indexer/data && chown kora:kora /opt/kora/indexer/data

USER kora

# ── Environment defaults (override at runtime via --env-file or docker-compose)
# These mirror the variables written by deploy-indexer.sh's .env template so
# both deployment methods stay in sync.
ENV RUST_LOG=info \
    STELLAR_NETWORK=testnet \
    SOROBAN_RPC_URL=http://soroban-rpc:8000/soroban/rpc \
    DATABASE_URL=postgresql://kora:kora@postgres:5432/kora_indexer \
    START_LEDGER=0 \
    CHECKPOINT_INTERVAL_SECONDS=60 \
    POLL_INTERVAL_MS=1000 \
    CONTRACT_INVOICE_NFT="" \
    CONTRACT_MARKETPLACE="" \
    CONTRACT_FINANCING_POOL="" \
    CONTRACT_RISK_REGISTRY="" \
    CONTRACT_PRICE_ORACLE="" \
    CONTRACT_GOVERNANCE="" \
    CONTRACT_ACCESS_CONTROL=""

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/kora-indexer"]
