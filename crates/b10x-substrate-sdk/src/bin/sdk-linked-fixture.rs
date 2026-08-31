#[cfg(feature = "linked-daemon")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if b10x_substrate_sdk::run_daemon_child_if_requested().await? {
        return Ok(());
    }
    let data_dir = std::env::args_os()
        .nth(1)
        .ok_or("the fixture needs a data directory")?;
    let managed = b10x_substrate_sdk::ManagedDaemon::builder()
        .data_dir(data_dir)
        .deployment("sdk_linked_test")
        .linked_current_exe()
        .start()
        .await?;
    let parent_pid = std::process::id();
    let child_pid = managed.process_id().ok_or("managed child has no pid")?;
    assert_ne!(child_pid, parent_pid, "linked mode must re-execute a child");
    let workspace = managed.client().workspace().empty().create().await?;
    assert!(!workspace.id().is_empty());
    let events = managed.client().events(None, 16).await?;
    let principal = events
        .events
        .iter()
        .find(|event| event.transition == "workspace.created")
        .and_then(|event| event.principal.as_deref());
    let expected_principal = format!("pid:{parent_pid}");
    assert_eq!(principal, Some(expected_principal.as_str()));
    managed.shutdown().await?;
    Ok(())
}

#[cfg(not(feature = "linked-daemon"))]
fn main() {}
