use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::time::Duration;

use b10x_substrate_sdk::{
    AppliedWritableRoot, Client, ExecMeasurement, ExecutionPolicy, ExpectedFileState,
    FileEditInput, FilePatchInput, LinePatchEdit, MAX_WRITABLE_ROOTS, ManagedDaemon,
    MetricsObservation, MetricsResourceKind, PipeFrame, PtyWindow, RefusalClass, SdkError,
    TextMatchPolicy, WritableHostRoot, WritableRoot,
};

#[path = "../../../test-support/readiness.rs"]
mod readiness;
#[path = "../../../test-support/unix_socket.rs"]
mod unix_socket;

fn daemon_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("SUBSTRATE_TEST_DAEMON") {
        return path.into();
    }
    let mut path = std::env::current_exe().expect("current test executable");
    path.pop();
    if path.file_name().is_some_and(|name| name == "deps") {
        path.pop();
    }
    path.push("substrate-daemon");
    path
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One clean-room SDK journey proves ordering across every API family.
async fn managed_external_daemon_serves_the_typed_workspace_journey() {
    let data = unix_socket::tempdir("owned/substrate.sock");
    let root = data.path().join("owned");
    let mut managed = ManagedDaemon::builder()
        .data_dir(&root)
        .deployment("sdk_test")
        .external_binary(daemon_binary())
        .startup_timeout(readiness::DAEMON_READINESS)
        .start()
        .await
        .expect("managed daemon starts");

    let workspace = managed
        .client()
        .workspace()
        .empty()
        .label("test", "sdk")
        .create()
        .await
        .expect("workspace create");
    let written = workspace
        .write_file("hello.txt", b"hello from the SDK")
        .await
        .expect("file write");
    assert_eq!(written.size, 18);
    let read = workspace
        .read_file("hello.txt", 0, 1024)
        .await
        .expect("file read");
    assert_eq!(read.bytes, b"hello from the SDK");

    let replace_operation = ulid::Ulid::generate().to_string();
    let replaced = workspace
        .replace_file(
            "nested/message.txt",
            b"one\ntwo\n",
            ExpectedFileState::Absent,
            true,
            Some(replace_operation.clone()),
        )
        .await
        .expect("guarded v2 replacement");
    let slice = workspace
        .read_file_v2("nested/message.txt", 0, 1024)
        .await
        .expect("digested v2 read");
    assert_eq!(slice.bytes, b"one\ntwo\n");
    assert_eq!(slice.sha256, replaced.after_sha256);
    let edit_operation = ulid::Ulid::generate().to_string();
    let edited = workspace
        .edit_file(
            "nested/message.txt",
            FileEditInput {
                expected_sha256: slice.sha256,
                old_text: "two".to_owned(),
                new_text: "second".to_owned(),
                match_policy: TextMatchPolicy::Exact,
            },
            Some(edit_operation),
        )
        .await
        .expect("guarded text edit");
    let patch_operation = ulid::Ulid::generate().to_string();
    workspace
        .patch_file(
            "nested/message.txt",
            FilePatchInput {
                expected_sha256: edited.after_sha256,
                edits: vec![LinePatchEdit::InsertAfter {
                    line: 2,
                    text: "third\n".to_owned(),
                }],
            },
            Some(patch_operation),
        )
        .await
        .expect("guarded line patch");
    let directory = workspace
        .read_directory("nested", None, 32)
        .await
        .expect("bounded directory page");
    assert_eq!(directory.items.len(), 1);
    let tree = workspace.tree(32, false).await.expect("bounded tree");
    assert!(
        tree.items
            .iter()
            .any(|entry| entry.path == "nested/message.txt")
    );
    let recorded = managed
        .client()
        .operation(&replace_operation)
        .await
        .expect("caller operation id is durable");
    assert_eq!(recorded.id, replace_operation);

    match managed.client().session_capabilities().await {
        Ok(session_capabilities) => {
            assert_eq!(session_capabilities.contract, b10x_substrate_sdk::CONTRACT);
        }
        Err(SdkError::Refusal(refusal)) => {
            assert_eq!(refusal.code, "session.confinement-unavailable");
        }
        Err(error) => panic!("session capabilities lost their named outcome: {error}"),
    }
    let snapshot = managed
        .client()
        .create_reconciliation_snapshot()
        .await
        .expect("snapshot create");
    let snapshot_page = managed
        .client()
        .reconciliation_snapshot_page(&snapshot.id, None, 64)
        .await
        .expect("snapshot page");
    assert_eq!(snapshot_page.snapshot, snapshot.id);

    let Err(SdkError::Refusal(metrics_refusal)) = managed
        .client()
        .metrics(MetricsResourceKind::Exec, "ex_missing")
        .await
    else {
        panic!("metrics for an absent exec must be a named refusal")
    };
    assert_eq!(metrics_refusal.code, "resource.not-found");
    let Err(SdkError::Refusal(stream_refusal)) =
        managed.client().metrics_stream("ex_missing").await
    else {
        panic!("metrics stream handshake must preserve its named refusal")
    };
    // ADR 0042 decision 5: a host that does not publish `metrics.stream` refuses the route before
    // it reads the exec.
    let expected = if managed.client().machine().facts.metrics_stream.is_some() {
        "resource.not-found"
    } else {
        "exec.metrics-unserved"
    };
    assert_eq!(stream_refusal.code, expected);
    // ADR 0036 S12: the process event stream handshake preserves its named refusal too.
    let Err(SdkError::Refusal(process_refusal)) =
        managed.client().process_events("proc_missing", None).await
    else {
        panic!("a process event stream of an absent record must be a named refusal")
    };
    assert_eq!(process_refusal.code, "process.not-found");

    let policy = ExecutionPolicy::builder()
        .timeout(Duration::from_secs(5))
        .cpu_time(Duration::from_secs(1))
        .memory_bytes(64 * 1024 * 1024)
        .processes(16)
        .output_bytes(64 * 1024)
        .build()
        .expect("complete execution policy");
    let operation_id = ulid::Ulid::generate().to_string();
    let result = workspace
        .command("/usr/bin/true")
        .policy(policy)
        .operation_id(&operation_id)
        .start()
        .await;
    let Err(SdkError::Refusal(refusal)) = result else {
        panic!("a daemon without cgroup delegation must refuse dispatch")
    };
    assert_eq!(refusal.class, RefusalClass::Unserved);
    assert_eq!(refusal.code, "exec.sandbox-unavailable");
    assert_eq!(refusal.operation_id.as_deref(), Some(operation_id.as_str()));
    let recorded = managed
        .client()
        .operation(&operation_id)
        .await
        .expect("durable refused operation");
    assert_eq!(
        recorded.refusal.as_ref().map(|value| value.code.as_str()),
        Some("exec.sandbox-unavailable")
    );
    let events = managed.client().events(None, 64).await.expect("event page");
    assert!(
        events
            .events
            .iter()
            .any(|event| event.transition == "workspace.created")
    );
    let destroy_operation = ulid::Ulid::generate().to_string();
    workspace
        .destroy_with_operation_id(Some(destroy_operation.clone()))
        .await
        .expect("workspace destroy");
    assert_eq!(
        managed
            .client()
            .operation(&destroy_operation)
            .await
            .expect("destroy operation is durable")
            .id,
        destroy_operation
    );
    managed.shutdown().await.expect("managed shutdown");

    assert!(root.join("state.db").is_file(), "durable state is retained");
    assert!(
        !root.join("substrate.sock").exists(),
        "owned child removes its socket"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One delegated lifecycle must retain the active process and cgroup identities.
async fn delegated_external_daemon_serves_pty_metrics_and_cleans_the_process_tree() {
    let Some(cgroup_root) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT") else {
        eprintln!("delegated SDK lane absent: SUBSTRATE_VECTORS_CGROUP_ROOT is not set");
        return;
    };
    let data = unix_socket::tempdir("delegated-owned/substrate.sock");
    let root = data.path().join("delegated-owned");
    let mut managed = ManagedDaemon::builder()
        .data_dir(&root)
        .deployment("sdk_delegated_test")
        .external_binary(daemon_binary())
        .startup_timeout(readiness::DAEMON_READINESS)
        .cgroup_root(&cgroup_root)
        .start()
        .await
        .expect("delegated managed daemon starts");
    assert_eq!(managed.client().machine().facts.sessions_pty, Some(true));
    let metrics_supported = managed
        .client()
        .machine()
        .facts
        .exec_resource_usage
        .is_some();

    let workspace = managed
        .client()
        .workspace()
        .empty()
        .create()
        .await
        .expect("workspace create");
    let policy = ExecutionPolicy::builder()
        .timeout(Duration::from_secs(30))
        .cpu_time(Duration::from_secs(5))
        .memory_bytes(64 * 1024 * 1024)
        .processes(16)
        .output_bytes(64 * 1024)
        .build()
        .expect("complete execution policy");
    let mut session_builder = workspace
        .pty_session(
            "/usr/bin/sh",
            PtyWindow {
                columns: 80,
                rows: 24,
            },
        )
        .args([
            "-c",
            "echo $$ >/workspace/child.pid; read line; stty size; printf ':%s\\n' \"$line\"; sleep 600",
        ])
        .policy(policy)
        .lease(Duration::from_secs(20))
        .input_limit_bytes(4096)
        .frame_limit_bytes(4096)
        .queued_frames(16);
    if metrics_supported {
        session_builder = session_builder.measure(ExecMeasurement::ResourceUsage);
    }
    let session = session_builder.start().await.expect("pty session starts");
    let exec_id = session.observation().exec_id.clone();
    let mut channel = session.attach().await.expect("pty attachment");
    channel
        .resize(PtyWindow {
            columns: 100,
            rows: 40,
        })
        .await
        .expect("pty resize");
    channel.write(b"hello\n").await.expect("pty input");

    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(frame) = channel.next_frame().await.expect("pty frame") {
            if let PipeFrame::Output { bytes, .. } = frame {
                output.extend(bytes);
                if String::from_utf8_lossy(&output).contains("40 100")
                    && String::from_utf8_lossy(&output).contains(":hello")
                {
                    break;
                }
            }
        }
    })
    .await
    .expect("resized terminal output deadline");
    assert!(String::from_utf8_lossy(&output).contains(":hello"));

    if metrics_supported {
        let metrics = managed
            .client()
            .metrics(MetricsResourceKind::Exec, &exec_id)
            .await
            .expect("live metrics");
        assert!(matches!(metrics, MetricsObservation::Exec { .. }));
        // ADR 0032: the block I/O counters are present exactly when the snapshot says so, and
        // absent rather than zero on a delegation root without the io controller.
        let block_io = managed
            .client()
            .machine()
            .facts
            .exec_resource_usage
            .expect("metrics are supported")
            .block_io;
        if let MetricsObservation::Exec {
            usage: b10x_substrate_sdk::ExecUsage::Observed(usage),
            ..
        } = &metrics
        {
            assert_eq!(usage.io_read_bytes.is_some(), block_io, "{usage:?}");
            assert_eq!(usage.io_write_bytes.is_some(), block_io, "{usage:?}");
        }
        let mut stream = managed
            .client()
            .metrics_stream(&exec_id)
            .await
            .expect("metrics stream");
        assert!(
            tokio::time::timeout(Duration::from_secs(3), stream.next_sample())
                .await
                .expect("metrics sample deadline")
                .expect("metrics stream read")
                .is_some()
        );
    } else {
        assert!(
            managed
                .client()
                .machine()
                .facts
                .exec_resource_usage
                .is_none()
        );
        let Err(SdkError::Refusal(refusal)) = managed
            .client()
            .metrics(MetricsResourceKind::Exec, &exec_id)
            .await
        else {
            panic!("metrics absence must remain a named refusal")
        };
        assert_eq!(refusal.code, "exec.metrics-not-requested");
    }

    let exec_cgroup = loop {
        let exec = managed
            .client()
            .get_exec(&exec_id)
            .await
            .expect("exec observation");
        if let Some(applied) = &exec.observation().applied {
            break applied
                .cgroup
                .clone()
                .expect("the host driver names the exec cgroup");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };

    let child_pid = loop {
        match workspace.read_file("child.pid", 0, 64).await {
            Ok(file) => {
                break String::from_utf8(file.bytes)
                    .expect("child pid is utf8")
                    .trim()
                    .parse::<i32>()
                    .expect("child pid is numeric");
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
        }
    };
    managed
        .shutdown()
        .await
        .expect("shutdown cleans an active pty tree");
    assert!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(child_pid), None).is_err(),
        "the workload process survived managed-daemon shutdown"
    );
    assert!(
        !PathBuf::from(&cgroup_root).join(exec_cgroup).exists(),
        "this SDK run's exec cgroup survived orderly shutdown"
    );
}

