FROM rust:1.88 AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY services ./services
COPY .sqlx ./.sqlx
ENV SQLX_OFFLINE=true
RUN cargo build --release


FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y curl ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/PaaSTech ./app

EXPOSE 8080

CMD ["./app"]
