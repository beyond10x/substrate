//! The confinement contract's delegated lane for `linux`
//! (`story:confinement-contract-on-linux-and-local`).
//!
//! Executes every authored `-on-delegated` scenario of `spec/confinement-suite.json` that opens
//! only `linux`, against the real adapter over `HostDriver` with a delegated cgroup v2 root, and
//! starts one confined run through the adapter. The count is read from the suite and must equal
//! what ran.
//!
//! Without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a cgroup v2 subtree this process is inside, or
//! without bubblewrap, these cases are **absent, never reported as passed** (invariant 3):
//! each prints `ABSENT` and its reason. `bash scripts/delegated-lane.sh` runs them and requires
//! the executed count this file prints.

#![cfg(target_os = "linux")]

mod support;

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::confinement::{
    self, AdapterName, AdapterRequest, Command, ConfinementAdapter, ConfinementError,
    ConfinementProfile, ContractFamily, Fact, ProcessBounds, RunState, linux::LinuxRequest,
};
use substrate_host::{EgressAperture, HostConfig};

/// The delegated cgroup v2 subtree this process is inside, or the reason the lane is absent.
fn delegated() -> Result<PathBuf, &'static str> {
    let root = PathBuf::from(
        std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")
            .ok_or("SUBSTRATE_VECTORS_CGROUP_ROOT is unset")?,
    );
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids")
        .map_err(|_| "the delegated cgroup root does not accept controllers")?;
    if !Path::new("/usr/bin/bwrap").is_file() {
        return Err("no bubblewrap at /usr/bin/bwrap");
    }
    Ok(root)
}

fn open_linux(
    host: &Path,
    cgroup_root: &Path,
    aperture: bool,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    let mut config = HostConfig::minimum(host.join("workspaces"));
    if aperture {
        // The setup the egress scenario names: an operator-declared aperture. The probe proves
        // the mechanism in a throwaway sandbox and never dials the destination.
        config.egress_apertures = vec![EgressAperture {
            name: "registry".to_owned(),
            host: "localhost".to_owned(),
            port: 9,
            pinned: SocketAddr::from((Ipv4Addr::LOCALHOST, 9)),
            max_bytes: None,
        }];
    }
    confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root.to_path_buf())),
    )))
}

#[test]
fn ess_confinement_delegated_lane_executes_every_authored_linux_scenario() {
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("ESS confinement linux delegated lane: ABSENT ({reason}); 0 executed");
            return;
        }
    };
    let scenarios = support::scenarios("delegated", |adapter| adapter == "linux");
    assert!(
        !scenarios.is_empty(),
        "the suite has no delegated linux scenario"
    );
    let declared = scenarios.len();
    let mut hosts = Vec::new();
    let (executed, audited) = support::run(&scenarios, |step| {
        assert_eq!(step.adapter, "linux");
        let host = tempfile::tempdir().expect("host root");
        let aperture = step
            .served_families
            .iter()
            .any(|family| family == "network_egress");
        let adapter = open_linux(host.path(), &cgroup_root, aperture);
        hosts.push(host);
        adapter
    });
    assert_eq!(executed, declared, "a delegated linux scenario did not run");
    let others = support::lane_size("delegated") - declared;
    println!(
        "ESS confinement linux delegated lane: {executed} scenarios executed of {declared} \
         declared for linux, {audited} refusal audit records asserted, 0 failed; {others} \
         delegated scenarios on other adapters not executed by this case"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_linux_run_is_confined_bounded_and_not_declared_insecure() {
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("ESS confinement linux run: ABSENT ({reason})");
            return;
        }
    };
    let host = tempfile::tempdir().expect("host root");
    let adapter = open_linux(host.path(), &cgroup_root, false).expect("linux opens");
    for family in [ContractFamily::ProcessRun, ContractFamily::Limits] {
        assert!(adapter.facts().serves(&family), "{:?}", adapter.facts());
    }
    let workspace = host.path().join("workspaces").join("ws_run");
    std::fs::create_dir_all(workspace.join("sub")).expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.clone()),
            workdir: Some("sub".into()),
            facts: vec![Fact::WholeTreeKill, Fact::IpcSignalsContained],
            process_bounds: Some(ProcessBounds {
                timeout_ms: 30_000,
                processes: 16,
                memory_bytes: 128 << 20,
                cpu_millis: 10_000,
            }),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    assert!(!admitted.declared_insecure());
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "pwd; printf done > /workspace/marker; exit 4".into(),
                ],
                env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
            },
        )
        .await
        .expect("started");
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    assert_eq!(ended.exit.as_ref().and_then(|exit| exit.code), Some(4));
    assert_eq!(ended.adapter, AdapterName::Linux);
    assert_eq!(ended.declared_insecure, None);
    let output = adapter.output(&run).await.expect("output");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "/workspace/sub\n");
    assert_eq!(
        std::fs::read_to_string(workspace.join("marker")).expect("marker"),
        "done"
    );
    println!("ESS confinement linux run: executed");
}
