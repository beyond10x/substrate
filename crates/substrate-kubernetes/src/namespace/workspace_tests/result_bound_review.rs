//! Independent verification of the result bound through the namespace driver
//! (review of story:file-change-result-fits-the-ledger-before-the-file-changes).
//!
//! Adapted after the review. The namespace driver is handed no bound and sends the executor the
//! header every release since 0.16.1 sends, so a keeper created by an earlier release serves a
//! later daemon. The real executor behind the fake `pods/exec` answers a diff that states no
//! bound; the daemon holds that answer to what its ledger reserved with
//! `FileMutationResult::fit_json_bytes`, which these cases apply as the daemon does. The case
//! that had the driver relay the executor's refusal of a bound no result fits is the daemon's,
//! which decides it before dispatch
//! (`crates/substrate-daemon/tests/http/driver_answer_bound.rs`).

use serde_json::{Value, json};
use substrate_host::Driver;
use substrate_wire::{FileEditInput, FilePatchInput, FileReplaceInput};

use super::{WORKSPACE, lock, pvc_name, setup, sha};

/// A text that costs far more as JSON than the bound of these cases.
fn costly_text() -> String {
    "\"\\\u{1}\t\u{1f600}".repeat(4_000)
}

const BOUND: u64 = 4_096;

fn recorded_bytes(result: &substrate_wire::FileMutationResult) -> u64 {
    u64::try_from(result.json_bytes()).expect("usize fits u64")
}

/// The answer as the daemon records it: held to [`BOUND`] by the one cut.
fn held(mut result: substrate_wire::FileMutationResult) -> substrate_wire::FileMutationResult {
    assert!(
        recorded_bytes(&result) > BOUND,
        "the executor states no bound, so its answer is not cut to one"
    );
    result.fit_json_bytes(BOUND);
    result
}

/// The member names of every `fs` request header the driver sent, in order, as the bytes on the
/// executor's stdin state them: a `u32be` length, then the JSON header.
fn fs_header_members(s: &super::Setup) -> Vec<(String, Vec<String>)> {
    lock(&s.executor.calls)
        .iter()
        .filter(|call| call.command.last().is_some_and(|part| part == "fs"))
        .map(|call| {
            let length = u32::from_be_bytes(call.stdin[..4].try_into().expect("a length"));
            let length = usize::try_from(length).expect("u32 fits usize");
            let header: Value =
                serde_json::from_slice(&call.stdin[4..4 + length]).expect("a JSON header");
            let object = header.as_object().expect("an object");
            let mut members: Vec<String> = object.keys().cloned().collect();
            members.sort_unstable();
            (object["op"].as_str().expect("an op").to_owned(), members)
        })
        .collect()
}

#[tokio::test]
async fn each_v2_change_is_sent_as_0_24_0_sends_it_and_its_answer_fits_the_bound_it_is_held_to() {
    use base64::Engine as _;

    let s = setup();
    s.created().await;
    let pvc = pvc_name();
    let d = &s.driver;
    let text = costly_text();

    Driver::write_workspace_file(d, WORKSPACE, &pvc, "a.txt", b"one\n")
        .await
        .expect("seeded");

    let first = format!("{text}\n");
    let replace: FileReplaceInput = serde_json::from_value(json!({
        "content": {
            "encoding": "base64",
            "data": base64::engine::general_purpose::STANDARD.encode(&first),
        },
        "expected": {"state": "sha256", "sha256": sha(b"one\n")},
    }))
    .expect("a replacement");
    let replaced = held(
        Driver::replace_workspace_file_v2(d, WORKSPACE, &pvc, "a.txt", &replace)
            .await
            .expect("replaced"),
    );
    assert!(
        recorded_bytes(&replaced) <= BOUND,
        "the replacement answered {} bytes of JSON for a bound of {BOUND}",
        recorded_bytes(&replaced)
    );
    assert!(replaced.diff.truncated);
    assert_eq!(
        std::fs::read(s.file("a.txt")).expect("on the volume"),
        first.as_bytes()
    );

    let second = format!("{text}{text}\n");
    let edit: FileEditInput = serde_json::from_value(json!({
        "expected_sha256": sha(first.as_bytes()),
        "old_text": "\n",
        "new_text": format!("{text}\n"),
        "match_policy": "exact",
    }))
    .expect("an edit");
    let edited = held(
        Driver::edit_workspace_file_v2(d, WORKSPACE, &pvc, "a.txt", &edit)
            .await
            .expect("edited"),
    );
    assert!(
        recorded_bytes(&edited) <= BOUND,
        "the edit answered {} bytes of JSON for a bound of {BOUND}",
        recorded_bytes(&edited)
    );
    assert!(edited.diff.truncated);
    assert_eq!(
        std::fs::read(s.file("a.txt")).expect("on the volume"),
        second.as_bytes()
    );

    let patch: FilePatchInput = serde_json::from_value(json!({
        "expected_sha256": sha(second.as_bytes()),
        "edits": [{"kind": "insert_before", "line": 1, "text": format!("{text}\n")}],
    }))
    .expect("a patch");
    let patched = held(
        Driver::patch_workspace_file_v2(d, WORKSPACE, &pvc, "a.txt", &patch)
            .await
            .expect("patched"),
    );
    assert!(
        recorded_bytes(&patched) <= BOUND,
        "the patch answered {} bytes of JSON for a bound of {BOUND}",
        recorded_bytes(&patched)
    );
    assert!(patched.diff.truncated);
    assert_eq!(
        std::fs::read(s.file("a.txt")).expect("on the volume"),
        format!("{text}\n{text}{text}\n").as_bytes()
    );
    assert_eq!(
        s.executor.fs_ops(),
        ["write", "replace-v2", "edit-v2", "patch-v2"]
    );
    let v2_members = ["input", "op", "path", "workspace"].map(str::to_owned);
    assert_eq!(
        fs_header_members(&s)[1..],
        [
            ("replace-v2".to_owned(), v2_members.to_vec()),
            ("edit-v2".to_owned(), v2_members.to_vec()),
            ("patch-v2".to_owned(), v2_members.to_vec()),
        ],
        "each v2 header holds exactly the members 0.24.0 sends"
    );
}
