FROM rust:1.88-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --locked --release -p substrate-daemon --bin substrated

FROM gcr.io/distroless/cc-debian12:nonroot
ARG SOURCE_SHA=unknown
LABEL org.opencontainers.image.revision=$SOURCE_SHA
COPY --from=builder /src/target/release/substrated /usr/local/bin/substrated
VOLUME ["/var/lib/daemonloom-substrate"]
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/substrated"]
