//! The acceptance scenarios of `story:a-client-cli-drives-the-daemon`, each a test named after
//! it. Every case spawns the shipped `substrate-daemon` binary, drives the shipped
//! `b10x-substrate` binary against its Unix socket, and asserts only on what the CLI prints and
//! the status it exits with.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::OnceLock;

use b10x_substrate_sdk::ManagedDaemon;
use serde_json::Value;

/// The shipped daemon binary. `SUBSTRATE_TEST_DAEMON` names one; otherwise the one beside this
/// test, which a workspace run builds; otherwise the workspace's own `substrate-daemon` target is
/// built once, in this test's profile and target directory, so the lane never runs without one.
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
            // `cargo test --workspace` has already built the daemon beside this test, as the SDK's
            // own tests rely on; only a package-scoped run builds it here.
            let sibling = profile_dir.join("substrate-daemon");
            if sibling.is_file() {
                return sibling;
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
            let status = build
                .status()
                .expect("starting cargo to build substrate-daemon");
            assert!(
                status.success(),
                "building substrate-daemon failed: {status}"
            );
            let binary = profile_dir.join("substrate-daemon");
            assert!(
                binary.is_file(),
                "{} is absent after the build",
                binary.display()
            );
            binary
        })
        .clone()
}

struct Fixture {
    _data: tempfile::TempDir,
    daemon: ManagedDaemon,
    socket: PathBuf,
}

async fn fixture() -> Fixture {
    let data = unix_socket::tempdir("owned/substrate.sock");
    let root = data.path().join("owned");
    let mut builder = ManagedDaemon::builder()
        .data_dir(&root)
        .deployment("cli_test")
        .external_binary(daemon_binary())
        .startup_timeout(readiness::DAEMON_READINESS);
    if let Some(cgroup) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT") {
        builder = builder.cgroup_root(cgroup);
    }
    let daemon = builder.start().await.expect("managed daemon starts");
    Fixture {
        _data: data,
        daemon,
        socket: root.join("substrate.sock"),
    }
}

