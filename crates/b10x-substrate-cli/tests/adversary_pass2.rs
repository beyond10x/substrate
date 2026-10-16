//! Adversarial cases, second pass, against `b10x-substrate`: the commands the first pass did not
//! drive (session show and signal, operation show, metrics, events), the bounds the CLI documents
//! for them, and the required-ness rule of the binding comparison (`tests/common/mod.rs`).
//!
//! Like `scenarios.rs`, each daemon case spawns the shipped daemon, drives the shipped binary and
//! asserts only on what it prints and the status it exits with. The `live_session_*` case needs
//! confined exec; without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree it
//! says so on stderr and returns: absent, not passed.

mod common;

use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::OnceLock;
use std::time::Duration;

use b10x_substrate_cli::{Cli, Command, SessionCommand};
use b10x_substrate_sdk::{ExecutionPolicy, ManagedDaemon};
use clap::{CommandFactory as _, Parser as _};
use serde_json::Value;

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
            let status = build.status().expect("starting cargo");
            assert!(status.success(), "building substrate-daemon failed");
            profile_dir.join("substrate-daemon")
        })
        .clone()
}

struct Fixture {
    _data: tempfile::TempDir,
    daemon: ManagedDaemon,
    socket: PathBuf,
}

async fn fixture() -> Fixture {
    let data = tempfile::tempdir().expect("temporary parent");
    let root = data.path().join("owned");
    let mut builder = ManagedDaemon::builder()
        .data_dir(&root)
        .deployment("cli_adversary_two")
        .external_binary(daemon_binary());
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

async fn cli(socket: &Path, args: &[&str]) -> Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_b10x-substrate"));
    command.env_remove("SUBSTRATE_SOCKET");
    command.arg("--socket").arg(socket).args(args);
    command.output().await.expect("the CLI runs")
}

fn shown(output: &Output) -> String {
    format!(
        "status {:?}, stdout {:?}, stderr {:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json_of(bytes: &[u8], output: &Output) -> Value {
    serde_json::from_slice(bytes)
        .unwrap_or_else(|error| panic!("not one JSON document ({error}): {}", shown(output)))
}

fn ok_result(output: &Output) -> Value {
    assert_eq!(output.status.code(), Some(0), "{}", shown(output));
    let document = json_of(&output.stdout, output);
    assert_eq!(document["ok"], true, "{}", shown(output));
    document["result"].clone()
}

/// The JSON failure envelope's error, after asserting the exit status and an empty stdout.
fn failure(output: &Output, status: i32) -> Value {
    assert_eq!(output.status.code(), Some(status), "{}", shown(output));
    assert!(output.stdout.is_empty(), "{}", shown(output));
    let document = json_of(&output.stderr, output);
    assert_eq!(document["ok"], false, "{}", shown(output));
    document["error"].clone()
}

fn without_read_instant(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.remove("observed_at");
    }
    value
}

const UNKNOWN_SESSION: &str = "ses_01JAAAAAAAAAAAAAAAAAAAAAAA";

// ---------------------------------------------------------------------------------------------
// events
// ---------------------------------------------------------------------------------------------

/// `--limit` is documented as `1..=1000` (`EventsArgs::limit`, src/lib.rs:79) and an invalid
/// argument is a usage error with exit 2 (`EXIT_USAGE`, src/lib.rs:31). Out of range, the SDK
/// refuses before any request, and the CLI reports the daemon `unavailable` for reason
/// `protocol`, exit 1: a daemon that was never asked is named as the fault.
#[tokio::test(flavor = "multi_thread")]
async fn events_limit_out_of_range_is_a_usage_error() {
    let fixture = fixture().await;
    for limit in ["0", "1001"] {
        let output = cli(
            &fixture.socket,
            &["--output", "json", "events", "--limit", limit],
        )
        .await;
        let error = failure(&output, 2);
        assert_eq!(
            error["code"],
            "cli_input",
            "--limit {limit}: {}",
            shown(&output)
        );
    }
    let human = cli(&fixture.socket, &["events", "--limit", "0"]).await;
    assert_eq!(human.status.code(), Some(2), "{}", shown(&human));
    assert!(
        !String::from_utf8_lossy(&human.stderr).contains("unavailable"),
        "a limit the CLI refuses is not an unavailable daemon: {}",
        shown(&human)
    );
}

