# syntax=docker/dockerfile:1.7
FROM rust:1.97-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS builder
WORKDIR /src
COPY . .
RUN --mount=type=cache,id=b10x-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=b10x-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=b10x-substrate-target,target=/src/target,sharing=locked \
    find Cargo.toml Cargo.lock crates -path '*/target' -prune -o -type f -exec touch {} + && \
    cargo build --locked --release -p substrate-daemon --bin substrate-daemon && \
    install -D /src/target/release/substrate-daemon /out/substrate-daemon && \
    install -d -m 0700 /out/state

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:adcd20c7b4c988b73cbfbddb26d2eee574571e6d7c9ffea29b3821e0690efb77
ARG SOURCE_SHA=unknown
LABEL org.opencontainers.image.revision=$SOURCE_SHA \
      org.opencontainers.image.source="https://github.com/beyond10x/substrate"
COPY --from=builder /out/substrate-daemon /usr/local/bin/substrate-daemon
COPY LICENSE THIRD_PARTY_LICENSES.html /usr/share/licenses/substrate/
COPY --from=builder --chown=65532:65532 --chmod=0700 /out/state /var/lib/b10x-substrate
VOLUME ["/var/lib/b10x-substrate"]
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/substrate-daemon"]
