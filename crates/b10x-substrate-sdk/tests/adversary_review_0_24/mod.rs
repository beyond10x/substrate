//! Adversary pass on the SDK and CLI review of 0.24.0: destroy by id, the recovered Git create,
//! the workspace builder's source selection and the working directory of an exec and a session.
//!
//! Every case starts the shipped daemon once. The cases that lose a reply put a relay in front of
//! it: one Unix socket that forwards each request to the daemon, records it, and on request drops
//! one reply after the daemon answered, or one request before the daemon saw it. The daemon behind
//! it is the real one, so what the ledger stores and replays is the daemon's own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use b10x_substrate_sdk::{
    Client, ManagedDaemon, ReadOnlyRoot, Refusal, RefusalClass, SdkError, WorkspaceBuilder,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};

use super::{daemon_binary, readiness, unix_socket, writable_policy};

const AUTHORITY_HEADER: &str = "x-b10x-workspace-source-authorization";
const AUTHORITY: &str = "adversary-source-authority-of-the-first-selection";
const SECOND_AUTHORITY: &str = "adversary-source-authority-of-the-second-selection";

/// What the relay does with one request instead of relaying its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// The daemon never sees the request.
    LoseRequest,
    /// The daemon answers and the answer is dropped.
    LoseReply,
}

#[derive(Clone, Default)]
struct Relay {
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    faults: Arc<Mutex<Vec<(String, Fault)>>>,
}

impl Relay {
    fn start(listen: &Path, daemon: PathBuf) -> Self {
        let listener = UnixListener::bind(listen).expect("bind the relay");
        let relay = Self::default();
        let shared = relay.clone();
        tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                let relay = shared.clone();
                let daemon = daemon.clone();
                tokio::spawn(async move {
                    let request = read_message(&mut client).await;
                    let fault = relay.take_fault(&request);
                    relay
                        .requests
                        .lock()
                        .expect("relay requests")
                        .push(request.clone());
                    if fault == Some(Fault::LoseRequest) {
                        return;
                    }
                    let mut upstream = UnixStream::connect(&daemon)
                        .await
                        .expect("the relay reaches the daemon");
                    upstream
                        .write_all(&request)
                        .await
                        .expect("the relay forwards the request");
                    let response = read_message(&mut upstream).await;
                    if fault == Some(Fault::LoseReply) {
                        return;
                    }
                    client
                        .write_all(&response)
                        .await
                        .expect("the relay answers");
                });
            }
        });
        relay
    }

    /// The next request whose line starts with `head` meets `fault`.
    fn fault(&self, head: &str, fault: Fault) {
        self.faults
            .lock()
            .expect("relay faults")
            .push((head.to_owned(), fault));
    }

    fn take_fault(&self, request: &[u8]) -> Option<Fault> {
        let mut faults = self.faults.lock().expect("relay faults");
        let index = faults
            .iter()
            .position(|(head, _)| request.starts_with(head.as_bytes()))?;
        Some(faults.remove(index).1)
    }

    /// Every request seen so far, and forgets them.
    fn drain(&self) -> Vec<Vec<u8>> {
        std::mem::take(&mut *self.requests.lock().expect("relay requests"))
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// One HTTP/1.1 message with a `content-length` body, or none.
async fn read_message(stream: &mut UnixStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    let boundary = loop {
        if let Some(index) = find(&bytes, b"\r\n\r\n") {
            break index + 4;
        }
        let read = stream.read(&mut chunk).await.expect("relay read");
        assert_ne!(read, 0, "the peer closed before its head ended");
        bytes.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&bytes[..boundary]).to_ascii_lowercase();
    let length = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .map_or(0, |value| {
            value.trim().parse::<usize>().expect("a content length")
        });
    while bytes.len() < boundary + length {
        let read = stream.read(&mut chunk).await.expect("relay read");
        assert_ne!(read, 0, "the peer closed before its body ended");
        bytes.extend_from_slice(&chunk[..read]);
    }
    bytes
}

fn line(request: &[u8]) -> String {
    let end = find(request, b"\r\n").expect("a request line");
    String::from_utf8_lossy(&request[..end]).into_owned()
}

fn lines(requests: &[Vec<u8>]) -> Vec<String> {
    requests.iter().map(|request| line(request)).collect()
}

fn body(request: &[u8]) -> &[u8] {
    let boundary = find(request, b"\r\n\r\n").expect("a request head") + 4;
    &request[boundary..]
}

fn json(request: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body(request)).expect("a JSON request body")
}