/// Both ends of `1..=1000` are served.
#[tokio::test(flavor = "multi_thread")]
async fn events_limit_bounds_are_served() {
    let fixture = fixture().await;
    for limit in ["1", "1000"] {
        let result = ok_result(
            &cli(
                &fixture.socket,
                &["--output", "json", "events", "--limit", limit],
            )
            .await,
        );
        let items = result["items"].as_array().expect("items is a list");
        assert!(items.len() <= limit.parse().expect("a number"), "{result}");
        assert!(result["next_cursor"].is_string(), "{result}");
    }
}

/// Reading to the end and resuming from `next_cursor` answers an empty page, in both outputs,
/// with exit 0; the empty page carries the same `next_cursor`.
#[tokio::test(flavor = "multi_thread")]
async fn events_empty_page_after_the_last_cursor() {
    let fixture = fixture().await;
    let client = fixture.daemon.client();
    client
        .workspace()
        .empty()
        .create()
        .await
        .expect("a workspace emits an event");
    let first = ok_result(
        &cli(
            &fixture.socket,
            &["--output", "json", "events", "--limit", "1000"],
        )
        .await,
    );
    assert!(
        !first["items"].as_array().expect("items").is_empty(),
        "a created workspace is in the log: {first}"
    );
    let cursor = first["next_cursor"]
        .as_str()
        .expect("next_cursor")
        .to_owned();
    let empty = ok_result(
        &cli(
            &fixture.socket,
            &["--output", "json", "events", "--cursor", &cursor],
        )
        .await,
    );
    assert_eq!(empty["items"], Value::Array(Vec::new()), "{empty}");
    assert_eq!(empty["next_cursor"], cursor.as_str(), "{empty}");

    let human = cli(&fixture.socket, &["events", "--cursor", &cursor]).await;
    assert_eq!(human.status.code(), Some(0), "{}", shown(&human));
    let text = String::from_utf8_lossy(&human.stdout);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "a header and the cursor, no rows: {text}");
    assert_eq!(lines[1], format!("next_cursor {cursor}"), "{text}");
}

/// A malformed cursor is the daemon's named refusal `event.cursor-invalid`, class `conflict`,
/// exit 1, and human output names the code.
#[tokio::test(flavor = "multi_thread")]
async fn events_bad_cursor_is_a_named_refusal() {
    let fixture = fixture().await;
    let output = cli(
        &fixture.socket,
        &["--output", "json", "events", "--cursor", "not-a-cursor"],
    )
    .await;
    let error = failure(&output, 1);
    assert_eq!(error["code"], "refused", "{}", shown(&output));
    assert_eq!(
        error["data"]["code"],
        "event.cursor-invalid",
        "{}",
        shown(&output)
    );
    assert_eq!(error["data"]["class"], "conflict", "{}", shown(&output));
    let human = cli(&fixture.socket, &["events", "--cursor", "not-a-cursor"]).await;
    assert_eq!(human.status.code(), Some(1), "{}", shown(&human));
    assert!(
        String::from_utf8_lossy(&human.stderr).contains("event.cursor-invalid"),
        "{}",
        shown(&human)
    );
}

/// An unset `--limit` reads a page of 100 (`spec/client/client.yaml`, `EventsRequest`). With more
/// than 100 events in the log, `events` answers exactly 100. Guards the default the binding cannot
/// state: the comparison reads required-ness, never a default value.
#[tokio::test(flavor = "multi_thread")]
async fn events_unset_limit_reads_a_page_of_one_hundred() {
    let fixture = fixture().await;
    let client = fixture.daemon.client();
    for _ in 0..101 {
        client
            .workspace()
            .empty()
            .create()
            .await
            .expect("workspace created");
    }
    let page = ok_result(&cli(&fixture.socket, &["--output", "json", "events"]).await);
    assert_eq!(
        page["items"].as_array().expect("items").len(),
        100,
        "an unset limit reads 100: {}",
        page["next_cursor"]
    );
}

