# syntax=docker/dockerfile:1.7

# ── deps planner ───────────────────────────────────────────────────────
FROM rust:1.94-slim-bookworm AS chef
RUN cargo install cargo-chef --locked --version 0.1.77
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── build ──────────────────────────────────────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --bin pkdealer_service --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin pkdealer_service

# ── runtime ────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 10001 --create-home --shell /bin/false pkdealer
USER pkdealer
COPY --from=builder /app/target/release/pkdealer_service /usr/local/bin/
EXPOSE 50051
ENV PKDEALER_ADDR=0.0.0.0:50051 \
    OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector:4317 \
    OTEL_SERVICE_NAME=pkdealer_service \
    RUST_LOG=pkdealer_service=info,info
ENTRYPOINT ["/usr/local/bin/pkdealer_service"]