fn names(request: &[u8], secret: &str) -> bool {
    find(request, secret.as_bytes()).is_some()
}

fn carries(request: &[u8], authority: &str) -> bool {
    names(request, &format!("{AUTHORITY_HEADER}: {authority}\r\n"))
}

struct Relayed {
    _data: tempfile::TempDir,
    root: PathBuf,
    managed: ManagedDaemon,
    relay: Relay,
    /// A client whose requests pass the relay.
    client: Client,
}

async fn relayed(deployment: &str) -> Relayed {
    let data = unix_socket::tempdir("adv/substrate.sock");
    let root = data.path().join("adv");
    let managed = ManagedDaemon::builder()
        .data_dir(&root)
        .deployment(deployment)
        .external_binary(daemon_binary())
        .startup_timeout(readiness::DAEMON_READINESS)
        .start()
        .await
        .expect("managed daemon starts");
    let listen = data.path().join("relay.sock");
    let relay = Relay::start(&listen, root.join("substrate.sock"));
    let client = Client::builder()
        .unix_socket(&listen)
        .connect()
        .await
        .expect("the SDK connects through the relay");
    relay.drain();
    Relayed {
        _data: data,
        root,
        managed,
        relay,
        client,
    }
}

fn operation_id() -> String {
    ulid::Ulid::generate().to_string()
}

fn git(builder: WorkspaceBuilder, commit: &str, authority: &str) -> WorkspaceBuilder {
    builder.git(
        "origin",
        "example/repository",
        "refs/heads/main",
        commit,
        1,
        authority,
    )
}

const FIRST_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const SECOND_COMMIT: &str = "89abcdef0123456789abcdef0123456789abcdef";

fn refusal_of<T>(result: Result<T, SdkError>, what: &str) -> Refusal {
    match result {
        Err(SdkError::Refusal(refusal)) => refusal,
        Err(error) => panic!("{what}: expected the daemon's refusal, got {error:?}"),
        Ok(_) => panic!("{what}: expected the daemon's refusal, got a success"),
    }
}

/// The refusal without the operation it names, which differs between two attempts.
fn without_operation(refusal: &Refusal) -> Refusal {
    let mut refusal = refusal.clone();
    refusal.operation_id = None;
    refusal
}

/// Claim A, boundary: `Client::destroy_workspace` destroys a workspace. An id that is not one
/// path component must not reach another route: `DELETE /v1/workspaces/{id}/files/{path}` takes
/// the same empty input and deletes a file. The read the CLI made before this change refused such
/// an id, because the answer was no workspace.
#[tokio::test]
async fn destroy_by_id_does_not_delete_a_file_of_the_workspace() {
    let fixture = relayed("adv_destroy_route").await;
    let direct = fixture.managed.client();
    let workspace = direct
        .workspace()
        .empty()
        .create()
        .await
        .expect("workspace create");
    workspace
        .write_file("keep.txt", b"kept")
        .await
        .expect("file write");

    let target = format!("{}/files/keep.txt", workspace.id());
    // The control: the read that came before the delete refuses this id, so nothing was sent.
    assert!(
        direct.get_workspace(&target).await.is_err(),
        "{target:?} reads as a workspace"
    );
    let operation = operation_id();
    let answer = direct
        .destroy_workspace(&target, Some(operation.clone()))
        .await;

    let kept = workspace.read_file("keep.txt", 0, 16).await;
    let recorded = direct
        .operation(&operation)
        .await
        .map(|operation| operation.kind);
    assert!(
        matches!(&kept, Ok(contents) if contents.bytes == b"kept"),
        "destroy_workspace({target:?}) answered {answer:?}, the ledger records the operation as \
         {recorded:?}, and the workspace's file is now {kept:?}"
    );
}

