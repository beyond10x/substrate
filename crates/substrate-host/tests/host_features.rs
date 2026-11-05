//! What the host crate serves with and without its `git` and `kubernetes` features
//! (`story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes`).
//!
//! A confinement-only consumer depends on `b10x-substrate-host` with `default-features = false`.
//! Its build then has no Git and no Kubernetes client, and each absent feature is a named refusal,
//! never a silent degradation (invariant 3):
//!
//! - without `git`, a host configured with a Git source publishes no `workspace.git` fact and
//!   refuses a Git source `workspace.git-source-unserved` before dispatch;
//! - without `kubernetes`, opening the `pod` adapter is `confinement.adapter-unserved` naming
//!   `pod` and the feature.
//!
//! Each case has a default-feature twin that asserts the opposite on the same setup, so the lane
//! that runs one set cannot pass on a build of the other. The no-default cases run with
//! `cargo test -p b10x-substrate-host --no-default-features --test host_features`.

// The Git cases drive the Linux host driver, which refuses to open elsewhere.
#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use substrate_host::confinement::pod::PodRequest;
use substrate_host::confinement::{self, AdapterName, AdapterRequest, ConfinementError};
use substrate_host::{DispatchOutcome, Driver as _, GitSourceBinding, HostConfig, HostDriver};
use substrate_wire::{GitSource, GitSourceEnvelope, WorkspaceCreateInput, WorkspaceSource};
use tempfile::TempDir;

/// The words every refusal of the `pod` adapter in a build without `kubernetes` carries.
const KUBERNETES_ABSENT: &str = "`kubernetes` feature";

/// The words the refusal of a configured Git source carries in a build without `git`.
#[cfg(not(feature = "git"))]
const GIT_ABSENT: &str = "no `git` feature";

/// A host with one Git source, on a loopback port nothing listens on, and no sandbox binary: the
/// probe needs neither a reachable provider nor confinement to decide `workspace.git`.
fn git_host() -> (TempDir, Arc<HostDriver>) {
    let directory = tempfile::tempdir().expect("test-owned workspace root");
    let mut config = HostConfig::minimum(directory.path().join("workspaces"));
    config.bubblewrap = directory.path().join("no-sandbox-binary");
    config.git_sources.push(
        GitSourceBinding::new("connector", "https://127.0.0.1:1/", None).expect("Git source"),
    );
    std::fs::create_dir(&config.workspace_root).expect("workspace root");
    let driver = HostDriver::open(config).expect("a host with a Git source opens");
    (directory, driver)
}

fn git_input() -> WorkspaceCreateInput {
    WorkspaceCreateInput {
        source: WorkspaceSource::Git(GitSourceEnvelope {
            git: GitSource {
                source: "connector".to_owned(),
                locator: "https://127.0.0.1:1/repository.git".to_owned(),
                reference: "main".to_owned(),
                commit: "0123456789012345678901234567890123456789".to_owned(),
                depth: 1,
            },
        }),
        labels: BTreeMap::new(),
        storage: None,
        lease_ttl_ms: None,
    }
}

fn cluster() -> PodRequest {
    PodRequest {
        cluster: Some(PathBuf::from(
            "/nonexistent/substrate-host-features/kubeconfig",
        )),
    }
}

fn pod_refusal(error: &ConfinementError) -> &str {
    let ConfinementError::AdapterUnserved {
        adapter: AdapterName::Pod,
        detail,
    } = error
    else {
        panic!("pod must be refused confinement.adapter-unserved naming pod: {error}");
    };
    assert_eq!(error.code(), "confinement.adapter-unserved");
    detail
}

/// The gate's no-default lane sets this, so a build that has the features anyway fails instead of
/// running the default twins and passing: the host crate dev-depends on `b10x-substrate-cli`, and a
/// CLI edge that takes the host's defaults turns both features back on in every host test build.
const NO_DEFAULT_LANE: &str = "SUBSTRATE_HOST_NO_DEFAULT_FEATURES";

#[test]
fn the_no_default_lane_builds_neither_feature() {
    if std::env::var_os(NO_DEFAULT_LANE).is_some() {
        assert_eq!(
            [cfg!(feature = "git"), cfg!(feature = "kubernetes")],
            [false, false],
            "{NO_DEFAULT_LANE} is set, so this build must have neither `git` nor `kubernetes`"
        );
    }
}

#[cfg(not(feature = "git"))]
#[tokio::test]
async fn without_git_no_workspace_git_fact_and_a_git_source_is_unserved() {
    let (directory, driver) = git_host();
    assert_eq!(
        driver.machine().facts.workspace_git,
        None,
        "a build without `git` must not publish workspace.git"
    );
    let outcome = driver
        .create_workspace_authorized("ws_git", "ws_git", &git_input(), Some("authority"))
        .await;
    let DispatchOutcome::NotDispatched(error) = outcome else {
        panic!("a Git source must be refused before dispatch without `git`");
    };
    assert_eq!(error.code, "workspace.git-source-unserved", "{error}");
    assert_eq!(
        error.class,
        substrate_host::DriverErrorClass::Unserved,
        "{error}"
    );
    // The source is configured, so the refusal names what is missing: the feature, not the source.
    assert!(
        error.message.contains(GIT_ABSENT),
        "a configured source is refused naming the `git` feature: {error}"
    );
    assert!(
        !directory.path().join("workspaces/ws_git").exists(),
        "a refused Git source leaves no workspace"
    );
    driver.shutdown().await.expect("host shutdown");
}

