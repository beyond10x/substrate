// Each case attacks the Seatbelt profile from inside it, and pairs the attack with a control that
// shows the same command succeeding where the profile allows it, so a refusal is the profile's.
//
// The cases after `the_environment_is_cleared` run through the `macos` confinement adapter with
// the build policy of `story:macos-seatbelt-policy-runs-cargo-test` (ADR 0109 S2): `cargo test`
// of a fixture crate, sealed paths, exec subpaths, the private `TMPDIR`, an unrepresentable path
// and the network under that policy.
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use substrate_host::confinement::{
    self, AdapterRequest, AuditRecord, AuditSink, ConfinementAdapter, ConfinementProfile, RunState,
    macos::MacosRequest,
};
use substrate_host::seatbelt::{self, SeatbeltPolicy};
use substrate_wire::ReadOnlyRoot;

const BASH: &str = "/bin/bash";

struct Host {
    _root: tempfile::TempDir,
    workspace: PathBuf,
    outside: PathBuf,
}

fn host() -> Host {
    let root = tempfile::tempdir().expect("temporary root");
    let workspace = root.path().join("workspace");
    let outside = root.path().join("outside");
    fs::create_dir(&workspace).expect("workspace");
    fs::create_dir(&outside).expect("outside");
    Host {
        _root: root,
        workspace,
        outside,
    }
}

fn bash(policy: &SeatbeltPolicy, script: &str, args: &[&str]) -> Output {
    let mut arguments = vec!["-c", script, "bash"];
    arguments.extend_from_slice(args);
    policy
        .command(Path::new(BASH), arguments)
        .expect("a confined command")
        .output()
        .expect("sandbox-exec runs")
}

fn unconfined_bash(script: &str, args: &[&str]) -> Output {
    Command::new(BASH)
        .arg("-c")
        .arg(script)
        .arg("bash")
        .args(args)
        .output()
        .expect("bash runs")
}

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8 path")
}

fn report(output: &Output) -> String {
    format!(
        "status {}; stdout {:?}; stderr {:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_write_outside_the_workspace_is_refused() {
    let host = host();
    let policy = SeatbeltPolicy::new(&host.workspace).executable(BASH);
    let inside = host.workspace.join("inside");
    let outside = host.outside.join("escaped");

    let control = bash(&policy, "printf x > \"$1\"", &[text(&inside)]);
    assert!(control.status.success(), "{}", report(&control));
    assert_eq!(fs::read(&inside).expect("written inside"), b"x");

    let attack = bash(&policy, "printf x > \"$1\"", &[text(&outside)]);
    assert!(!attack.status.success(), "{}", report(&attack));
    assert!(
        !outside.exists(),
        "the confined command wrote outside its workspace"
    );
}

#[test]
fn a_read_only_root_is_readable_and_not_writable() {
    let host = host();
    let root = host.outside.join("runtime");
    fs::create_dir(&root).expect("read-only root");
    let file = root.join("input");
    fs::write(&file, b"original").expect("input");
    let policy = SeatbeltPolicy::new(&host.workspace)
        .read_only(&root)
        .executable(BASH);

    let control = bash(
        &policy,
        "IFS= read -r line < \"$1\"; printf %s \"$line\"",
        &[text(&file)],
    );
    assert!(control.status.success(), "{}", report(&control));
    assert_eq!(control.stdout, b"original");

    let attack = bash(&policy, "printf tampered > \"$1\"", &[text(&file)]);
    assert!(!attack.status.success(), "{}", report(&attack));
    let created = bash(&policy, "printf new > \"$1\"", &[text(&root.join("new"))]);
    assert!(!created.status.success(), "{}", report(&created));
    assert_eq!(fs::read(&file).expect("input"), b"original");
    assert!(
        !root.join("new").exists(),
        "the confined command created a file in a read-only root"
    );
}

#[test]
fn an_undeclared_host_file_is_unreadable() {
    let host = host();
    let secret = host.outside.join("token");
    fs::write(&secret, b"secret\n").expect("secret");
    let script = "IFS= read -r line < \"$1\" && printf %s \"$line\"";

    let control = unconfined_bash(script, &[text(&secret)]);
    assert!(control.status.success(), "{}", report(&control));
    assert_eq!(control.stdout, b"secret", "{}", report(&control));

    let policy = SeatbeltPolicy::new(&host.workspace).executable(BASH);
    let attack = bash(&policy, script, &[text(&secret)]);
    assert!(!attack.status.success(), "{}", report(&attack));
    assert!(attack.stdout.is_empty(), "{}", report(&attack));
}

fn accepted_within(listener: &TcpListener, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        match listener.accept() {
            Ok(_) => return true,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("accept: {error}"),
        }
    }
    false
}