/// The path every create shares, against the real ledger: an empty create whose reply is lost
/// answers the workspace the daemon made, as a later read observes it, and makes no second one.
#[tokio::test]
async fn a_lost_empty_create_reply_answers_the_workspace_the_daemon_made() {
    let fixture = relayed("adv_create_lost").await;
    let relay = &fixture.relay;
    let operation = operation_id();
    relay.fault("POST /v1/workspaces HTTP/1.1", Fault::LoseReply);
    let recovered = fixture
        .client
        .workspace()
        .empty()
        .label("pass", "adversary")
        .lease(Duration::from_mins(1))
        .operation_id(&operation)
        .create()
        .await
        .expect("the ledger's stored answer recovers the lost reply");
    assert_eq!(
        lines(&relay.drain()),
        [
            "POST /v1/workspaces HTTP/1.1".to_owned(),
            format!("GET /v1/ops/{operation} HTTP/1.1"),
        ]
    );
    let record = fixture
        .client
        .operation(&operation)
        .await
        .expect("the operation is durable");
    assert_eq!(record.resource.as_deref(), Some(recovered.id()));
    let read = fixture
        .client
        .get_workspace(recovered.id())
        .await
        .expect("the recovered workspace exists");
    let mut expected = read.observation().clone();
    expected.observed_at = recovered.observation().observed_at;
    assert_eq!(recovered.observation(), &expected);
    assert_eq!(
        expected.labels.get("pass").map(String::as_str),
        Some("adversary")
    );
}

/// Claim A: a destroy by id whose reply is lost is recovered from the ledger without a second
/// delete; one the daemon never saw is replayed once; a retry under the same operation id answers
/// the stored outcome; and an adopted workspace's directory and files stay on the host.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One daemon answers the recoveries and the adoption, in order.
async fn a_lost_destroy_reply_is_recovered_by_id_and_an_adopted_tree_is_left() {
    let fixture = relayed("adv_destroy_lost").await;
    let relay = &fixture.relay;
    let workspace = fixture
        .client
        .workspace()
        .empty()
        .create()
        .await
        .expect("workspace create");
    let id = workspace.id().to_owned();
    relay.drain();

    let operation = operation_id();
    relay.fault("DELETE /v1/workspaces/", Fault::LoseReply);
    let absent = fixture
        .client
        .destroy_workspace(&id, Some(operation.clone()))
        .await
        .expect("the ledger's stored answer recovers the lost reply");
    assert!(absent);
    assert_eq!(
        lines(&relay.drain()),
        [
            format!("DELETE /v1/workspaces/{id} HTTP/1.1"),
            format!("GET /v1/ops/{operation} HTTP/1.1"),
        ],
        "one delete and one ledger read"
    );
    assert!(
        fixture
            .client
            .destroy_workspace(&id, Some(operation))
            .await
            .expect("a retry answers the stored outcome")
    );
    let gone = refusal_of(fixture.client.get_workspace(&id).await.map(drop), "read");
    assert_eq!(gone.code, "resource.not-found");

    let second = fixture
        .client
        .workspace()
        .empty()
        .create()
        .await
        .expect("workspace create");
    let second = second.id().to_owned();
    relay.drain();
    let operation = operation_id();
    relay.fault("DELETE /v1/workspaces/", Fault::LoseRequest);
    assert!(
        fixture
            .client
            .destroy_workspace(&second, Some(operation.clone()))
            .await
            .expect("an absent ledger row replays the delete")
    );
    let requests = relay.drain();
    assert_eq!(
        lines(&requests),
        [
            format!("DELETE /v1/workspaces/{second} HTTP/1.1"),
            format!("GET /v1/ops/{operation} HTTP/1.1"),
            format!("DELETE /v1/workspaces/{second} HTTP/1.1"),
        ]
    );
    assert_eq!(body(&requests[0]), body(&requests[2]), "the same bytes");

    // Another operation id is a new attempt, and a conflict when the id names another request.
    let conflict = refusal_of(
        fixture
            .client
            .destroy_workspace(&id, Some(operation.clone()))
            .await,
        "one operation id, another workspace",
    );
    assert_eq!(conflict.class, RefusalClass::Conflict, "{conflict:?}");

    let tree = fixture.root.join("workspaces").join("operator_tree");
    std::fs::create_dir_all(&tree).expect("the operator's directory");
    std::fs::write(tree.join("kept.txt"), b"operator bytes").expect("the operator's file");
    let adopted = fixture
        .client
        .adopt_workspace("operator_tree", BTreeMap::new(), None)
        .await
        .expect("workspace adopt");
    assert!(
        fixture
            .client
            .destroy_workspace(adopted.id(), None)
            .await
            .expect("destroy of an adopted workspace by id")
    );
    assert_eq!(
        std::fs::read(tree.join("kept.txt")).expect("the operator's file is left"),
        b"operator bytes"
    );
}