/// ADR 0043 through the SDK: a FIFO in the workspace is counted by the two `_with_omissions`
/// methods, while `tree` and `read_directory` keep their signatures and `tree` still answers
/// `truncated: true` for the omission, as a consumer that reads it as "incomplete" relies on.
#[tokio::test]
async fn managed_listings_answer_the_omitted_entry_count() {
    let data = unix_socket::tempdir("owned/substrate.sock");
    let root = data.path().join("owned");
    let mut managed = ManagedDaemon::builder()
        .data_dir(&root)
        .deployment("sdk_omitted")
        .external_binary(daemon_binary())
        .startup_timeout(readiness::DAEMON_READINESS)
        .start()
        .await
        .expect("managed daemon starts");
    let workspace = managed
        .client()
        .workspace()
        .empty()
        .create()
        .await
        .expect("workspace create");
    workspace
        .replace_file("listing/b.txt", b"b", ExpectedFileState::Absent, true, None)
        .await
        .expect("file write with its parent");
    let fifo = root
        .join("workspaces")
        .join(workspace.id())
        .join("listing/a-fifo");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("run mkfifo");
    assert!(made.success(), "mkfifo {}", fifo.display());

    let page = workspace
        .read_directory_with_omissions("listing", None, 32)
        .await
        .expect("directory page with its count");
    assert_eq!(page.omitted_entries, 1, "the FIFO is counted");
    let names: Vec<&str> = page
        .listing
        .items
        .iter()
        .map(|item| item.name.as_str())
        .collect();
    assert_eq!(names, ["b.txt"]);
    let plain = workspace
        .read_directory("listing", None, 32)
        .await
        .expect("directory page");
    assert_eq!(plain.items, page.listing.items, "same page either way");

    let tree = workspace
        .tree_with_omissions(32, false)
        .await
        .expect("tree with its count");
    assert_eq!(tree.omitted_entries, 1, "the FIFO is counted");
    assert!(tree.listing.truncated, "an omission still sets truncated");
    let plain = workspace.tree(32, false).await.expect("tree");
    assert!(
        plain.truncated,
        "tree() keeps answering truncated for an omission (ADR 0043)"
    );
    assert_eq!(plain.items, tree.listing.items);

    std::fs::remove_file(&fifo).expect("remove the FIFO");
    let tree = workspace
        .tree_with_omissions(32, false)
        .await
        .expect("complete tree");
    assert_eq!(tree.omitted_entries, 0, "no header means nothing omitted");
    assert!(!tree.listing.truncated);
    let page = workspace
        .read_directory_with_omissions("listing", None, 32)
        .await
        .expect("complete page");
    assert_eq!(page.omitted_entries, 0);

    managed.shutdown().await.expect("managed shutdown");
}

