// Each case attacks the Seatbelt profile from inside it, and pairs the attack with a control that
// shows the same command succeeding where the profile allows it, so a refusal is the profile's.
#![cfg(target_os = "macos")]

use std::fs;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use substrate_host::seatbelt::SeatbeltPolicy;

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