/// Claim B, against the real ledger: a Git create whose reply is lost after the daemon refused
/// surfaces that refusal, field for field (design 18: "Refusals retain their canonical class,
/// code, address, retriable fact and operation id"); one the daemon never saw is replayed once
/// with the same bytes and its authority; the ledger read never carries the authority; and when
/// nothing answers, the error names the operation id and not the authority.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One daemon answers the four recoveries, in order.
async fn a_lost_git_create_reply_surfaces_the_daemons_own_refusal() {
    let fixture = relayed("adv_git_lost").await;
    let relay = &fixture.relay;
    let create = "POST /v1/workspaces HTTP/1.1";

    let direct_operation = operation_id();
    let direct = refusal_of(
        git(fixture.client.workspace(), FIRST_COMMIT, AUTHORITY)
            .operation_id(&direct_operation)
            .create()
            .await
            .map(drop),
        "a Git create on a daemon that declares no Git source",
    );
    assert_eq!(
        direct.operation_id.as_deref(),
        Some(direct_operation.as_str())
    );
    let requests = relay.drain();
    assert_eq!(lines(&requests), [create]);
    assert!(carries(&requests[0], AUTHORITY));

    // The daemon refused and its reply is lost.
    let lost_operation = operation_id();
    relay.fault(create, Fault::LoseReply);
    let recovered = refusal_of(
        git(fixture.client.workspace(), FIRST_COMMIT, AUTHORITY)
            .operation_id(&lost_operation)
            .create()
            .await
            .map(drop),
        "the stored refusal of a Git create whose reply is lost",
    );
    let requests = relay.drain();
    for request in &requests {
        if line(request).starts_with("GET ") {
            assert!(
                !names(request, AUTHORITY),
                "the ledger read carries the source authority"
            );
        }
    }
    assert_eq!(
        without_operation(&recovered),
        without_operation(&direct),
        "the recovered refusal is the daemon's own; requests: {:?}",
        lines(&requests)
    );
    assert_eq!(
        recovered.operation_id.as_deref(),
        Some(lost_operation.as_str()),
        "the recovered refusal names its operation; requests: {:?}",
        lines(&requests)
    );

    // The daemon never saw the request: one replay, the same bytes, the authority again.
    let unseen_operation = operation_id();
    relay.fault(create, Fault::LoseRequest);
    let replayed = refusal_of(
        git(fixture.client.workspace(), FIRST_COMMIT, AUTHORITY)
            .operation_id(&unseen_operation)
            .create()
            .await
            .map(drop),
        "the replay of a Git create the daemon never saw",
    );
    assert_eq!(replayed.code, direct.code);
    assert_eq!(
        replayed.operation_id.as_deref(),
        Some(unseen_operation.as_str())
    );
    let requests = relay.drain();
    assert_eq!(
        lines(&requests),
        [
            create.to_owned(),
            format!("GET /v1/ops/{unseen_operation} HTTP/1.1"),
            create.to_owned(),
        ]
    );
    assert_eq!(body(&requests[0]), body(&requests[2]), "the same bytes");
    assert!(carries(&requests[0], AUTHORITY) && carries(&requests[2], AUTHORITY));
    assert!(!names(&requests[1], AUTHORITY));

    // Neither the create, the ledger nor the replay answers: at most one replay, and an error
    // that names the generated operation id and never the authority.
    relay.fault(create, Fault::LoseRequest);
    relay.fault(create, Fault::LoseRequest);
    let open = git(fixture.client.workspace(), FIRST_COMMIT, AUTHORITY)
        .create()
        .await
        .map(drop);
    let requests = relay.drain();
    assert_eq!(requests.len(), 3, "{:?}", lines(&requests));
    let sent = json(&requests[0])["op"]
        .as_str()
        .expect("an operation id")
        .to_owned();
    assert_eq!(json(&requests[2])["op"], sent.as_str());
    let Err(SdkError::UnknownOperation { operation_id }) = &open else {
        panic!("an open outcome carries its operation id, got {open:?}")
    };
    assert_eq!(operation_id, &sent);
    let error = open.expect_err("open");
    assert!(!format!("{error} {error:?}").contains(AUTHORITY));

    relay.fault(create, Fault::LoseRequest);
    relay.fault("GET /v1/ops/", Fault::LoseRequest);
    let open = git(fixture.client.workspace(), FIRST_COMMIT, AUTHORITY)
        .create()
        .await
        .map(drop);
    let requests = relay.drain();
    assert_eq!(requests.len(), 2, "{:?}", lines(&requests));
    let error = open.expect_err("neither the create nor the ledger answered");
    assert!(
        matches!(&error, SdkError::UnknownOperation { operation_id }
            if json(&requests[0])["op"] == operation_id.as_str()),
        "{error:?}"
    );
    assert!(!format!("{error} {error:?}").contains(AUTHORITY));
}