// ---------------------------------------------------------------------------------------------
// operation show, metrics
// ---------------------------------------------------------------------------------------------

/// `operation show` answers the daemon's ledger record for an operation the CLI itself named, and
/// an unknown operation is a named refusal.
#[tokio::test(flavor = "multi_thread")]
async fn operation_show_reads_the_ledger_record() {
    let fixture = fixture().await;
    let client = fixture.daemon.client();
    let operation = "op-cli-adversary-two-0001";
    let created = ok_result(
        &cli(
            &fixture.socket,
            &[
                "--output",
                "json",
                "workspace",
                "create",
                "--operation-id",
                operation,
            ],
        )
        .await,
    );
    let workspace = created["workspace"].as_str().expect("workspace id");
    let shown_record = ok_result(
        &cli(
            &fixture.socket,
            &["--output", "json", "operation", "show", operation],
        )
        .await,
    );
    assert_eq!(shown_record["operation"], operation);
    let record = client
        .operation(operation)
        .await
        .expect("the daemon has it");
    assert_eq!(
        shown_record["observation"],
        serde_json::to_value(&record).expect("serialize"),
        "operation show answers the daemon's record"
    );
    assert_eq!(shown_record["observation"]["resource"], workspace);

    let human = cli(&fixture.socket, &["operation", "show", operation]).await;
    assert_eq!(human.status.code(), Some(0), "{}", shown(&human));
    let line = String::from_utf8_lossy(&human.stdout);
    assert_eq!(line.lines().count(), 1, "{line}");
    assert!(
        line.contains(operation) && line.contains(workspace),
        "{line}"
    );

    let unknown = cli(
        &fixture.socket,
        &[
            "--output",
            "json",
            "operation",
            "show",
            "op-cli-adversary-two-none",
        ],
    )
    .await;
    let error = failure(&unknown, 1);
    assert_eq!(error["code"], "refused", "{}", shown(&unknown));
    assert!(error["data"]["code"].is_string(), "{}", shown(&unknown));
}

/// `metrics` for a live workspace answers what the SDK observes, or the same named refusal; for
/// an unknown resource it is a named refusal with exit 1.
#[tokio::test(flavor = "multi_thread")]
async fn metrics_answers_the_daemon_or_its_refusal() {
    let fixture = fixture().await;
    let client = fixture.daemon.client();
    let workspace = client
        .workspace()
        .empty()
        .create()
        .await
        .expect("workspace created");
    let id = workspace.id().to_owned();
    let output = cli(
        &fixture.socket,
        &[
            "--output",
            "json",
            "metrics",
            "--kind",
            "workspace",
            "--id",
            &id,
        ],
    )
    .await;
    match client
        .metrics(b10x_substrate_sdk::MetricsResourceKind::Workspace, &id)
        .await
    {
        Ok(observed) => {
            let result = ok_result(&output);
            assert_eq!(result["resource_kind"], "workspace");
            assert_eq!(result["resource_id"], id.as_str());
            let mut expected = serde_json::to_value(&observed).expect("serialize");
            let mut actual = result["observation"].clone();
            for value in [&mut expected, &mut actual] {
                if let Some(object) = value.as_object_mut() {
                    object.remove("observed_at");
                    object.remove("usage");
                }
            }
            assert_eq!(actual, expected, "{}", shown(&output));
        }
        Err(b10x_substrate_sdk::SdkError::Refusal(refusal)) => {
            let error = failure(&output, 1);
            assert_eq!(
                error["data"]["code"],
                refusal.code.as_str(),
                "{}",
                shown(&output)
            );
        }
        Err(other) => panic!("the SDK metrics read failed outside the contract: {other}"),
    }

    let unknown = cli(
        &fixture.socket,
        &[
            "--output",
            "json",
            "metrics",
            "--kind",
            "exec",
            "--id",
            "exec_01JAAAAAAAAAAAAAAAAAAAAAAA",
        ],
    )
    .await;
    let error = failure(&unknown, 1);
    assert_eq!(error["code"], "refused", "{}", shown(&unknown));
    assert!(error["data"]["code"].is_string(), "{}", shown(&unknown));
}

