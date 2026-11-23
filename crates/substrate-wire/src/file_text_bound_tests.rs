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
    MAX_FILE_CONTENT_BASE64_CHARS_BELOW_FILE_LIMIT, MAX_FILE_DIFF_TEXT_CHARS, MAX_FILE_TEXT_CHARS,
    TextMatchPolicy, UnifiedDiff, WireValidationError, json_char_bytes, json_text_prefix,
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
    // The content is one of exactly two forms, and `validate_text_bounds` holds both: any
    // canonical base64 up to the first form's length, or the second form's one length ending
    // `==`.
    let forms = content
        .pointer("/oneOf")
        .and_then(Value::as_array)
        .expect("the two content forms");
    assert_eq!(forms.len(), 2);
    assert_eq!(
        forms[0],
        json!({
            "maxLength": MAX_FILE_CONTENT_BASE64_CHARS_BELOW_FILE_LIMIT,
            "pattern": "^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$",
            "type": "string"
        })
    );
    assert_eq!(
        forms[1],
        json!({
            "maxLength": MAX_FILE_CONTENT_BASE64_CHARS,
            "minLength": MAX_FILE_CONTENT_BASE64_CHARS,
            "pattern": format!("^[A-Za-z0-9+/]{{{}}}==$", MAX_FILE_CONTENT_BASE64_CHARS - 2),
            "type": "string"
        })
    );
    assert_eq!(
        content.pointer("/x-b10x-max-decoded-bytes"),
        Some(&json!(MAX_FILE_BYTES))
    );
    // The second form is the encoding of a file at the limit, and the first form's length is
    // the encoding of a file one byte smaller.
    assert_eq!(
        u64::try_from(MAX_FILE_CONTENT_BASE64_CHARS).expect("usize fits u64"),
        MAX_FILE_BYTES.div_ceil(3) * 4
    );
    assert_eq!(
        u64::try_from(MAX_FILE_CONTENT_BASE64_CHARS_BELOW_FILE_LIMIT).expect("usize fits u64"),
        (MAX_FILE_BYTES - 1).div_ceil(3) * 4
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
fn a_replacement_content_is_within_bounds_in_exactly_the_two_published_forms() {
    let bounds = |data: String| {
        FileReplaceInput {
            content: Base64Content {
                encoding: Base64Encoding::Base64,
                data,
            },
            expected: ExpectedFileState::Absent,
            create_parents: false,
        }
        .validate_text_bounds()
    };
    let below = MAX_FILE_CONTENT_BASE64_CHARS_BELOW_FILE_LIMIT;
    let at = MAX_FILE_CONTENT_BASE64_CHARS;
    let refused = Err(WireValidationError::FileTextBounds);
    // The first form: any length up to its bound.
    assert_eq!(bounds(String::new()), Ok(()));
    assert_eq!(bounds("A".repeat(below)), Ok(()));
    // Between the two forms nothing is admitted.
    for length in below + 1..at {
        assert_eq!(bounds("A".repeat(length)), refused, "{length}");
        assert_eq!(
            bounds(format!("{}==", "A".repeat(length - 2))),
            refused,
            "{length}"
        );
    }
    // The second form: its one length, ending `==`.
    assert_eq!(bounds(format!("{}==", "A".repeat(at - 2))), Ok(()));
    assert_eq!(bounds("A".repeat(at)), refused);
    assert_eq!(bounds(format!("{}=", "A".repeat(at - 1))), refused);
    assert_eq!(bounds(format!("{}==", "A".repeat(at - 1))), refused);
    assert_eq!(bounds("A".repeat(at + 4)), refused);
    // Characters, not bytes, are published; data in another alphabet that is over the length in
    // bytes is refused, as the published pattern refuses all of it.
    assert_eq!(bounds("\u{e9}".repeat(below)), refused);
}

fn result_with_diff(path: &str, text: &str) -> FileMutationResult {
    FileMutationResult {
        kind: FileKind::File,
        workspace: "ws_01".to_owned(),
        path: path.to_owned(),
        size: 12,
        before_sha256: Some("e".repeat(64)),
        after_sha256: "f".repeat(64),
        atomic_replacement: true,
        diff: UnifiedDiff {
            text: text.to_owned(),
            truncated: false,
            binary: false,
        },
        observed_at: DateTime::UNIX_EPOCH,
    }
}

/// The cut falls between characters and never inside one character's escape, and it holds both
/// bounds: the JSON bytes and the character count.
#[test]
fn a_cut_falls_between_characters_and_holds_the_byte_and_the_character_bound() {
    // `a` costs 1 byte, U+0001 costs 6 (`\u0001`), `b` costs 1.
    let text = "a\u{1}b";
    assert_eq!(json_text_prefix(text, 0, usize::MAX), 0);
    for json_bytes in 1..=6 {
        assert_eq!(
            json_text_prefix(text, json_bytes, usize::MAX),
            1,
            "{json_bytes}"
        );
    }
    assert_eq!(json_text_prefix(text, 7, usize::MAX), 2);
    assert_eq!(json_text_prefix(text, 8, usize::MAX), 3);
    // One character outside the basic plane is four bytes, written as it is.
    assert_eq!(json_text_prefix("\u{1f600}", 3, usize::MAX), 0);
    assert_eq!(json_text_prefix("\u{1f600}", 4, usize::MAX), 4);
    // The character bound counts characters, not bytes.
    assert_eq!(json_text_prefix("\u{e9}\u{e9}\u{e9}", usize::MAX, 2), 4);
    assert_eq!(json_text_prefix("\u{e9}\u{e9}\u{e9}", usize::MAX, 3), 6);
}

/// The cost this crate counts for a character is the cost the result's own serializer writes, for
/// every character there is.
#[test]
fn the_json_cost_counted_for_every_character_is_what_the_result_serializer_writes() {
    let empty = result_with_diff("p", "").json_bytes();
    for code in 0..=0xff_u32 {
        let character = char::from_u32(code).expect("a character");
        assert_eq!(
            result_with_diff("p", &character.to_string()).json_bytes() - empty,
            json_char_bytes(character),
            "U+{code:04X}"
        );
    }
    let every: String = (0..=0x0010_ffff_u32).filter_map(char::from_u32).collect();
    let counted: usize = every.chars().map(json_char_bytes).sum();
    assert_eq!(result_with_diff("p", &every).json_bytes() - empty, counted);
}

/// `cut_text` marks a diff `truncated` exactly when it dropped text, and leaves a mark that was
/// already there.
#[test]
fn a_cut_marks_the_diff_truncated_only_when_it_dropped_text() {
    let mut diff = UnifiedDiff {
        text: "a\u{1}b".to_owned(),
        truncated: false,
        binary: false,
    };
    diff.cut_text(8, 3);
    assert_eq!((diff.text.as_str(), diff.truncated), ("a\u{1}b", false));
    diff.cut_text(7, 3);
    assert_eq!((diff.text.as_str(), diff.truncated), ("a\u{1}", true));
    diff.cut_text(usize::MAX, usize::MAX);
    assert_eq!((diff.text.as_str(), diff.truncated), ("a\u{1}", true));
    diff.cut_text(usize::MAX, 1);
    assert_eq!((diff.text.as_str(), diff.truncated), ("a", true));
}

/// A result held to exactly the bytes it occupies is unchanged; held to one byte less, and to
/// every bound down to its widest fixed part, it fits the bound with only its diff text cut.
#[test]
fn a_result_is_held_to_a_bound_by_cutting_only_its_diff_text() {
    let path = "dir/\u{1}file \"x\".txt";
    let whole = result_with_diff(
        path,
        "--- a\n+++ b\n@@\n-\"\\\u{1}\u{1f}\t\n+\u{1f600}\u{e9}",
    );
    let needed = whole.json_bytes();
    let widest = FileMutationResult::widest_json_bytes("ws_01", path);
    assert!(widest < needed);

    let mut at_the_bound = whole.clone();
    at_the_bound.fit_json_bytes(u64::try_from(needed).expect("u64"));
    assert_eq!(at_the_bound, whole);

    for bound in widest..needed {
        let mut held = whole.clone();
        held.fit_json_bytes(u64::try_from(bound).expect("u64"));
        assert!(held.json_bytes() <= bound, "bound {bound}");
        assert!(held.diff.truncated, "bound {bound}");
        assert!(
            whole.diff.text.starts_with(&held.diff.text),
            "bound {bound}"
        );
        let mut expected = whole.clone();
        expected.diff = held.diff.clone();
        assert_eq!(held, expected, "bound {bound}: only the diff changed");
    }
}

/// Whatever bound a result is held to, its diff text is within the length its schema publishes:
/// a longer text is cut to that length and marked.
#[test]
fn a_result_held_to_any_bound_has_a_diff_within_its_published_length() {
    let mut result = result_with_diff("p", &"a".repeat(MAX_FILE_DIFF_TEXT_CHARS + 1));
    result.fit_json_bytes(u64::MAX);
    assert_eq!(result.diff.text.len(), MAX_FILE_DIFF_TEXT_CHARS);
    assert!(result.diff.truncated);

    let mut within = result_with_diff("p", &"\u{e9}".repeat(MAX_FILE_DIFF_TEXT_CHARS));
    within.fit_json_bytes(u64::MAX);
    assert_eq!(within.diff.text.chars().count(), MAX_FILE_DIFF_TEXT_CHARS);
    assert!(!within.diff.truncated);
}

/// No result for a workspace and a path is wider, apart from its diff text, than the figure a
/// caller decides with before it dispatches: each member at its widest is counted.
#[test]
fn the_widest_result_without_its_diff_text_bounds_every_result_for_its_path() {
    let path = "dir/\u{1}file \"x\".txt";
    let widest = FileMutationResult::widest_json_bytes("ws_01", path);
    let results = [
        result_with_diff(path, ""),
        FileMutationResult {
            size: u64::MAX,
            atomic_replacement: false,
            observed_at: DateTime::from_timestamp_nanos(1_999_999_999_123_456_789),
            ..result_with_diff(path, "")
        },
        FileMutationResult {
            before_sha256: None,
            diff: UnifiedDiff {
                text: String::new(),
                truncated: true,
                binary: true,
            },
            ..result_with_diff(path, "")
        },
    ];
    for result in results {
        assert!(result.json_bytes() <= widest, "{result:?}");
    }
    assert_eq!(
        FileMutationResult {
            size: u64::MAX,
            atomic_replacement: false,
            observed_at: DateTime::from_timestamp_nanos(1_999_999_999_123_456_789),
            ..result_with_diff(path, "")
        }
        .json_bytes(),
        widest,
        "the figure is reached, so it reserves nothing no result needs"
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
