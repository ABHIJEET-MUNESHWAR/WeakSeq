# ---- Build stage ----
# Dependencies (ruint >= 1.19 needs rustc 1.90, plus edition-2024 crates); pin
# to the same stable CI uses. `-bookworm` matches the runtime base (glibc).
FROM rust:1.95-slim-bookworm AS builder
WORKDIR /build

# aws-lc-rs (pulled transitively via the OTLP/tonic TLS stack) needs a C
# toolchain + cmake + perl to build its crypto backend.
RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc g++ make cmake perl && rm -rf /var/lib/apt/lists/*

# Cache dependencies first.
COPY Cargo.toml Cargo.lock ./
COPY crates/types/Cargo.toml crates/types/Cargo.toml
COPY crates/auction/Cargo.toml crates/auction/Cargo.toml
COPY crates/consensus/Cargo.toml crates/consensus/Cargo.toml
COPY crates/api/Cargo.toml crates/api/Cargo.toml
COPY crates/node/Cargo.toml crates/node/Cargo.toml
RUN mkdir -p crates/types/src crates/auction/src crates/consensus/src \
    crates/api/src crates/node/src \
    && echo "fn main() {}" > crates/node/src/main.rs \
    && echo "" > crates/types/src/lib.rs \
    && echo "" > crates/auction/src/lib.rs \
    && echo "" > crates/consensus/src/lib.rs \
    && echo "" > crates/api/src/lib.rs \
    && echo "" > crates/node/src/lib.rs \
    && cargo build --release --bin weakseq-node || true

# Real sources.
COPY . .
RUN cargo build --release --bin weakseq-node

# ---- Runtime stage ----
FROM debian:bookworm-slim AS runtime
RUN useradd -r -u 10001 weakseq
COPY --from=builder /build/target/release/weakseq-node /usr/local/bin/weakseq-node
USER weakseq
EXPOSE 8081
ENTRYPOINT ["weakseq-node"]
CMD ["--listen", "0.0.0.0:8081"]
