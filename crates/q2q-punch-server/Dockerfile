# syntax=docker/dockerfile:1

# Built inside the image rather than cross-compiled on the runner: rustls pulls
# in aws-lc-sys, which needs cmake and a C toolchain and does not take kindly
# to being cross-compiled to musl.

FROM rust:1-alpine AS deps
RUN apk add --no-cache build-base cmake perl
WORKDIR /src
# Manifest first, with placeholder sources, so that changing our code does not
# invalidate the layer that builds every dependency.
COPY Cargo.toml Cargo.lock ./
COPY crates/q2q-core/Cargo.toml crates/q2q-core/Cargo.toml
COPY crates/q2q-punch/Cargo.toml crates/q2q-punch/Cargo.toml
COPY crates/q2q-ratelimit/Cargo.toml crates/q2q-ratelimit/Cargo.toml
COPY crates/q2q-punch-server/Cargo.toml crates/q2q-punch-server/Cargo.toml
RUN mkdir -p crates/q2q-core/src crates/q2q-punch/src crates/q2q-ratelimit/src \
  && : > crates/q2q-core/src/lib.rs \
  && : > crates/q2q-punch/src/lib.rs \
  && : > crates/q2q-ratelimit/src/lib.rs \
  && mkdir -p crates/q2q-punch-server/src \
  && echo 'fn main() {}' > crates/q2q-punch-server/src/main.rs \
  && cargo build --release --bin q2q-punch-server

FROM rust:1-alpine AS build
RUN apk add --no-cache build-base cmake perl
WORKDIR /src
COPY --from=deps /src/target ./target
COPY --from=deps /src/Cargo.lock ./Cargo.lock
COPY Cargo.toml rustfmt.toml ./
COPY crates ./crates
RUN cargo build --release --bin q2q-punch-server

FROM alpine:3.21
# Punch talks to Track over HTTPS when it registers, so it needs roots
RUN apk add --no-cache ca-certificates
COPY --from=build /src/target/release/q2q-punch-server /usr/local/bin/q2q-punch-server

# QUIC is UDP: exposing TCP here would be misleading
EXPOSE 4433/udp
WORKDIR /etc/q2q
ENTRYPOINT ["q2q-punch-server"]
CMD ["/etc/q2q/punch.toml"]
