//! The text bounds of the three v2 file changes, held to the schemas bundle `0.27.0` publishes
//! (story:file-change-result-fits-the-ledger-before-the-file-changes).
//!
//! No bundle binds these numbers with a `$wire` marker: the schemas state them as literals. The
//! constants are therefore held to the released schema files here, so one that moves without a
//! successor bundle fails.

use chrono::DateTime;
use serde_json::{Value, json};

use super::{
    Base64Content, Base64Encoding, ExpectedFileState, FileEditInput, FileKind, FileMutationResult,
    FilePatchInput, FileReplaceInput, LinePatchEdit, MAX_FILE_BYTES, MAX_FILE_CONTENT_BASE64_CHARS,
    MAX_FILE_DIFF_TEXT_CHARS, MAX_FILE_TEXT_CHARS, TextMatchPolicy, UnifiedDiff,
    WireValidationError,
};

fn schema(text: &str) -> Value {
    serde_json::from_str(text).expect("a released schema is JSON")
}

#[test]
fn the_file_text_bounds_are_the_lengths_bundle_0_27_0_publishes() {
    let edit = schema(include_str!(
        "../../../contracts/substrate-wire/0.27.0/schemas/inputs/workspace-file-edit-v2.json"
    ));
    for member in ["old_text", "new_text"] {
        assert_eq!(
            edit.pointer(&format!("/properties/{member}/maxLength")),
            Some(&json!(MAX_FILE_TEXT_CHARS)),
            "{member}"
        );
    }

    let patch = schema(include_str!(
        "../../../contracts/substrate-wire/0.27.0/schemas/inputs/workspace-file-patch-v2.json"
    ));
    assert_eq!(
        patch.pointer("/$defs/text/maxLength"),
        Some(&json!(MAX_FILE_TEXT_CHARS))
    );
    let variants = patch
        .pointer("/properties/edits/items/oneOf")
        .and_then(Value::as_array)
        .expect("the edit variants");
    let texts: Vec<&Value> = variants
        .iter()
        .filter_map(|variant| variant.pointer("/properties/text"))
        .collect();
    assert_eq!(texts.len(), 3, "three edit kinds carry a text");
    for text in texts {
        assert_eq!(text, &json!({ "$ref": "#/$defs/text" }));
    }

    let result = schema(include_str!(
        "../../../contracts/substrate-wire/0.27.0/schemas/results/workspace-file-mutation-v2.json"
    ));
    assert_eq!(
        result.pointer("/properties/diff/properties/text/maxLength"),
        Some(&json!(MAX_FILE_DIFF_TEXT_CHARS))
    );

    let replace = schema(include_str!(
        "../../../contracts/substrate-wire/0.27.0/schemas/inputs/workspace-file-replace-v2.json"
    ));
    assert_eq!(
        replace.pointer("/properties/content/properties/data"),
        Some(&json!({ "$ref": "../common.json#/$defs/canonical-base64-file" }))
    );
    let common = schema(include_str!(
        "../../../contracts/substrate-wire/0.27.0/schemas/common.json"
    ));
    let content = common
        .pointer("/$defs/canonical-base64-file")
        .expect("the file content definition");
    let longest = content
        .pointer("/oneOf")
        .and_then(Value::as_array)
        .expect("the two content forms")
        .iter()
        .filter_map(|form| form.pointer("/maxLength").and_then(Value::as_u64))
        .max();
    assert_eq!(
        longest,
        Some(u64::try_from(MAX_FILE_CONTENT_BASE64_CHARS).expect("usize fits u64"))
    );
    assert_eq!(
        content.pointer("/x-b10x-max-decoded-bytes"),
        Some(&json!(MAX_FILE_BYTES))
    );
    // The longest canonical base64 is the encoding of a file at the limit.
    assert_eq!(
        u64::try_from(MAX_FILE_CONTENT_BASE64_CHARS).expect("usize fits u64"),
        MAX_FILE_BYTES.div_ceil(3) * 4
    );
}

fn edit(old_text: String, new_text: String) -> FileEditInput {
    FileEditInput {
        expected_sha256: "0".repeat(64),
        old_text,
        new_text,
        match_policy: TextMatchPolicy::Exact,
    }
}

fn patch(text: String) -> FilePatchInput {
    FilePatchInput {
        expected_sha256: "0".repeat(64),
        edits: vec![
            LinePatchEdit::DeleteRange {
                start_line: 1,
                end_line: 1,
            },
            LinePatchEdit::InsertAfter { line: 2, text },
        ],
    }
}

/// `maxLength` counts characters, so a text of exactly the bound in two-byte characters is
/// within it, and one character more is not.
#[test]
fn an_edit_or_patch_text_is_within_bounds_up_to_the_published_character_count() {
    let at = "\u{e9}".repeat(MAX_FILE_TEXT_CHARS);
    let over = format!("{at}a");
    assert_eq!(edit(at.clone(), at.clone()).validate_text_bounds(), Ok(()));
    assert_eq!(
        edit(over.clone(), "new".to_owned()).validate_text_bounds(),
        Err(WireValidationError::FileTextBounds)
    );
    assert_eq!(
        edit("old".to_owned(), over.clone()).validate_text_bounds(),
        Err(WireValidationError::FileTextBounds)
    );
    assert_eq!(patch(at).validate_text_bounds(), Ok(()));
    assert_eq!(
        patch(over).validate_text_bounds(),
        Err(WireValidationError::FileTextBounds)
    );
}

#[test]
fn a_replacement_content_is_within_bounds_up_to_the_published_base64_length() {
    let replacement = |length: usize| FileReplaceInput {
        content: Base64Content {
            encoding: Base64Encoding::Base64,
            data: "A".repeat(length),
        },
        expected: ExpectedFileState::Absent,
        create_parents: false,
    };
    assert_eq!(
        replacement(MAX_FILE_CONTENT_BASE64_CHARS).validate_text_bounds(),
        Ok(())
    );
    assert_eq!(
        replacement(MAX_FILE_CONTENT_BASE64_CHARS + 1).validate_text_bounds(),
        Err(WireValidationError::FileTextBounds)
    );
}

/// `json_bytes` is the length of the compact JSON a result is recorded and answered as, with
/// every escape counted.
#[test]
fn a_mutation_result_states_the_bytes_of_its_compact_json() {
    let result = FileMutationResult {
        kind: FileKind::File,
        workspace: "ws_01".to_owned(),
        path: "dir/\u{1}file \"x\".txt".to_owned(),
        size: 12,
        before_sha256: None,
        after_sha256: "f".repeat(64),
        atomic_replacement: true,
        diff: UnifiedDiff {
            text: "--- a\n+++ b\n@@\n-\"\\\u{1}\u{1f}\t\n+\u{1f600}\u{e9}".to_owned(),
            truncated: true,
            binary: false,
        },
        observed_at: DateTime::UNIX_EPOCH,
    };
    assert_eq!(
        result.json_bytes(),
        serde_json::to_string(&result).expect("JSON").len()
    );
}
