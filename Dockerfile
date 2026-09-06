# syntax=docker/dockerfile:1.7
FROM rust:1.97-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS builder
WORKDIR /src
ARG CARGO_BUILD_JOBS=2
ARG CARGO_INCREMENTAL=0
COPY . .
# The host pins curl's static-curl feature. OpenSSL and zlib remain the same runtime dependencies
# as libgit2; protocol v2 must not introduce an undeclared system libcurl dependency here.
RUN --mount=type=cache,id=b10x-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=b10x-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=b10x-substrate-target,target=/src/target,sharing=locked \
    find Cargo.toml Cargo.lock crates -path '*/target' -prune -o -type f -exec touch {} + && \
    cargo build --locked --release \
      -p b10x-substrate-daemon --bin substrate-daemon --bin substrate-container-exec --bin substrate-container-profiles \
      -p b10x-substrate-mcp --bin substrate-mcp \
      -p b10x-substrate-sdk --example container-pty-check && \
    install -D /src/target/release/substrate-daemon /out/substrate-daemon && \
    install -D /src/target/release/substrate-container-exec /out/substrate-container-exec && \
    install -D /src/target/release/substrate-container-profiles /out/substrate-container-profiles && \
    install -D /src/target/release/examples/container-pty-check /out/container-pty-check && \
    install -D /src/target/release/substrate-mcp /out/substrate-mcp && \
    install -D /usr/lib/x86_64-linux-gnu/libz.so.1 /out/lib/libz.so.1 && \
    install -d -m 0700 /out/state

FROM builder AS quota-executable
# Finish ownership and mode before setting the xattr; the runtime COPY must preserve it.
RUN apt-get update && \
    apt-get install -y --no-install-recommends libcap2-bin && \
    rm -rf /var/lib/apt/lists/* && \
    install --owner=0 --group=0 --mode=0755 /out/substrate-daemon /out/substrate-daemon-quota && \
    setcap cap_sys_admin=ep /out/substrate-daemon-quota

FROM rust:1.97-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97 AS sandbox-builder
# A non-setuid build of the exact audited backend; service startup never downloads tools.
RUN apt-get update && \
    apt-get install -y --no-install-recommends meson ninja-build pkg-config libcap-dev && \
    rm -rf /var/lib/apt/lists/* && \
    curl -fL https://github.com/containers/bubblewrap/releases/download/v0.11.2/bubblewrap-0.11.2.tar.xz -o /tmp/bubblewrap.tar.xz && \
    echo '69abc30005d2186baf7737feacd8da35633b93cf5af38838ecff17c5f8e924f6  /tmp/bubblewrap.tar.xz' | sha256sum -c - && \
    tar -xJf /tmp/bubblewrap.tar.xz -C /tmp && \
    meson setup /tmp/bubblewrap-build /tmp/bubblewrap-0.11.2 --prefix=/usr \
      -Dsupport_setuid=false -Dtests=false -Dman=disabled \
      -Dbash_completion=disabled -Dzsh_completion=disabled -Dselinux=disabled && \
    meson compile -C /tmp/bubblewrap-build && \
    install -D -m 0755 /tmp/bubblewrap-build/bwrap /out/bwrap && \
    install -D -m 0644 /tmp/bubblewrap-0.11.2/COPYING /out/bubblewrap-COPYING

FROM ubuntu:24.04@sha256:33ceb71981b602c1a7443a53469e4dba065f7503eab3078a2d7a57a2ab987517 AS execution-prerequisites
# Basic file tools are the admitted shell environment. Keep Debian package copyright notices.
# AppArmor's parser is used only by an explicit deployment-owned node setup, never by the daemon.
RUN apt-get update && \
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      apparmor bash ca-certificates coreutils findutils grep libcap2 libssl3t64 sed socat tini util-linux zlib1g && \
    rm -rf /var/lib/apt/lists/*
COPY --from=sandbox-builder /out/bwrap /usr/bin/bwrap
COPY --from=sandbox-builder /out/bubblewrap-COPYING /usr/share/licenses/bubblewrap/COPYING
COPY deploy/apparmor/substrate-host-execution /usr/share/substrate/apparmor/substrate-host-execution

FROM execution-prerequisites AS daemon
ARG SOURCE_SHA=unknown
LABEL org.opencontainers.image.revision=$SOURCE_SHA \
      org.opencontainers.image.source="https://github.com/beyond10x/substrate"
COPY --from=builder /out/substrate-daemon /usr/local/bin/substrate-daemon
COPY --from=builder /out/substrate-container-exec /usr/local/bin/substrate-container-exec
COPY --from=builder /out/substrate-container-profiles /usr/local/bin/substrate-container-profiles
COPY --from=quota-executable /out/substrate-daemon-quota /usr/local/bin/substrate-daemon-quota
COPY LICENSE THIRD_PARTY_LICENSES.html /usr/share/licenses/substrate/
COPY --from=builder --chown=65532:65532 --chmod=0700 /out/state /var/lib/b10x-substrate
VOLUME ["/var/lib/b10x-substrate"]
EXPOSE 8080
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/substrate-daemon"]

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:adcd20c7b4c988b73cbfbddb26d2eee574571e6d7c9ffea29b3821e0690efb77 AS mcp
ARG SOURCE_SHA=unknown
LABEL org.opencontainers.image.revision=$SOURCE_SHA \
      org.opencontainers.image.source="https://github.com/beyond10x/substrate" \
      dev.b10x.substrate.surface="disposable-mcp-testing"
COPY --from=builder /out/substrate-mcp /usr/local/bin/substrate-mcp
COPY --from=builder /out/lib/libz.so.1 /usr/lib/x86_64-linux-gnu/libz.so.1
COPY LICENSE THIRD_PARTY_LICENSES.html /usr/share/licenses/substrate/
ENTRYPOINT ["/usr/local/bin/substrate-mcp"]

# Local conformance helper, never one of the published runtime targets.
FROM execution-prerequisites AS container-checker
COPY --from=builder /out/container-pty-check /usr/local/bin/container-pty-check
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/container-pty-check"]

# Preserve `docker build .` as the daemon image while release automation selects both named
# runtime targets explicitly. The alias adds no bytes to the daemon image.
FROM daemon AS release
