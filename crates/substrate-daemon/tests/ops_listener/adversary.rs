//! Adversary pass on the operational listener (ADR 0045).

use super::*;

const ADVERSARY_BEARER_FILL: char = 'b';

fn adversary_bearer() -> String {
    format!(
        "dl_substrate_v1_{}",
        String::from(ADVERSARY_BEARER_FILL).repeat(43)
    )
}

/// A development static-bearer TCP daemon on `listen`, its operational listener on `ops`.
fn development_daemon(root: &Path, listen: SocketAddr, ops: &str) -> Command {
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
        .expect("private daemon root");
    let bearer = root.join("bearer");
    if !bearer.exists() {
        std::fs::write(&bearer, adversary_bearer()).expect("bearer");
        std::fs::set_permissions(&bearer, std::fs::Permissions::from_mode(0o600))
            .expect("bearer permissions");
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_substrate-daemon"));
    command
        .arg("--socket")
        .arg(root.join("unused.sock"))
        .arg("--state")
        .arg(root.join("state.sqlite"))
        .arg("--workspaces")
        .arg(root.join("workspaces"))
        .arg("--deployment")
        .arg("ops-adversary")
        .arg("--tcp-listen")
        .arg(listen.to_string())
        .arg("--tcp-bearer-file")
        .arg(&bearer)
        .arg("--tcp-subject")
        .arg("development:adversary")
        .arg("--tcp-actor")
        .arg("adversary")
        .arg("--tcp-private-overlay")
        .arg("--tcp-development-only")
        .arg("--ops-listen")
        .arg(ops)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

/// One plaintext HTTP/1.1 exchange with the development control listener: status and body.
async fn development_exchange(
    address: SocketAddr,
    method: &str,
    path: &str,
    bearer: Option<&str>,
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(address)
        .await
        .expect("development control connection");
    let mut head =
        format!("{method} {path} HTTP/1.1\r\nHost: substrate.test\r\nConnection: close\r\n");
    if let Some(bearer) = bearer {
        write!(head, "Authorization: Bearer {bearer}\r\n").expect("format bearer");
    }
    if let Some(body) = body {
        write!(
            head,
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        )
        .expect("format body headers");
    }
    head.push_str("\r\n");
    if let Some(body) = body {
        head.push_str(body);
    }
    stream
        .write_all(head.as_bytes())
        .await
        .expect("development request");
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut bytes))
        .await
        .expect("development answer timeout")
        .expect("development answer");
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let status = text
        .get(9..12)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("development status: {text}"));
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_owned())
        .unwrap_or_default();
    (status, body)
}

async fn scrape(ops: SocketAddr) -> String {
    let answer = ops_get(ops, "/metrics", "").await;
    assert_eq!(answer.status, 200);
    String::from_utf8(answer.body).expect("UTF-8 exposition")
}

fn refusal_sum(exposition: &str) -> u64 {
    samples(exposition)
        .iter()
        .filter(|line| line.starts_with("substrate_refusals_total{"))
        .map(|line| {
            line.rsplit(' ')
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .expect("refusal count")
        })
        .sum()
}

async fn stop(child: Child) {
    signal(&child, Signal::SIGTERM);
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .expect("daemon shutdown timed out")
        .expect("wait for daemon");
    assert!(output.status.success(), "daemon: {output:?}");
}

/// Waits at most `limit` for a daemon expected to refuse its configuration. `None` when it is
/// still running, so the case can say what it observed instead of hanging.
async fn exit_within(child: &mut Child, limit: Duration) -> Option<(bool, String)> {
    let status = tokio::time::timeout(limit, child.wait()).await.ok()?;
    let status = status.expect("wait for daemon");
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        pipe.read_to_string(&mut stderr).await.expect("stderr");
    }
    Some((status.success(), stderr))
}

/// The help text of `substrate_refusals_total` says it counts failure answers, by named code or
/// `other`. A request the development listener refuses for a missing bearer is counted as a
/// `refused` request and must therefore be counted once among the refusals too.
#[tokio::test]
async fn adversary_every_refused_request_is_counted_among_the_refusals() {
    let root = TempDir::new().expect("temporary root");
    let listen = unused_address();
    let ops = unused_address();
    let mut child = development_daemon(root.path(), listen, &ops.to_string())
        .spawn()
        .expect("spawn development daemon");
    ops_when_ready(&mut child, ops).await;

    let (status, _) = development_exchange(listen, "GET", "/v1/machine", None, None).await;
    assert_eq!(status, 401, "unauthenticated development request");

    let exposition = scrape(ops).await;
    let series = samples(&exposition);
    assert!(
        series.contains(
            &"substrate_http_requests_total{listener=\"tcp-development\",family=\"observe\",outcome=\"refused\"} 1"
        ),
        "{exposition}"
    );
    assert_eq!(
        refusal_sum(&exposition),
        1,
        "a refused answer is in substrate_http_requests_total but in no substrate_refusals_total series"
    );
    stop(child).await;
}

