//! Independent verification of the result bound
//! (review of story:file-change-result-fits-the-ledger-before-the-file-changes).
//!
//! Every cost here is measured with the result's own serializer
//! ([`FileMutationResult::json_bytes`]), never with the crate's `json_char_bytes`, so a case
//! cannot agree with the code by sharing its table.

use std::time::{Duration, Instant};

use substrate_wire::{
    ExpectedFileState, FileKind, FileMode, FileMutationResult, FileReadQuery, MAX_FILE_BYTES,
    MAX_FILE_DIFF_TEXT_CHARS, UnifiedDiff,
};

use super::super::bounded_diff as answered_diff;
use super::{
    SEED, WORKSPACE, replacement, sha256, workspace_holding, workspace_holding_with_file_limit,
};

// Adapted after the review: the cut and the widest fixed part are one implementation in the wire
// crate (`UnifiedDiff::cut_text`, `FileMutationResult::widest_json_bytes`,
// `FileMutationResult::fit_json_bytes`), which the daemon applies to every driver's answer. The
// three functions below give these cases the shapes they were written against; every assertion
// in the cases is the reviewer's.

/// The length the one cut keeps of `text`.
fn fitting_prefix(text: &str, json_bytes: usize, max_chars: usize) -> usize {
    let mut diff = UnifiedDiff {
        text: text.to_owned(),
        truncated: false,
        binary: false,
    };
    diff.cut_text(json_bytes, max_chars);
    assert_eq!(diff.truncated, diff.text.len() < text.len());
    diff.text.len()
}

/// The diff a change answers, held to `room` bytes of JSON by the one cut.
fn bounded_diff(path: &str, before: Option<&[u8]>, after: &[u8], room: usize) -> UnifiedDiff {
    let mut diff = answered_diff(path, before, after);
    diff.cut_text(room, MAX_FILE_DIFF_TEXT_CHARS);
    diff
}

fn widest_result_json_bytes(workspace_id: &str, path: &str) -> usize {
    FileMutationResult::widest_json_bytes(workspace_id, path)
}

/// What the daemon passes with the shipped ledger: the default terminal headroom, 512 KiB, less
/// `terminal_at` (35), the status charge (8) and the outcome envelope (28).
const SHIPPED_RESULT_JSON_BYTES: u64 = 512 * 1024 - 35 - 8 - 28;

/// The bytes `text` adds to a serialized result as its `diff.text`.
fn serialized_cost(text: &str) -> usize {
    let result = |text: &str| {
        FileMutationResult {
            kind: FileKind::File,
            workspace: WORKSPACE.to_owned(),
            path: "p".to_owned(),
            size: 0,
            before_sha256: None,
            after_sha256: "0".repeat(64),
            atomic_replacement: true,
            diff: UnifiedDiff {
                text: text.to_owned(),
                truncated: false,
                binary: false,
            },
            observed_at: chrono::DateTime::UNIX_EPOCH,
        }
        .json_bytes()
    };
    result(text) - result("")
}

/// Texts made of the characters a serializer may treat specially: every control character, the
/// quote and the backslash, the solidus, DEL, the two line separators, a two-byte character and
/// one outside the basic plane.
fn adversarial_texts() -> Vec<String> {
    let controls: String = (0..0x20_u32).filter_map(char::from_u32).collect();
    vec![
        controls.repeat(4),
        "\"".repeat(150),
        "\\".repeat(150),
        "\"\\/\u{7f}\u{2028}\u{2029}\u{e9}\u{1f600}\u{0}\u{1f}\n\t\r\u{8}\u{c} a".repeat(12),
        "\u{1f600}".repeat(60),
    ]
}

#[test]
fn the_cut_is_the_longest_prefix_the_serializer_fits_for_every_room() {
    for text in adversarial_texts() {
        let whole = serialized_cost(&text);
        for room in 0..=whole + 3 {
            let cut = fitting_prefix(&text, room, usize::MAX);
            assert!(
                text.is_char_boundary(cut),
                "room {room}: inside a character"
            );
            assert!(
                serialized_cost(&text[..cut]) <= room,
                "room {room}: the prefix costs more than the room"
            );
            if let Some(next) = text[cut..].chars().next() {
                assert!(
                    serialized_cost(&text[..cut + next.len_utf8()]) > room,
                    "room {room}: one more character would have fitted"
                );
            } else {
                assert!(whole <= room, "room {room}: the whole text was kept");
            }
        }
        let characters = text.chars().count();
        for max_chars in 0..=characters + 1 {
            let cut = fitting_prefix(&text, usize::MAX, max_chars);
            assert!(text.is_char_boundary(cut));
            assert_eq!(text[..cut].chars().count(), max_chars.min(characters));
        }
    }
}