/// Claim C, on the wire: the selection made last decides the source and the authority, whatever
/// came before it, and an authority selected earlier is never sent.
#[tokio::test]
async fn the_last_source_selection_decides_the_request_and_its_authority() {
    let fixture = relayed("adv_source_last").await;
    let relay = &fixture.relay;

    let twice = git(
        git(fixture.client.workspace(), FIRST_COMMIT, AUTHORITY),
        SECOND_COMMIT,
        SECOND_AUTHORITY,
    );
    refusal_of(twice.create().await.map(drop), "git(a).git(b)");
    let alternating = git(
        git(fixture.client.workspace().empty(), FIRST_COMMIT, AUTHORITY).empty(),
        SECOND_COMMIT,
        SECOND_AUTHORITY,
    );
    refusal_of(
        alternating.create().await.map(drop),
        "empty().git(a).empty().git(b)",
    );
    let requests = relay.drain();
    assert_eq!(requests.len(), 2, "{:?}", lines(&requests));
    for request in &requests {
        assert_eq!(
            json(request)["input"]["source"]["git"]["commit"],
            SECOND_COMMIT
        );
        assert!(carries(request, SECOND_AUTHORITY));
        assert!(!names(request, AUTHORITY), "an earlier authority is sent");
    }

    let emptied = git(
        git(fixture.client.workspace(), FIRST_COMMIT, AUTHORITY),
        SECOND_COMMIT,
        SECOND_AUTHORITY,
    )
    .empty()
    .create()
    .await
    .expect("git(a).git(b).empty() creates an empty workspace");
    let requests = relay.drain();
    assert_eq!(requests.len(), 1);
    assert_eq!(json(&requests[0])["input"]["source"], "empty");
    assert!(
        !names(&requests[0], AUTHORITY_HEADER),
        "an empty source carries a source authority"
    );
    emptied.destroy().await.expect("workspace destroy");
}

