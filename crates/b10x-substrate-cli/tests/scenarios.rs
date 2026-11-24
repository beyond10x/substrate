//! The acceptance scenarios of `story:a-client-cli-drives-the-daemon`, each a test named after
//! it. Every case spawns the shipped `substrate-daemon` binary, drives the shipped
//! `b10x-substrate` binary against its Unix socket, and asserts only on what the CLI prints and
//! the status it exits with.

use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::Duration;

use b10x_substrate_sdk::{ExecutionPolicy, ManagedDaemon, PipeSessionState, SdkError};
use serde_json::Value;

/// The shipped daemon binary: the one `SUBSTRATE_TEST_DAEMON` names, else the one beside this
/// test, which a workspace build leaves there. A test builds none.
#[path = "../../../test-support/daemon_binary.rs"]
mod daemon_binary;
#[path = "../../../test-support/readiness.rs"]
mod readiness;
#[path = "../../../test-support/unix_socket.rs"]
mod unix_socket;

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
        .external_binary(daemon_binary::daemon_binary())
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

/// A retried command replays the stored outcome (`website/docs/concepts/operations.md`, *Retry
/// identity*): `workspace destroy` with the same `--operation-id` exits 0 and prints the same
/// result both times, though the first run removed the workspace. A renewal, which leaves its
/// workspace, replays as well. Another operation id is a new attempt and names the absence.
#[tokio::test(flavor = "multi_thread")]
async fn cli_retried_destroy_replays_the_stored_outcome() {
    const RENEW: &str = "01JCLRETRYRENEW00000000000";
    const DESTROY: &str = "01JCLRETRYDESTROY000000000";
    const SECOND_ATTEMPT: &str = "01JCLRETRYDESTROYAGAIN0000";
    let fixture = fixture().await;
    let socket = Some(fixture.socket.as_path());
    let created = ok_result(
        &cli(
            socket,
            &[
                "--output",
                "json",
                "workspace",
                "create",
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

    let renew = [
        "--output",
        "json",
        "workspace",
        "renew",
        &id,
        "--ttl-ms",
        "120000",
        "--operation-id",
        RENEW,
    ];
    let first = ok_result(&cli(socket, &renew).await);
    let second = ok_result(&cli(socket, &renew).await);
    assert_eq!(second, first, "a retried renewal prints the stored result");

    let destroy = [
        "--output",
        "json",
        "workspace",
        "destroy",
        &id,
        "--operation-id",
        DESTROY,
    ];
    let first = cli(socket, &destroy).await;
    let result = ok_result(&first);
    assert_eq!(result["workspace"], id.as_str());
    assert_eq!(result["observation"]["absent"], true);

    let second = cli(socket, &destroy).await;
    assert_eq!(
        second.status.code(),
        Some(0),
        "the retried destroy replays its stored outcome; stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&first.stdout),
        "the retried destroy prints the same result"
    );
    assert!(second.stderr.is_empty());

    let human = cli(
        socket,
        &["workspace", "destroy", &id, "--operation-id", DESTROY],
    )
    .await;
    assert_eq!(human.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&human.stdout),
        format!("workspace {id} absent\n")
    );

    let again = cli(
        socket,
        &[
            "--output",
            "json",
            "workspace",
            "destroy",
            &id,
            "--operation-id",
            SECOND_ATTEMPT,
        ],
    )
    .await;
    assert_ne!(again.status.code(), Some(0), "a new attempt is refused");
    assert!(again.stdout.is_empty());
    assert_eq!(
        stderr_json(&again)["error"]["data"]["code"],
        "resource.not-found"
    );
}

/// `story:commands-that-read-before-a-mutation-replay-their-stored-outcome`: `workspace renew`
/// with the same `--operation-id` exits 0 and prints the same result both times, though the
/// workspace was destroyed between the two runs. The command reads nothing before the renewal,
/// so the retry reaches the daemon's ledger. Another operation id is a new attempt and names the
/// absence.
#[tokio::test(flavor = "multi_thread")]
async fn cli_retried_renew_replays_the_stored_outcome_once_the_workspace_is_gone() {
    const RENEW: &str = "01JCLRENEWGONE000000000000";
    const SECOND_ATTEMPT: &str = "01JCLRENEWGONEAGAIN0000000";
    let fixture = fixture().await;
    let socket = Some(fixture.socket.as_path());
    let created = ok_result(
        &cli(
            socket,
            &[
                "--output",
                "json",
                "workspace",
                "create",
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

    let renew = [
        "--output",
        "json",
        "workspace",
        "renew",
        &id,
        "--ttl-ms",
        "120000",
        "--operation-id",
        RENEW,
    ];
    let first = cli(socket, &renew).await;
    let result = ok_result(&first);
    assert_eq!(result["workspace"], id.as_str());
    assert_eq!(
        result["observation"]["lease"]["ttl"]["secs"], 120,
        "the lease was renewed"
    );

    let destroyed =
        ok_result(&cli(socket, &["--output", "json", "workspace", "destroy", &id]).await);
    assert_eq!(destroyed["observation"]["absent"], true);
    let shown = cli(socket, &["--output", "json", "workspace", "show", &id]).await;
    assert_ne!(shown.status.code(), Some(0), "the workspace is gone");
    assert_eq!(
        stderr_json(&shown)["error"]["data"]["code"],
        "resource.not-found"
    );

    let second = cli(socket, &renew).await;
    assert_eq!(
        second.status.code(),
        Some(0),
        "the retried renewal replays its stored outcome; stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&first.stdout),
        "the retried renewal prints the same result"
    );
    assert!(second.stderr.is_empty());

    let again = cli(
        socket,
        &[
            "--output",
            "json",
            "workspace",
            "renew",
            &id,
            "--ttl-ms",
            "120000",
            "--operation-id",
            SECOND_ATTEMPT,
        ],
    )
    .await;
    assert_ne!(again.status.code(), Some(0), "a new attempt is refused");
    assert!(again.stdout.is_empty());
    assert_eq!(
        stderr_json(&again)["error"]["data"]["code"],
        "resource.not-found"
    );
}

/// `story:commands-that-read-before-a-mutation-replay-their-stored-outcome`: `session signal`
/// with the same `--operation-id` exits 0 and prints the same result both times, though the
/// session ended and was retired between the two runs, so its record is gone. The command reads
/// nothing before the signal, so the retry reaches the daemon's ledger.
///
/// A session needs confined exec. On a host that serves none, this case says so on stderr,
/// prints no `executed` line and holds the part that needs no session: a signal to a session
/// that never existed is a refusal the daemon recorded under the operation id, and the retry
/// prints it again.
#[tokio::test(flavor = "multi_thread")]
async fn cli_retried_session_signal_replays_the_stored_outcome_once_the_session_is_gone() {
    const SIGNAL: &str = "01JCLSIGNALGONE00000000000";
    let fixture = fixture().await;
    let socket = Some(fixture.socket.as_path());
    let client = fixture.daemon.client();
    let workspace = client
        .workspace()
        .empty()
        .create()
        .await
        .expect("workspace created");
    let policy = ExecutionPolicy::builder()
        .timeout(Duration::from_secs(30))
        .cpu_time(Duration::from_secs(5))
        .memory_bytes(64 * 1024 * 1024)
        .processes(16)
        .output_bytes(4096)
        .build()
        .expect("complete policy");
    let started = workspace
        .pipe_session("/bin/sh")
        .args(["-c", "sleep 30"])
        .policy(policy)
        .lease(Duration::from_secs(30))
        .input_limit_bytes(4096)
        .frame_limit_bytes(4096)
        .queued_frames(16)
        .start()
        .await;
    let mut session = match started {
        Ok(session) => session,
        Err(SdkError::Refusal(refusal)) => {
            eprintln!(
                "cli session signal replay: ABSENT (sessions unserved here: {}); asserting the \
                 recorded refusal of a session that never existed",
                refusal.code
            );
            session_signal_to_no_session_is_recorded_and_replayed(socket, SIGNAL).await;
            return;
        }
        Err(other) => panic!("starting a session failed outside the contract: {other}"),
    };
    let id = session.id().to_owned();

    let signal = [
        "--output",
        "json",
        "session",
        "signal",
        &id,
        "--signal",
        "kill",
        "--grace-ms",
        "0",
        "--operation-id",
        SIGNAL,
    ];
    let first = cli(socket, &signal).await;
    let result = ok_result(&first);
    assert_eq!(result["session"], id.as_str());

    // The session ends, and retiring it removes its record.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = session.refresh().await.expect("session read").state;
            if matches!(
                state,
                PipeSessionState::Exited | PipeSessionState::Cancelled | PipeSessionState::Expired
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the killed session ends within 10 s");
    assert!(
        session.retire().await.expect("the ended session retires"),
        "the retired session is absent"
    );
    let shown = cli(socket, &["--output", "json", "session", "show", &id]).await;
    assert_ne!(shown.status.code(), Some(0), "the session is gone");
    assert_eq!(
        stderr_json(&shown)["error"]["data"]["code"],
        "resource.not-found"
    );

    let second = cli(socket, &signal).await;
    assert_eq!(
        second.status.code(),
        Some(0),
        "the retried signal replays its stored outcome; stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&first.stdout),
        "the retried signal prints the same result"
    );
    assert!(second.stderr.is_empty());
    eprintln!("cli session signal replay: executed");
}

/// `exec run` with the same `--operation-id` prints the same output and exits with the same
/// status both times, though the workspace was destroyed between the two runs. The command reads
/// nothing before the start, so the retry reaches the daemon's ledger, which replays the stored
/// exec; its output is read from the exec's record, which outlives the workspace. Once that exec
/// is retired the record is gone, and the same command is refused `resource.not-found`.
///
/// Confined exec is needed. On a host that serves none this case says so on stderr and prints no
/// `executed` line.
#[tokio::test(flavor = "multi_thread")]
async fn cli_retried_exec_run_replays_the_stored_outcome_once_the_workspace_is_gone() {
    const RUN: &str = "01JCLEXECRUNGONE0000000000";
    let fixture = fixture().await;
    let socket = Some(fixture.socket.as_path());
    let machine = ok_result(&cli(socket, &["--output", "json", "machine"]).await);
    if !exec_served(&machine["observation"]["facts"]) {
        eprintln!("cli exec run replay: ABSENT (confined exec unserved on this host)");
        return;
    }
    let created = ok_result(&cli(socket, &["--output", "json", "workspace", "create"]).await);
    let workspace = created["workspace"]
        .as_str()
        .expect("workspace id")
        .to_owned();
    let run = [
        "--output",
        "json",
        "exec",
        "run",
        "--workspace",
        &workspace,
        "--operation-id",
        RUN,
        "--",
        "/bin/sh",
        "-c",
        "printf out; printf err >&2",
    ];
    let first = cli(socket, &run).await;
    let result = ok_result(&first);
    assert_eq!(result["stdout"], "out");
    assert_eq!(result["stderr"], "err");
    assert_eq!(result["observation"]["exit"]["code"], 0);

    let destroyed = ok_result(
        &cli(
            socket,
            &["--output", "json", "workspace", "destroy", &workspace],
        )
        .await,
    );
    assert_eq!(destroyed["observation"]["absent"], true);

    let second = cli(socket, &run).await;
    assert_eq!(
        second.status.code(),
        Some(0),
        "the retried run replays its stored outcome; stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&first.stdout),
        "the retried run prints the same result"
    );
    assert!(second.stderr.is_empty());

    let human = cli(socket, &run[2..]).await;
    assert_eq!(human.status.code(), Some(0));
    assert_eq!(human.stdout, b"out", "raw stdout passes through");
    assert_eq!(human.stderr, b"err", "raw stderr passes through");

    // Retiring the exec removes the record that holds its output. The start still replays, the
    // output read is refused by name, and the command prints no output and exits non-zero.
    let exec = result["exec"].as_str().expect("exec id");
    let record = fixture.daemon.client().get_exec(exec).await;
    let retired = record.expect("the exec's record outlives the workspace");
    assert!(retired.retire().await.expect("the ended exec retires"));
    for form in [&run[..], &run[2..]] {
        let third = cli(socket, form).await;
        assert_ne!(third.status.code(), Some(0), "never an empty success");
        assert!(third.stdout.is_empty(), "no output is printed");
        let refusal = String::from_utf8_lossy(&third.stderr);
        assert!(refusal.contains("resource.not-found"), "{refusal}");
    }
    assert_eq!(
        stderr_json(&cli(socket, &run).await)["error"]["data"]["code"],
        "resource.not-found"
    );
    eprintln!("cli exec run replay: executed");
}

/// The part of the session-signal case that needs no session: the signal reaches the daemon
/// under its operation id, so the refusal is a ledger record and the retry prints it again.
async fn session_signal_to_no_session_is_recorded_and_replayed(
    socket: Option<&Path>,
    operation: &str,
) {
    let signal = [
        "--output",
        "json",
        "session",
        "signal",
        "ses_01JAAAAAAAAAAAAAAAAAAAAAAA",
        "--signal",
        "kill",
        "--grace-ms",
        "0",
        "--operation-id",
        operation,
    ];
    let first = cli(socket, &signal).await;
    assert_ne!(first.status.code(), Some(0));
    assert!(first.stdout.is_empty());
    let error = stderr_json(&first)["error"].clone();
    assert_eq!(error["data"]["code"], "resource.not-found");
    assert_eq!(
        error["data"]["operation"], operation,
        "the refusal is recorded under the operation id"
    );
    let record = ok_result(
        &cli(
            socket,
            &["--output", "json", "operation", "show", operation],
        )
        .await,
    );
    assert_eq!(
        record["observation"]["refusal"]["code"],
        "resource.not-found"
    );

    let second = cli(socket, &signal).await;
    assert_eq!(second.status.code(), first.status.code());
    assert_eq!(
        String::from_utf8_lossy(&second.stderr),
        String::from_utf8_lossy(&first.stderr),
        "the retried signal prints the recorded refusal"
    );
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

/// Adversary pass on the SDK and CLI review of 0.24.0 (`tests/adversary_review_0_24/mod.rs`).
#[path = "adversary_review_0_24/mod.rs"]
mod adversary_review_0_24;

/// ADVERSARY CASES. Written by the adversary pass of 2026-11-23 on
/// `story:commands-that-read-before-a-mutation-replay-their-stored-outcome`, not by the
/// implementor of that change; the `limit_` case and the local refusal of a word that is not one
/// path segment state what was settled after that pass. Every case spawns the shipped daemon and
/// the shipped CLI and runs its scenario once.
mod adversary_cli_replay {
    use std::path::Path;
    use std::process::Output;

    use b10x_substrate_sdk::ManagedDaemon;
    use serde_json::Value;

    use super::{
        cli, daemon_binary, exec_served, fixture, ok_result, readiness, stderr_json, stdout_json,
    };

    /// What a caller of the CLI sees: the exit status and both streams.
    type Answer = (Option<i32>, String, String);

    fn answer(output: &Output) -> Answer {
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    async fn create(socket: Option<&Path>) -> String {
        let created = ok_result(
            &cli(
                socket,
                &[
                    "--output",
                    "json",
                    "workspace",
                    "create",
                    "--lease-ttl-ms",
                    "60000",
                ],
            )
            .await,
        );
        created["workspace"]
            .as_str()
            .expect("workspace id")
            .to_owned()
    }

    async fn renew(socket: Option<&Path>, workspace: &str, ttl: &str, operation: &str) -> Output {
        cli(
            socket,
            &[
                "--output",
                "json",
                "workspace",
                "renew",
                workspace,
                "--ttl-ms",
                ttl,
                "--operation-id",
                operation,
            ],
        )
        .await
    }

    async fn signal(socket: Option<&Path>, session: &str, name: &str, operation: &str) -> Output {
        cli(
            socket,
            &[
                "--output",
                "json",
                "session",
                "signal",
                session,
                "--signal",
                name,
                "--grace-ms",
                "0",
                "--operation-id",
                operation,
            ],
        )
        .await
    }

    async fn run(socket: Option<&Path>, workspace: &str, operation: &str, script: &str) -> Output {
        cli(
            socket,
            &[
                "--output",
                "json",
                "exec",
                "run",
                "--workspace",
                workspace,
                "--operation-id",
                operation,
                "--",
                "/bin/sh",
                "-c",
                script,
            ],
        )
        .await
    }

    async fn lease_secs(socket: Option<&Path>, workspace: &str) -> Value {
        let shown = ok_result(
            &cli(
                socket,
                &["--output", "json", "workspace", "show", workspace],
            )
            .await,
        );
        shown["observation"]["lease"]["ttl"]["secs"].clone()
    }

    #[track_caller]
    fn conflict(output: &Output, what: &str) {
        assert_ne!(output.status.code(), Some(0), "{what}");
        assert!(output.stdout.is_empty(), "{what}");
        assert_eq!(
            stderr_json(output)["error"]["data"]["code"],
            "operation.request-conflict",
            "{what}"
        );
    }

    /// The exec ids the events page lists, and the page's items.
    async fn events(socket: Option<&Path>) -> (Vec<String>, Vec<Value>) {
        let page =
            ok_result(&cli(socket, &["--output", "json", "events", "--limit", "1000"]).await);
        let items = page["items"].as_array().expect("event items").clone();
        let mut execs: Vec<String> = items
            .iter()
            .filter(|item| item["resource_kind"] == "exec")
            .map(|item| item["resource"].as_str().expect("an exec id").to_owned())
            .collect();
        execs.sort();
        execs.dedup();
        (execs, items)
    }

    /// LIMIT. A retried `exec run` replays while the daemon publishes the capability snapshot
    /// the first run named. The start's input carries the snapshot the CLI read at connect, and
    /// a daemon that starts again publishes another one, so the same command line is then
    /// another request under the same operation id: the retry is refused
    /// `operation.request-conflict` naming that id, exits non-zero and prints no result, and
    /// nothing is dispatched a second time. What a caller does then: `operation show <id>`
    /// answers the first run's stored record, the same before the restart, after it and after
    /// the refused retry.
    ///
    /// Control, in the same daemon and across the same restart: a retried `workspace renew`,
    /// whose input names no snapshot, replays.
    ///
    /// Runs on every host. Where confined exec is served the stored record names the one exec
    /// of the first run, and the events page lists that exec and no other. Where it is unserved
    /// the stored record is the refusal `exec.sandbox-unavailable` and the page lists no exec.
    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::too_many_lines)] // One ordered journey: run, restart, control, retry.
    async fn limit_exec_run_retried_under_another_capability_snapshot_is_a_request_conflict() {
        const RENEW: &str = "01JCLADVRESTARTRENEW000000";
        const RUN: &str = "01JCLADVRESTARTRUN00000000";
        let mut fixture = fixture().await;
        let socket_path = fixture.socket.clone();
        let socket = Some(socket_path.as_path());
        let machine = ["--output", "json", "machine"];
        let show = ["--output", "json", "operation", "show", RUN];
        let script = "printf out; printf err >&2";
        let probed = ok_result(&cli(socket, &machine).await);
        let before = probed["snapshot"].clone();
        let workspace = create(socket).await;

        let first_renew = renew(socket, &workspace, "120000", RENEW).await;
        assert_eq!(first_renew.status.code(), Some(0), "the renewal succeeds");
        let first_run = run(socket, &workspace, RUN, script).await;
        let recorded = ok_result(&cli(socket, &show).await)["observation"].clone();
        assert_eq!(recorded["kind"], "exec.start");
        // What the first run dispatched: one exec, or none where confined exec is unserved.
        let dispatched: Vec<String> = recorded["resource"]
            .as_str()
            .map(str::to_owned)
            .into_iter()
            .collect();
        if exec_served(&probed["observation"]["facts"]) {
            assert_eq!(first_run.status.code(), Some(0), "the first run succeeds");
            assert_eq!(
                dispatched,
                [stdout_json(&first_run)["result"]["exec"]
                    .as_str()
                    .expect("the first run's exec")],
                "the stored record names the first run's exec"
            );
        } else {
            assert_eq!(
                stderr_json(&first_run)["error"]["data"]["code"],
                "exec.sandbox-unavailable"
            );
            assert_eq!(recorded["refusal"]["code"], "exec.sandbox-unavailable");
            assert!(dispatched.is_empty(), "a refused start names no exec");
        }

        // The daemon stops and starts again on the same data directory.
        fixture.daemon.shutdown().await.expect("the daemon stops");
        let root = socket_path.parent().expect("the socket's directory");
        let mut builder = ManagedDaemon::builder()
            .data_dir(root)
            .deployment("cli_test")
            .external_binary(daemon_binary::daemon_binary())
            .startup_timeout(readiness::DAEMON_READINESS);
        if let Some(cgroup) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT") {
            builder = builder.cgroup_root(cgroup);
        }
        fixture.daemon = builder
            .start()
            .await
            .expect("the daemon starts again on its data directory");
        let after = ok_result(&cli(socket, &machine).await)["snapshot"].clone();
        assert_ne!(
            after, before,
            "a daemon that starts again publishes another capability snapshot"
        );

        assert_eq!(
            ok_result(&cli(socket, &show).await)["observation"],
            recorded,
            "`operation show` answers the first run's record after the restart"
        );
        // Control: a retried mutation whose input names no snapshot replays across the restart.
        let second_renew = renew(socket, &workspace, "120000", RENEW).await;
        assert_eq!(
            answer(&second_renew),
            answer(&first_renew),
            "control: the retried renewal replays across the restart"
        );

        let (execs, events_before) = events(socket).await;
        assert_eq!(
            execs, dispatched,
            "the events page lists what was dispatched"
        );
        let second_run = run(socket, &workspace, RUN, script).await;
        conflict(&second_run, "the same command line under another snapshot");
        assert_eq!(
            stderr_json(&second_run)["error"]["data"]["operation"],
            RUN,
            "the refusal names the operation id"
        );
        let (execs, events_after) = events(socket).await;
        assert_eq!(execs, dispatched, "the refused retry dispatched no exec");
        assert_eq!(
            events_after, events_before,
            "the refused retry recorded no event"
        );
        assert_eq!(
            ok_result(&cli(socket, &show).await)["observation"],
            recorded,
            "`operation show` answers the first run's record after the refused retry"
        );
        eprintln!(
            "limit exec run under another capability snapshot: executed, {} exec dispatched",
            dispatched.len()
        );
    }

    /// ADVERSARY CASE. The portable assertion on `exec run` without a read before the start:
    /// `cli_retried_exec_run_replays_the_stored_outcome_once_the_workspace_is_gone` needs
    /// confined exec and prints `ABSENT` before its first assertion on a host without it, which
    /// is every host the gate's test step runs on. This case needs none, as the session-signal
    /// case has such a part: a start that names a workspace which never existed reaches the
    /// daemon, is a refusal recorded under its operation id, and the retry prints that record
    /// again.
    ///
    /// A `--workspace` word that is not one path segment is refused before anything is sent and
    /// records no operation, as `workspace renew` and `session signal` refuse it. A word that is
    /// one segment and no workspace id is the daemon's refusal, recorded under its operation id.
    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::too_many_lines)] // The recorded refusal, then three words that name none.
    async fn cli_exec_run_to_a_workspace_that_never_existed_is_recorded_and_replayed() {
        const RUN: &str = "01JCLADVNOWORKSPACERUN0000";
        let fixture = fixture().await;
        let socket = Some(fixture.socket.as_path());
        let none = "ws_01JAAAAAAAAAAAAAAAAAAAAAAA";
        let show = ["--output", "json", "operation", "show", RUN];

        let first = run(socket, none, RUN, "exit 0").await;
        assert_ne!(first.status.code(), Some(0), "nothing ran");
        assert!(first.stdout.is_empty());
        let error = stderr_json(&first)["error"].clone();
        eprintln!(
            "adversary exec run, workspace never existed: refused {}",
            error["data"]["code"]
        );
        assert_eq!(
            error["data"]["operation"], RUN,
            "the start reached the daemon and is recorded under its operation id: {error}"
        );
        let record = ok_result(&cli(socket, &show).await);
        assert_eq!(record["observation"]["kind"], "exec.start");
        assert_eq!(
            record["observation"]["refusal"]["code"],
            error["data"]["code"]
        );
        let second = run(socket, none, RUN, "exit 0").await;
        assert_eq!(
            answer(&second),
            answer(&first),
            "the retried start prints the recorded refusal"
        );

        // A `--workspace` word that is no workspace id: refused by the CLI and unrecorded where it
        // is not one path segment, the daemon's recorded refusal where it is.
        for (word, operation, one_segment) in [
            ("", "01JCLADVEMPTYWORKSPACE0000", false),
            (
                "../ws_01JAAAAAAAAAAAAAAAAAAAAAAA",
                "01JCLADVDOTDOTWORKSPACE000",
                false,
            ),
            (
                "exe_01JAAAAAAAAAAAAAAAAAAAAAAA",
                "01JCLADVEXECASWORKSPACE000",
                true,
            ),
        ] {
            let output = run(socket, word, operation, "exit 0").await;
            assert_ne!(output.status.code(), Some(0), "{word:?} names no workspace");
            assert!(output.stdout.is_empty());
            let error = stderr_json(&output)["error"].clone();
            let shown = cli(
                socket,
                &["--output", "json", "operation", "show", operation],
            )
            .await;
            if one_segment {
                assert_eq!(error["data"]["operation"], operation, "{word:?}: {error}");
                assert_eq!(
                    ok_result(&shown)["observation"]["refusal"]["code"],
                    error["data"]["code"],
                    "{word:?} is the daemon's refusal, recorded under its operation id"
                );
            } else {
                assert_eq!(error["code"], "unavailable", "{word:?}: {error}");
                assert_eq!(error["data"]["reason"], "protocol", "{word:?}: {error}");
                assert_ne!(
                    shown.status.code(),
                    Some(0),
                    "{word:?} recorded no operation"
                );
                assert_eq!(
                    stderr_json(&shown)["error"]["data"]["code"],
                    "resource.not-found",
                    "{word:?} was refused before anything was sent"
                );
            }
        }
    }

    /// ADVERSARY CASE. One operation id names one request, for the three commands that send
    /// their mutation by id: the same `--operation-id` with another workspace, another TTL,
    /// another signal or another argv is the daemon's `operation.request-conflict`, exits
    /// non-zero, prints no result and changes nothing, and the first request still replays
    /// afterwards.
    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::too_many_lines)] // One ordered journey over three commands.
    async fn cli_one_operation_id_names_one_renewal_one_signal_and_one_run() {
        const RENEW: &str = "01JCLADVCONFLICTRENEW00000";
        const SIGNAL: &str = "01JCLADVCONFLICTSIGNAL0000";
        const RUN: &str = "01JCLADVCONFLICTRUN0000000";
        let fixture = fixture().await;
        let socket = Some(fixture.socket.as_path());
        let one = create(socket).await;
        let other = create(socket).await;

        let first = renew(socket, &one, "120000", RENEW).await;
        assert_eq!(first.status.code(), Some(0), "the renewal succeeds");
        conflict(
            &renew(socket, &other, "120000", RENEW).await,
            "another workspace",
        );
        conflict(&renew(socket, &one, "90000", RENEW).await, "another TTL");
        assert_eq!(
            lease_secs(socket, &one).await,
            120,
            "the first renewal stands"
        );
        assert_eq!(
            lease_secs(socket, &other).await,
            60,
            "the other workspace is as it was"
        );
        // The human form of the retry is one line that names the workspace, twice the same.
        let human = [
            "workspace",
            "renew",
            &one,
            "--ttl-ms",
            "120000",
            "--operation-id",
            RENEW,
        ];
        let human_run = cli(socket, &human).await;
        let human_retry = cli(socket, &human).await;
        assert_eq!(human_run.status.code(), Some(0));
        assert_eq!(
            answer(&human_retry),
            answer(&human_run),
            "the human replay repeats"
        );
        let line = String::from_utf8_lossy(&human_run.stdout).into_owned();
        assert!(line.starts_with(&format!("workspace {one} ")), "{line:?}");
        assert_eq!(line.lines().count(), 1, "{line:?}");

        let none = "ses_01JAAAAAAAAAAAAAAAAAAAAAAA";
        let first = signal(socket, none, "kill", SIGNAL).await;
        assert_eq!(
            stderr_json(&first)["error"]["data"]["code"],
            "resource.not-found"
        );
        conflict(
            &signal(socket, none, "term", SIGNAL).await,
            "another signal",
        );
        conflict(
            &signal(socket, "ses_01JBBBBBBBBBBBBBBBBBBBBBBB", "kill", SIGNAL).await,
            "another session",
        );
        assert_eq!(
            answer(&signal(socket, none, "kill", SIGNAL).await),
            answer(&first),
            "the same signal still replays its recorded refusal"
        );

        let first = run(socket, &one, RUN, "printf one").await;
        conflict(&run(socket, &one, RUN, "printf two").await, "another argv");
        conflict(
            &run(socket, &other, RUN, "printf one").await,
            "another workspace",
        );
        assert_eq!(
            answer(&run(socket, &one, RUN, "printf one").await),
            answer(&first),
            "the same run still replays its stored outcome"
        );
    }

    /// ADVERSARY CASE. What a replayed `exec run` prints once its workspace is gone, beyond the
    /// zero exit and untruncated output the change's own case covers: a non-zero exit status,
    /// both truncation flags, and an exec the daemon ended at its wall-clock limit, in the JSON
    /// and the human form. Each replay must be the first run's answer.
    ///
    /// Confined exec is needed. On a host that serves none this case says so on stderr and
    /// prints no `executed` line.
    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::too_many_lines)] // Two runs in two forms, before and after the destroy.
    async fn cli_replayed_exec_run_keeps_status_truncation_and_a_timeout() {
        let fixture = fixture().await;
        let socket = Some(fixture.socket.as_path());
        let machine = ok_result(&cli(socket, &["--output", "json", "machine"]).await);
        if !exec_served(&machine["observation"]["facts"]) {
            eprintln!(
                "adversary exec run replay fidelity: ABSENT (confined exec unserved on this host)"
            );
            return;
        }
        let created = ok_result(&cli(socket, &["--output", "json", "workspace", "create"]).await);
        let workspace = created["workspace"]
            .as_str()
            .expect("workspace id")
            .to_owned();
        let truncated = [
            "exec",
            "run",
            "--workspace",
            &workspace,
            "--output-bytes",
            "4",
            "--operation-id",
            "01JCLADVFIDELITYTRUNCATED0",
            "--",
            "/bin/sh",
            "-c",
            "printf abcdefgh; printf 12345678 >&2; exit 7",
        ];
        let timed_out = [
            "exec",
            "run",
            "--workspace",
            &workspace,
            "--timeout-ms",
            "300",
            "--operation-id",
            "01JCLADVFIDELITYTIMEOUT000",
            "--",
            "/bin/sh",
            "-c",
            "printf begun; sleep 20",
        ];
        let mut first: Vec<(Answer, Answer)> = Vec::new();
        for words in [&truncated[..], &timed_out[..]] {
            let mut json = vec!["--output", "json"];
            json.extend(words);
            let json = cli(socket, &json).await;
            let human = cli(socket, words).await;
            first.push((answer(&json), answer(&human)));
        }
        let (json, human) = &first[0];
        assert_eq!(json.0, Some(7), "the command's exit status: {json:?}");
        let document: Value = serde_json::from_str(&json.1).expect("a JSON result");
        assert_eq!(document["result"]["stdout_truncated"], true, "{json:?}");
        assert_eq!(document["result"]["stderr_truncated"], true, "{json:?}");
        assert_eq!(human.0, Some(7), "{human:?}");
        assert!(human.2.contains("stdout was truncated"), "{human:?}");
        assert!(human.2.contains("stderr was truncated"), "{human:?}");
        let (json, human) = &first[1];
        assert_ne!(json.0, Some(0), "a run ended at its limit: {json:?}");
        assert!(human.2.contains("b10x-substrate: exec "), "{human:?}");

        let destroyed = ok_result(
            &cli(
                socket,
                &["--output", "json", "workspace", "destroy", &workspace],
            )
            .await,
        );
        assert_eq!(destroyed["observation"]["absent"], true);

        for (words, (first_json, first_human)) in
            [&truncated[..], &timed_out[..]].into_iter().zip(&first)
        {
            let mut json = vec!["--output", "json"];
            json.extend(words);
            assert_eq!(
                &answer(&cli(socket, &json).await),
                first_json,
                "the JSON replay once the workspace is gone"
            );
            assert_eq!(
                &answer(&cli(socket, words).await),
                first_human,
                "the human replay once the workspace is gone"
            );
        }
        eprintln!("adversary exec run replay fidelity: executed");
    }
}
