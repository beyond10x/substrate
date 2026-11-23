//! A v2 file change is written only when its result fits what the caller can record
//! (story:file-change-result-fits-the-ledger-before-the-file-changes).
//!
//! The caller names the bytes the whole result may occupy as compact JSON. The diff is cut to
//! that, with JSON escaping counted, and to the length its schema publishes, before the file is
//! written; a change for which no result fits is refused before anything is written. The texts of
//! an edit, of a patch and of a replacement over their published length are refused the same way.

use sha2::{Digest as _, Sha256};
use substrate_wire::{
    Base64Content, Base64Encoding, ExpectedFileState, FileEditInput, FileKind, FileMutationResult,
    FilePatchInput, FileReplaceInput, LinePatchEdit, MAX_FILE_BYTES, MAX_FILE_CONTENT_BASE64_CHARS,
    MAX_FILE_DIFF_TEXT_CHARS, MAX_FILE_TEXT_CHARS, MAX_IO_BYTES, TextMatchPolicy, UnifiedDiff,
};
use tempfile::{TempDir, tempdir};

use base64::Engine as _;

use super::{GuardedFilesystem, fitting_prefix, json_char_bytes, widest_result_json_bytes};
use crate::DriverError;

const WORKSPACE: &str = "ws";
const PATH: &str = "main.txt";
const SEED: &str = "seed\n";

/// A caller's bound far below what the texts of these cases cost as JSON.
const RESULT_JSON_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug)]
enum Change {
    Replacement,
    Edit,
    Patch,
}

const CHANGES: [Change; 3] = [Change::Replacement, Change::Edit, Change::Patch];

/// Three lines of 250 000 characters: far fewer lines than a diff holds.
fn text_with_long_lines() -> String {
    vec!["a".repeat(250_000); 3].join("\n")
}

