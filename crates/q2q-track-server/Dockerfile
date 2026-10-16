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
COPY crates/q2q-punch-server/Cargo.toml crates/q2q-punch-server/Cargo.toml
COPY crates/q2q-ratelimit/Cargo.toml crates/q2q-ratelimit/Cargo.toml
COPY crates/q2q-track/Cargo.toml crates/q2q-track/Cargo.toml
COPY crates/q2q-track-server/Cargo.toml crates/q2q-track-server/Cargo.toml
# The workspace manifest lists every member, so cargo needs all of them present
# even though only one binary is built here.
RUN for c in q2q-core q2q-punch q2q-ratelimit q2q-track; do \
      mkdir -p crates/$c/src && : > crates/$c/src/lib.rs; \
    done \
 && for c in q2q-punch-server q2q-track-server; do \
      mkdir -p crates/$c/src && echo 'fn main() {}' > crates/$c/src/main.rs; \
    done \
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