#[test]
fn an_outbound_connection_is_refused() {
    let host = host();
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    listener
        .set_nonblocking(true)
        .expect("non-blocking listener");
    let port = listener.local_addr().expect("address").port().to_string();
    let script = "exec 3<>\"/dev/tcp/127.0.0.1/$1\"";

    let control = unconfined_bash(script, &[&port]);
    assert!(control.status.success(), "{}", report(&control));
    assert!(
        accepted_within(&listener, Duration::from_secs(5)),
        "the unconfined control did not connect"
    );

    let policy = SeatbeltPolicy::new(&host.workspace).executable(BASH);
    let attack = bash(&policy, script, &[&port]);
    assert!(!attack.status.success(), "{}", report(&attack));
    assert!(
        !accepted_within(&listener, Duration::from_secs(1)),
        "the confined command reached the listener"
    );
}

#[test]
fn an_unlisted_executable_is_refused() {
    let host = host();
    let script = "/bin/ls \"$1\"";

    let listed = SeatbeltPolicy::new(&host.workspace)
        .executable(BASH)
        .executable("/bin/ls");
    let control = bash(&listed, script, &[text(&host.workspace)]);
    assert!(control.status.success(), "{}", report(&control));

    let unlisted = SeatbeltPolicy::new(&host.workspace).executable(BASH);
    let attack = bash(&unlisted, script, &[text(&host.workspace)]);
    assert!(!attack.status.success(), "{}", report(&attack));
}

#[test]
fn a_signal_to_a_process_outside_the_sandbox_is_refused() {
    let host = host();
    let policy = SeatbeltPolicy::new(&host.workspace)
        .executable(BASH)
        .executable("/bin/sleep");

    let control = bash(
        &policy,
        "/bin/sleep 30 & kill -TERM $!; wait $!; printf %s $?",
        &[],
    );
    assert_eq!(control.stdout, b"143", "{}", report(&control));

    let mut victim = Command::new("/bin/sleep")
        .arg("30")
        .spawn()
        .expect("an unconfined process");
    let pid = victim.id().to_string();
    let attack = bash(&policy, "kill -TERM \"$1\"", &[&pid]);
    let alive = victim.try_wait().expect("victim status").is_none();
    victim.kill().expect("stop the victim");
    victim.wait().expect("reap the victim");
    assert!(!attack.status.success(), "{}", report(&attack));
    assert!(
        alive,
        "the confined command signalled a process outside its sandbox"
    );
}

#[test]
fn the_environment_is_cleared() {
    let host = host();
    // macOS gives every login session a per-user TMPDIR, and bash never sets it itself.
    let script = "printf %s \"${TMPDIR-unset}\"";

    let control = unconfined_bash(script, &[]);
    assert!(control.status.success(), "{}", report(&control));
    assert_ne!(
        control.stdout, b"unset",
        "the test process has no TMPDIR to leak"
    );

    let policy = SeatbeltPolicy::new(&host.workspace).executable(BASH);
    let confined = bash(&policy, script, &[]);
    assert_eq!(confined.stdout, b"unset", "{}", report(&confined));
}

// The build policy, through the `macos` adapter.

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cargo-test");

fn macos() -> Arc<dyn ConfinementAdapter> {
    confinement::open(AdapterRequest::Macos(MacosRequest::default()))
        .expect("macos opens on a macOS host whose Seatbelt sandbox enforces")
}

fn recording() -> (AuditSink, Arc<Mutex<Vec<AuditRecord>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink_records = Arc::clone(&records);
    (
        AuditSink::new(move |record: &AuditRecord| sink_records.lock().push(record.clone())),
        records,
    )
}

fn run_bash(script: &str, extra: &[&str]) -> confinement::Command {
    let mut words = vec![BASH.to_owned(), "-c".into(), script.into(), "bash".into()];
    words.extend(extra.iter().map(|word| (*word).to_owned()));
    confinement::Command {
        argv: words,
        env: BTreeMap::new(),
    }
}