/// 100 lines of characters that JSON writes in more bytes than the file holds: a quote, a
/// backslash, two control characters, a tab, and one character outside the basic plane, which
/// JSON writes as it is.
fn text_that_json_escapes() -> String {
    vec!["\"\\\u{1}\u{1f}\t\u{1f600}".repeat(400); 100].join("\n")
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A workspace `ws` whose `main.txt` holds `seed`.
fn workspace_holding(seed: &[u8]) -> (TempDir, GuardedFilesystem) {
    let directory = tempdir().expect("tempdir");
    let filesystem = GuardedFilesystem::open(directory.path(), MAX_FILE_BYTES, MAX_IO_BYTES, 100)
        .expect("guarded filesystem");
    assert!(
        filesystem.openat2_available(),
        "this case needs openat2; absent, it proves nothing and must not read as passed"
    );
    std::fs::create_dir(directory.path().join(WORKSPACE)).expect("workspace root");
    std::fs::write(directory.path().join(WORKSPACE).join(PATH), seed).expect("seed file");
    (directory, filesystem)
}

fn on_disk(directory: &TempDir) -> Vec<u8> {
    std::fs::read(directory.path().join(WORKSPACE).join(PATH)).expect("file")
}

fn replacement(content: &[u8], expected: ExpectedFileState) -> FileReplaceInput {
    FileReplaceInput {
        content: Base64Content {
            encoding: Base64Encoding::Base64,
            data: base64::engine::general_purpose::STANDARD.encode(content),
        },
        expected,
        create_parents: false,
    }
}

fn edit(old_text: &str, new_text: &str) -> FileEditInput {
    FileEditInput {
        expected_sha256: sha256(SEED.as_bytes()),
        old_text: old_text.to_owned(),
        new_text: new_text.to_owned(),
        match_policy: TextMatchPolicy::Exact,
    }
}

fn patch(text: &str) -> FilePatchInput {
    FilePatchInput {
        expected_sha256: sha256(SEED.as_bytes()),
        edits: vec![LinePatchEdit::ReplaceRange {
            start_line: 1,
            end_line: 1,
            text: text.to_owned(),
        }],
    }
}

/// Puts `text` where [`SEED`] was, by one of the three changes, and returns the bytes the file
/// must hold afterwards beside what the change answered.
fn change_seed_to(
    filesystem: &GuardedFilesystem,
    change: Change,
    text: &str,
    result_json_bytes: u64,
) -> (Vec<u8>, Result<FileMutationResult, DriverError>) {
    match change {
        Change::Replacement => (
            text.as_bytes().to_vec(),
            filesystem.replace_cas(
                WORKSPACE,
                WORKSPACE,
                PATH,
                &replacement(
                    text.as_bytes(),
                    ExpectedFileState::Sha256 {
                        sha256: sha256(SEED.as_bytes()),
                    },
                ),
                result_json_bytes,
            ),
        ),
        Change::Edit => (
            format!("{text}\n").into_bytes(),
            filesystem.edit_cas(
                WORKSPACE,
                WORKSPACE,
                PATH,
                &edit("seed", text),
                result_json_bytes,
            ),
        ),
        Change::Patch => (
            format!("{text}\n").into_bytes(),
            filesystem.patch_cas(WORKSPACE, WORKSPACE, PATH, &patch(text), result_json_bytes),
        ),
    }
}

/// The rule for one change: the file holds the new content, and the answered result, as the JSON
/// the caller records, is within the caller's bound and within the published diff length, cut and
/// marked `truncated`.
fn assert_the_file_changes_and_the_result_fits(change: Change, text: &str) {
    let (directory, filesystem) = workspace_holding(SEED.as_bytes());
    let (content, answer) = change_seed_to(&filesystem, change, text, RESULT_JSON_BYTES);
    assert!(u64::try_from(content.len()).expect("usize fits u64") <= MAX_FILE_BYTES);
    let result = answer.unwrap_or_else(|error| panic!("{change:?} is admitted: {error}"));
    assert_eq!(on_disk(&directory), content, "{change:?} changed the file");
    assert_eq!(result.after_sha256, sha256(&content));
    assert_eq!(
        result.before_sha256.as_deref(),
        Some(sha256(SEED.as_bytes()).as_str())
    );
    assert!(
        u64::try_from(result.json_bytes()).expect("usize fits u64") <= RESULT_JSON_BYTES,
        "{change:?} answered {} bytes of JSON for a bound of {RESULT_JSON_BYTES}",
        result.json_bytes()
    );
    assert!(result.diff.truncated, "a cut diff says so");
    assert!(!result.diff.binary);
    assert!(
        result
            .diff
            .text
            .starts_with("--- a/main.txt\n+++ b/main.txt\n")
    );
    assert!(result.diff.text.chars().count() <= MAX_FILE_DIFF_TEXT_CHARS);
}

#[test]
fn a_replacement_with_long_lines_changes_the_file_and_its_result_fits_the_callers_bound() {
    assert_the_file_changes_and_the_result_fits(Change::Replacement, &text_with_long_lines());
}

#[test]
fn an_edit_with_long_lines_changes_the_file_and_its_result_fits_the_callers_bound() {
    assert_the_file_changes_and_the_result_fits(Change::Edit, &text_with_long_lines());
}

#[test]
fn a_patch_with_long_lines_changes_the_file_and_its_result_fits_the_callers_bound() {
    assert_the_file_changes_and_the_result_fits(Change::Patch, &text_with_long_lines());
}

#[test]
fn a_replacement_with_json_escaped_text_changes_the_file_and_its_result_fits_the_callers_bound() {
    assert_the_file_changes_and_the_result_fits(Change::Replacement, &text_that_json_escapes());
}

#[test]
fn an_edit_with_json_escaped_text_changes_the_file_and_its_result_fits_the_callers_bound() {
    assert_the_file_changes_and_the_result_fits(Change::Edit, &text_that_json_escapes());
}

#[test]
fn a_patch_with_json_escaped_text_changes_the_file_and_its_result_fits_the_callers_bound() {
    assert_the_file_changes_and_the_result_fits(Change::Patch, &text_that_json_escapes());
}

/// The bound is exact. `needed` is what the widest result for this workspace and path costs
/// without its diff text, plus what the whole diff text costs as JSON. A caller that allows
/// exactly that gets the whole diff; one byte less and the diff is cut and says so. The file is
/// changed in both.
#[test]
fn a_result_exactly_at_the_bound_keeps_its_whole_diff_and_one_byte_less_cuts_it() {
    let text = "line \"one\"\n\tline \\two\\ \u{1} \u{1f600}";
    for change in CHANGES {
        let (_directory, filesystem) = workspace_holding(SEED.as_bytes());
        let (_, whole) = change_seed_to(&filesystem, change, text, u64::MAX);
        let whole = whole.expect("unbounded change").diff;
        assert!(
            !whole.truncated,
            "{change:?}: an unbounded caller gets the whole diff"
        );
        let text_json_bytes: usize = whole.text.chars().map(json_char_bytes).sum();
        let needed = u64::try_from(widest_result_json_bytes(WORKSPACE, PATH) + text_json_bytes)
            .expect("usize fits u64");

        let (directory, filesystem) = workspace_holding(SEED.as_bytes());
        let (content, at_the_bound) = change_seed_to(&filesystem, change, text, needed);
        let at_the_bound = at_the_bound.expect("change at the bound");
        assert_eq!(at_the_bound.diff, whole, "{change:?}: the whole diff fits");
        assert!(u64::try_from(at_the_bound.json_bytes()).expect("usize fits u64") <= needed);
        assert_eq!(on_disk(&directory), content);

        let (directory, filesystem) = workspace_holding(SEED.as_bytes());
        let (content, one_less) = change_seed_to(&filesystem, change, text, needed - 1);
        let one_less = one_less.expect("change one byte beneath the bound");
        assert!(one_less.diff.truncated, "{change:?}: a cut diff says so");
        assert!(whole.text.starts_with(&one_less.diff.text));
        assert!(one_less.diff.text.len() < whole.text.len());
        assert!(u64::try_from(one_less.json_bytes()).expect("usize fits u64") < needed);
        assert_eq!(
            on_disk(&directory),
            content,
            "{change:?}: the file still changes"
        );
    }
}

/// A caller whose bound holds no result at all, not even one with an empty diff, gets a refusal
/// and an untouched workspace: the file is the one that was there, and a replacement asked to
/// create its parents created none.
#[test]
fn a_change_whose_smallest_result_exceeds_the_bound_is_refused_before_anything_is_written() {
    let too_small = u64::try_from(widest_result_json_bytes(WORKSPACE, PATH) - 1).expect("u64");
    for change in CHANGES {
        let (directory, filesystem) = workspace_holding(SEED.as_bytes());
        let (_, answer) = change_seed_to(&filesystem, change, "changed", too_small);
        let refusal = answer.expect_err("no result fits");
        assert_eq!(refusal.code, "workspace.write-limit", "{change:?}");
        assert_eq!(on_disk(&directory), SEED.as_bytes(), "{change:?}");
    }
    let (directory, filesystem) = workspace_holding(SEED.as_bytes());
    let mut input = replacement(b"changed", ExpectedFileState::Absent);
    input.create_parents = true;
    let too_small =
        u64::try_from(widest_result_json_bytes(WORKSPACE, "new/dir/file.txt") - 1).expect("u64");
    let refusal = filesystem
        .replace_cas(WORKSPACE, WORKSPACE, "new/dir/file.txt", &input, too_small)
        .expect_err("no result fits");
    assert_eq!(refusal.code, "workspace.write-limit");
    assert!(!directory.path().join(WORKSPACE).join("new").exists());
}

/// Two files at the file limit, one line each, make a diff longer than `diff.text` may be. The
/// answered text is within the length the result schema publishes, whatever the caller allows.
#[test]
fn a_diff_never_exceeds_the_length_its_result_schema_publishes() {
    let size = usize::try_from(MAX_FILE_BYTES).expect("u64 fits usize");
    let (directory, filesystem) = workspace_holding("a".repeat(size).as_bytes());
    let after = "b".repeat(size);
    let result = filesystem
        .replace_cas(
            WORKSPACE,
            WORKSPACE,
            PATH,
            &replacement(
                after.as_bytes(),
                ExpectedFileState::Sha256 {
                    sha256: sha256("a".repeat(size).as_bytes()),
                },
            ),
            u64::MAX,
        )
        .expect("replacement");
    assert_eq!(on_disk(&directory), after.as_bytes());
    assert_eq!(result.diff.text.chars().count(), MAX_FILE_DIFF_TEXT_CHARS);
    assert!(result.diff.truncated);
}

/// The cut falls between characters and never inside one character's escape, and it holds both
/// bounds: the JSON bytes and the character count.
#[test]
fn a_cut_falls_between_characters_and_holds_the_byte_and_the_character_bound() {
    // `a` costs 1 byte, U+0001 costs 6 (`\u0001`), `b` costs 1.
    let text = "a\u{1}b";
    assert_eq!(fitting_prefix(text, 0, usize::MAX), 0);
    for json_bytes in 1..=6 {
        assert_eq!(
            fitting_prefix(text, json_bytes, usize::MAX),
            1,
            "{json_bytes}"
        );
    }
    assert_eq!(fitting_prefix(text, 7, usize::MAX), 2);
    assert_eq!(fitting_prefix(text, 8, usize::MAX), 3);
    // One character outside the basic plane is four bytes, written as it is.
    assert_eq!(fitting_prefix("\u{1f600}", 3, usize::MAX), 0);
    assert_eq!(fitting_prefix("\u{1f600}", 4, usize::MAX), 4);
    // The character bound counts characters, not bytes.
    assert_eq!(fitting_prefix("\u{e9}\u{e9}\u{e9}", usize::MAX, 2), 4);
    assert_eq!(fitting_prefix("\u{e9}\u{e9}\u{e9}", usize::MAX, 3), 6);
}

/// The cost this crate counts for a character is the cost the result's own serializer writes, for
/// every character there is.
#[test]
fn the_json_cost_counted_for_every_character_is_what_the_result_serializer_writes() {
    let result = |text: String| FileMutationResult {
        kind: FileKind::File,
        workspace: WORKSPACE.to_owned(),
        path: PATH.to_owned(),
        size: 0,
        before_sha256: None,
        after_sha256: "0".repeat(64),
        atomic_replacement: true,
        diff: UnifiedDiff {
            text,
            truncated: false,
            binary: false,
        },
        observed_at: chrono::DateTime::UNIX_EPOCH,
    };
    let empty = result(String::new()).json_bytes();
    for code in 0..=0xff_u32 {
        let character = char::from_u32(code).expect("a character");
        assert_eq!(
            result(character.to_string()).json_bytes() - empty,
            json_char_bytes(character),
            "U+{code:04X}"
        );
    }
    let every: String = (0..=0x0010_ffff_u32).filter_map(char::from_u32).collect();
    let counted: usize = every.chars().map(json_char_bytes).sum();
    assert_eq!(result(every).json_bytes() - empty, counted);
}

/// The server holds the published `maxLength` of an edit's two texts itself. JSON Schema counts
/// characters: a text of exactly that many two-byte characters is within it and is answered by
/// the edit's own rules, and one character more is `request.schema-invalid` with the file
/// untouched.
#[test]
fn an_edit_text_over_its_published_length_is_refused_before_anything_is_written() {
    let over = "a".repeat(MAX_FILE_TEXT_CHARS + 1);
    for input in [edit(&over, "changed"), edit("seed", &over)] {
        let (directory, filesystem) = workspace_holding(SEED.as_bytes());
        let refusal = filesystem
            .edit_cas(WORKSPACE, WORKSPACE, PATH, &input, RESULT_JSON_BYTES)
            .expect_err("a text over the published length");
        assert_eq!(refusal.code, "request.schema-invalid");
        assert_eq!(on_disk(&directory), SEED.as_bytes());
    }
    let at_the_bound = "\u{e9}".repeat(MAX_FILE_TEXT_CHARS);
    assert!(at_the_bound.len() > MAX_FILE_TEXT_CHARS);
    let (directory, filesystem) = workspace_holding(SEED.as_bytes());
    let refusal = filesystem
        .edit_cas(
            WORKSPACE,
            WORKSPACE,
            PATH,
            &edit(&at_the_bound, "changed"),
            RESULT_JSON_BYTES,
        )
        .expect_err("a text the file does not hold");
    assert_eq!(refusal.code, "workspace.edit-not-found");
    assert_eq!(on_disk(&directory), SEED.as_bytes());
}

#[test]
fn a_patch_text_over_its_published_length_is_refused_before_anything_is_written() {
    let (directory, filesystem) = workspace_holding(SEED.as_bytes());
    let refusal = filesystem
        .patch_cas(
            WORKSPACE,
            WORKSPACE,
            PATH,
            &patch(&"a".repeat(MAX_FILE_TEXT_CHARS + 1)),
            RESULT_JSON_BYTES,
        )
        .expect_err("a text over the published length");
    assert_eq!(refusal.code, "request.schema-invalid");
    assert_eq!(on_disk(&directory), SEED.as_bytes());
}

/// A replacement's `content.data` longer than the published base64 of a file at the limit is
/// `request.schema-invalid`, and the parents it asked for are not created.
#[test]
fn a_replacement_content_over_its_published_length_is_refused_before_anything_is_written() {
    let (directory, filesystem) = workspace_holding(SEED.as_bytes());
    let input = FileReplaceInput {
        content: Base64Content {
            encoding: Base64Encoding::Base64,
            data: "A".repeat(MAX_FILE_CONTENT_BASE64_CHARS + 4),
        },
        expected: ExpectedFileState::Absent,
        create_parents: true,
    };
    let refusal = filesystem
        .replace_cas(
            WORKSPACE,
            WORKSPACE,
            "new/dir/file.txt",
            &input,
            RESULT_JSON_BYTES,
        )
        .expect_err("content over the published length");
    assert_eq!(refusal.code, "request.schema-invalid");
    assert!(!directory.path().join(WORKSPACE).join("new").exists());
    assert_eq!(on_disk(&directory), SEED.as_bytes());
}
