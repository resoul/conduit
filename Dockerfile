# syntax=docker/dockerfile:1

FROM rust:1.85-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked --bin conduit \
    && strip target/release/conduit

FROM alpine:3.21
LABEL org.opencontainers.image.title="Conduit" \
      org.opencontainers.image.description="Conduit statically typed language & bytecode VM" \
      org.opencontainers.image.source="https://github.com/resoul/conduit"
COPY --from=builder /src/target/release/conduit /usr/local/bin/conduit
WORKDIR /work
ENTRYPOINT ["conduit"]
CMD ["--help"]