/// One pass over a megabyte: a quadratic cut would not end inside the bound stated here.
#[test]
fn a_megabyte_of_quotes_or_of_control_characters_is_cut_in_one_pass() {
    let size = usize::try_from(MAX_FILE_BYTES).expect("u64 fits usize");
    let room = usize::try_from(SHIPPED_RESULT_JSON_BYTES).expect("u64 fits usize");
    for (text, cost) in [("\"".repeat(size), 2), ("\u{1}".repeat(size), 6)] {
        let started = Instant::now();
        assert_eq!(fitting_prefix(&text, room, usize::MAX), room / cost);
        assert_eq!(fitting_prefix(&text, usize::MAX, usize::MAX), size);
        assert_eq!(fitting_prefix(&text, 0, usize::MAX), 0);
        assert_eq!(fitting_prefix(&text, cost - 1, usize::MAX), 0);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "four cuts of {size} characters took {:?}",
            started.elapsed()
        );
    }
}

/// For every room from nothing to more than the whole diff: the answered text is a prefix of the
/// whole diff, costs at most the room as the serializer writes it, and is marked `truncated`
/// exactly when text was dropped.
#[test]
fn a_diff_is_marked_truncated_exactly_when_text_was_dropped_for_every_room() {
    let path = "d/\u{1}\"x";
    let before = "a\u{1}\"\n\u{1f600}\u{e9}\n\\tail";
    let after = "\u{2028}b\u{1f}\"\n\t\u{7f}/\n";
    let whole = bounded_diff(path, Some(before.as_bytes()), after.as_bytes(), usize::MAX);
    assert!(!whole.truncated);
    let whole_cost = serialized_cost(&whole.text);
    for room in 0..=whole_cost + 2 {
        let diff = bounded_diff(path, Some(before.as_bytes()), after.as_bytes(), room);
        assert!(whole.text.starts_with(&diff.text), "room {room}");
        assert!(serialized_cost(&diff.text) <= room, "room {room}");
        assert_eq!(
            diff.truncated,
            diff.text.len() < whole.text.len(),
            "room {room}"
        );
        assert_eq!(diff.truncated, room < whole_cost, "room {room}");
        assert!(!diff.binary);
    }
}

/// The line cap and the byte cap compose: a diff over 200 lines is `truncated` whatever the room,
/// and a smaller room cuts the capped text further.
#[test]
fn the_line_cap_and_the_byte_cap_compose() {
    let after = "line\n".repeat(300);
    let capped = bounded_diff("f", None, after.as_bytes(), usize::MAX);
    assert!(capped.truncated, "300 lines are over the line cap");
    assert_eq!(capped.text.matches("+line").count(), 197);
    let capped_cost = serialized_cost(&capped.text);
    let exact = bounded_diff("f", None, after.as_bytes(), capped_cost);
    assert_eq!(exact, capped, "a room for the capped text keeps all of it");
    let cut = bounded_diff("f", None, after.as_bytes(), capped_cost - 1);
    assert!(cut.truncated);
    assert!(cut.text.len() < capped.text.len());
    assert!(capped.text.starts_with(&cut.text));
    assert!(serialized_cost(&cut.text) < capped_cost);
}

/// A binary change carries no text, so it fits the smallest bound that admits any result, and is
/// not marked `truncated`.
#[test]
fn a_binary_change_fits_the_smallest_admitted_bound() {
    let diff = bounded_diff("f", Some(&[0xff, 0xfe]), b"text", 0);
    assert_eq!(
        diff,
        UnifiedDiff {
            text: String::new(),
            truncated: false,
            binary: true,
        }
    );
    let (directory, filesystem) = workspace_holding(SEED.as_bytes());
    let smallest = u64::try_from(widest_result_json_bytes(WORKSPACE, super::PATH)).expect("u64");
    let content = [0xff_u8, 0x00, 0xfe];
    let mut result = filesystem
        .replace_cas(
            WORKSPACE,
            WORKSPACE,
            super::PATH,
            &replacement(
                &content,
                ExpectedFileState::Sha256 {
                    sha256: sha256(SEED.as_bytes()),
                },
            ),
        )
        .expect("a binary replacement at the smallest admitted bound");
    // Adapted: the answer is held to the bound as the daemon holds it.
    result.fit_json_bytes(smallest);
    assert!(result.diff.binary);
    assert!(!result.diff.truncated);
    assert!(u64::try_from(result.json_bytes()).expect("u64") <= smallest);
    assert_eq!(super::on_disk(&directory), content);
}

