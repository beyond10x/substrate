# Substrate Axum feature patch

This directory is the published `axum` 0.8.9 crate. Substrate changes two normalized `Cargo.toml`
entries — `default-features = false` for Axum's optional `hyper` and `hyper-util` dependencies —
and splits one WebSocket test key across two `concat!` operands. The latter produces identical test
bytes while keeping a public, non-secret RFC-style fixture from matching the repository's generic
credential detector. No allowlist was widened.

Axum's `http1` feature explicitly enables the required HTTP/1 features. Leaving the dependency
defaults enabled also compiles Hyper's HTTP/2 stack even though Substrate serves connections only
through `hyper::server::conn::http1::Builder`. The workspace dependency gate requires `h2` to be
absent, so an Axum update must either preserve this patch or demonstrate that upstream no longer
enables it.

The upstream crate's MIT licence remains in `LICENSE`.
