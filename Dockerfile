FROM rust:1.88-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p substrate-daemon --bin substrate-daemon

FROM gcr.io/distroless/cc-debian12:nonroot
ARG SOURCE_SHA=unknown
LABEL org.opencontainers.image.revision=$SOURCE_SHA
COPY --from=builder /src/target/release/substrate-daemon /usr/local/bin/substrate-daemon
VOLUME ["/var/lib/daemonloom-substrate"]
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/substrate-daemon"]