/// Claim D: the lexical rule's boundaries are refused by both builders before anything is sent,
/// the last `workdir(..)` decides, a path at the byte bound reaches the daemon, and the daemon's
/// `exec.workdir-unserved` survives a lost reply field for field.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One daemon answers the boundaries, the parity and the recovery.
async fn a_working_directory_keeps_its_rule_its_last_selection_and_its_refusal() {
    let fixture = relayed("adv_workdir").await;
    let relay = &fixture.relay;
    let workspace = fixture
        .client
        .workspace()
        .empty()
        .create()
        .await
        .expect("workspace create");
    relay.drain();
    let command = || workspace.command("/usr/bin/true").policy(writable_policy());
    let session = || {
        workspace
            .pipe_session("/usr/bin/true")
            .policy(writable_policy())
            .lease(Duration::from_secs(20))
            .input_limit_bytes(1024)
            .frame_limit_bytes(1024)
            .queued_frames(4)
    };

    let longest = format!(
        "/workspace/{}",
        "d".repeat(MAX_WORKDIR_BYTES - "/workspace/".len())
    );
    let too_long = format!("{longest}d");
    let refused = [
        "",
        ".",
        "/",
        "/abs",
        "a/../b",
        "/workspace/a/../b",
        "/workspace/./a",
        "/workspace//a",
        "/workspace/",
        "/workspace/a/",
        "/workspace/a\0b",
        "/workspacex",
        "/runtime",
        "/scratch",
        "/srv/tool",
        too_long.as_str(),
    ];
    for workdir in refused {
        let exec = command().workdir(workdir).start().await.map(drop);
        assert!(
            matches!(exec, Err(SdkError::Protocol(_))),
            "an exec in {workdir:?}: {exec:?}"
        );
        let started = session().workdir(workdir).start().await.map(drop);
        assert!(
            matches!(started, Err(SdkError::Protocol(_))),
            "a session in {workdir:?}: {started:?}"
        );
    }
    assert!(
        relay.drain().is_empty(),
        "a refused working directory reached the daemon"
    );

    // Selected twice: the last one, and nothing of the first.
    let direct_operation = operation_id();
    let direct = refusal_of(
        command()
            .workdir("/workspace/first")
            .workdir("/workspace/second")
            .operation_id(&direct_operation)
            .start()
            .await
            .map(drop),
        "an exec with a working directory on a host without the fact",
    );
    assert_eq!(direct.code, "exec.workdir-unserved");
    assert_eq!(direct.class, RefusalClass::Unserved);
    assert_eq!(direct.address.as_deref(), Some("workdir"));
    let requests = relay.drain();
    assert_eq!(requests.len(), 1);
    assert_eq!(json(&requests[0])["input"]["workdir"], "/workspace/second");
    assert!(!names(&requests[0], "/workspace/first"));

    // At the byte bound and beneath a mount the start places itself: the daemon's answer.
    let bound = refusal_of(
        command().workdir(&longest).start().await.map(drop),
        "a working directory of exactly the byte bound",
    );
    assert_eq!(bound.code, "exec.workdir-unserved");
    let beneath_root = refusal_of(
        command()
            .read_only_root(ReadOnlyRoot {
                host_path: "/srv/tool".to_owned(),
                mount: "/srv/tool".to_owned(),
            })
            .workdir("/srv/tool/bin")
            .start()
            .await
            .map(drop),
        "a working directory beneath a declared root",
    );
    assert_ne!(
        beneath_root.code, "exec.workdir-outside-mounts",
        "{beneath_root:?}"
    );
    relay.drain();

    // The daemon refused and its reply is lost: the stored refusal, field for field.
    let lost_operation = operation_id();
    relay.fault("POST /v1/execs HTTP/1.1", Fault::LoseReply);
    let recovered = refusal_of(
        command()
            .workdir("/workspace/second")
            .operation_id(&lost_operation)
            .start()
            .await
            .map(drop),
        "the stored refusal of an exec start whose reply is lost",
    );
    let requests = relay.drain();
    assert_eq!(
        without_operation(&recovered),
        without_operation(&direct),
        "the recovered refusal is the daemon's own; requests: {:?}",
        lines(&requests)
    );
    assert_eq!(
        recovered.operation_id.as_deref(),
        Some(lost_operation.as_str()),
        "the recovered refusal names its operation; requests: {:?}",
        lines(&requests)
    );
}

/// `substrate_wire::MAX_WORKDIR_BYTES`, which the SDK does not export: design 26 § 1 states it.
const MAX_WORKDIR_BYTES: usize = 4096;