/// ADR 0045: `substrate_dispatch_total` counts driver dispatch after durable admission. One
/// accepted workspace creation is one dispatch; its replay under the same operation id is a
/// second request and no second dispatch. Nothing in the suite observed this counter.
#[tokio::test]
async fn adversary_dispatch_counts_one_admission_and_no_replay() {
    let root = TempDir::new().expect("temporary root");
    let listen = unused_address();
    let ops = unused_address();
    let mut child = development_daemon(root.path(), listen, &ops.to_string())
        .spawn()
        .expect("spawn development daemon");
    ops_when_ready(&mut child, ops).await;
    let bearer = adversary_bearer();
    let body = json!({
        "op": "01JOPSADVERSARYWORKSPACE01",
        "input": {"source": "empty", "labels": {}}
    })
    .to_string();
    for attempt in ["first", "replay"] {
        let (status, answer) =
            development_exchange(listen, "POST", "/v1/workspaces", Some(&bearer), Some(&body))
                .await;
        assert_eq!(status, 201, "{attempt}: {answer}");
    }
    let exposition = scrape(ops).await;
    let series = samples(&exposition);
    for expected in [
        "substrate_http_requests_total{listener=\"tcp-development\",family=\"workspaces\",outcome=\"ok\"} 2",
        "substrate_dispatch_total{family=\"workspaces\",outcome=\"ok\"} 1",
        "substrate_dispatch_duration_seconds_count{family=\"workspaces\"} 1",
    ] {
        assert!(
            series.contains(&expected),
            "missing {expected:?}:\n{exposition}"
        );
    }
    stop(child).await;
}

/// ADR 0045 decision 1: the operational address must differ from every control listener's, and
/// the daemon names that refusal `ops.listener-config-invalid`. `[::ffff:127.0.0.1]:p` is the
/// control listener's `127.0.0.1:p` in its IPv4-mapped spelling: on Linux the two binds conflict.
#[tokio::test]
async fn adversary_the_mapped_spelling_of_the_control_address_is_refused_by_name() {
    let root = TempDir::new().expect("temporary root");
    let listen = unused_address();
    let ops = format!("[::ffff:127.0.0.1]:{}", listen.port());
    let mut child = development_daemon(root.path(), listen, &ops)
        .spawn()
        .expect("spawn development daemon");
    let exited = exit_within(&mut child, Duration::from_secs(10)).await;
    let (succeeded, stderr) =
        exited.expect("the daemon bound both listeners on one address and kept running");
    assert!(!succeeded);
    // The address rule, not a bind failure: a port another process took first would also fail
    // the operational bind under `ops.listener-config-invalid`, and must not pass this case.
    assert!(
        stderr.contains(
            "ops.listener-config-invalid: the operational listener must not share a control listener's address"
        ),
        "refused under another name: {stderr}"
    );
}

/// `--ops-require-fact` refuses a value that "is not a capability fact name". The fact names are
/// a closed set (`CapabilityFacts`), so a well-shaped name outside it can never be published and
/// a daemon started with it is unready for its whole life: it must be refused at startup.
#[tokio::test]
async fn adversary_a_required_fact_outside_the_capability_facts_is_refused_at_startup() {
    let root = TempDir::new().expect("temporary root");
    let listen = unused_address();
    let ops = unused_address();
    let mut child = development_daemon(root.path(), listen, &ops.to_string())
        .arg("--ops-require-fact")
        .arg("exec.no-egres")
        .spawn()
        .expect("spawn development daemon");
    let Some((succeeded, stderr)) = exit_within(&mut child, Duration::from_secs(5)).await else {
        let ready = ops_get(ops, "/readyz", "").await;
        panic!(
            "a misspelt fact was accepted; readiness is {} {:?} for the daemon's life",
            ready.status,
            String::from_utf8_lossy(&ready.body)
        );
    };
    assert!(!succeeded);
    assert!(stderr.contains("ops.listener-config-invalid"), "{stderr}");
}