// ---------------------------------------------------------------------------------------------
// session show, session signal
// ---------------------------------------------------------------------------------------------

/// An unknown session is `resource.not-found` for both `session show` and `session signal`.
#[tokio::test(flavor = "multi_thread")]
async fn session_unknown_is_a_named_refusal() {
    let fixture = fixture().await;
    for args in [
        vec!["--output", "json", "session", "show", UNKNOWN_SESSION],
        vec![
            "--output",
            "json",
            "session",
            "signal",
            UNKNOWN_SESSION,
            "--signal",
            "term",
        ],
    ] {
        let output = cli(&fixture.socket, &args).await;
        let error = failure(&output, 1);
        assert_eq!(error["code"], "refused", "{}", shown(&output));
        assert_eq!(
            error["data"]["code"],
            "resource.not-found",
            "{}",
            shown(&output)
        );
    }
}

/// A live pipe session, started through the SDK: `session show` answers its observation, a grace
/// over the daemon's 30 s bound is a named refusal, and `session signal --signal kill` ends it.
#[tokio::test(flavor = "multi_thread")]
async fn live_session_show_and_signal() {
    let fixture = fixture().await;
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
    let session = match started {
        Ok(session) => session,
        Err(b10x_substrate_sdk::SdkError::Refusal(refusal)) => {
            eprintln!(
                "live_session_show_and_signal: absent, sessions unserved here ({})",
                refusal.code
            );
            return;
        }
        Err(other) => panic!("starting a session failed outside the contract: {other}"),
    };
    let id = session.observation().id.clone();

    let shown_session = ok_result(
        &cli(
            &fixture.socket,
            &["--output", "json", "session", "show", &id],
        )
        .await,
    );
    assert_eq!(shown_session["session"], id.as_str());
    let observed = client.get_pipe_session(&id).await.expect("observed");
    let mut expected = serde_json::to_value(observed.observation()).expect("serialize");
    let mut actual = shown_session["observation"].clone();
    for value in [&mut expected, &mut actual] {
        if let Some(object) = value.as_object_mut() {
            object.remove("observed_at");
            object.remove("attachment");
            object.remove("queued_frames");
        }
    }
    assert_eq!(without_read_instant(actual), without_read_instant(expected));

    let over = cli(
        &fixture.socket,
        &[
            "--output",
            "json",
            "session",
            "signal",
            &id,
            "--signal",
            "term",
            "--grace-ms",
            "30001",
        ],
    )
    .await;
    let error = failure(&over, 1);
    assert_eq!(error["code"], "refused", "{}", shown(&over));
    assert!(error["data"]["code"].is_string(), "{}", shown(&over));

    let killed = cli(
        &fixture.socket,
        &[
            "--output",
            "json",
            "session",
            "signal",
            &id,
            "--signal",
            "kill",
            "--grace-ms",
            "0",
        ],
    )
    .await;
    let result = ok_result(&killed);
    assert_eq!(result["session"], id.as_str());
    let human = cli(&fixture.socket, &["session", "show", &id]).await;
    assert_eq!(human.status.code(), Some(0), "{}", shown(&human));
    let line = String::from_utf8_lossy(&human.stdout);
    assert_eq!(line.lines().count(), 1, "{line}");
    assert!(line.contains(&id), "{line}");
}

// ---------------------------------------------------------------------------------------------
// defaults the binding cannot state, read from the parsed command line
// ---------------------------------------------------------------------------------------------

/// `spec/client/client.yaml`: "An unset `grace_ms` waits 5000 ms before the kill."
#[test]
fn session_signal_unset_grace_is_five_seconds() {
    let cli = Cli::try_parse_from([
        "b10x-substrate",
        "session",
        "signal",
        UNKNOWN_SESSION,
        "--signal",
        "term",
    ])
    .expect("parses");
    let Command::Session(SessionCommand::Signal(args)) = cli.command else {
        panic!("not session signal");
    };
    assert_eq!(args.grace_ms, 5_000);
}