#[cfg(feature = "linked-daemon")]
#[tokio::test]
async fn linked_mode_reexecutes_a_separate_process() {
    let data = unix_socket::tempdir("substrate.sock");
    let status = tokio::process::Command::new(env!("CARGO_BIN_EXE_sdk-linked-fixture"))
        .arg(data.path())
        .status()
        .await
        .expect("run linked fixture");
    assert!(status.success(), "linked fixture failed: {status}");
}

/// The name the writable-root cases declare their one host root under.
const WRITABLE_ROOT_NAME: &str = "shared";
/// Where those cases mount it inside the run.
const WRITABLE_ROOT_MOUNT: &str = "/srv/substrate-shared";

/// A shipped daemon started with `--writable-root shared=<root>` beside the flags
/// `ManagedDaemon` passes; `ManagedDaemon` declares no writable root, so the case starts the binary
/// itself and talks to it through the public `Client` only. The declared root is a canonical
/// directory under `CARGO_TARGET_TMPDIR`, apart from the daemon's state and workspaces.
struct DeclaredRootDaemon {
    child: tokio::process::Child,
    client: Client,
    root: PathBuf,
    _data: tempfile::TempDir,
    _fixture: tempfile::TempDir,
}

impl DeclaredRootDaemon {
    async fn start(deployment: &str, cgroup_root: Option<&std::ffi::OsStr>) -> Self {
        let data = unix_socket::tempdir("declared/substrate.sock");
        let owned = data.path().join("declared");
        std::fs::create_dir_all(&owned).expect("daemon data directory");
        std::fs::set_permissions(&owned, std::fs::Permissions::from_mode(0o700))
            .expect("owner-private data directory");
        let fixture = tempfile::Builder::new()
            .prefix("sdk-writable-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .expect("fixture under CARGO_TARGET_TMPDIR");
        let root = std::fs::canonicalize(fixture.path())
            .expect("canonical fixture")
            .join(WRITABLE_ROOT_NAME);
        std::fs::create_dir(&root).expect("declared root directory");
        let socket = owned.join("substrate.sock");
        let log = owned.join("daemon.log");
        let mut command = tokio::process::Command::new(daemon_binary());
        command
            .arg("--socket")
            .arg(&socket)
            .arg("--state")
            .arg(owned.join("state.db"))
            .arg("--workspaces")
            .arg(owned.join("workspaces"))
            .arg("--deployment")
            .arg(deployment)
            .arg("--allow-uid")
            .arg(nix::unistd::geteuid().as_raw().to_string())
            .arg("--bubblewrap")
            .arg("/usr/bin/bwrap")
            .arg("--exit-on-stdin-close")
            .arg("--writable-root")
            .arg(format!("{WRITABLE_ROOT_NAME}={}", root.display()))
            .env_remove("SUBSTRATE_WRITABLE_ROOT")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&log).expect("daemon log"))
            .kill_on_drop(true);
        if let Some(cgroup_root) = cgroup_root {
            command.arg("--cgroup-root").arg(cgroup_root);
        }
        let mut child = command.spawn().expect("spawn the shipped daemon");
        let deadline = tokio::time::Instant::now() + readiness::DAEMON_READINESS;
        let client = loop {
            if let Some(status) = child.try_wait().expect("daemon status") {
                panic!(
                    "daemon exited with {status}: {}",
                    std::fs::read_to_string(&log).unwrap_or_default()
                );
            }
            match Client::builder().unix_socket(&socket).connect().await {
                Ok(client) => break client,
                Err(_) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(error) => panic!(
                    "daemon not ready ({error}): {}",
                    std::fs::read_to_string(&log).unwrap_or_default()
                ),
            }
        };
        Self {
            child,
            client,
            root,
            _data: data,
            _fixture: fixture,
        }
    }

    /// Closes the daemon's stdin, which `--exit-on-stdin-close` answers with an orderly stop, and
    /// waits for it.
    async fn stop(mut self) {
        drop(self.child.stdin.take());
        let status = tokio::time::timeout(Duration::from_secs(30), self.child.wait())
            .await
            .expect("daemon stops after stdin closes")
            .expect("daemon exit status");
        assert!(status.success(), "daemon exited with {status}");
    }
}

