//! Design 34 § 7.2: every `fs` operation, framed exactly as the keeper receives it, against a
//! temporary single-workspace root with the guarded `openat2` I/O.

#![cfg(target_os = "linux")]

use std::io::Cursor;
use std::os::unix::fs::symlink;
use std::path::Path;

use base64::Engine as _;
use substrate_executor::message::{
    PROTOCOL_FAILURE, Request, Response, read_response, write_message,
};
use substrate_executor::serve::{open_workspace, serve};
use substrate_wire::{
    Base64Content, Base64Encoding, DigestedFileSlice, ExpectedFileState, FileAbsence,
    FileEditInput, FileMode, FileMutationResult, FileObservation, FilePatchInput, FileReadQuery,
    FileReadResult, FileReplaceInput, LinePatchEdit, TextMatchPolicy, WorkspaceTree,
    WorkspaceTreeQuery,
};

const WORKSPACE: &str = "ws_01";

fn call(root: &Path, request: &Request, content: &[u8]) -> Response {
    let mut input = Vec::new();
    request
        .write_to(&mut input, content)
        .expect("encode request");
    call_raw(root, &input)
}

fn call_raw(root: &Path, input: &[u8]) -> Response {
    let mut output = Vec::new();
    serve(&mut Cursor::new(input), &mut output, || {
        open_workspace(root)
    })
    .expect("serve");
    read_response(&mut Cursor::new(&output)).expect("decode response")
}

fn ok<T: serde::de::DeserializeOwned>(response: Response) -> (T, u64) {
    match response {
        Response::Ok {
            value,
            omitted_entries,
        } => (
            serde_json::from_value(value).expect("typed observation"),
            omitted_entries,
        ),
        Response::Error(error) => panic!("expected ok, got {error:?}"),
    }
}

fn error_code(response: Response) -> String {
    match response {
        Response::Error(error) => error.code,
        Response::Ok { value, .. } => panic!("expected an error, got {value}"),
    }
}

fn file_query(limit: u64) -> FileReadQuery {
    FileReadQuery {
        mode: FileMode::File,
        offset: Some(0),
        limit_bytes: Some(limit),
        cursor: None,
        limit_items: None,
    }
}

/// `sha256sum` of `one\ntwo\n`.
const ONE_TWO_SHA256: &str = "c3f9c8c283a2b1f2f1896f27a01cbe3cddc0c9d93f752e4639035a0f5b36f6e8";

fn openat2_served(root: &Path) -> bool {
    let (value, _) = ok::<serde_json::Value>(call(root, &Request::Check {}, b""));
    value["openat2_beneath"].as_bool().expect("bool")
}

