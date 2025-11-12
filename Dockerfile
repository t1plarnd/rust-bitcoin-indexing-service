FROM rust:1.90 as builder

WORKDIR /app
COPY . .

RUN cargo build --release --bin app

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

RUN useradd -m -u 1000 appuser && \
    mkdir -p /app/data/nakamoto_db && \
    chown -R appuser:appuser /app

USER appuser

COPY --from=builder --chown=appuser:appuser /app/target/release/app /app/

EXPOSE 3000

CMD ["/app/app"]