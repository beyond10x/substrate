//! Adversarial cases against `b10x-substrate`: usage and socket errors in the documented envelope,
//! and the exec refusals human mode must name. Like
//! `scenarios.rs`, each case drives the shipped binary and asserts only on what it prints and the
//! status it exits with.
//!
//! The `exec_*` cases need confined exec. Without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a
//! delegated cgroup v2 subtree the daemon reports exec unserved, and each says so on stderr and
//! returns: absent, not passed. Run them single-threaded under a delegated scope, because every
//! fixture daemon reconciles the cgroups beneath the one shared root.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::OnceLock;

use b10x_substrate_sdk::ManagedDaemon;
use serde_json::Value;

#[path = "../../../test-support/readiness.rs"]
mod readiness;
#[path = "../../../test-support/unix_socket.rs"]
mod unix_socket;

fn daemon_binary() -> PathBuf {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY
        .get_or_init(|| {
            if let Some(path) = std::env::var_os("SUBSTRATE_TEST_DAEMON") {
                return path.into();
            }
            let mut profile_dir = std::env::current_exe().expect("current test executable");
            profile_dir.pop();
            if profile_dir.file_name().is_some_and(|name| name == "deps") {
                profile_dir.pop();
            }
            let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
            let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
            let mut build = std::process::Command::new(cargo);
            build
                .arg("build")
                .arg("--locked")
                .arg("--manifest-path")
                .arg(&manifest)
                .args(["-p", "b10x-substrate-daemon", "--bin", "substrate-daemon"]);
            if profile_dir
                .file_name()
                .is_some_and(|name| name == "release")
            {
                build.arg("--release");
            }
            let status = build.status().expect("starting cargo");
            assert!(status.success(), "building substrate-daemon failed");
            profile_dir.join("substrate-daemon")
        })
        .clone()
}

struct Fixture {
    _data: tempfile::TempDir,
    _daemon: ManagedDaemon,
    socket: PathBuf,
}

async fn fixture() -> Fixture {
    let data = unix_socket::tempdir("owned/substrate.sock");
    let root = data.path().join("owned");
    let mut builder = ManagedDaemon::builder()
        .data_dir(&root)
        .deployment("cli_adversary")
        .external_binary(daemon_binary())
        .startup_timeout(readiness::DAEMON_READINESS);
    if let Some(cgroup) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT") {
        builder = builder.cgroup_root(cgroup);
    }
    let daemon = builder.start().await.expect("managed daemon starts");
    Fixture {
        _data: data,
        _daemon: daemon,
        socket: root.join("substrate.sock"),
    }
}

/// Runs the shipped CLI. `env` is applied after `SUBSTRATE_SOCKET` is removed.
async fn run(cwd: Option<&Path>, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_b10x-substrate"));
    command.env_remove("SUBSTRATE_SOCKET");
    for (name, value) in env {
        command.env(name, value);
    }
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.args(args);
    command.output().await.expect("the CLI runs")
}

async fn cli(socket: &Path, args: &[&str]) -> Output {
    let socket = socket.to_str().expect("UTF-8 socket path");
    let mut all = vec!["--socket", socket];
    all.extend(args);
    run(None, &[], &all).await
}

fn shown(output: &Output) -> String {
    format!(
        "status {:?}\nstdout {:?}\nstderr {:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json_of(bytes: &[u8], output: &Output) -> Value {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|error| panic!("not one JSON document ({error}):\n{}", shown(output)))
}

/// The confined-exec gate the scenario suite uses (`scenarios.rs`, `exec_served`).
async fn exec_workspace(fixture: &Fixture, case: &str) -> Option<String> {
    let machine = cli(&fixture.socket, &["--output", "json", "machine"]).await;
    let facts = json_of(&machine.stdout, &machine)["result"]["observation"]["facts"].clone();
    let served = facts.get("exec.namespaces").is_some()
        && facts.get("exec.cgroup-limits").is_some()
        && facts.get("exec.cgroup-kill") == Some(&Value::Bool(true))
        && facts.get("exec.no-egress") == Some(&Value::Bool(true));
    if !served {
        eprintln!("{case}: ABSENT, confined exec is unserved on this host; nothing was asserted");
        return None;
    }
    let created = cli(
        &fixture.socket,
        &["--output", "json", "workspace", "create"],
    )
    .await;
    Some(
        json_of(&created.stdout, &created)["result"]["workspace"]
            .as_str()
            .expect("workspace id")
            .to_owned(),
    )
}

async fn exec_run(fixture: &Fixture, json: bool, extra: &[&str], argv: &[&str]) -> Output {
    let workspace_args = exec_args(json, extra, argv);
    let refs: Vec<&str> = workspace_args.iter().map(String::as_str).collect();
    cli(&fixture.socket, &refs).await
}

fn exec_args(json: bool, extra: &[&str], argv: &[&str]) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    if json {
        words.extend(["--output".to_owned(), "json".to_owned()]);
    }
    words.extend(["exec".to_owned(), "run".to_owned()]);
    words.extend(extra.iter().map(|word| (*word).to_owned()));
    words.push("--".to_owned());
    words.extend(argv.iter().map(|word| (*word).to_owned()));
    words
}