fn shared_root() -> WritableRoot {
    WritableRoot::Host(WritableHostRoot {
        host: WRITABLE_ROOT_NAME.to_owned(),
        mount: WRITABLE_ROOT_MOUNT.to_owned(),
    })
}

fn undeclared_root() -> WritableRoot {
    WritableRoot::Host(WritableHostRoot {
        host: "undeclared".to_owned(),
        mount: WRITABLE_ROOT_MOUNT.to_owned(),
    })
}

fn writable_policy() -> ExecutionPolicy {
    ExecutionPolicy::builder()
        .timeout(Duration::from_secs(30))
        .cpu_time(Duration::from_secs(5))
        .memory_bytes(64 * 1024 * 1024)
        .processes(16)
        .output_bytes(64 * 1024)
        .build()
        .expect("complete execution policy")
}

/// `story:sdk-writable-roots`, portable lane: a root the daemon does not declare, and portable the
/// declared root too (no probe proved the bind), is refused with the daemon's own
/// `exec.writable-roots-host-unserved`, naming it; a session start reaches the daemon and keeps its
/// named refusal; and more than `MAX_WRITABLE_ROOTS` are refused by the SDK, so the daemon never
/// records the operation.
#[tokio::test]
async fn writable_roots_are_refused_by_the_daemon_by_name_and_bounded_by_the_sdk() {
    let daemon = DeclaredRootDaemon::start("sdk_writable_refused", None).await;
    assert!(
        daemon
            .client
            .machine()
            .facts
            .exec_writable_roots_host
            .is_none(),
        "without confinement no probe proves the bind"
    );
    let workspace = daemon
        .client
        .workspace()
        .empty()
        .create()
        .await
        .expect("workspace create");
    for (root, name) in [
        (undeclared_root(), "undeclared"),
        (shared_root(), WRITABLE_ROOT_NAME),
    ] {
        let Err(SdkError::Refusal(refusal)) = workspace
            .command("/bin/true")
            .policy(writable_policy())
            .writable_root(root.clone())
            .start()
            .await
        else {
            panic!("an unserved writable root is refused by the daemon")
        };
        assert_eq!(refusal.code, "exec.writable-roots-host-unserved");
        assert_eq!(
            refusal.message,
            format!("Writable host root {name} is not served by this host.")
        );
        let Err(SdkError::Refusal(refusal)) = workspace
            .pipe_session("/bin/true")
            .policy(writable_policy())
            .lease(Duration::from_secs(20))
            .input_limit_bytes(1024)
            .frame_limit_bytes(1024)
            .queued_frames(4)
            .writable_root(root)
            .start()
            .await
        else {
            panic!("a session start keeps the daemon's named refusal")
        };
        // Without confinement the session route refuses before it reads the roots; the delegated
        // case observes a session's refusal of an undeclared root.
        assert_eq!(refusal.code, "session.confinement-unavailable");
    }

    let operation_id = ulid::Ulid::generate().to_string();
    let mut command = workspace
        .command("/bin/true")
        .policy(writable_policy())
        .operation_id(&operation_id);
    for index in 0..=MAX_WRITABLE_ROOTS {
        command = command.writable_root(WritableRoot::Host(WritableHostRoot {
            host: WRITABLE_ROOT_NAME.to_owned(),
            mount: format!("/srv/root-{index}"),
        }));
    }
    let Err(SdkError::Protocol(message)) = command.start().await else {
        panic!("more than MAX_WRITABLE_ROOTS are refused by the SDK")
    };
    assert!(message.contains("writable roots"), "{message}");
    let Err(SdkError::Refusal(refusal)) = daemon.client.operation(&operation_id).await else {
        panic!("the over-bound start never reached the daemon")
    };
    assert_eq!(refusal.code, "resource.not-found");
    assert_eq!(
        std::fs::read_dir(&daemon.root)
            .expect("declared root")
            .count(),
        0,
        "nothing was written to the declared root"
    );
    daemon.stop().await;
}

