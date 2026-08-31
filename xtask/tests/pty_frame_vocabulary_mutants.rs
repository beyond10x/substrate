#![forbid(unsafe_code)]
//! Two mutants of the pty frame vocabulary that `cargo xtask check-bundle` accepts.
//!
//! `check_pty_frames` (`xtask/src/bundle.rs:746`) reads the `oneOf` of
//! `schemas/pty-channel-frame.json` into a `BTreeSet` of `kind` consts and asks which kinds are
//! present and which are absent. `check_pty_window_bounds` (`:814`) reads the resize bounds out of
//! `oneOf` branch `RESIZE_BRANCH` — index `1` — after guarding that branch's own `kind`. Both
//! questions are answered correctly by a document with a **duplicate** branch, because a set forgets
//! multiplicity and an index reads one position.
//!
//! A duplicate is not a stylistic defect in a `oneOf`. Draft 2020-12 `oneOf` requires *exactly one*
//! subschema to match, so a second `resize` branch with wider bounds inverts the published contract:
//! `{"columns": 80, "rows": 24}` — inside the declared bound — matches both branches and is
//! **invalid**, while `{"columns": 5000, "rows": 24}` matches only the wide one and is **valid**. A
//! second `output` branch attributing `stderr` does the same to `x-b10x-one-file`, because
//! `check_pty_frames` reads the first `output` branch it finds and stops.
//!
//! Each case renders the mutated source into a scratch tree and asserts `check-bundle` refuses it.
//! Nothing under `contracts/` or `xtask/bundle-source/` is written; both are copied first. The
//! scratch tree is a `tempfile::TempDir` and is removed when the case ends.
//!
//! Portable lane. Runs the `xtask` binary this package builds and reads the repository's own trees.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const VERSION: &str = "0.10.0";
const DOCUMENT: &str = "documents/schemas/pty-channel-frame.json";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("scratch directory");
    for entry in fs::read_dir(from).expect("readable tree") {
        let entry = entry.expect("directory entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// The branch of the pty vocabulary whose `kind` is `kind`, cloned.
fn branch(document: &Value, kind: &str) -> Value {
    document
        .get("oneOf")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|candidate| {
            candidate
                .pointer("/properties/kind/const")
                .and_then(Value::as_str)
                == Some(kind)
        })
        .unwrap_or_else(|| panic!("the pty vocabulary has a {kind} branch"))
        .clone()
}

fn append_branch(document: &mut Value, extra: Value) {
    document
        .get_mut("oneOf")
        .and_then(Value::as_array_mut)
        .expect("the pty vocabulary is a oneOf")
        .push(extra);
}

/// Renders the mutated authored source into a scratch tree and runs `check-bundle` over it.
///
/// Returns the checker's exit status and everything it printed.
fn check_bundle_after<F>(mutate: F) -> (bool, String)
where
    F: FnOnce(&mut Value),
{
    let scratch = tempfile::tempdir().expect("scratch tree");
    let source = scratch.path().join("source");
    let contracts = scratch.path().join("contracts");
    copy_tree(&repo_root().join("xtask/bundle-source"), &source);
    copy_tree(&repo_root().join("contracts/substrate-wire"), &contracts);

    let authored = source.join(VERSION).join(DOCUMENT);
    let mut document: Value =
        serde_json::from_slice(&fs::read(&authored).expect("authored pty vocabulary"))
            .expect("JSON");
    mutate(&mut document);
    fs::write(
        &authored,
        serde_json::to_vec_pretty(&document).expect("serialize mutant"),
    )
    .expect("write mutant");

    let rendered = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["render-bundle", VERSION])
        .arg("--source")
        .arg(&source)
        .arg("--contracts-root")
        .arg(&contracts)
        .arg("--out")
        .arg(contracts.join(VERSION))
        .arg("--force")
        .output()
        .expect("run render-bundle");
    assert!(
        rendered.status.success(),
        "the mutant has to render, or this case proves nothing about the checker:\n{}{}",
        String::from_utf8_lossy(&rendered.stdout),
        String::from_utf8_lossy(&rendered.stderr)
    );

    let checked = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["check-bundle", VERSION])
        .arg("--source")
        .arg(&source)
        .arg("--contracts-root")
        .arg(&contracts)
        .output()
        .expect("run check-bundle");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    (checked.status.success(), said)
}

