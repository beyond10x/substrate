# syntax=docker/dockerfile:1.7
FROM rust:1.97-bookworm AS builder
WORKDIR /src
COPY . .
RUN --mount=type=cache,id=daemonloom-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=daemonloom-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=daemonloom-substrate-target,target=/src/target,sharing=locked \
    cargo build --locked --release -p substrate-daemon --bin substrate-daemon && \
    install -D /src/target/release/substrate-daemon /out/substrate-daemon

FROM gcr.io/distroless/cc-debian12:nonroot
ARG SOURCE_SHA=unknown
LABEL org.opencontainers.image.revision=$SOURCE_SHA
COPY --from=builder /out/substrate-daemon /usr/local/bin/substrate-daemon
VOLUME ["/var/lib/daemonloom-substrate"]
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/substrate-daemon"]