// --- 1. Exit status and refusal mapping -------------------------------------------------------

/// The guide (`website/docs/guides/cli.md`, *Machine-readable output*) says `--output json` prints
/// one JSON document per call and that an unusable invocation is `cli_input`, exit 2. A clap
/// parse failure (a missing required option, a `--labels` that is not JSON) is the most common
/// unusable invocation, and `main_with` hands it to `clap::Error::exit`, which prints clap's own
/// text.
#[tokio::test(flavor = "multi_thread")]
async fn json_usage_error_is_a_cli_input_envelope() {
    for args in [
        &["--output", "json", "workspace", "renew", "ws_x"][..],
        &[
            "--output",
            "json",
            "workspace",
            "create",
            "--labels",
            "not-json",
        ][..],
    ] {
        let output = run(None, &[("SUBSTRATE_SOCKET", "/nonexistent/s.sock")], args).await;
        assert_eq!(output.status.code(), Some(2), "{}", shown(&output));
        let document: Result<Value, _> = serde_json::from_slice(&output.stderr);
        let Ok(document) = document else {
            panic!(
                "{args:?}: --output json usage error is not a JSON envelope:\n{}",
                shown(&output)
            );
        };
        assert_eq!(document["ok"], false, "{}", shown(&output));
        assert_eq!(document["error"]["code"], "cli_input", "{}", shown(&output));
    }
}

// --- 7. Socket selection ----------------------------------------------------------------------

/// An empty `SUBSTRATE_SOCKET` selects no socket, the same as an unset one: the guide says that
/// with neither the client exits 2 and names both. `main_with` turns `""` into `PathBuf::from("")`
/// and reports a transport failure, exit 1.
#[tokio::test(flavor = "multi_thread")]
async fn empty_socket_variable_is_no_socket() {
    let output = run(None, &[("SUBSTRATE_SOCKET", "")], &["machine"]).await;
    assert_eq!(output.status.code(), Some(2), "{}", shown(&output));
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        text.contains("--socket") && text.contains("SUBSTRATE_SOCKET"),
        "{}",
        shown(&output)
    );
}

// --- 2. exec run ------------------------------------------------------------------------------
/// The daemon names `exec.memory-limit` on an exec whose memory ceiling OOM-killed a process in
/// it (`record_resource_bound`, `crates/substrate-host/src/process.rs`) and leaves the state
/// `exited`, because a kernel OOM kill is not a forced cancellation. `report_run` prints the exec
/// line only when the state is not `exited` or there is no exit, so the named code never reaches
/// the user: on stderr is the command's output and nothing else.
#[tokio::test(flavor = "multi_thread")]
async fn exec_memory_limit_refusal_is_named() {
    let fixture = fixture().await;
    let Some(workspace) = exec_workspace(&fixture, "exec_memory_limit_refusal_is_named").await
    else {
        return;
    };
    let script = "head -c 268435456 /dev/zero | tail > /dev/null; exit 0";
    let json = exec_run(
        &fixture,
        true,
        &["--workspace", &workspace, "--memory-bytes", "33554432"],
        &["/bin/sh", "-c", script],
    )
    .await;
    let document = json_of(&json.stdout, &json);
    assert_eq!(
        document["result"]["observation"]["refusal"]["code"],
        "exec.memory-limit",
        "precondition: the daemon names the memory bound:\n{}",
        shown(&json)
    );
    let human = exec_run(
        &fixture,
        false,
        &["--workspace", &workspace, "--memory-bytes", "33554432"],
        &["/bin/sh", "-c", script],
    )
    .await;
    let text = String::from_utf8_lossy(&human.stderr);
    assert!(
        text.contains("exec.memory-limit") && human.status.code() != Some(0),
        "the daemon's named refusal is swallowed by human exec run:\n{}",
        shown(&human)
    );
}