struct Ended {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Ended {
    fn report(&self) -> String {
        format!(
            "exit {:?}; stdout {:?}; stderr {:?}",
            self.code, self.stdout, self.stderr
        )
    }
}

async fn run_to_end(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    command: &confinement::Command,
    limit: Duration,
) -> Ended {
    let admitted = adapter.admit(profile).expect("admitted");
    let run = adapter.start(&admitted, command).await.expect("started");
    let ended = tokio::time::timeout(limit, adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    let output = adapter.output(&run).await.expect("output");
    Ended {
        code: ended.exit.and_then(|exit| exit.code),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("directory");
    for entry in fs::read_dir(from).expect("fixture directory") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy");
        }
    }
}

/// The toolchain `rust-toolchain.toml` selects for this crate, resolved.
fn sysroot() -> PathBuf {
    let output = Command::new("rustc")
        .args(["--print", "sysroot"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("rustc runs");
    assert!(output.status.success(), "{}", report(&output));
    let text = String::from_utf8(output.stdout).expect("UTF-8 sysroot");
    fs::canonicalize(text.trim()).expect("the sysroot resolves")
}

/// `cargo test --offline` of a copy of the fixture under the build policy: the toolchain is a
/// declared root and an exec subpath, so are `target/`, `/usr/bin` (the `cc` and `xcrun` shims)
/// and the selected developer tools; `.cargo` and `vendor` are sealed; `TMPDIR` is private.
async fn cargo_test(filter: Option<&str>, port: u16) -> Ended {
    let root = tempfile::tempdir().expect("temporary root");
    let workspace = fs::canonicalize(root.path())
        .expect("canonical")
        .join("fixture");
    copy_tree(Path::new(FIXTURE), &workspace);
    let sysroot = sysroot();
    let toolchain = text(&sysroot).to_owned();
    let developer = seatbelt::developer_root().expect("the runner has developer tools");
    let profile = ConfinementProfile {
        workspace: Some(workspace.clone()),
        read_only_roots: vec![ReadOnlyRoot {
            host_path: toolchain.clone(),
            mount: toolchain.clone(),
        }],
        exec_subpaths: vec![
            "target".into(),
            toolchain.clone(),
            "/usr/bin".into(),
            text(&developer).to_owned(),
        ],
        sealed_paths: vec![".cargo".into(), "vendor".into()],
        private_tmpdir: true,
        ..ConfinementProfile::default()
    };
    let mut argv = vec![
        format!("{toolchain}/bin/cargo"),
        "test".into(),
        "--offline".into(),
    ];
    argv.extend(filter.map(str::to_owned));
    let env = [
        ("PATH", "/usr/bin:/bin".to_owned()),
        ("HOME", text(&workspace).to_owned()),
        ("CARGO_HOME", text(&workspace.join("cargo-home")).to_owned()),
        ("RUSTC", format!("{toolchain}/bin/rustc")),
        ("CARGO_TERM_COLOR", "never".to_owned()),
        ("FIXTURE_PORT", port.to_string()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value))
    .collect();
    let ended = run_to_end(
        &macos(),
        &profile,
        &confinement::Command { argv, env },
        Duration::from_mins(10),
    )
    .await;
    let vendored = workspace.join("vendor/seatbelt-fixture-dep/src/lib.rs");
    assert_eq!(
        fs::read(&vendored).expect("vendored source"),
        fs::read(Path::new(FIXTURE).join("vendor/seatbelt-fixture-dep/src/lib.rs"))
            .expect("fixture source"),
        "the sealed vendor directory changed"
    );
    ended
}

fn listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    listener
        .set_nonblocking(true)
        .expect("non-blocking listener");
    let port = listener.local_addr().expect("address").port();
    (listener, port)
}

#[tokio::test(flavor = "multi_thread")]
async fn cargo_test_of_a_fixture_with_a_vendored_dependency_and_a_build_script_passes() {
    let (listener, port) = listener();
    let ended = cargo_test(None, port).await;
    assert_eq!(ended.code, Some(0), "{}", ended.report());
    assert!(
        ended.stdout.contains("test result: ok. 4 passed; 0 failed"),
        "{}",
        ended.report()
    );
    assert!(
        !accepted_within(&listener, Duration::from_secs(1)),
        "the fixture's test reached the listener"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_network_connection_from_the_fixtures_test_is_refused() {
    let (listener, port) = listener();
    let control = TcpStream::connect(("127.0.0.1", port));
    assert!(control.is_ok(), "the unconfined control did not connect");
    assert!(
        accepted_within(&listener, Duration::from_secs(5)),
        "the unconfined control was not accepted"
    );
    let ended = cargo_test(Some("the_network_is_unreachable"), port).await;
    assert_eq!(ended.code, Some(0), "{}", ended.report());
    assert!(
        ended.stdout.contains("test result: ok. 1 passed; 0 failed"),
        "{}",
        ended.report()
    );
    assert!(
        !accepted_within(&listener, Duration::from_secs(1)),
        "the fixture's test reached the listener under the build policy"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_path_and_an_undeclared_exec_subpath_are_refused() {
    let adapter = macos();
    let root = tempfile::tempdir().expect("temporary root");
    let workspace = fs::canonicalize(root.path()).expect("canonical");
    let sealed = workspace.join("deep/sealed");
    fs::create_dir_all(&sealed).expect("sealed");
    fs::write(sealed.join("config"), b"original").expect("config");
    for directory in ["allowed", "other"] {
        fs::create_dir(workspace.join(directory)).expect(directory);
        fs::copy("/bin/echo", workspace.join(directory).join("echo")).expect("echo");
    }
    let profile = ConfinementProfile {
        workspace: Some(workspace.clone()),
        sealed_paths: vec!["deep/sealed".into()],
        exec_subpaths: vec!["allowed".into(), "/bin".into()],
        ..ConfinementProfile::default()
    };
    let limit = Duration::from_secs(30);
    let run = |script: &'static str| {
        let adapter = Arc::clone(&adapter);
        let profile = profile.clone();
        async move { run_to_end(&adapter, &profile, &run_bash(script, &[]), limit).await }
    };

    for control in [
        "printf x > deep/beside",
        "/bin/mv deep/beside deep/moved-beside",
        "allowed/echo from-allowed",
    ] {
        let ended = run(control).await;
        assert_eq!(ended.code, Some(0), "control {control}: {}", ended.report());
    }
    assert!(workspace.join("deep/moved-beside").is_file());

    for attack in [
        "printf tampered > deep/sealed/config",
        "printf new > deep/sealed/new",
        "/bin/rm deep/sealed/config",
        "/bin/mv deep/sealed deep/unsealed",
        "/bin/mv deep elsewhere",
        "/bin/mkdir deep/sealed/sub",
    ] {
        let ended = run(attack).await;
        assert_ne!(ended.code, Some(0), "{attack}: {}", ended.report());
    }
    assert_eq!(
        fs::read(sealed.join("config")).expect("config"),
        b"original"
    );
    assert!(!sealed.join("new").exists());
    assert!(!sealed.join("sub").exists());
    assert!(!workspace.join("deep/unsealed").exists());
    assert!(!workspace.join("elsewhere").exists());

    let ended = run("other/echo from-other").await;
    assert_ne!(ended.code, Some(0), "{}", ended.report());
    assert!(
        !ended.stdout.contains("from-other"),
        "a program outside every exec subpath ran: {}",
        ended.report()
    );
}

/// A second run, while the first still runs, gets its own `TMPDIR`, can neither read nor list
/// the first run's, and its own is gone after `wait`.
async fn a_second_run_sees_nothing_of(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    private: &Path,
    limit: Duration,
) {
    let second = run_to_end(
        adapter,
        profile,
        &run_bash(
            "printf 'own:%s\\n' \"$TMPDIR\"; \
             if IFS= read -r line < \"$1/mark\"; then printf 'read:%s\\n' \"$line\"; fi; \
             for entry in \"$1\"/*; do printf 'listed:%s\\n' \"$entry\"; done",
            &[text(private)],
        ),
        limit,
    )
    .await;
    let own = second
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("own:"))
        .unwrap_or_else(|| panic!("the second run has no TMPDIR: {}", second.report()));
    assert!(!own.is_empty(), "{}", second.report());
    assert_ne!(Path::new(own), private, "{}", second.report());
    assert!(
        !second.stdout.contains("read:") && !second.stdout.contains("/mark"),
        "the second run saw the first run's file: {}",
        second.report()
    );
    assert!(
        !Path::new(own).exists(),
        "the second run's TMPDIR outlived its wait"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_private_tmpdir_is_per_run_and_gone_after_wait() {
    let adapter = macos();
    let root = tempfile::tempdir().expect("temporary root");
    let workspace = fs::canonicalize(root.path()).expect("canonical");
    let gate = workspace.join("gate");
    let made = Command::new("/usr/bin/mkfifo")
        .arg(&gate)
        .output()
        .expect("mkfifo runs");
    assert!(made.status.success(), "{}", report(&made));
    let profile = ConfinementProfile {
        workspace: Some(workspace.clone()),
        private_tmpdir: true,
        ..ConfinementProfile::default()
    };
    let limit = Duration::from_mins(1);

    let admitted = adapter.admit(&profile).expect("admitted");
    let first = adapter
        .start(
            &admitted,
            &run_bash(
                "exec 3<> gate || exit 2; printf secret > \"$TMPDIR/mark\" || exit 3; \
                 printf '%s\\n' \"$TMPDIR\" > announce; read -t 60 -r _ <&3; exit 0",
                &[],
            ),
        )
        .await
        .expect("started");
    let announce = workspace.join("announce");
    let deadline = Instant::now() + limit;
    let private = loop {
        if let Ok(text) = fs::read_to_string(&announce)
            && let Some(line) = text.strip_suffix('\n')
        {
            break PathBuf::from(line);
        }
        assert!(Instant::now() < deadline, "the first run never announced");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    let host = std::env::var("TMPDIR").expect("the test process has a TMPDIR");
    assert_ne!(
        private,
        Path::new(host.trim_end_matches('/')),
        "TMPDIR is the host's"
    );
    let host_temp = fs::canonicalize(std::env::temp_dir()).expect("host temporary directory");
    assert_ne!(private, host_temp, "TMPDIR is the host's");
    assert!(
        !private.starts_with(&workspace),
        "TMPDIR is in the workspace"
    );
    assert_eq!(
        fs::read(private.join("mark")).expect("the first run's file"),
        b"secret"
    );

    a_second_run_sees_nothing_of(&adapter, &profile, &private, limit).await;

    assert!(private.is_dir(), "the first run's TMPDIR went early");
    let mut release = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&gate)
        .expect("the gate opens");
    release.write_all(b"\n").expect("release the first run");
    let ended = tokio::time::timeout(limit, adapter.wait(&first))
        .await
        .expect("the first run ends")
        .expect("waited");
    drop(release);
    let output = adapter.output(&first).await.expect("output");
    assert_eq!(
        ended.exit.and_then(|exit| exit.code),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !private.exists(),
        "the first run's TMPDIR outlived its wait"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_path_holding_a_double_quote_is_unrepresentable_before_anything_runs() {
    let adapter = macos();
    let root = tempfile::tempdir().expect("temporary root");
    let workspace = fs::canonicalize(root.path()).expect("canonical");
    fs::create_dir(workspace.join("quo\"te")).expect("a directory with a double quote");
    for profile in [
        ConfinementProfile {
            sealed_paths: vec!["quo\"te".into()],
            ..ConfinementProfile::default()
        },
        ConfinementProfile {
            exec_subpaths: vec!["quo\"te".into()],
            ..ConfinementProfile::default()
        },
    ] {
        let (sink, records) = recording();
        let error = adapter
            .admit(&ConfinementProfile {
                workspace: Some(workspace.clone()),
                audit: Some(sink),
                ..profile
            })
            .expect_err("a double quote in a build path");
        assert_eq!(error.code(), seatbelt::PATH_UNREPRESENTABLE, "{error}");
        let records = records.lock();
        assert_eq!(records.len(), 1, "{records:?}");
        let record = serde_json::to_value(&records[0]).expect("record");
        assert_eq!(record["event"], "profile_refused", "{record}");
        assert_eq!(record["code"], seatbelt::PATH_UNREPRESENTABLE, "{record}");
    }
    assert_eq!(
        fs::read_dir(&workspace).expect("workspace").count(),
        1,
        "something ran in the workspace"
    );
}
