//! Independent verification of the result bound on the executor's wire
//! (review of story:file-change-result-fits-the-ledger-before-the-file-changes).
//!
//! Adapted after the review. The executor's request did not change: the three v2 changes are
//! served with the header every release since 0.16.1 sends, which states no bound, and the
//! header stays closed, so a header with any member beside those changes no file. The bound is
//! the daemon's: it holds the executor's answer to what its ledger reserved with
//! `FileMutationResult::fit_json_bytes`, and it decides before dispatch whether a result can fit
//! at all. The case that had the executor refuse a bound no result fits is therefore the
//! daemon's (`crates/substrate-daemon/tests/http/driver_answer_bound.rs`).

use std::path::Path;

use base64::Engine as _;
use serde_json::{Value, json};
use substrate_executor::message::{PROTOCOL_FAILURE, Request, write_message};
use substrate_wire::{
    Base64Content, Base64Encoding, ExpectedFileState, FileEditInput, FileMutationResult,
    FilePatchInput, FileReplaceInput, LinePatchEdit, TextMatchPolicy,
};

use super::{ONE_TWO_SHA256, WORKSPACE, call, call_raw, error_code, ok, openat2_served};

/// The file every case starts from; its digest is [`ONE_TWO_SHA256`].
const SEED: &[u8] = b"one\ntwo\n";

/// The three v2 changes of `a.txt` that put `text` where the first line of [`SEED`] was.
fn changes(text: &str) -> Vec<Request> {
    vec![
        Request::ReplaceV2 {
            workspace: WORKSPACE.to_owned(),
            path: "a.txt".to_owned(),
            input: FileReplaceInput {
                content: Base64Content {
                    encoding: Base64Encoding::Base64,
                    data: base64::engine::general_purpose::STANDARD
                        .encode(format!("{text}\ntwo\n")),
                },
                expected: ExpectedFileState::Sha256 {
                    sha256: ONE_TWO_SHA256.to_owned(),
                },
                create_parents: false,
            },
        },
        Request::EditV2 {
            workspace: WORKSPACE.to_owned(),
            path: "a.txt".to_owned(),
            input: FileEditInput {
                expected_sha256: ONE_TWO_SHA256.to_owned(),
                old_text: "one".to_owned(),
                new_text: text.to_owned(),
                match_policy: TextMatchPolicy::Exact,
            },
        },
        Request::PatchV2 {
            workspace: WORKSPACE.to_owned(),
            path: "a.txt".to_owned(),
            input: FilePatchInput {
                expected_sha256: ONE_TWO_SHA256.to_owned(),
                edits: vec![LinePatchEdit::ReplaceRange {
                    start_line: 1,
                    end_line: 1,
                    text: text.to_owned(),
                }],
            },
        },
    ]
}

fn seeded_root() -> Option<tempfile::TempDir> {
    let directory = tempfile::tempdir().expect("tempdir");
    if !openat2_served(directory.path()) {
        eprintln!(
            "openat2 is unavailable on this kernel; the guarded operations are not exercised"
        );
        return None;
    }
    std::fs::write(directory.path().join("a.txt"), SEED).expect("seed");
    Some(directory)
}

fn raw_header(root: &Path, header: &Value) -> super::Response {
    let mut wire = Vec::new();
    write_message(
        &mut wire,
        &serde_json::to_vec(header).expect("header JSON"),
        b"",
    )
    .expect("encode");
    call_raw(root, &wire)
}

/// A v2 change header as every release since 0.16.1 sends it, with exactly `op`, `workspace`,
/// `path` and `input`, is served: the file changes. The header is closed, so the same header with
/// a member beside those, whatever its name or value, is the protocol failure, and the file is
/// the one that was there.
#[test]
fn a_v2_change_header_as_0_24_0_sends_it_is_served_and_a_member_beside_it_is_refused() {
    let Some(directory) = seeded_root() else {
        return;
    };
    let root = directory.path();
    for request in changes("changed") {
        let header = serde_json::to_value(&request).expect("header");
        let mut members: Vec<&str> = header
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        members.sort_unstable();
        assert_eq!(members, ["input", "op", "path", "workspace"], "{header}");

        let mut beside = header.clone();
        beside["result_json_bytes"] = json!(65_536);
        let mut renamed = header.clone();
        renamed["result_bytes"] = json!(65_536);
        let mut other = header.clone();
        other["result_text_bytes"] = json!(1);
        let mut negative = header.clone();
        negative["result_json_bytes"] = json!(-1);
        let mut fraction = header.clone();
        fraction["result_json_bytes"] = json!(65_536.5);
        let mut text = header.clone();
        text["result_json_bytes"] = json!("65536");
        let mut null = header.clone();
        null["result_json_bytes"] = Value::Null;

        for (name, refused) in [
            ("a bound beside the members", beside),
            ("a bound under another name", renamed),
            ("another member beside them", other),
            ("a negative bound", negative),
            ("a fractional bound", fraction),
            ("a bound as text", text),
            ("a null bound", null),
        ] {
            assert_eq!(
                error_code(raw_header(root, &refused)),
                PROTOCOL_FAILURE,
                "{name}: {}",
                header["op"]
            );
            assert_eq!(
                std::fs::read(root.join("a.txt")).expect("file"),
                SEED,
                "{name}: {}",
                header["op"]
            );
        }

        let (result, _) = ok::<FileMutationResult>(raw_header(root, &header));
        assert_eq!(
            std::fs::read(root.join("a.txt")).expect("file"),
            b"changed\ntwo\n",
            "the header as 0.24.0 sends it: {}",
            header["op"]
        );
        assert_eq!(result.before_sha256.as_deref(), Some(ONE_TWO_SHA256));
        std::fs::write(root.join("a.txt"), SEED).expect("seed again");
    }
}

/// The executor's answer, held to a bound by the function the daemon holds it with, is within
/// that bound: 4 096 bytes for a text that costs far more as JSON gives a result within 4 096
/// bytes, cut and marked, with the file changed.
#[test]
fn a_v2_change_is_answered_within_the_bound_its_answer_is_held_to() {
    const BOUND: u64 = 4_096;
    let Some(directory) = seeded_root() else {
        return;
    };
    let root = directory.path();
    let text = "\"\\\u{1}\t\u{1f600}".repeat(4_000);
    for request in changes(&text) {
        std::fs::write(root.join("a.txt"), SEED).expect("seed again");
        let (mut result, _) = ok::<FileMutationResult>(call(root, &request, b""));
        assert!(
            u64::try_from(result.json_bytes()).expect("u64") > BOUND,
            "the executor states no bound, so its answer is not cut to one"
        );
        result.fit_json_bytes(BOUND);
        let recorded = serde_json::to_vec(&result).expect("result JSON").len();
        assert!(
            u64::try_from(recorded).expect("u64") <= BOUND,
            "the answer is {recorded} bytes of JSON for a bound of {BOUND}"
        );
        assert!(result.diff.truncated);
        assert_eq!(
            std::fs::read(root.join("a.txt")).expect("file"),
            format!("{text}\ntwo\n").as_bytes()
        );
    }
}
