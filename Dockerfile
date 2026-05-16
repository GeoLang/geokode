FROM rust:1.83 AS builder

WORKDIR /app
COPY . .
RUN cargo build --release -p geokode-cli

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/geokode /usr/local/bin/geokode

EXPOSE 3000

ENTRYPOINT ["geokode"]
CMD ["serve", "--data", "/data/addresses.csv", "--bind", "0.0.0.0:3000"]