/// A second `resize` branch admitting 0..=65535 cells is refused.
///
/// Nothing in the checker counts branches. `check_pty_window_bounds` reads index `1` and, having
/// confirmed that branch says `resize`, believes it has read *the* resize bound; `check_pty_frames`
/// asks a set whether `resize` is present, and it is. So the bundle ships a `resize` vocabulary in
/// which the window the daemon actually enforces — `substrate_wire::MAX_PTY_WINDOW_COLUMNS` — is the
/// one a conforming client is told it may not send, and the amplification window the ioctl guard
/// exists to refuse is the one it is told it may.
#[test]
fn a_second_resize_branch_with_wider_bounds_is_refused() {
    let (accepted, said) = check_bundle_after(|document| {
        let mut wide = branch(document, "resize");
        for axis in ["columns", "rows"] {
            let window = wide
                .pointer_mut(&format!("/properties/window/properties/{axis}"))
                .expect("resize window axis");
            window["maximum"] = Value::from(65_535);
            window["minimum"] = Value::from(0);
        }
        append_branch(document, wide);
    });
    assert!(
        !accepted,
        "check-bundle accepted a pty vocabulary with two resize branches, one admitting \
         0..=65535 cells — under oneOf that makes an in-bounds resize invalid and an \
         out-of-bounds one valid:\n{said}"
    );
}

/// A second `output` branch attributing `stderr` is refused.
///
/// `schemas/pty-channel-frame.json` carries
/// `x-b10x-one-file: "stdout-and-stderr-are-the-same-descriptor-on-a-terminal"`, and
/// `check_pty_frames` enforces it by finding the first `output` branch and reading its
/// `stream.const`. A second one restores to the published vocabulary exactly the per-stream
/// attribution design 13 removed, and the first branch still answers `stdout`.
#[test]
fn a_second_output_branch_attributing_stderr_is_refused() {
    let (accepted, said) = check_bundle_after(|document| {
        let mut second = branch(document, "output");
        second["properties"]["stream"]["const"] = Value::from("stderr");
        append_branch(document, second);
    });
    assert!(
        !accepted,
        "check-bundle accepted a pty vocabulary whose output frame can attribute stderr, which \
         x-b10x-one-file says a terminal has no way to mean:\n{said}"
    );
}

/// The `code` pattern the closed-code argument is *stated in* is not checked by anything.
///
/// `SessionProtocolErrorCode`'s own documentation gives the reason the type exists as
/// "a frame whose published `code` is `^session\.[a-z0-9-]+$` — a frame the bundle says cannot
/// exist" (`crates/substrate-wire/src/lib.rs:146-147`), and the same sentence is repeated on
/// `send_pipe_protocol_error` (`crates/substrate-daemon/src/app/sessions.rs:1197-1201`). Two halves
/// of that claim are mechanised: the crate cannot emit a non-member because the parameter is the
/// enum, and `check_pty_refusal_class` (`xtask/src/bundle.rs:494`) compares `x-b10x-codes` against
/// `substrate_wire::SESSION_PROTOCOL_ERROR_CODES` in both directions. The third half — that the
/// pattern the frame publishes admits those codes — is mechanised nowhere: `check_pty_frames`
/// (`:794`) reads `kind` consts and the `output` branch's `stream`, and never looks at `code`.
///
/// So the bundle may publish a `protocol-error` branch whose `code` pattern rejects every entry of
/// its own `x-b10x-codes`, and `check-bundle` exits 0. A client generating a validator from the
/// released schema would then reject every `protocol-error` frame the daemon can send — which is
/// worse than the defect the enum was introduced to prevent, because it is a total failure rather
/// than a per-code one, and it is invisible in exactly the document a client reads.
#[test]
fn a_protocol_error_code_pattern_that_rejects_the_published_codes_is_refused() {
    let (accepted, said) = check_bundle_after(|document| {
        let branch = document
            .get_mut("oneOf")
            .and_then(Value::as_array_mut)
            .expect("the pty vocabulary is a oneOf")
            .iter_mut()
            .find(|candidate| {
                candidate
                    .pointer("/properties/kind/const")
                    .and_then(Value::as_str)
                    == Some("protocol-error")
            })
            .expect("the pty vocabulary has a protocol-error branch");
        branch["properties"]["code"]["pattern"] = Value::from("^exec\\.[a-z0-9-]+$");
    });
    assert!(
        !accepted,
        "check-bundle accepted a pty vocabulary whose protocol-error code pattern is \
         ^exec\\.[a-z0-9-]+$ while its own x-b10x-codes are all session.*, so a client validating \
         against the released schema rejects every protocol-error frame the daemon can send:\n{said}"
    );
}