/// The widest members there are: a path of 4 095 bytes whose every character costs six bytes of
/// JSON, in sixteen directories the replacement creates, and a file at the limit made of one such
/// character. For the smallest admitted bound, the bounds around one character's cost and the
/// shipped one, the file is written and the result is within the bound it was given.
#[test]
fn the_longest_escaped_path_and_a_file_of_control_characters_fit_every_admitted_bound() {
    let workspace_id = "ws_01JFILECHANGEREVIEW00000001";
    let path = vec!["\u{1}".repeat(255); 16].join("/");
    assert_eq!(path.len(), 4_095);
    let content = "\u{1}".repeat(usize::try_from(MAX_FILE_BYTES).expect("u64 fits usize"));
    let smallest = u64::try_from(widest_result_json_bytes(workspace_id, &path)).expect("u64");
    assert!(smallest < SHIPPED_RESULT_JSON_BYTES);
    for bound in [
        smallest,
        smallest + 1,
        smallest + 5,
        smallest + 6,
        smallest + 7,
        SHIPPED_RESULT_JSON_BYTES,
    ] {
        let (directory, filesystem) = workspace_holding(SEED.as_bytes());
        let mut input = replacement(content.as_bytes(), ExpectedFileState::Absent);
        input.create_parents = true;
        let mut result = filesystem
            .replace_cas(workspace_id, WORKSPACE, &path, &input)
            .unwrap_or_else(|error| panic!("bound {bound} admits the change: {error}"));
        // Adapted: the answer is held to the bound as the daemon holds it.
        result.fit_json_bytes(bound);
        assert!(
            u64::try_from(result.json_bytes()).expect("u64") <= bound,
            "bound {bound}: the result is {} bytes of JSON",
            result.json_bytes()
        );
        assert!(result.diff.truncated, "bound {bound}");
        assert!(result.diff.text.chars().count() <= MAX_FILE_DIFF_TEXT_CHARS);
        assert_eq!(result.kind, FileKind::File);
        assert_eq!(result.path, path);
        assert_eq!(result.workspace, workspace_id);
        // The absolute path is longer than a path argument may be, so the file is read back
        // through the guarded read, which resolves it beneath the workspace descriptor.
        let read = filesystem
            .read_digested(
                workspace_id,
                WORKSPACE,
                &path,
                &FileReadQuery {
                    mode: FileMode::File,
                    offset: Some(0),
                    limit_bytes: Some(1),
                    cursor: None,
                    limit_items: None,
                },
            )
            .expect("the written file reads");
        assert_eq!(read.size, MAX_FILE_BYTES, "bound {bound}");
        assert_eq!(
            read.sha256,
            sha256(content.as_bytes()),
            "bound {bound}: the file is written"
        );
        assert_eq!(result.after_sha256, read.sha256);
        drop(directory);
    }
}

/// A replacement that is refused leaves the workspace as it was. Its precondition names a digest,
/// the file is absent, and the answer is `workspace.stale-content`; the parents it asked for are
/// not there afterwards.
#[test]
fn a_replacement_refused_for_its_precondition_creates_no_parent_directory() {
    let (directory, filesystem) = workspace_holding(SEED.as_bytes());
    let mut input = replacement(
        b"changed",
        ExpectedFileState::Sha256 {
            sha256: sha256(SEED.as_bytes()),
        },
    );
    input.create_parents = true;
    let refusal = filesystem
        .replace_cas(WORKSPACE, WORKSPACE, "new/dir/file.txt", &input)
        .expect_err("the file is absent, so the expected digest does not hold");
    assert_eq!(refusal.code, "workspace.stale-content");
    assert!(
        !directory.path().join(WORKSPACE).join("new").exists(),
        "a refused replacement created the directory `new`"
    );
}

/// The same for a replacement refused for its size: content one byte over the file limit, whose
/// base64 is within the published length, is `workspace.write-limit`, and the parents it asked
/// for are not there afterwards.
///
/// Adapted: the published length now holds both of its forms, so at the largest file limit no
/// content is over the limit and within the published length. The case serves the workspace with
/// a lower file limit, where one is.
#[test]
fn a_replacement_refused_for_its_size_creates_no_parent_directory() {
    const FILE_LIMIT: u64 = 1_024;
    let (directory, filesystem) = workspace_holding_with_file_limit(SEED.as_bytes(), FILE_LIMIT);
    let content = vec![b'a'; usize::try_from(FILE_LIMIT).expect("u64 fits usize") + 1];
    let mut input = replacement(&content, ExpectedFileState::Absent);
    input.create_parents = true;
    assert!(input.validate_text_bounds().is_ok());
    let refusal = filesystem
        .replace_cas(WORKSPACE, WORKSPACE, "new/dir/file.txt", &input)
        .expect_err("content over the file limit");
    assert_eq!(refusal.code, "workspace.write-limit");
    assert!(
        !directory.path().join(WORKSPACE).join("new").exists(),
        "a refused replacement created the directory `new`"
    );
}
