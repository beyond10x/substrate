//! Security review of bundle `0.25.0` (design 34 U7, ADR 0047 N5 and N7): the host driver's
//! answer does not change. It names `driver: host` and never publishes the mechanism-neutral
//! `confinement.no-egress`, which only a driver that observed a failed connection from inside a
//! run may state. The host publishes the Linux `exec.no-egress` instead.
//!
//! Portable: the bubblewrap path names nothing, so the probe proves no confinement backend.

#![cfg(target_os = "linux")]

use substrate_host::{Driver as _, HostConfig, HostDriver};
use substrate_wire::{CONFINEMENT_NO_EGRESS_FACT, HostDriverKind};

#[test]
fn security_review_a_host_snapshot_names_host_and_never_states_confinement_no_egress() {
    let directory = tempfile::tempdir().expect("test-owned root");
    let mut config = HostConfig::minimum(directory.path().join("workspaces"));
    config.bubblewrap = directory.path().join("no-sandbox-binary");
    let driver = HostDriver::open(config).expect("open a portable host");

    let snapshot = driver.machine();
    assert_eq!(snapshot.driver, HostDriverKind::Host);
    assert_eq!(snapshot.facts.confinement_no_egress, None);

    let rendered = serde_json::to_value(&snapshot).expect("snapshot serialises");
    assert_eq!(rendered["driver"], "host");
    assert!(
        rendered["facts"].get(CONFINEMENT_NO_EGRESS_FACT).is_none(),
        "a host snapshot states {CONFINEMENT_NO_EGRESS_FACT}: {}",
        rendered["facts"]
    );
}
