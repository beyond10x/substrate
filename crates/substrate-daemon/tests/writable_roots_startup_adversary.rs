#![forbid(unsafe_code)]
//! Adversarial pass on the writable-roots wave (ADR 0038 kind 1): startup must refuse a writable
//! host root that holds a path the daemon trusts.
//!
//! `protected_host_paths` (`crates/substrate-daemon/src/runtime.rs`) documents itself as "Every
//! host path the daemon trusts outside the workspace root and the state directory" and lists the
//! API socket, the bubblewrap binary, secret-slot sources, the CA bundle and the delegated cgroup
//! root. The daemon also trusts, from configuration, the production TLS private key and
//! certificate chain, the CA bundle that authenticates hosted Identity, and the development
//! static-bearer file. A run given a writable root over the TLS key reads the daemon's identity
//! key or replaces it; over the bearer file it reads or replaces the credential the TCP listener
//! admits; over the Identity CA bundle it chooses which Identity the daemon trusts on restart.
//!
//! Each case declares a writable root `cache` that holds exactly one of those paths, with every
//! other path outside it, and calls the public `serve`. The refusal the existing protected paths
//! get is `config.writable-root-invalid: writable root "cache" contains ...`, before any TLS,
//! Identity or listener material is read (`serve`, the `check_writable_roots_clear_of` call).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use substrate_daemon::{
    DaemonConfig, HostedIdentityConfig, TcpDaemonConfig, TlsDaemonConfig, WritableRootConfig, serve,
};
use tempfile::TempDir;

fn tree() -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temporary tree");
    let base = std::fs::canonicalize(directory.path()).expect("canonical tree");
    // `state` is left for `serve` to create owner-private, as it requires.
    for child in ["workspaces", "cache", "other"] {
        std::fs::create_dir_all(base.join(child)).expect("tree directory");
    }
    (directory, base)
}

fn config(base: &Path) -> DaemonConfig {
    let mut config = DaemonConfig::minimum(
        base.join("other/api.sock"),
        base.join("state/state.db"),
        base.join("workspaces"),
        "d",
        vec![1],
    );
    config.bubblewrap = base.join("other/bwrap");
    config.writable_roots = vec![WritableRootConfig {
        name: "cache".to_owned(),
        path: base.join("cache"),
    }];
    config
}

fn production(base: &Path, key: PathBuf, chain: PathBuf, identity_ca: PathBuf) -> DaemonConfig {
    let mut config = config(base);
    config.tls = Some(TlsDaemonConfig {
        listen: SocketAddr::from(([127, 0, 0, 1], 0)),
        certificate_chain: chain,
        private_key: key,
    });
    config.hosted_identity = Some(HostedIdentityConfig {
        origin: "https://identity.invalid".to_owned(),
        ca_bundle: identity_ca,
    });
    config
}

async fn refusal(config: DaemonConfig) -> String {
    match tokio::time::timeout(std::time::Duration::from_secs(10), serve(config)).await {
        Ok(Err(error)) => error.to_string(),
        Ok(Ok(())) => "serve returned Ok(())".to_owned(),
        Err(_) => "serve did not return within 10 s".to_owned(),
    }
}

fn assert_contains_refusal(observed: &str, what: &str) {
    assert!(
        observed.starts_with("config.writable-root-invalid: writable root \"cache\" contains "),
        "a writable root holding {what} started past the protected-path check; serve answered: \
         {observed:?}"
    );
}

/// Control: a path already on the protected list reaches the refusal through the same `serve`
/// call and the same tree, so a red case below is the list, not the harness.
#[tokio::test(flavor = "multi_thread")]
async fn control_a_writable_root_containing_the_api_socket_stops_startup() {
    let (_directory, base) = tree();
    let mut config = config(&base);
    config.socket = base.join("cache/run/api.sock");
    assert_contains_refusal(&refusal(config).await, "the API socket");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_writable_root_containing_the_tls_private_key_stops_startup() {
    let (_directory, base) = tree();
    let config = production(
        &base,
        base.join("cache/tls/key.pem"),
        base.join("other/chain.pem"),
        base.join("other/identity-ca.pem"),
    );
    assert_contains_refusal(&refusal(config).await, "the TLS private key");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_writable_root_containing_the_tls_certificate_chain_stops_startup() {
    let (_directory, base) = tree();
    let config = production(
        &base,
        base.join("other/key.pem"),
        base.join("cache/tls/chain.pem"),
        base.join("other/identity-ca.pem"),
    );
    assert_contains_refusal(&refusal(config).await, "the TLS certificate chain");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_writable_root_containing_the_hosted_identity_ca_bundle_stops_startup() {
    let (_directory, base) = tree();
    let config = production(
        &base,
        base.join("other/key.pem"),
        base.join("other/chain.pem"),
        base.join("cache/identity-ca.pem"),
    );
    assert_contains_refusal(&refusal(config).await, "the hosted Identity CA bundle");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_writable_root_containing_the_static_bearer_file_stops_startup() {
    let (_directory, base) = tree();
    let mut config = config(&base);
    config.allow_uids = Vec::new();
    // Not the development profile, so a `serve` past the protected-path check also returns, at
    // its listener refusal, rather than serving.
    config.tcp = Some(TcpDaemonConfig {
        listen: SocketAddr::from(([127, 0, 0, 1], 0)),
        path_prefix: "/substrate".to_owned(),
        bearer_file: base.join("cache/bearer"),
        subject: "local:1000".to_owned(),
        actor: "adversary".to_owned(),
        private_overlay: true,
        development_only: false,
    });
    assert_contains_refusal(&refusal(config).await, "the static-bearer file");
}
