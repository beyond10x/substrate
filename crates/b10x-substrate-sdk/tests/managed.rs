use std::path::PathBuf;
use std::time::Duration;

use b10x_substrate_sdk::{ExecutionPolicy, ManagedDaemon, RefusalClass, SdkError};

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
async fn managed_external_daemon_serves_the_typed_workspace_journey() {
    let data = tempfile::tempdir().expect("temporary parent");
    let root = data.path().join("owned");
    let managed = ManagedDaemon::builder()
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

    let policy = ExecutionPolicy::builder()
        .timeout(Duration::from_secs(5))
        .cpu_time(Duration::from_secs(1))
        .memory_bytes(64 * 1024 * 1024)
        .processes(16)
        .output_bytes(64 * 1024)
        .build()
        .expect("complete execution policy");
    let operation_id = ulid::Ulid::new().to_string();
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
    workspace.destroy().await.expect("workspace destroy");
    managed.shutdown().await.expect("managed shutdown");

    assert!(root.join("state.db").is_file(), "durable state is retained");
    assert!(
        !root.join("substrate.sock").exists(),
        "owned child removes its socket"
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
