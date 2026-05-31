FROM rust:bookworm AS builder

WORKDIR /app
COPY . .
RUN cargo build --release -p geokode-cli

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -r -s /bin/false geokode

COPY --from=builder /app/target/release/geokode /usr/local/bin/geokode

USER geokode

ENV RUST_LOG=info,geokode=debug
ENV GEOKODE_PORT=3000

EXPOSE 3000
VOLUME /data

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:3000/health || exit 1

ENTRYPOINT ["geokode"]
CMD ["serve", "--data", "/data/addresses.csv", "--bind", "0.0.0.0:3000"]
