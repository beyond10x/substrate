//! A v2 replacement that does not write its file leaves the workspace as it was
//! (story:failed-file-replacement-removes-the-directories-it-created; the first case is the
//! independent verification of story:file-change-result-fits-the-ledger-before-the-file-changes).
//!
//! The rule: a v2 replacement that asks for its parent directories and does not write its file
//! leaves the workspace as it was, whatever the reason the file was not written. The directories
//! the request created are removed again; a directory that was there before stays.
//!
//! The write these cases make fail is one the file system refuses by itself: a name of 256
//! bytes, one more than a directory entry holds.

use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use super::{Harness, mutation};

/// Every path beneath `root` whose last name is `name`.
fn named(root: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name() == name {
                found.push(path.clone());
            }
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                pending.push(path);
            }
        }
    }
    found
}

/// A replacement with `create_parents` whose path holds a name one byte longer than a directory
/// entry can be, as its file name and as its last parent, is not written, and the parents before
/// that name are not left behind.
#[tokio::test(flavor = "multi_thread")]
async fn a_replacement_that_does_not_write_its_file_creates_no_parent_directory() {
    let harness = Harness::open();
    let workspace = harness.workspace_with_seed_file().await;
    let over_long = "n".repeat(256);
    for (index, (first_parent, path)) in [
        ("redesignfirst", format!("redesignfirst/dir/{over_long}")),
        (
            "redesignsecond",
            format!("redesignsecond/{over_long}/main.txt"),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let operation = format!("01JREDESIGNPARENTS0000{index}");
        let (status, answered) = harness
            .call(
                "local:1000",
                Method::PUT,
                &format!("/v2/workspaces/{workspace}/files/{path}"),
                "req_redesign_parents",
                mutation(
                    &operation,
                    json!({
                        "content": { "encoding": "base64", "data": "eA==" },
                        "expected": { "state": "absent" },
                        "create_parents": true
                    }),
                ),
            )
            .await;
        assert_ne!(
            status,
            StatusCode::OK,
            "{operation}: a name over the entry limit is not written: {answered}"
        );
        let left = named(harness.directory.path(), first_parent);
        assert!(
            left.is_empty(),
            "{operation}: answered {status} {} and left {} director{} named {first_parent}",
            answered["error"]["code"],
            left.len(),
            if left.len() == 1 { "y" } else { "ies" }
        );
    }
}

impl Harness {
    /// Every entry of the workspace as the tree route lists it, hidden ones too.
    async fn listing(&self, workspace: &str) -> Value {
        let (status, tree) = self
            .call(
                "local:1000",
                Method::GET,
                &format!("/v2/workspaces/{workspace}/tree?limit_items=100&include_hidden=true"),
                "req_cleanup_tree",
                Body::empty(),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{tree}");
        assert_eq!(tree["result"]["truncated"], false, "{tree}");
        tree["result"]["items"].clone()
    }

    /// Asks for `path` to be created with its parents, holding one byte.
    async fn create_with_parents(
        &self,
        workspace: &str,
        path: &str,
        operation: &str,
    ) -> (StatusCode, Value) {
        self.call(
            "local:1000",
            Method::PUT,
            &format!("/v2/workspaces/{workspace}/files/{path}"),
            "req_cleanup_replace",
            mutation(
                operation,
                json!({
                    "content": { "encoding": "base64", "data": "eA==" },
                    "expected": { "state": "absent" },
                    "create_parents": true
                }),
            ),
        )
        .await
    }
}

/// The rule for each path the file system refuses: the request is answered `workspace.io-failed`
/// (500), the refusal a failed guarded open has always had, and the tree route lists exactly
/// what it listed before the request.
async fn each_refused_path_leaves_the_listing_as_it_was(paths: [(&str, &str); 2]) {
    let harness = Harness::open();
    let workspace = harness.workspace_with_seed_file().await;
    let (status, kept) = harness
        .create_with_parents(&workspace, "kept/held.txt", "01JCLEANUPKEPT000000001")
        .await;
    assert_eq!(status, StatusCode::OK, "{kept}");
    let before = harness.listing(&workspace).await;
    assert_eq!(
        before,
        json!([
            {"path": "kept", "kind": "directory", "size": null},
            {"path": "kept/held.txt", "kind": "file", "size": 1},
            {"path": "main.txt", "kind": "file", "size": 5}
        ])
    );

    for (path, operation) in paths {
        let (status, answered) = harness
            .create_with_parents(&workspace, path, operation)
            .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{answered}");
        assert_eq!(
            answered["error"]["code"], "workspace.io-failed",
            "{answered}"
        );
        assert_eq!(
            harness.listing(&workspace).await,
            before,
            "{path}: answered {status} {} and changed the listing",
            answered["error"]["code"]
        );
    }
}

/// A file name of 256 bytes beneath two parents the request creates, in a directory that was
/// there before and in the workspace's root: both created parents are gone each time, and that
/// directory and the file in it stay.
#[tokio::test(flavor = "multi_thread")]
async fn a_replacement_whose_file_name_is_over_long_removes_the_parents_it_created() {
    let over_long = "n".repeat(256);
    each_refused_path_leaves_the_listing_as_it_was([
        (
            &format!("kept/made/deeper/{over_long}"),
            "01JCLEANUPFILENAME00001",
        ),
        (
            &format!("made/deeper/{over_long}"),
            "01JCLEANUPFILENAME00002",
        ),
    ])
    .await;
}

/// A last parent of 256 bytes: the parents created before it are gone.
#[tokio::test(flavor = "multi_thread")]
async fn a_replacement_whose_last_parent_is_over_long_removes_the_parents_it_created() {
    let over_long = "n".repeat(256);
    each_refused_path_leaves_the_listing_as_it_was([
        (
            &format!("kept/made/{over_long}/file.txt"),
            "01JCLEANUPLASTPARENT001",
        ),
        (
            &format!("made/deeper/{over_long}/file.txt"),
            "01JCLEANUPLASTPARENT002",
        ),
    ])
    .await;
}
