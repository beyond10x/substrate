//! Adversary pass on the SDK and CLI review of 0.24.0: `workspace destroy` without the read that
//! came before the delete. Every case spawns the shipped daemon and the shipped CLI.

use super::{cli, fixture, ok_result, stderr_json};

/// `workspace destroy <id>` destroys a workspace. The word after `destroy` is sent as the path of
/// the delete, and `DELETE /v1/workspaces/{id}/files/{path}` takes the same empty input: a word
/// that is not one workspace id must not delete a file. Before this change the command read the
/// workspace first, and a word that named no workspace ended there.
#[tokio::test(flavor = "multi_thread")]
async fn cli_workspace_destroy_does_not_delete_a_file_of_the_workspace() {
    let fixture = fixture().await;
    let socket = Some(fixture.socket.as_path());
    let workspace = fixture
        .daemon
        .client()
        .workspace()
        .empty()
        .create()
        .await
        .expect("workspace create");
    workspace
        .write_file("keep.txt", b"kept")
        .await
        .expect("file write");

    let word = format!("{}/files/keep.txt", workspace.id());
    // The control: the read that came before the delete, which `workspace show` still makes,
    // refuses this word.
    let shown = cli(socket, &["--output", "json", "workspace", "show", &word]).await;
    assert_ne!(shown.status.code(), Some(0), "{word} reads as a workspace");
    let output = cli(socket, &["--output", "json", "workspace", "destroy", &word]).await;

    let kept = workspace.read_file("keep.txt", 0, 16).await;
    assert!(
        matches!(&kept, Ok(contents) if contents.bytes == b"kept"),
        "`workspace destroy {word}` exited {:?} with stdout {:?} and stderr {:?}; the file is now \
         {kept:?}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_ne!(output.status.code(), Some(0), "no workspace was destroyed");
    assert!(output.stdout.is_empty());
}

/// One operation id names one request: the same `--operation-id` with another workspace is the
/// daemon's conflict and leaves that workspace, and the first destroy's human line names the id.
#[tokio::test(flavor = "multi_thread")]
async fn cli_destroy_under_one_operation_id_destroys_one_workspace() {
    const DESTROY: &str = "01JCLADVERSARYDESTROY00000";
    let fixture = fixture().await;
    let socket = Some(fixture.socket.as_path());
    let mut ids = Vec::new();
    for _ in 0..2 {
        let created = ok_result(&cli(socket, &["--output", "json", "workspace", "create"]).await);
        ids.push(
            created["workspace"]
                .as_str()
                .expect("workspace id")
                .to_owned(),
        );
    }

    let first = cli(
        socket,
        &["workspace", "destroy", &ids[0], "--operation-id", DESTROY],
    )
    .await;
    assert_eq!(first.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&first.stdout),
        format!("workspace {} absent\n", ids[0])
    );

    let second = cli(
        socket,
        &[
            "--output",
            "json",
            "workspace",
            "destroy",
            &ids[1],
            "--operation-id",
            DESTROY,
        ],
    )
    .await;
    assert_ne!(second.status.code(), Some(0));
    assert!(second.stdout.is_empty());
    let error = stderr_json(&second);
    assert_eq!(error["error"]["code"], "refused", "{error}");
    assert_eq!(error["error"]["data"]["class"], "conflict", "{error}");
    let shown = ok_result(&cli(socket, &["--output", "json", "workspace", "show", &ids[1]]).await);
    assert_eq!(shown["workspace"], ids[1].as_str());
}

/// A workspace that never existed: the delete now reaches the daemon, which names the absence
/// under the operation id, and a retry prints the same refusal.
#[tokio::test(flavor = "multi_thread")]
async fn cli_destroy_of_a_workspace_that_never_existed_is_the_named_absence_both_times() {
    const DESTROY: &str = "01JCLADVERSARYNEVERWAS0000";
    let fixture = fixture().await;
    let socket = Some(fixture.socket.as_path());
    let destroy = [
        "--output",
        "json",
        "workspace",
        "destroy",
        "ws_01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "--operation-id",
        DESTROY,
    ];
    let first = cli(socket, &destroy).await;
    let second = cli(socket, &destroy).await;
    for output in [&first, &second] {
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert_eq!(
            stderr_json(output)["error"]["data"]["code"],
            "resource.not-found"
        );
    }
    assert_eq!(
        stderr_json(&first)["error"]["data"],
        stderr_json(&second)["error"]["data"],
        "a retried refusal prints the stored refusal"
    );
}

/// Two at once under one `--operation-id` (`website/docs/concepts/operations.md`, *Retry
/// identity*: "A stored answer is replayed; a pending operation remains something to reconcile").
/// Each run prints the result or is told the outcome is still open, retriable, under its
/// operation id; at least one prints the result; and a later retry prints that same result.
#[tokio::test(flavor = "multi_thread")]
async fn cli_two_destroys_at_once_under_one_operation_id_end_in_one_stored_result() {
    const DESTROY: &str = "01JCLADVERSARYATONCE000000";
    let fixture = fixture().await;
    let socket = Some(fixture.socket.as_path());
    let created = ok_result(&cli(socket, &["--output", "json", "workspace", "create"]).await);
    let id = created["workspace"]
        .as_str()
        .expect("workspace id")
        .to_owned();
    let destroy = [
        "--output",
        "json",
        "workspace",
        "destroy",
        &id,
        "--operation-id",
        DESTROY,
    ];
    let (first, second) = tokio::join!(cli(socket, &destroy), cli(socket, &destroy));
    let mut printed = Vec::new();
    for output in [&first, &second] {
        if output.status.code() == Some(0) {
            assert_eq!(ok_result(output)["observation"]["absent"], true);
            printed.push(output.stdout.clone());
        } else {
            assert!(output.stdout.is_empty());
            let error = stderr_json(output);
            let data = &error["error"]["data"];
            assert_eq!(data["code"], "operation.outcome-unknown", "{error}");
            assert_eq!(data["retriable"], true, "{error}");
            assert_eq!(data["operation"], DESTROY, "{error}");
        }
    }
    assert!(!printed.is_empty(), "neither run printed the result");
    let later = cli(socket, &destroy).await;
    assert_eq!(later.status.code(), Some(0));
    for stdout in &printed {
        assert_eq!(
            &later.stdout, stdout,
            "a later retry prints the same result"
        );
    }
}
