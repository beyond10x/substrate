#![forbid(unsafe_code)]
//! Security review of the local endpoint (ADR 0054): what startup must refuse about a declared
//! directory of sockets.
//!
//! ADR 0054 decision 2 makes the directory one of the daemon's private host paths and the agent's
//! alone: owned by the daemon's uid, writable by nobody else, and holding nothing the daemon
//! creates. Decision 6 holds a socket to one live run ("so a socket is never shared by two runs").
//! The relay connects to `<directory>/<key>` with the daemon's own credentials, so every entry a
//! key can name is reached as the daemon's uid.
//!
//! Each case calls the public `serve` with one declaration and reads which check answered. A
//! declaration startup admits runs on to `check_ca_bundle`, the last host declaration check, which
//! the configuration fails on purpose: so an admitted declaration is told apart from a refused one
//! without serving anything.

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use substrate_daemon::{DaemonConfig, LocalEndpointConfig, serve};
use tempfile::TempDir;

/// The answer of a configuration that passed every local endpoint check.
const ADMITTED: &str = "inspect the configured certificate bundle";

fn tree() -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temporary tree");
    let base = std::fs::canonicalize(directory.path()).expect("canonical tree");
    for child in ["workspaces", "other"] {
        std::fs::create_dir_all(base.join(child)).expect("tree directory");
    }
    (directory, base)
}

fn private_directory(path: &Path) -> PathBuf {
    std::fs::create_dir_all(path).expect("directory");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).expect("mode");
    path.to_path_buf()
}

fn endpoint(name: &str, port: u16, directory: PathBuf) -> LocalEndpointConfig {
    LocalEndpointConfig {
        name: name.to_owned(),
        port,
        directory,
    }
}

fn config(base: &Path, local_endpoints: Vec<LocalEndpointConfig>) -> DaemonConfig {
    let mut config = DaemonConfig::minimum(
        base.join("other/api.sock"),
        base.join("state/state.db"),
        base.join("workspaces"),
        "d",
        vec![1],
    );
    config.bubblewrap = base.join("other/bwrap");
    config.ca_bundle = Some(base.join("other/absent-ca.pem"));
    config.local_endpoints = local_endpoints;
    config
}

async fn answer(config: DaemonConfig) -> String {
    match tokio::time::timeout(std::time::Duration::from_secs(10), serve(config)).await {
        Ok(Err(error)) => format!("{error:#}"),
        Ok(Ok(())) => "serve returned Ok(())".to_owned(),
        Err(_) => "serve did not return within 10 s".to_owned(),
    }
}

/// Control: a private directory apart from every other path passes every local endpoint check and
/// reaches the certificate bundle, so `ADMITTED` is what an admitted declaration answers here.
#[tokio::test(flavor = "multi_thread")]
async fn control_a_private_directory_apart_from_the_daemon_paths_is_admitted() {
    let (_directory, base) = tree();
    let agent = private_directory(&base.join("agent"));
    let observed = answer(config(&base, vec![endpoint("agent", 7000, agent)])).await;
    assert!(observed.contains(ADMITTED), "{observed}");
}

/// Control: a directory its group may write is refused by the local endpoint check, so a red case
/// below is the check, not the harness.
#[tokio::test(flavor = "multi_thread")]
async fn control_a_group_writable_directory_stops_startup() {
    let (_directory, base) = tree();
    let agent = private_directory(&base.join("agent"));
    std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o770)).expect("mode");
    let observed = answer(config(&base, vec![endpoint("agent", 7000, agent)])).await;
    assert!(
        observed.starts_with("config.local-endpoint-invalid: "),
        "{observed}"
    );
    assert!(
        !observed.contains(&base.display().to_string()),
        "{observed}"
    );
}

/// Rule: a run never writes the directory of a local endpoint (ADR 0054 decision 2, and its
/// consequence "That holds while the directory is the agent's alone").
///
/// Every directory beneath the workspace root is one that a run may hold read-write: a workspace
/// is a directory there, and `workspace.adopt` serves any directory directly beneath the root that
/// the daemon's uid owns, by its name, to any admitted subject. A local endpoint directory is such
/// a directory, so a run of a workspace holding it creates entries in it, each one reached by a
/// relay as the daemon's uid. Startup refuses a writable root around or under the directory; the
/// workspace root is the root of every workspace and is held to the same rule here.
#[tokio::test(flavor = "multi_thread")]
async fn a_local_endpoint_directory_beneath_the_workspace_root_stops_startup() {
    let (_directory, base) = tree();
    let agent = private_directory(&base.join("workspaces/agent"));
    let observed = answer(config(&base, vec![endpoint("agent", 7000, agent)])).await;
    assert!(
        observed.starts_with("config.local-endpoint-invalid: "),
        "a local endpoint directory beneath the workspace root passed every startup check; \
         serve answered: {observed:?}"
    );
}

/// Rule: the directory holds nothing the daemon creates (ADR 0054 decision 2), so no key names a
/// socket of the daemon's own.
///
/// The API socket is the daemon's, and every relay connects with the daemon's uid, which the
/// API admits as `local:<uid>` when that uid is allowed. A socket name that is a valid key
/// (`[a-z0-9][a-z0-9-]*`) inside the declared directory is one a start can name.
#[tokio::test(flavor = "multi_thread")]
async fn the_api_socket_inside_a_local_endpoint_directory_stops_startup() {
    let (_directory, base) = tree();
    let agent = private_directory(&base.join("agent"));
    let mut config = config(&base, vec![endpoint("agent", 7000, agent.clone())]);
    config.socket = agent.join("api");
    let observed = answer(config).await;
    assert!(
        observed.starts_with("config.local-endpoint-invalid: "),
        "the API socket inside a local endpoint directory passed every startup check; serve \
         answered: {observed:?}"
    );
}

/// Rule: one live run holds a socket (ADR 0054 decision 6, "so a socket is never shared by two
/// runs").
///
/// The live set is keyed by the endpoint's name and the key, and the socket a relay reaches is
/// the directory and the key. Two names declared over one directory give two runs one socket: a
/// run holding key `k` on the first and a run holding `k` on the second both connect to
/// `<directory>/k`, and neither start is refused. Either startup refuses two declarations of one
/// directory, or the live set is keyed by the directory; this case holds the first form.
#[tokio::test(flavor = "multi_thread")]
async fn two_local_endpoints_over_one_directory_stop_startup() {
    let (_directory, base) = tree();
    let agent = private_directory(&base.join("agent"));
    let observed = answer(config(
        &base,
        vec![
            endpoint("agent", 7000, agent.clone()),
            endpoint("audit", 7001, agent),
        ],
    ))
    .await;
    assert!(
        observed.starts_with("config.local-endpoint-invalid: "),
        "two local endpoints over one directory passed every startup check; serve answered: \
         {observed:?}"
    );
}