#[cfg(feature = "git")]
#[tokio::test]
async fn with_git_the_same_host_publishes_workspace_git_and_dispatches_the_source() {
    let (_directory, driver) = git_host();
    assert_eq!(driver.machine().facts.workspace_git, Some(true));
    let outcome = driver
        .create_workspace_authorized("ws_git", "ws_git", &git_input(), Some("authority"))
        .await;
    if let DispatchOutcome::NotDispatched(error) = &outcome {
        assert_ne!(error.code, "workspace.git-source-unserved", "{error}");
    }
    driver.shutdown().await.expect("host shutdown");
}

#[cfg(not(feature = "kubernetes"))]
#[test]
fn without_kubernetes_every_way_to_open_pod_is_refused_by_name() {
    let opened = [
        confinement::open(AdapterRequest::Pod(cluster())).err(),
        confinement::open(AdapterRequest::Pod(PodRequest { cluster: None })).err(),
        confinement::pod::open_pod(cluster()).err(),
        confinement::pod::open_pod_timed(cluster(), confinement::pod::PodTiming::default()).err(),
    ];
    for error in opened {
        let error = error.expect("pod must not open without `kubernetes`");
        let detail = pod_refusal(&error);
        assert!(detail.contains(KUBERNETES_ABSENT), "{error}");
    }
}

#[cfg(feature = "kubernetes")]
#[test]
fn with_kubernetes_pod_is_refused_for_its_cluster_configuration_not_the_feature() {
    let error = confinement::open(AdapterRequest::Pod(cluster()))
        .err()
        .expect("no cluster at the path");
    let detail = pod_refusal(&error);
    assert!(!detail.contains(KUBERNETES_ABSENT), "{error}");
}

/// A materialization baseline an earlier build with `git` wrote goes when its workspace is
/// forgotten, in a build with or without `git`, so a later build with `git` never reads a
/// same-named workspace as materialized from Git. The workspace directory exists, so the open's
/// reconciliation of a build with `git` keeps the baseline and only the forget can remove it.
#[tokio::test]
async fn forgetting_a_workspace_removes_the_baseline_an_earlier_git_build_wrote() {
    let directory = tempfile::tempdir().expect("test-owned workspace root");
    let mut config = HostConfig::minimum(directory.path().join("workspaces"));
    config.bubblewrap = directory.path().join("no-sandbox-binary");
    config.git_sources.push(
        GitSourceBinding::new("connector", "https://127.0.0.1:1/", None).expect("Git source"),
    );
    let baselines = config.workspace_root.join(".substrate-git-baselines");
    std::fs::create_dir_all(&baselines).expect("baseline root");
    std::fs::create_dir(config.workspace_root.join("ws_old")).expect("operator directory");
    let baseline = baselines.join("ws_old");
    std::fs::write(&baseline, "0123456789012345678901234567890123456789\n").expect("baseline");
    let driver = HostDriver::open(config).expect("a host with a Git source opens");
    assert!(
        baseline.exists(),
        "the open keeps a baseline whose workspace directory exists"
    );
    driver
        .forget_workspace("ws_old", "ws_old")
        .await
        .expect("forget");
    assert!(
        !baseline.exists(),
        "forgetting the workspace removes its baseline in every build"
    );
    assert!(
        directory.path().join("workspaces/ws_old").is_dir(),
        "forgetting never touches the operator's directory"
    );
    driver.shutdown().await.expect("host shutdown");
}

/// The packages a confinement-only consumer must not link: the Git stack, `openssl-sys` and the
/// Kubernetes client (the story's first acceptance line).
const FENCED: [&str; 7] = [
    "gix",
    "git2",
    "libgit2-sys",
    "curl",
    "curl-sys",
    "openssl-sys",
    "b10x-substrate-kubernetes-client",
];

/// The package names of `cargo tree --locked -p b10x-substrate-host -e normal`, with or without
/// the default features. The tree is resolved for the host alone, whatever this test build has.
fn normal_tree(default_features: bool) -> Vec<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let mut command = std::process::Command::new(cargo);
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("CARGO_TERM_COLOR", "never")
        .args([
            "tree",
            "--locked",
            "-p",
            "b10x-substrate-host",
            "-e",
            "normal",
            "--prefix",
            "none",
            "-f",
            "{p}",
        ]);
    if !default_features {
        command.arg("--no-default-features");
    }
    let output = command.output().expect("cargo tree runs");
    assert!(
        output.status.success(),
        "cargo tree: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = String::from_utf8(output.stdout).expect("cargo tree writes UTF-8");
    let names: Vec<String> = tree
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    assert_eq!(
        names.first().map(String::as_str),
        Some("b10x-substrate-host"),
        "the tree starts at the host: {tree}"
    );
    names
}

/// `story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes`, its first
/// acceptance line, held by a program: the no-default normal tree names no fenced package, by any
/// path, a transitive one included, and the default tree still names the Git stack and the client.
#[test]
fn the_no_default_tree_names_no_git_stack_openssl_or_kubernetes_client() {
    let bare = normal_tree(false);
    let linked: Vec<&String> = bare
        .iter()
        .filter(|name| FENCED.contains(&name.as_str()))
        .collect();
    assert!(
        linked.is_empty(),
        "a confinement-only consumer links {linked:?}"
    );
    let full = normal_tree(true);
    for name in ["gix", "git2", "curl", "b10x-substrate-kubernetes-client"] {
        assert!(
            full.iter().any(|linked| linked == name),
            "the default tree must still name {name}"
        );
    }
}
