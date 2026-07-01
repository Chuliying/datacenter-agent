FROM rust:1.96-bookworm AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --locked --release --bin datacenter-agent

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --no-install-recommends --yes ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home app

WORKDIR /app

COPY --from=builder /app/target/release/datacenter-agent /usr/local/bin/datacenter-agent
COPY config ./config

USER app

EXPOSE 8080

ENTRYPOINT ["datacenter-agent"]
CMD ["--config", "config/config.toml"]