#[test]
#[allow(clippy::too_many_lines)] // One workspace walked through every operation in order.
fn every_fs_operation_is_served_beneath_the_root() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    if !openat2_served(root) {
        eprintln!(
            "openat2 is unavailable on this kernel; the guarded operations are not exercised"
        );
        return;
    }

    let (written, _) = ok::<FileObservation>(call(
        root,
        &Request::Write {
            workspace: WORKSPACE.to_owned(),
            path: "notes.txt".to_owned(),
        },
        b"one\ntwo\n",
    ));
    assert!(written.atomic_replacement);
    assert_eq!(written.workspace, WORKSPACE);
    assert_eq!(
        std::fs::read(root.join("notes.txt")).expect("file"),
        b"one\ntwo\n"
    );

    let (read, _) = ok::<FileReadResult>(call(
        root,
        &Request::Read {
            workspace: WORKSPACE.to_owned(),
            path: "notes.txt".to_owned(),
            query: file_query(64),
        },
        b"",
    ));
    let FileReadResult::File(slice) = read else {
        panic!("a file read answers a slice");
    };
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(slice.content.data)
            .expect("base64"),
        b"one\ntwo\n"
    );

    std::fs::create_dir(root.join("dir")).expect("dir");
    std::fs::write(root.join("dir/inner.txt"), b"inner").expect("inner");
    let fifo = std::ffi::CString::new(root.join("dir/pipe").to_str().expect("utf-8")).expect("c");
    // SAFETY: the path is a valid NUL-terminated string inside the temporary root.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let (listing, omitted) = ok::<FileReadResult>(call(
        root,
        &Request::Read {
            workspace: WORKSPACE.to_owned(),
            path: "dir".to_owned(),
            query: FileReadQuery {
                mode: FileMode::Directory,
                offset: None,
                limit_bytes: None,
                cursor: None,
                limit_items: Some(10),
            },
        },
        b"",
    ));
    let FileReadResult::Directory(page) = listing else {
        panic!("a directory read answers a page");
    };
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].name, "inner.txt");
    assert_eq!(omitted, 1, "the FIFO is counted, not listed (ADR 0043)");

    let (digested, _) = ok::<DigestedFileSlice>(call(
        root,
        &Request::ReadV2 {
            workspace: WORKSPACE.to_owned(),
            path: "notes.txt".to_owned(),
            query: file_query(3),
        },
        b"",
    ));
    assert_eq!(digested.size, 8);
    assert_eq!(digested.returned_bytes, 3);
    let digest = digested.sha256.clone();
    assert_eq!(digest, ONE_TWO_SHA256);

    let (tree, omitted) = ok::<WorkspaceTree>(call(
        root,
        &Request::TreeV2 {
            workspace: WORKSPACE.to_owned(),
            query: WorkspaceTreeQuery {
                limit_items: 10,
                include_hidden: false,
            },
        },
        b"",
    ));
    let paths: Vec<&str> = tree.items.iter().map(|item| item.path.as_str()).collect();
    assert_eq!(paths, ["dir", "dir/inner.txt", "notes.txt"]);
    assert_eq!(omitted, 1);

    let (edited, _) = ok::<FileMutationResult>(call(
        root,
        &Request::EditV2 {
            workspace: WORKSPACE.to_owned(),
            path: "notes.txt".to_owned(),
            input: FileEditInput {
                expected_sha256: digest,
                old_text: "two".to_owned(),
                new_text: "$(HOME)".to_owned(),
                match_policy: TextMatchPolicy::Exact,
            },
        },
        b"",
    ));
    assert_eq!(
        std::fs::read(root.join("notes.txt")).expect("file"),
        b"one\n$(HOME)\n"
    );

    let (patched, _) = ok::<FileMutationResult>(call(
        root,
        &Request::PatchV2 {
            workspace: WORKSPACE.to_owned(),
            path: "notes.txt".to_owned(),
            input: FilePatchInput {
                expected_sha256: edited.after_sha256,
                edits: vec![LinePatchEdit::InsertBefore {
                    line: 1,
                    text: "zero".to_owned(),
                }],
            },
        },
        b"",
    ));
    assert_eq!(
        std::fs::read(root.join("notes.txt")).expect("file"),
        b"zero\none\n$(HOME)\n"
    );

    let stale = call(
        root,
        &Request::ReplaceV2 {
            workspace: WORKSPACE.to_owned(),
            path: "notes.txt".to_owned(),
            input: FileReplaceInput {
                expected: ExpectedFileState::Absent,
                content: Base64Content {
                    encoding: Base64Encoding::Base64,
                    data: "eA==".to_owned(),
                },
                create_parents: false,
            },
        },
        b"",
    );
    assert_eq!(error_code(stale), "workspace.stale-content");
    let (_replaced, _) = ok::<FileMutationResult>(call(
        root,
        &Request::ReplaceV2 {
            workspace: WORKSPACE.to_owned(),
            path: "new/deep/file.txt".to_owned(),
            input: FileReplaceInput {
                expected: ExpectedFileState::Absent,
                content: Base64Content {
                    encoding: Base64Encoding::Base64,
                    data: "eA==".to_owned(),
                },
                create_parents: true,
            },
        },
        b"",
    ));
    assert_eq!(
        std::fs::read(root.join("new/deep/file.txt")).expect("file"),
        b"x"
    );

    let (stat, _) = ok::<serde_json::Value>(call(
        root,
        &Request::Stat {
            workspace: WORKSPACE.to_owned(),
            path: "notes.txt".to_owned(),
        },
        b"",
    ));
    assert_eq!(stat["size"], 17);
    assert_eq!(stat["sha256"], patched.after_sha256.as_str());

    let (absence, _) = ok::<FileAbsence>(call(
        root,
        &Request::Delete {
            workspace: WORKSPACE.to_owned(),
            path: "notes.txt".to_owned(),
        },
        b"",
    ));
    assert!(absence.absent);
    assert!(!root.join("notes.txt").exists());
    let (stat, _) = ok::<serde_json::Value>(call(
        root,
        &Request::Stat {
            workspace: WORKSPACE.to_owned(),
            path: "notes.txt".to_owned(),
        },
        b"",
    ));
    assert_eq!(
        stat,
        serde_json::json!({"path": "notes.txt", "absent": true})
    );
}

