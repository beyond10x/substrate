# syntax=docker/dockerfile:1.7
FROM rust:1.97-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97
RUN apt-get update && \
    apt-get install -y --no-install-recommends e2fsprogs util-linux && \
    rm -rf /var/lib/apt/lists/* && \
    command -v setpriv && command -v mkfs.ext4 && command -v mount && command -v umount
RUN rustup toolchain install 1.97 --profile minimal --component rustfmt,clippy && \
    chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"
ENV B10X_SUBSTRATE_QUOTA_LAB=1
WORKDIR /src
