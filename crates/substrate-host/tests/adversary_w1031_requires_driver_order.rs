//! Adversary cases for wave 2026-10-31 (design 30 § 4.4, ADR 0048 D4), claim 1 at the host
//! driver: a start carrying `sandbox.requires` is checked after `exec.capability-stale` and
//! **before every field-specific check**, by the daemon and by the host driver alike.
//!
//! The daemon places `check_confinement_requires` straight after the stale check and before the
//! read-only-root, read-only-file, writable-root, workdir and secret-slot checks. The driver's
//! `ProcessRuntime::admit` checks read-only roots, read-only files, workspace access and secret
//! slots first, so a start that earns both refusals hears a field's refusal from the driver and
//! `confinement.family-unserved` from the daemon: two implementations of one contract naming
//! different refusals for one request.
//!
//! `network-ingress` is never published by any adapter (design 30 § 4.3), so the requirement is
//! unmet on every host, delegated or portable, and the read-only root names a path that exists
//! nowhere, so it is refused on every host too: the case is deterministic on both lanes.

#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};

use substrate_host::{DispatchOutcome, Driver as _, HostConfig, HostDriver};
use substrate_wire::{
    ConfinementFamily, ConfinementRequest, ConfinementRequires, ExecEnvironment, ExecLimits,
    ExecStartInput, NetworkMode, ReadOnlyRoot, SandboxProfile, WorkspaceAccess,
};

const WORKSPACE: &str = "ws_test";

fn start(snapshot: &str) -> ExecStartInput {
    ExecStartInput {
        writable_roots: Vec::new(),
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        workspace: WORKSPACE.to_owned(),
        argv: vec!["/bin/true".to_owned()],
        env: ExecEnvironment {
            allow: vec![],
            set: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
        },
        sandbox: ConfinementRequest {
            requires: Some(ConfinementRequires {
                families: Some(vec![ConfinementFamily::NetworkIngress]),
                facts: None,
            }),
            capability_snapshot: snapshot.to_owned(),
            network: NetworkMode::None,
            aperture: None,
            apertures: Vec::new(),
            profile: SandboxProfile::Workspace,
            required: true,
        },
        limits: ExecLimits {
            timeout_ms: 30_000,
            output_bytes: 65_536,
            processes: 16,
            memory_bytes: 134_217_728,
            cpu_millis: 10_000,
        },
        wait: false,
        workspace_access: WorkspaceAccess::ReadWrite,
        scratch: None,
        measurements: BTreeSet::new(),
        capsule: None,
        lease_ttl_ms: None,
        workdir: None,
    }
}

/// The driver answers an unmet `requires` before a read-only root it would refuse anyway, as the
/// daemon does (`validate_exec_input`: `check_confinement_requires` before
/// `check_read_only_roots`).
#[tokio::test]
async fn adversary_w1031_the_driver_checks_requires_before_a_read_only_root() {
    let directory = tempfile::Builder::new()
        .prefix("adversary-w1031-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("fixture directory");
    let driver = HostDriver::open(HostConfig::minimum(directory.path().join("workspaces")))
        .expect("host driver");
    let snapshot = driver.machine().snapshot;
    std::fs::create_dir_all(driver.root().join(WORKSPACE)).expect("workspace directory");

    let mut input = start(&snapshot);
    input.read_only_roots = vec![ReadOnlyRoot {
        host_path: directory
            .path()
            .join("no-such-root")
            .to_str()
            .expect("UTF-8 fixture path")
            .to_owned(),
        mount: "/opt/adversary".to_owned(),
    }];
    let DispatchOutcome::NotDispatched(error) =
        driver.start_exec("ex_adversary1", WORKSPACE, &input).await
    else {
        panic!("a start with an unmet requirement must be refused before dispatch");
    };
    assert_eq!(
        (error.code, error.address.as_deref()),
        (
            substrate_wire::CONFINEMENT_FAMILY_UNSERVED,
            Some("sandbox.requires.families")
        ),
        "the driver named a field's refusal before the requirement: {error:?}"
    );
}
