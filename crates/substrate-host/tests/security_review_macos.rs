// Security review conformance cases for the `macos` adapter (ADR 0037, ADR 0109 S1 and S2).
//
// Each case checks one property the adapter states about itself in its module documentation or in
// ADR 0037, through the real adapter on a macOS host. A red case is a property the adapter claims
// and the host does not show.
#![cfg(target_os = "macos")]

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command as HostCommand;
use std::sync::Arc;
use std::time::{Duration, Instant};

use substrate_host::confinement::{
    self, AdapterRequest, ConfinementAdapter, ConfinementProfile, RunState, macos::MacosRequest,
};

const BASH: &str = "/bin/bash";
const LIMIT: Duration = Duration::from_secs(45);

fn macos() -> Arc<dyn ConfinementAdapter> {
    confinement::open(AdapterRequest::Macos(MacosRequest::default()))
        .expect("macos opens on a macOS host whose Seatbelt sandbox enforces")
}

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8 path")
}

fn bash(script: &str, extra: &[&str]) -> confinement::Command {
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

/// Runs `command` to its end, or returns the refusal code of its admission or start.
async fn outcome(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    command: &confinement::Command,
) -> Result<Ended, String> {
    let admitted = adapter
        .admit(profile)
        .map_err(|error| error.code().to_owned())?;
    let run = adapter
        .start(&admitted, command)
        .await
        .map_err(|error| error.code().to_owned())?;
    let ended = tokio::time::timeout(LIMIT, adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    let output = adapter.output(&run).await.expect("output");
    Ok(Ended {
        code: ended.exit.and_then(|exit| exit.code),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn canonical_tempdir() -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().expect("temporary root");
    let path = fs::canonicalize(root.path()).expect("canonical");
    (root, path)
}

// exec_subpaths: "A relative entry is beneath the workspace (`target`); an absolute one must lie in
// the workspace, a declared read-only root, or a system or developer directory" (macos.rs module
// documentation). A relative entry that is a symbolic link out of the workspace must not make the
// link's target readable: either the start is refused, or the run cannot read beneath it.
#[tokio::test(flavor = "multi_thread")]
async fn a_relative_exec_subpath_through_a_symlink_does_not_reach_outside_the_workspace() {
    let adapter = macos();
    let (_root, root) = canonical_tempdir();
    let workspace = root.join("workspace");
    let outside = root.join("outside");
    fs::create_dir(&workspace).expect("workspace");
    fs::create_dir(&outside).expect("outside");
    fs::write(outside.join("marker"), b"outside-marker").expect("marker");
    std::os::unix::fs::symlink(&outside, workspace.join("target")).expect("symlink");
    let profile = ConfinementProfile {
        workspace: Some(workspace.clone()),
        exec_subpaths: vec!["target".into()],
        ..ConfinementProfile::default()
    };
    let marker = outside.join("marker");
    let script = "IFS= read -r line < \"$1\" && printf %s \"$line\"";
    match outcome(&adapter, &profile, &bash(script, &[text(&marker)])).await {
        Err(code) => {
            assert!(
                code == "exec.sandbox-exec-subpath-undeclared"
                    || code == "exec.sandbox-path-unrepresentable",
                "refused by an unexpected name: {code}"
            );
        }
        Ok(ended) => assert!(
            !ended.stdout.contains("outside-marker"),
            "a relative exec subpath that is a symlink made a directory outside the workspace, \
             every declared root and the system directories readable: {}",
            ended.report()
        ),
    }
}

/// Whether the volume holding `directory` folds case.
fn folds_case(directory: &Path) -> bool {
    let probe = directory.join("CaseProbe");
    fs::write(&probe, b"").expect("case probe");
    let folds = directory.join("caseprobe").exists();
    fs::remove_file(&probe).expect("remove case probe");
    folds
}

fn sealed_workspace() -> (tempfile::TempDir, PathBuf) {
    let (root, workspace) = canonical_tempdir();
    let sealed = workspace.join("deep/sealed");
    fs::create_dir_all(&sealed).expect("sealed");
    fs::write(sealed.join("config"), b"original").expect("config");
    (root, workspace)
}

fn sealed_profile(workspace: &Path, sealed: &[&str]) -> ConfinementProfile {
    ConfinementProfile {
        workspace: Some(workspace.to_path_buf()),
        sealed_paths: sealed.iter().map(|path| (*path).to_owned()).collect(),
        exec_subpaths: vec!["/bin".into()],
        ..ConfinementProfile::default()
    }
}

// sealed_paths: "a workspace-relative path stays unwritable". A hard link made inside the run to a
// sealed file, then written, must leave the sealed file unchanged.
#[tokio::test(flavor = "multi_thread")]
async fn a_hard_link_to_a_sealed_file_does_not_write_it() {
    let adapter = macos();
    let (_root, workspace) = sealed_workspace();
    fs::write(workspace.join("beside"), b"beside").expect("beside");
    let profile = sealed_profile(&workspace, &["deep/sealed"]);

    let control = outcome(&adapter, &profile, &bash("/bin/ln beside beside-link", &[]))
        .await
        .expect("the control starts");
    assert_eq!(control.code, Some(0), "control: {}", control.report());

    let attack = outcome(
        &adapter,
        &profile,
        &bash(
            "/bin/ln deep/sealed/config alias && printf tampered > alias",
            &[],
        ),
    )
    .await
    .expect("the case starts");
    assert_eq!(
        fs::read(workspace.join("deep/sealed/config")).expect("config"),
        b"original",
        "a hard link made a sealed file writable: {}",
        attack.report()
    );
}

// sealed_paths on a volume that folds case: the same sealed directory and file, named in another
// case, must stay unwritable, and a sealed path not created yet must not be creatable in another
// case. On a case-sensitive volume the other spelling is another path and the case is vacuous.
#[tokio::test(flavor = "multi_thread")]
async fn a_sealed_path_named_in_another_case_stays_unwritable() {
    let adapter = macos();
    let (_root, workspace) = sealed_workspace();
    if !folds_case(&workspace) {
        eprintln!("the volume is case-sensitive: nothing to check");
        return;
    }
    let profile = sealed_profile(&workspace, &["deep/sealed", "later/sealed"]);
    let mut reports = Vec::new();
    for attack in [
        "printf tampered > DEEP/SEALED/config",
        "printf tampered > deep/Sealed/config",
        "printf new > Deep/Sealed/new",
        "/bin/mkdir -p LATER/SEALED && printf created > LATER/SEALED/file",
    ] {
        let ended = outcome(&adapter, &profile, &bash(attack, &[]))
            .await
            .expect("the case starts");
        reports.push(format!("{attack}: {}", ended.report()));
    }
    let reports = reports.join("\n");
    assert_eq!(
        fs::read(workspace.join("deep/sealed/config")).expect("config"),
        b"original",
        "a sealed file was written through another case:\n{reports}"
    );
    assert!(
        !workspace.join("deep/sealed/new").exists(),
        "a file was created in a sealed directory through another case:\n{reports}"
    );
    assert!(
        !workspace.join("later/sealed/file").exists(),
        "a sealed path not created yet was created through another case:\n{reports}"
    );
}

// private_tmpdir: "removes it before `wait` returns" (macos.rs module documentation). A run that
// leaves a directory it made unreadable in its TMPDIR must not leave the TMPDIR behind.
#[tokio::test(flavor = "multi_thread")]
async fn the_private_tmpdir_is_gone_after_wait_whatever_the_run_left_in_it() {
    let adapter = macos();
    let (_root, workspace) = canonical_tempdir();
    let profile = ConfinementProfile {
        workspace: Some(workspace.clone()),
        exec_subpaths: vec!["/bin".into()],
        private_tmpdir: true,
        ..ConfinementProfile::default()
    };
    let ended = outcome(
        &adapter,
        &profile,
        &bash(
            "/bin/mkdir \"$TMPDIR/kept\" && printf x > \"$TMPDIR/kept/file\" && \
             /bin/chmod 000 \"$TMPDIR/kept\" && printf %s \"$TMPDIR\"",
            &[],
        ),
    )
    .await
    .expect("the case starts");
    assert_eq!(ended.code, Some(0), "{}", ended.report());
    let private = PathBuf::from(ended.stdout.trim());
    assert!(private.is_absolute(), "{}", ended.report());
    let outlived = private.exists();
    if outlived {
        // Leave the runner as it was.
        let _ = HostCommand::new("/bin/chmod")
            .args(["-R", "u+rwx"])
            .arg(&private)
            .status();
        let _ = fs::remove_dir_all(&private);
    }
    assert!(
        !outlived,
        "the private TMPDIR outlived wait: the run made a directory in it unremovable"
    );
}

// ipc_signals_contained, proved by the adapter without a probe: a run must not signal another run
// started under the same profile.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_cannot_signal_another_run_under_the_same_profile() {
    let adapter = macos();
    let (_root, workspace) = canonical_tempdir();
    let profile = ConfinementProfile {
        workspace: Some(workspace.clone()),
        exec_subpaths: vec!["/bin".into()],
        ..ConfinementProfile::default()
    };
    let admitted = adapter.admit(&profile).expect("admitted");
    let first = adapter
        .start(
            &admitted,
            &bash("printf '%s\\n' \"$$\" > first.pid; /bin/sleep 30", &[]),
        )
        .await
        .expect("the first run starts");
    let pid_file = workspace.join("first.pid");
    let deadline = Instant::now() + LIMIT;
    while !fs::read_to_string(&pid_file).is_ok_and(|text| text.ends_with('\n')) {
        assert!(
            Instant::now() < deadline,
            "the first run never wrote its pid"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let second = outcome(
        &adapter,
        &profile,
        &bash("IFS= read -r pid < first.pid && kill -TERM \"$pid\"", &[]),
    )
    .await
    .expect("the second run starts");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let state = adapter.observe(&first).await.expect("observed").state;
    adapter
        .signal(&first, substrate_wire::Signal::Kill)
        .await
        .expect("stop the first run");
    let _ = tokio::time::timeout(LIMIT, adapter.wait(&first)).await;
    assert_eq!(
        state,
        RunState::Running,
        "one run signalled another run: {}",
        second.report()
    );
}

fn accepted_within(listener: &UnixListener, limit: Duration) -> bool {
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

// No network (ADR 0037): no `network*` allow rule, so an AF_UNIX connect to a socket a host
// process listens on, in the run's own writable workspace, is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_unix_socket_connect_from_a_run_is_refused() {
    let adapter = macos();
    let (_root, workspace) = canonical_tempdir();
    let socket = workspace.join("s");
    let listener = UnixListener::bind(&socket).expect("listener");
    listener
        .set_nonblocking(true)
        .expect("non-blocking listener");

    // The control connects from the test process. An unconfined `nc -U` waited for the peer to
    // close a connection the listener only accepts after `nc` returned, and hung the job
    // (story:macos-seatbelt-policy-runs-cargo-test, macos-15 run 40452966164).
    let control = UnixStream::connect(&socket);
    assert!(
        control.is_ok(),
        "the unconfined control did not connect: {control:?}"
    );
    assert!(
        accepted_within(&listener, Duration::from_secs(5)),
        "the unconfined control was not accepted"
    );
    drop(control);

    let profile = ConfinementProfile {
        workspace: Some(workspace.clone()),
        exec_subpaths: vec!["/usr/bin".into()],
        ..ConfinementProfile::default()
    };
    let attack = outcome(
        &adapter,
        &profile,
        // `-w 2` ends a connection the profile wrongly allows, so the case reports it rather
        // than waiting out the run's limit.
        &bash("/usr/bin/nc -w 2 -U \"$1\" < /dev/null", &[text(&socket)]),
    )
    .await
    .expect("the case starts");
    assert!(
        !accepted_within(&listener, Duration::from_secs(1)),
        "a confined run connected to a host AF_UNIX socket: {}",
        attack.report()
    );
}

// The environment is the run's own and the shared shape rule: a loader-injection variable either
// is refused at the start or does not reach the confined program.
#[tokio::test(flavor = "multi_thread")]
async fn a_dyld_variable_does_not_reach_the_confined_program() {
    let adapter = macos();
    let (_root, workspace) = canonical_tempdir();
    let profile = ConfinementProfile {
        workspace: Some(workspace.clone()),
        ..ConfinementProfile::default()
    };
    let mut command = bash("printf %s \"${DYLD_INSERT_LIBRARIES-unset}\"", &[]);
    command.env.insert(
        "DYLD_INSERT_LIBRARIES".into(),
        text(&workspace.join("absent.dylib")).to_owned(),
    );
    match outcome(&adapter, &profile, &command).await {
        Err(code) => assert_eq!(code, "exec.environment-refused"),
        Ok(ended) => assert_eq!(
            ended.stdout,
            "unset",
            "a DYLD_ variable reached the confined program: {}",
            ended.report()
        ),
    }
}
