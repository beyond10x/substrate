use std::path::PathBuf;
use std::time::Duration;

use b10x_substrate_sdk::{
    ExecMeasurement, ExecutionPolicy, ExpectedFileState, FileEditInput, FilePatchInput,
    LinePatchEdit, ManagedDaemon, MetricsObservation, MetricsResourceKind, PipeFrame, PtyWindow,
    RefusalClass, SdkError, TextMatchPolicy,
};

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
    let data = tempfile::tempdir().expect("temporary parent");
    let root = data.path().join("owned");
    let mut managed = ManagedDaemon::builder()
        .data_dir(&root)
        .deployment("sdk_test")
        .external_binary(daemon_binary())
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
    assert_eq!(stream_refusal.code, "resource.not-found");

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
    let data = tempfile::tempdir().expect("temporary parent");
    let root = data.path().join("delegated-owned");
    let mut managed = ManagedDaemon::builder()
        .data_dir(&root)
        .deployment("sdk_delegated_test")
        .external_binary(daemon_binary())
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
            break applied.cgroup.clone();
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

#[cfg(feature = "linked-daemon")]
#[tokio::test]
async fn linked_mode_reexecutes_a_separate_process() {
    let data = tempfile::tempdir().expect("temporary parent");
    let status = tokio::process::Command::new(env!("CARGO_BIN_EXE_sdk-linked-fixture"))
        .arg(data.path())
        .status()
        .await
        .expect("run linked fixture");
    assert!(status.success(), "linked fixture failed: {status}");
}