async fn cli(socket: Option<&Path>, args: &[&str]) -> Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_b10x-substrate"));
    command.env_remove("SUBSTRATE_SOCKET");
    if let Some(socket) = socket {
        command.arg("--socket").arg(socket);
    }
    command.args(args);
    command.output().await.expect("the CLI runs")
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is one JSON document ({error}): {:?} / stderr {:?}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn stderr_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).unwrap_or_else(|error| {
        panic!(
            "stderr is one JSON document ({error}): {:?}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// A JSON success envelope's result, after asserting the envelope and a zero exit.
fn ok_result(output: &Output) -> Value {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document = stdout_json(output);
    assert_eq!(document["ok"], true);
    document["result"].clone()
}

/// The observation without its read instant, which differs on every read.
fn without_read_instant(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.remove("observed_at");
    }
    value
}

/// The facts the daemon requires before it dispatches confined exec
/// (`crates/substrate-daemon/src/app/operations.rs`, the `exec.sandbox-unavailable` check).
fn exec_served(facts: &Value) -> bool {
    facts.get("exec.namespaces").is_some()
        && facts.get("exec.cgroup-limits").is_some()
        && facts.get("exec.cgroup-kill") == Some(&Value::Bool(true))
        && facts.get("exec.no-egress") == Some(&Value::Bool(true))
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_machine_shows_facts() {
    let fixture = fixture().await;
    let machine = fixture.daemon.client().machine();
    let daemon_facts = serde_json::to_value(&machine.facts).expect("facts serialize");

    let output = cli(Some(&fixture.socket), &["--output", "json", "machine"]).await;
    let result = ok_result(&output);
    assert_eq!(result["snapshot"], machine.capability_snapshot.as_str());
    assert_eq!(
        result["observation"]["facts"], daemon_facts,
        "the CLI prints exactly the daemon's fact set"
    );

    // An unadvertised fact is absent, never assumed false.
    let human = cli(Some(&fixture.socket), &["machine"]).await;
    assert_eq!(human.status.code(), Some(0));
    let text = String::from_utf8(human.stdout).expect("human output is UTF-8");
    assert!(text.contains(&machine.capability_snapshot), "{text}");
    for name in [
        "exec.namespaces",
        "exec.cgroup-limits",
        "exec.cgroup-kill",
        "exec.no-egress",
        "events.pull",
    ] {
        let line = text
            .lines()
            .find(|line| line.split_whitespace().next() == Some(name))
            .unwrap_or_else(|| panic!("human output names {name}: {text}"));
        let value = line.split_whitespace().nth(1).expect("a value column");
        match daemon_facts.get(name) {
            None => {
                assert_eq!(value, "absent", "{line}");
                assert!(result["observation"]["facts"].get(name).is_none());
            }
            Some(_) => assert_ne!(value, "absent", "{line}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One ordered journey: create, show, renew, destroy.
async fn cli_workspace_lifecycle() {
    let fixture = fixture().await;
    let socket = Some(fixture.socket.as_path());
    let client = fixture.daemon.client();

    let created = ok_result(
        &cli(
            socket,
            &[
                "--output",
                "json",
                "workspace",
                "create",
                "--labels",
                r#"{"test":"cli"}"#,
                "--lease-ttl-ms",
                "60000",
            ],
        )
        .await,
    );
    let id = created["workspace"]
        .as_str()
        .expect("workspace id")
        .to_owned();
    assert_eq!(created["observation"]["labels"]["test"], "cli");
    let observed = client.get_workspace(&id).await.expect("daemon observes it");
    assert_eq!(
        without_read_instant(created["observation"].clone()),
        without_read_instant(serde_json::to_value(observed.observation()).expect("serialize")),
        "create answers the daemon's observation"
    );

    let shown = ok_result(&cli(socket, &["--output", "json", "workspace", "show", &id]).await);
    assert_eq!(shown["workspace"], id.as_str());
    let observed = client.get_workspace(&id).await.expect("daemon observes it");
    assert_eq!(
        without_read_instant(shown["observation"].clone()),
        without_read_instant(serde_json::to_value(observed.observation()).expect("serialize")),
        "show answers the daemon's observation"
    );

    let human = cli(socket, &["workspace", "show", &id]).await;
    assert_eq!(human.status.code(), Some(0));
    let line = String::from_utf8(human.stdout).expect("UTF-8");
    assert_eq!(line.lines().count(), 1, "human show is one line: {line}");
    assert!(line.contains(&id), "{line}");

    let renewed = ok_result(
        &cli(
            socket,
            &[
                "--output",
                "json",
                "workspace",
                "renew",
                &id,
                "--ttl-ms",
                "120000",
            ],
        )
        .await,
    );
    assert_eq!(renewed["workspace"], id.as_str());
    let observed = client.get_workspace(&id).await.expect("daemon observes it");
    let observed = serde_json::to_value(observed.observation()).expect("serialize");
    assert_eq!(
        without_read_instant(renewed["observation"].clone()),
        without_read_instant(observed.clone()),
        "renew answers the daemon's observation"
    );
    assert_eq!(
        observed["lease"]["ttl"]["secs"], 120,
        "the lease was renewed"
    );

    let destroyed =
        ok_result(&cli(socket, &["--output", "json", "workspace", "destroy", &id]).await);
    assert_eq!(destroyed["workspace"], id.as_str());
    assert_eq!(destroyed["observation"]["id"], id.as_str());
    assert_eq!(destroyed["observation"]["absent"], true);
    match client.get_workspace(&id).await {
        Ok(workspace) => assert_ne!(
            serde_json::to_value(workspace.observation().state).expect("state"),
            "Ready",
            "a destroyed workspace is no longer ready"
        ),
        Err(b10x_substrate_sdk::SdkError::Refusal(refusal)) => {
            assert_eq!(refusal.code, "resource.not-found");
        }
        Err(error) => panic!("unexpected error after destroy: {error}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_exec_runs_and_reports_exit() {
    let fixture = fixture().await;
    let socket = Some(fixture.socket.as_path());
    let machine = ok_result(&cli(socket, &["--output", "json", "machine"]).await);
    let served = exec_served(&machine["observation"]["facts"]);
    let created = ok_result(&cli(socket, &["--output", "json", "workspace", "create"]).await);
    let workspace = created["workspace"]
        .as_str()
        .expect("workspace id")
        .to_owned();
    let program = ["/bin/sh", "-c", "printf out; printf err >&2; exit 7"];

    let mut human_args = vec!["exec", "run", "--workspace", &workspace, "--"];
    human_args.extend(program);
    let human = cli(socket, &human_args).await;
    let mut json_args = vec!["--output", "json"];
    json_args.extend(&human_args);
    let json = cli(socket, &json_args).await;

    if served {
        eprintln!("cli-exec-runs-and-reports-exit: confined exec served; asserting the run");
        assert_eq!(human.stdout, b"out", "raw stdout passes through");
        assert_eq!(human.stderr, b"err", "raw stderr passes through");
        assert_eq!(human.status.code(), Some(7), "the command's exit code");

        assert_eq!(
            json.status.code(),
            Some(7),
            "json mode exits with the command's code"
        );
        let document = stdout_json(&json);
        assert_eq!(document["ok"], true);
        assert_eq!(document["result"]["stdout"], "out");
        assert_eq!(document["result"]["stderr"], "err");
        assert_eq!(document["result"]["observation"]["exit"]["code"], 7);
        let exec = document["result"]["exec"].as_str().expect("exec id");
        let shown = ok_result(&cli(socket, &["--output", "json", "exec", "show", exec]).await);
        assert_eq!(shown["exec"], exec);
        assert_eq!(shown["observation"]["exit"]["code"], 7);
    } else {
        eprintln!(
            "cli-exec-runs-and-reports-exit: confined exec unserved on this host; asserting the named refusal"
        );
        assert_ne!(
            human.status.code(),
            Some(0),
            "an unserved exec exits non-zero"
        );
        assert!(human.stdout.is_empty());
        let text = String::from_utf8_lossy(&human.stderr);
        assert!(text.contains("exec.sandbox-unavailable"), "{text}");

        assert_ne!(json.status.code(), Some(0));
        let document = stderr_json(&json);
        assert_eq!(document["ok"], false);
        assert_eq!(document["error"]["code"], "refused");
        assert_eq!(
            document["error"]["data"]["code"],
            "exec.sandbox-unavailable"
        );
        assert_eq!(document["error"]["data"]["class"], "unserved");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_refusal_is_named() {
    let fixture = fixture().await;
    let socket = Some(fixture.socket.as_path());
    let unknown = "ws_01JAAAAAAAAAAAAAAAAAAAAAAA";

    let human = cli(socket, &["workspace", "destroy", unknown]).await;
    assert_ne!(human.status.code(), Some(0), "a refusal exits non-zero");
    let text = String::from_utf8_lossy(&human.stderr);
    assert!(text.contains("resource.not-found"), "{text}");

    let json = cli(
        socket,
        &["--output", "json", "workspace", "destroy", unknown],
    )
    .await;
    assert_ne!(json.status.code(), Some(0));
    assert!(json.stdout.is_empty());
    let document = stderr_json(&json);
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "refused");
    assert_eq!(document["error"]["data"]["code"], "resource.not-found");
}

#[tokio::test(flavor = "multi_thread")]
async fn cli_socket_is_required() {
    let output = cli(None, &["machine"]).await;
    assert_eq!(output.status.code(), Some(2));
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        text.contains("--socket") && text.contains("SUBSTRATE_SOCKET"),
        "{text}"
    );
}
