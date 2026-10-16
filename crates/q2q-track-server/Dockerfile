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
COPY crates/q2q-track/Cargo.toml crates/q2q-track/Cargo.toml
COPY crates/q2q-ratelimit/Cargo.toml crates/q2q-ratelimit/Cargo.toml
COPY crates/q2q-track-server/Cargo.toml crates/q2q-track-server/Cargo.toml
RUN mkdir -p crates/q2q-core/src crates/q2q-track/src crates/q2q-ratelimit/src \
  && : > crates/q2q-core/src/lib.rs \
  && : > crates/q2q-track/src/lib.rs \
  && : > crates/q2q-ratelimit/src/lib.rs \
  && mkdir -p crates/q2q-track-server/src \
  && echo 'fn main() {}' > crates/q2q-track-server/src/main.rs \
  && cargo build --release --bin q2q-track-server

FROM rust:1-alpine AS build
RUN apk add --no-cache build-base cmake perl
WORKDIR /src
COPY --from=deps /src/target ./target
COPY --from=deps /src/Cargo.lock ./Cargo.lock
COPY Cargo.toml rustfmt.toml ./
COPY crates ./crates
RUN cargo build --release --bin q2q-track-server

FROM alpine:3.21
# the service does not make outbound TLS calls, but keeping the bundle means a
# deployment that adds one later does not have to rebuild the image
RUN apk add --no-cache ca-certificates
COPY --from=build /src/target/release/q2q-track-server /usr/local/bin/q2q-track-server

EXPOSE 8080/tcp
WORKDIR /etc/q2q
ENTRYPOINT ["q2q-track-server"]
CMD ["/etc/q2q/track.toml"]
