//! Security review of the operational listener (ADR 0045).

use super::*;

use std::os::unix::fs::MetadataExt as _;

/// A Unix-socket daemon (the default posture) with the operational listener on `ops`.
fn unix_daemon_with_ops(root: &Path, ops: &str) -> Command {
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
        .expect("private daemon root");
    let uid = std::fs::metadata(root).expect("root metadata").uid();
    let mut command = Command::new(env!("CARGO_BIN_EXE_substrate-daemon"));
    command
        .arg("--socket")
        .arg(root.join("control.sock"))
        .arg("--state")
        .arg(root.join("state.sqlite"))
        .arg("--workspaces")
        .arg(root.join("workspaces"))
        .arg("--deployment")
        .arg("ops-security")
        .arg("--allow-uid")
        .arg(uid.to_string())
        .arg("--ops-listen")
        .arg(ops)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

/// Runs a daemon that must refuse to start, bounded so a daemon that starts fails the case.
async fn refused_startup(mut command: Command) -> String {
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .expect("the daemon started instead of refusing")
        .expect("run daemon");
    assert!(!output.status.success(), "{output:?}");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Invariant 6 of the review brief: a store a newer binary stamped with a higher schema version
/// is either refused at open or keeps its stamp, so readiness's `schema` check can see it. Opening
/// it and silently lowering the stamp to this binary's version hides the incompatibility.
#[test]
fn ops_security_a_store_stamped_higher_is_not_silently_restamped() {
    let root = TempDir::new().expect("temporary root");
    let path = root.path().join("state.sqlite");
    drop(Store::open(&path).expect("first open"));
    let newer = substrate_store::SCHEMA_VERSION + 1;
    rusqlite::Connection::open(&path)
        .expect("foreign connection")
        .pragma_update(None, "user_version", newer)
        .expect("stamp as a newer binary would");
    let reopened = Store::open(&path);
    let stamp: i64 = rusqlite::Connection::open(&path)
        .expect("read connection")
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("read stamp");
    assert!(
        reopened.is_err() || stamp == newer,
        "opened a store stamped {newer} and rewrote the stamp to {stamp}"
    );
}

/// A store written before the stamp existed (every release up to 0.14.0 left `user_version` 0)
/// still opens and is stamped with this binary's version.
#[test]
fn ops_security_an_unstamped_store_opens_and_is_stamped() {
    let root = TempDir::new().expect("temporary root");
    let path = root.path().join("state.sqlite");
    drop(Store::open(&path).expect("first open"));
    rusqlite::Connection::open(&path)
        .expect("foreign connection")
        .pragma_update(None, "user_version", 0_i64)
        .expect("unstamp as 0.14.0 left it");
    let store = Store::open(&path).expect("an unstamped store opens");
    assert_eq!(
        store
            .schema_version(Duration::from_secs(1))
            .expect("schema version"),
        Some(substrate_store::SCHEMA_VERSION)
    );
}

/// Address rules: port 0, a Unix socket path, an any-address on the control port, and the control
/// address written as an IPv4-mapped IPv6 address are each refused before anything is served, and
/// a refusal of the operational address is named `ops.listener-config-invalid`.
#[tokio::test]
async fn ops_security_address_rules_refuse_every_spelling_of_the_control_address() {
    let root = TempDir::new().expect("temporary root");
    let server = current_identity();
    let (certificate, private_key) = write_identity(root.path(), &server);
    let control = unused_address();
    let port = control.port();
    let tls = |ops: String| {
        let mut command = daemon_command(
            root.path(),
            control,
            &certificate,
            &private_key,
            "https://127.0.0.1:9",
            &certificate,
        );
        command.arg("--ops-listen").arg(ops);
        command
    };
    for ops in [
        "127.0.0.1:0".to_owned(),
        format!("0.0.0.0:{port}"),
        format!("[::]:{port}"),
        format!("[::ffff:127.0.0.1]:{port}"),
    ] {
        let stderr = refused_startup(tls(ops.clone())).await;
        assert!(
            stderr.contains("ops.listener-config-invalid"),
            "{ops}: {stderr}"
        );
    }
    let unix = root.path().join("ops.sock");
    let stderr = refused_startup(unix_daemon_with_ops(
        root.path(),
        unix.to_str().expect("UTF-8 path"),
    ))
    .await;
    assert!(stderr.contains("--ops-listen"), "{stderr}");
    assert!(
        !unix.exists(),
        "a Unix socket was bound for the operational listener"
    );
}

/// The operational listener is TCP only: a Unix socket path is refused by the command line and
/// nothing is bound at it.
#[tokio::test]
async fn ops_security_a_unix_socket_path_is_not_an_operational_address() {
    let root = TempDir::new().expect("temporary root");
    let unix = root.path().join("ops.sock");
    let stderr = refused_startup(unix_daemon_with_ops(
        root.path(),
        unix.to_str().expect("UTF-8 path"),
    ))
    .await;
    assert!(stderr.contains("--ops-listen"), "{stderr}");
    assert!(
        !unix.exists(),
        "a Unix socket was bound for the operational listener"
    );
}

/// Asks `/livez` on `stream`: the bytes answered, empty when the connection was closed unanswered.
/// The read must end within a second: a refused connection is closed at once, not held.
async fn livez_on(mut stream: TcpStream) -> Vec<u8> {
    let _ = stream
        .write_all(b"GET /livez HTTP/1.1\r\nHost: operations.test\r\nConnection: close\r\n\r\n")
        .await;
    let mut bytes = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(1), stream.read_to_end(&mut bytes))
        .await
        .expect("the connection was held instead of answered or closed");
    bytes
}

/// story:operational-health-and-telemetry S3: one source holds at most four of the eight
/// operational connections. A fifth from it is closed unanswered while another source is still
/// served, and the global bound of eight still closes a ninth from a third source.
#[tokio::test]
async fn ops_security_one_source_holds_at_most_four_connections() {
    let root = TempDir::new().expect("temporary root");
    let ops = unused_address();
    let mut child = unix_daemon_with_ops(root.path(), &ops.to_string())
        .spawn()
        .expect("spawn Unix daemon with an operational listener");
    ops_when_ready(&mut child, ops).await;

    let (mut first, fifth, closed) = ops_beyond_held(
        ops,
        &[[127, 0, 0, 1]; 4],
        4,
        [127, 0, 0, 1],
        Duration::from_millis(100),
    )
    .await;
    assert!(
        closed,
        "the connection was held instead of answered or closed"
    );
    assert!(
        fifth.is_empty(),
        "a fifth connection from one source was answered: {}",
        String::from_utf8_lossy(&fifth)
    );
    let other = livez_on(ops_connect_from([127, 0, 0, 2], ops).await).await;
    assert!(
        other.starts_with(b"HTTP/1.1 200 "),
        "another source was not served beside a full one: {}",
        String::from_utf8_lossy(&other)
    );

    let (second, ninth, closed) = ops_beyond_held(
        ops,
        &[[127, 0, 0, 2]; 4],
        4,
        [127, 0, 0, 3],
        Duration::from_millis(100),
    )
    .await;
    assert!(
        closed,
        "the connection was held instead of answered or closed"
    );
    if !ninth.is_empty() {
        // The global bound counts the first source's four too; they must still be held.
        for stream in &mut first {
            let mut byte = [0_u8; 1];
            assert!(
                tokio::time::timeout(Duration::from_millis(20), stream.read(&mut byte))
                    .await
                    .is_err(),
                "a first-source connection reached the header deadline before the ninth was asked"
            );
        }
    }
    assert!(
        ninth.is_empty(),
        "a ninth connection was answered: {}",
        String::from_utf8_lossy(&ninth)
    );
    drop((first, second));

    signal(&child, Signal::SIGTERM);
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .expect("daemon shutdown timed out")
        .expect("wait for daemon");
    assert!(output.status.success(), "daemon: {output:?}");
}

/// The transport bounds of ADR 0045 decision 1 as the shipped daemon applies them: a ninth
/// concurrent connection is closed unanswered while eight are held, the eight are released by the
/// 2 s header deadline, keep-alive is off (two pipelined requests get one answer), and a head
/// over 32 headers or 16 KiB is not served.
#[tokio::test]
async fn ops_security_connection_bounds_hold_on_the_shipped_daemon() {
    let root = TempDir::new().expect("temporary root");
    let ops = unused_address();
    let mut child = unix_daemon_with_ops(root.path(), &ops.to_string())
        .spawn()
        .expect("spawn Unix daemon with an operational listener");
    ops_when_ready(&mut child, ops).await;

    // One source: four of the eight are admitted, the source's bound, and four are closed.
    let (mut held, bytes, _) = ops_beyond_held(
        ops,
        &[[127, 0, 0, 1]; 8],
        4,
        [127, 0, 0, 1],
        Duration::from_millis(200),
    )
    .await;
    assert!(
        bytes.is_empty(),
        "a ninth connection was answered: {}",
        String::from_utf8_lossy(&bytes)
    );

    tokio::time::sleep(Duration::from_millis(2_300)).await;
    for stream in &mut held {
        let mut rest = Vec::new();
        let closed = tokio::time::timeout(Duration::from_secs(1), stream.read_to_end(&mut rest))
            .await
            .is_ok();
        assert!(
            closed,
            "an idle connection outlived the 2 s header deadline"
        );
    }
    drop(held);
    assert_eq!(ops_get_after_release(ops, "/livez").await.status, 200);

    let pipelined = ops_exchange(
        ops,
        b"GET /livez HTTP/1.1\r\nHost: operations.test\r\n\r\nGET /livez HTTP/1.1\r\nHost: operations.test\r\n\r\n",
    )
    .await;
    assert_eq!(pipelined.status, 200);
    assert!(
        !String::from_utf8_lossy(&pipelined.body).contains("HTTP/1.1"),
        "keep-alive answered a second request"
    );

    let mut many = String::from("GET /livez HTTP/1.1\r\nHost: operations.test\r\n");
    for index in 0..40 {
        let _ = write!(many, "X-Filler-{index}: 1\r\n");
    }
    many.push_str("Connection: close\r\n\r\n");
    let large = format!(
        "GET /livez HTTP/1.1\r\nHost: operations.test\r\nX-Filler: {}\r\nConnection: close\r\n\r\n",
        "a".repeat(20 * 1024)
    );
    for request in [many, large] {
        let mut stream = TcpStream::connect(ops).await.expect("oversized head");
        let _ = stream.write_all(request.as_bytes()).await;
        let mut bytes = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut bytes)).await;
        assert!(
            !bytes.starts_with(b"HTTP/1.1 200"),
            "an oversized head was served: {}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(64)])
        );
    }

    signal(&child, Signal::SIGTERM);
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .expect("daemon shutdown timed out")
        .expect("wait for daemon");
    assert!(output.status.success(), "daemon: {output:?}");
}