#[test]
fn a_symlink_out_of_the_root_is_refused_for_every_operation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside");
    let root = directory.path();
    if !openat2_served(root) {
        eprintln!(
            "openat2 is unavailable on this kernel; the guarded operations are not exercised"
        );
        return;
    }
    std::fs::write(outside.path().join("target.txt"), b"outside").expect("outside file");
    symlink(outside.path(), root.join("out")).expect("directory link");
    symlink(outside.path().join("target.txt"), root.join("file-link")).expect("file link");
    let replace = FileReplaceInput {
        expected: ExpectedFileState::Absent,
        content: Base64Content {
            encoding: Base64Encoding::Base64,
            data: "eA==".to_owned(),
        },
        create_parents: true,
    };
    let workspace = WORKSPACE.to_owned();
    let cases = [
        (
            Request::Read {
                workspace: workspace.clone(),
                path: "out/target.txt".to_owned(),
                query: file_query(16),
            },
            Vec::new(),
        ),
        (
            Request::Read {
                workspace: workspace.clone(),
                path: "file-link".to_owned(),
                query: file_query(16),
            },
            Vec::new(),
        ),
        (
            Request::ReadV2 {
                workspace: workspace.clone(),
                path: "out/target.txt".to_owned(),
                query: file_query(16),
            },
            Vec::new(),
        ),
        (
            Request::Write {
                workspace: workspace.clone(),
                path: "out/new.txt".to_owned(),
            },
            b"escaped".to_vec(),
        ),
        (
            Request::Delete {
                workspace: workspace.clone(),
                path: "out/target.txt".to_owned(),
            },
            Vec::new(),
        ),
        (
            Request::ReplaceV2 {
                workspace: workspace.clone(),
                path: "out/new.txt".to_owned(),
                input: replace,
            },
            Vec::new(),
        ),
        (
            Request::Stat {
                workspace: workspace.clone(),
                path: "out/target.txt".to_owned(),
            },
            Vec::new(),
        ),
        (
            Request::Read {
                workspace,
                path: "../escape".to_owned(),
                query: file_query(16),
            },
            Vec::new(),
        ),
    ];
    for (request, content) in cases {
        assert_eq!(
            error_code(call(root, &request, &content)),
            "workspace.path-escape",
            "{request:?}"
        );
    }
    assert_eq!(
        std::fs::read(outside.path().join("target.txt")).expect("outside"),
        b"outside"
    );
    assert!(!outside.path().join("new.txt").exists());
}

#[test]
fn a_malformed_request_is_answered_as_a_protocol_failure() {
    let directory = tempfile::tempdir().expect("tempdir");
    let mut input = Vec::new();
    write_message(&mut input, br#"{"op":"run","argv":["sh"]}"#, b"").expect("encode");
    assert_eq!(
        error_code(call_raw(directory.path(), &input)),
        PROTOCOL_FAILURE
    );
    assert_eq!(
        error_code(call_raw(directory.path(), b"\0\0")),
        PROTOCOL_FAILURE
    );
}

/// The names in `root`, sorted.
fn names(root: &Path) -> Vec<std::ffi::OsString> {
    let mut names: Vec<_> = std::fs::read_dir(root)
        .expect("the root lists")
        .map(|entry| entry.expect("an entry").file_name())
        .collect();
    names.sort();
    names
}

#[test]
fn the_check_answers_each_of_its_three_proofs_and_leaves_the_root_as_it_found_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    if !openat2_served(root) {
        eprintln!("openat2 is unavailable on this kernel; the check's proofs are not exercised");
        return;
    }
    std::fs::write(root.join("kept.txt"), b"kept").expect("a workspace file");
    let (value, omitted) = ok::<serde_json::Value>(call(root, &Request::Check {}, b""));
    assert_eq!(omitted, 0);
    assert_eq!(
        value,
        serde_json::json!({
            "openat2_beneath": true,
            "symlink_escape_refused": true,
            "atomic_replace": true,
        }),
        "each proof of design 34 § 6.2 is answered on its own"
    );
    assert_eq!(
        names(root),
        ["kept.txt"],
        "nothing the check created outlives it"
    );
    assert_eq!(std::fs::read(root.join("kept.txt")).expect("kept"), b"kept");
}

#[test]
fn a_check_that_cannot_make_its_scratch_directory_proves_nothing() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = tempfile::tempdir().expect("tempdir");
    let root = directory.path();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o500)).expect("read-only");
    if std::fs::create_dir(root.join("probe")).is_ok() {
        std::fs::remove_dir(root.join("probe")).expect("remove");
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).expect("restore");
        eprintln!("this user writes a read-only directory; the unwritable case is not exercised");
        return;
    }
    let (value, _) = ok::<serde_json::Value>(call(root, &Request::Check {}, b""));
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).expect("restore");
    assert_eq!(
        value,
        serde_json::json!({
            "openat2_beneath": false,
            "symlink_escape_refused": false,
            "atomic_replace": false,
        })
    );
    assert!(names(root).is_empty());
}