/// `story:sdk-writable-roots`, delegated lane: an exec and a pipe session each name the declared
/// root, write a file through its mount, and the host reads both files; the exec's applied record
/// names what was bound, and an undeclared root is `exec.writable-roots-host-unserved` from both.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One confined journey: exec, session, and the refusal beside them.
async fn delegated_writable_root_is_written_by_an_exec_and_a_session_and_read_on_the_host() {
    let Some(cgroup_root) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT") else {
        eprintln!(
            "delegated SDK writable-root lane absent: SUBSTRATE_VECTORS_CGROUP_ROOT is not set"
        );
        return;
    };
    let daemon = DeclaredRootDaemon::start("sdk_writable_delegated", Some(&cgroup_root)).await;
    assert_eq!(
        daemon.client.machine().facts.exec_writable_roots_host,
        Some(vec![WRITABLE_ROOT_NAME.to_owned()]),
        "the probe proved the declared bind"
    );
    let workspace = daemon
        .client
        .workspace()
        .empty()
        .create()
        .await
        .expect("workspace create");

    let output = workspace
        .command("/bin/sh")
        .args(["-c", "echo from-exec >/srv/substrate-shared/exec.txt"])
        .policy(writable_policy())
        .writable_root(shared_root())
        .run()
        .await
        .expect("exec with a writable root runs");
    assert_eq!(
        output.exec.exit.as_ref().and_then(|exit| exit.code),
        Some(0),
        "{:?}",
        output.exec
    );
    assert_eq!(
        std::fs::read_to_string(daemon.root.join("exec.txt")).expect("exec write on the host"),
        "from-exec\n"
    );
    let applied = output.exec.applied.as_ref().expect("applied confinement");
    assert_eq!(
        applied.writable_roots,
        vec![AppliedWritableRoot::Host {
            name: WRITABLE_ROOT_NAME.to_owned(),
            host_path: daemon.root.display().to_string(),
            mount: WRITABLE_ROOT_MOUNT.to_owned(),
        }]
    );

    let session = workspace
        .pipe_session("/bin/sh")
        // It waits for its line, so it is still running when the attachment opens.
        .args([
            "-c",
            "read line; echo \"$line\" >/srv/substrate-shared/session.txt",
        ])
        .policy(writable_policy())
        .lease(Duration::from_secs(20))
        .input_limit_bytes(1024)
        .frame_limit_bytes(1024)
        .queued_frames(4)
        .writable_root(shared_root())
        .start()
        .await
        .expect("session with a writable root starts");
    let mut channel = session.attach().await.expect("session attachment");
    channel
        .write(b"from-session\n")
        .await
        .expect("session input");
    let exit = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(frame) = channel.next_frame().await.expect("session frame") {
            if let PipeFrame::Exit { exit, .. } = frame {
                return exit;
            }
        }
        None
    })
    .await
    .expect("session ends");
    assert_eq!(exit.and_then(|exit| exit.code), Some(0));
    assert_eq!(
        std::fs::read_to_string(daemon.root.join("session.txt"))
            .expect("session write on the host"),
        "from-session\n"
    );

    let Err(SdkError::Refusal(refusal)) = workspace
        .command("/bin/true")
        .policy(writable_policy())
        .writable_root(undeclared_root())
        .start()
        .await
    else {
        panic!("an undeclared writable root is refused by the daemon")
    };
    assert_eq!(refusal.code, "exec.writable-roots-host-unserved");
    assert_eq!(
        refusal.message,
        "Writable host root undeclared is not served by this host."
    );
    let Err(SdkError::Refusal(refusal)) = workspace
        .pipe_session("/bin/true")
        .policy(writable_policy())
        .lease(Duration::from_secs(20))
        .input_limit_bytes(1024)
        .frame_limit_bytes(1024)
        .queued_frames(4)
        .writable_root(undeclared_root())
        .start()
        .await
    else {
        panic!("an undeclared writable root is refused for a session")
    };
    assert_eq!(refusal.code, "exec.writable-roots-host-unserved");
    eprintln!("sdk writable root delegated: executed");
    daemon.stop().await;
}