/// `website/docs/guides/cli.md`: the execution limits default to 30 s wall clock, 30 s CPU,
/// 256 MiB memory, 64 processes and 1 MiB per stream.
#[test]
fn exec_run_unset_limits_are_the_documented_defaults() {
    let cli = Cli::try_parse_from([
        "b10x-substrate",
        "exec",
        "run",
        "--workspace",
        "ws",
        "--",
        "/bin/true",
    ])
    .expect("parses");
    let Command::Exec(b10x_substrate_cli::ExecCommand::Run(args)) = cli.command else {
        panic!("not exec run");
    };
    assert_eq!(
        (
            args.timeout_ms,
            args.cpu_millis,
            args.memory_bytes,
            args.processes,
            args.output_bytes
        ),
        (30_000, 30_000, 268_435_456, 64, 1_048_576)
    );
}

// ---------------------------------------------------------------------------------------------
// required-ness in the binding comparison
// ---------------------------------------------------------------------------------------------

fn leaf(
    group: Option<&'static str>,
    name: &'static str,
    mutate: impl FnOnce(clap::Command) -> clap::Command,
) -> clap::Command {
    match group {
        Some(group) => Cli::command().mut_subcommand(group, |g| g.mut_subcommand(name, mutate)),
        None => Cli::command().mut_subcommand(name, mutate),
    }
}

fn parses(command: clap::Command, words: &[&str]) -> bool {
    command.try_get_matches_from(words).is_ok()
}

/// Dropping `--limit`'s default, as the derive does when `default_value_t` is removed, makes it
/// required; the binding says optional, and the comparison must refuse it.
#[test]
fn required_mutant_a_dropped_default_is_refused() {
    let mutant = leaf(None, "events", |leaf| {
        leaf.mut_arg("limit", |arg| {
            arg.default_values(Vec::<&str>::new()).required(true)
        })
    });
    assert!(
        !common::accepted_by_binding_check(mutant),
        "events --limit without a default is required and the check passes"
    );
}

/// A required option given a default, the flag left set: the comparison reads
/// `is_required_set()` and agrees with the binding, so it must be that the parse still demands
/// the option. Otherwise the binary accepts `workspace renew <id>` with no `--ttl-ms` while the
/// check says the option is required.
#[test]
fn required_mutant_a_default_on_a_required_option_is_seen() {
    let mutant = leaf(Some("workspace"), "renew", |leaf| {
        leaf.mut_arg("ttl_ms", |arg| arg.default_value("60000"))
    });
    let accepted = common::accepted_by_binding_check(mutant.clone());
    let parse_needs_it = !parses(mutant, &["b10x-substrate", "workspace", "renew", "ws"]);
    assert!(
        !accepted || parse_needs_it,
        "the check accepts it and `workspace renew ws` parses without --ttl-ms"
    );
}

/// An optional option made conditionally required (`required_if_eq`): the binding says
/// `EventsRequest.cursor` is optional whatever `--limit` is, so a check that accepts it must leave
/// `events --limit 5` parseable.
#[test]
fn required_mutant_a_conditional_requirement_is_seen() {
    let mutant = leaf(None, "events", |leaf| {
        leaf.mut_arg("cursor", |arg| arg.required_if_eq("limit", "5"))
    });
    let accepted = common::accepted_by_binding_check(mutant.clone());
    let parses_without_cursor = parses(mutant, &["b10x-substrate", "events", "--limit", "5"]);
    assert!(
        !accepted || parses_without_cursor,
        "the check accepts it and `events --limit 5` now demands --cursor"
    );
}

/// The same for a requirement on another argument's presence (`required_unless_present`).
#[test]
fn required_mutant_required_unless_present_is_seen() {
    let mutant = leaf(Some("workspace"), "create", |leaf| {
        leaf.mut_arg("operation_id", |arg| {
            arg.required_unless_present("lease_ttl_ms")
        })
    });
    let accepted = common::accepted_by_binding_check(mutant.clone());
    let parses_bare = parses(mutant, &["b10x-substrate", "workspace", "create"]);
    assert!(
        !accepted || parses_bare,
        "the check accepts it and a bare `workspace create` now demands --operation-id"
    );
}
