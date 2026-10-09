//! Where a host keeps its egress-aperture state (`story:aperture-state-outside-workspace-root`).
//!
//! An embedder may use a directory of sibling checkouts as its workspace root. Substrate state
//! written there sits among those checkouts, so a host with no declared aperture writes none, and
//! a host with one writes it where `HostConfig::aperture_root` says.
//!
//! Portable: every case opens a host whose bubblewrap path names nothing, so the probe proves no
//! confinement backend and no case needs a delegated cgroup. The aperture exec itself is the
//! delegated case beside the other aperture runtime cases in `src/process.rs`.

use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use substrate_host::{EgressAperture, HostConfig, HostDriver};
use tempfile::TempDir;

const DEFAULT_APERTURE_ROOT: &str = ".substrate-apertures";

fn config(directory: &TempDir) -> HostConfig {
    let mut config = HostConfig::minimum(directory.path().join("workspaces"));
    config.bubblewrap = directory.path().join("no-sandbox-binary");
    config
}

fn declared_aperture() -> EgressAperture {
    let pinned = SocketAddr::from((Ipv4Addr::LOCALHOST, 443));
    EgressAperture {
        name: "model".to_owned(),
        host: "app.example.invalid".to_owned(),
        port: pinned.port(),
        pinned,
        max_bytes: None,
    }
}

fn assert_private_directory(path: &Path) {
    let metadata = std::fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("{} was not created: {error}", path.display()));
    assert!(metadata.is_dir(), "{} is not a directory", path.display());
    assert_eq!(
        metadata.permissions().mode() & 0o777,
        0o700,
        "{} is not owner-private",
        path.display()
    );
}

#[test]
fn a_host_without_an_aperture_writes_no_aperture_state_in_the_workspace_root() {
    let directory = tempfile::tempdir().expect("test-owned root");
    let config = config(&directory);
    assert!(config.egress_apertures.is_empty());
    let workspace_root = config.workspace_root.clone();
    let aperture_root = config.aperture_root.clone();

    let _driver = HostDriver::open(config).expect("open a host with no aperture");

    assert!(
        !workspace_root.join(DEFAULT_APERTURE_ROOT).exists(),
        "a host with no egress aperture created {DEFAULT_APERTURE_ROOT} in its workspace root"
    );
    assert!(
        !aperture_root.exists(),
        "a host with no egress aperture created its aperture root"
    );
}

#[test]
fn an_explicit_aperture_root_holds_the_aperture_state_outside_the_workspace_root() {
    let directory = tempfile::tempdir().expect("test-owned root");
    let mut config = config(&directory);
    config.egress_apertures = vec![declared_aperture()];
    let explicit = directory.path().join("state").join("apertures");
    config.aperture_root = explicit.clone();
    let workspace_root = config.workspace_root.clone();

    let _driver = HostDriver::open(config).expect("open a host with an aperture");

    assert_private_directory(&explicit);
    assert!(
        !workspace_root.join(DEFAULT_APERTURE_ROOT).exists(),
        "an explicit aperture root still wrote {DEFAULT_APERTURE_ROOT} into the workspace root"
    );
}

#[test]
fn the_default_aperture_root_keeps_the_existing_workspace_layout() {
    let directory = tempfile::tempdir().expect("test-owned root");
    let mut config = config(&directory);
    assert_eq!(
        config.aperture_root,
        config.workspace_root.join(DEFAULT_APERTURE_ROOT),
        "the default aperture root moved"
    );
    config.egress_apertures = vec![declared_aperture()];
    let workspace_root = config.workspace_root.clone();

    let _driver = HostDriver::open(config).expect("open a host with an aperture");

    assert_private_directory(&workspace_root.join(DEFAULT_APERTURE_ROOT));
}
