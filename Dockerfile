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

ARG PACK_VERSION=v0.36.4
RUN curl -sSL "https://github.com/buildpacks/pack/releases/download/${PACK_VERSION}/pack-${PACK_VERSION}-linux.tgz" \
    | tar -xz -C /usr/local/bin pack

WORKDIR /app

COPY --from=builder /app/target/release/PaaSTech ./app

EXPOSE 8080

CMD ["./app"]
