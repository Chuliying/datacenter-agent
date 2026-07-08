FROM rust:1.91-alpine AS builder

RUN apk add --no-cache musl-dev build-base

WORKDIR /app

COPY . .

RUN cargo build --release --bin datacenter-agent

RUN strip -s ./target/release/datacenter-agent

FROM scratch

WORKDIR /app

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=builder /app/target/release/datacenter-agent /app/datacenter-agent
COPY --from=builder /app/config /app/config

EXPOSE 8080

ENTRYPOINT ["/app/datacenter-agent"]
CMD ["--host", "0.0.0.0", "--port", "8080"]