/// ADR 0045 decision 6: `schema` passes when the version the statement reads equals the version
/// this binary opens. A database a newer binary stamped (a rollback onto a migrated volume) is
/// not one this binary opens, and must not be reported ready after this binary starts on it.
#[tokio::test]
async fn adversary_a_database_stamped_by_a_newer_binary_is_not_ready() {
    let root = TempDir::new().expect("temporary root");
    let listen = unused_address();
    let ops = unused_address();
    let mut child = development_daemon(root.path(), listen, &ops.to_string())
        .spawn()
        .expect("spawn development daemon");
    ops_when_ready(&mut child, ops).await;
    stop(child).await;

    let newer = substrate_store::SCHEMA_VERSION + 1;
    {
        let state =
            rusqlite::Connection::open(root.path().join("state.sqlite")).expect("open state");
        state
            .pragma_update(None, "user_version", newer)
            .expect("stamp a newer schema version");
    }

    let mut child = development_daemon(root.path(), listen, &ops.to_string())
        .spawn()
        .expect("restart development daemon");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().expect("inspect daemon") {
            assert!(!status.success(), "daemon exited cleanly on a newer schema");
            return;
        }
        if TcpStream::connect(ops).await.is_ok() {
            let ready = ops_get(ops, "/readyz", "").await;
            let stamp: i64 = rusqlite::Connection::open(root.path().join("state.sqlite"))
                .and_then(|state| state.query_row("PRAGMA user_version", [], |row| row.get(0)))
                .expect("read stamp");
            assert_ne!(
                ready.status,
                200,
                "a database stamped {newer} was re-stamped {stamp} at open and reported ready: {:?}",
                String::from_utf8_lossy(&ready.body)
            );
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    stop(child).await;
}

/// ADR 0045 decision 1: 8 concurrent connections and a 2 s header deadline. A ninth connection
/// is closed unanswered while eight hold their slots; once the deadline has closed the eight, the
/// listener answers again. Nothing in the suite exercised either bound.
#[tokio::test]
async fn adversary_the_ninth_connection_is_closed_and_the_header_deadline_frees_slots() {
    let root = TempDir::new().expect("temporary root");
    let listen = unused_address();
    let ops = unused_address();
    let mut child = development_daemon(root.path(), listen, &ops.to_string())
        .spawn()
        .expect("spawn development daemon");
    ops_when_ready(&mut child, ops).await;

    // One source: four of the eight are admitted, the source's bound, and four are closed.
    let (idle, bytes, closed) = ops_beyond_held(
        ops,
        &[[127, 0, 0, 1]; 8],
        4,
        [127, 0, 0, 1],
        Duration::from_millis(200),
    )
    .await;
    assert!(closed, "the ninth connection is closed at once, not held");
    assert!(
        bytes.is_empty(),
        "the ninth connection was answered: {}",
        String::from_utf8_lossy(&bytes)
    );

    // The eight sent nothing: the header deadline closes each within 2 s of its accept.
    let started = tokio::time::Instant::now();
    for mut connection in idle {
        let mut rest = Vec::new();
        let read =
            tokio::time::timeout(Duration::from_secs(4), connection.read_to_end(&mut rest)).await;
        assert!(
            read.is_ok(),
            "an idle connection outlived the header deadline"
        );
    }
    assert!(started.elapsed() < Duration::from_secs(4));
    let live = ops_get_after_release(ops, "/livez").await;
    assert_eq!(
        (live.status, live.body.as_slice()),
        (200, b"ok\n".as_slice())
    );
    stop(child).await;
}

/// The production Unix-socket posture: a method the route does not serve is answered by the
/// router's default `405`, which carries no error detail. It is counted as a `refused` request,
/// so it must be counted once among the refusals as well (`other` when no constant names it).
#[tokio::test]
async fn adversary_a_method_not_allowed_on_the_unix_listener_is_counted_among_the_refusals() {
    use std::os::unix::fs::MetadataExt as _;
    let root = TempDir::new().expect("temporary root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private daemon root");
    let uid = std::fs::metadata(root.path()).expect("root metadata").uid();
    let ops = unused_address();
    let socket = root.path().join("control.sock");
    let mut child = Command::new(env!("CARGO_BIN_EXE_substrate-daemon"))
        .arg("--socket")
        .arg(&socket)
        .arg("--state")
        .arg(root.path().join("state.sqlite"))
        .arg("--workspaces")
        .arg(root.path().join("workspaces"))
        .arg("--deployment")
        .arg("ops-adversary")
        .arg("--allow-uid")
        .arg(uid.to_string())
        .arg("--ops-listen")
        .arg(ops.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn Unix daemon");
    ops_when_ready(&mut child, ops).await;

    let mut stream = tokio::net::UnixStream::connect(&socket)
        .await
        .expect("control socket");
    stream
        .write_all(
            b"DELETE /v1/machine HTTP/1.1\r\nHost: substrate.test\r\nConnection: close\r\n\r\n",
        )
        .await
        .expect("control request");
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut bytes))
        .await
        .expect("control answer timeout")
        .expect("control answer");
    assert!(
        bytes.starts_with(b"HTTP/1.1 405 "),
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    let exposition = scrape(ops).await;
    assert!(
        samples(&exposition).contains(
            &"substrate_http_requests_total{listener=\"unix\",family=\"observe\",outcome=\"refused\"} 1"
        ),
        "{exposition}"
    );
    assert_eq!(
        refusal_sum(&exposition),
        1,
        "a refused answer is in substrate_http_requests_total but in no substrate_refusals_total series"
    );
    stop(child).await;
}
