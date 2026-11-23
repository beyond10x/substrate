//! What the three v2 file changes hold themselves, and how their result is held to a bound
//! (story:file-change-result-fits-the-ledger-before-the-file-changes).
//!
//! A change answers a diff within the length its schema publishes. A caller that can record only
//! so many bytes for a result holds the answer to that with `FileMutationResult::fit_json_bytes`,
//! which counts every JSON escape; the cases here hold real answers of the three changes to a
//! bound that way. The texts of an edit, of a patch and of a replacement outside their published
//! length are refused before anything is read, created or written, and a refused replacement
//! creates no directory.

use sha2::{Digest as _, Sha256};
use substrate_wire::{
    Base64Content, Base64Encoding, ExpectedFileState, FileEditInput, FileMutationResult,
    FilePatchInput, FileReplaceInput, LinePatchEdit, MAX_FILE_BYTES, MAX_FILE_CONTENT_BASE64_CHARS,
    MAX_FILE_DIFF_TEXT_CHARS, MAX_FILE_TEXT_CHARS, MAX_IO_BYTES, TextMatchPolicy,
};
use tempfile::{TempDir, tempdir};

use base64::Engine as _;

use super::GuardedFilesystem;
use crate::DriverError;

const WORKSPACE: &str = "ws";
const PATH: &str = "main.txt";
const SEED: &str = "seed\n";

/// The bound these cases hold a result to, as a caller with that much room would.
const RESULT_JSON_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug)]
enum Change {
    Replacement,
    Edit,
    Patch,
}

const CHANGES: [Change; 3] = [Change::Replacement, Change::Edit, Change::Patch];

/// A text within the file limit in three lines. A result's bound is in bytes, whatever the
/// number of lines.
fn text_with_long_lines() -> String {
    vec!["a".repeat(250_000); 3].join("\n")
}

/// A text within the file limit made of a quote, a backslash, control characters and a character
/// outside the basic plane. A result's bound counts every character as JSON writes it.
fn text_that_json_escapes() -> String {
    vec!["\"\\\u{1}\u{1f}\t\u{1f600}".repeat(400); 100].join("\n")
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A workspace `ws` whose `main.txt` holds `seed`, served with the largest file limit.
fn workspace_holding(seed: &[u8]) -> (TempDir, GuardedFilesystem) {
    workspace_holding_with_file_limit(seed, MAX_FILE_BYTES)
}

/// A workspace `ws` whose `main.txt` holds `seed`, served with the file limit `max_file_bytes`.
fn workspace_holding_with_file_limit(
    seed: &[u8],
    max_file_bytes: u64,
) -> (TempDir, GuardedFilesystem) {
    let directory = tempdir().expect("tempdir");
    let filesystem = GuardedFilesystem::open(directory.path(), max_file_bytes, MAX_IO_BYTES, 100)
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
            ),
        ),
        Change::Edit => (
            format!("{text}\n").into_bytes(),
            filesystem.edit_cas(WORKSPACE, WORKSPACE, PATH, &edit("seed", text)),
        ),
        Change::Patch => (
            format!("{text}\n").into_bytes(),
            filesystem.patch_cas(WORKSPACE, WORKSPACE, PATH, &patch(text)),
        ),
    }
}

fn json_bytes(result: &FileMutationResult) -> u64 {
    u64::try_from(result.json_bytes()).expect("usize fits u64")
}

/// The rule for one change: the file holds the new content, the answered diff is within its
/// published length, and the answer held to a caller's bound is within that bound as the JSON
/// the caller records, cut and marked `truncated`, with every other member as it was answered.
fn assert_the_file_changes_and_the_result_fits(change: Change, text: &str) {
    let (directory, filesystem) = workspace_holding(SEED.as_bytes());
    let (content, answer) = change_seed_to(&filesystem, change, text);
    assert!(u64::try_from(content.len()).expect("usize fits u64") <= MAX_FILE_BYTES);
    let answered = answer.unwrap_or_else(|error| panic!("{change:?} is admitted: {error}"));
    assert_eq!(on_disk(&directory), content, "{change:?} changed the file");
    assert_eq!(answered.after_sha256, sha256(&content));
    assert_eq!(
        answered.before_sha256.as_deref(),
        Some(sha256(SEED.as_bytes()).as_str())
    );
    assert!(answered.diff.text.chars().count() <= MAX_FILE_DIFF_TEXT_CHARS);

    let mut held = answered.clone();
    held.fit_json_bytes(RESULT_JSON_BYTES);
    assert!(
        json_bytes(&held) <= RESULT_JSON_BYTES,
        "{change:?}: {} bytes of JSON for a bound of {RESULT_JSON_BYTES}",
        json_bytes(&held)
    );
    assert!(held.diff.truncated, "a cut diff says so");
    assert!(!held.diff.binary);
    assert!(
        held.diff
            .text
            .starts_with("--- a/main.txt\n+++ b/main.txt\n")
    );
    assert!(answered.diff.text.starts_with(&held.diff.text));
    let mut answered_without_diff = answered;
    answered_without_diff.diff = held.diff.clone();
    assert_eq!(held, answered_without_diff, "only the diff was cut");
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

/// The bound is exact. An answer held to exactly the bytes it occupies is unchanged, and held to
/// one byte less its diff is cut and says so, with the result beneath the bound.
#[test]
fn a_result_exactly_at_the_bound_keeps_its_whole_diff_and_one_byte_less_cuts_it() {
    let text = "line \"one\"\n\tline \\two\\ \u{1} \u{1f600}";
    for change in CHANGES {
        let (directory, filesystem) = workspace_holding(SEED.as_bytes());
        let (content, answer) = change_seed_to(&filesystem, change, text);
        let whole = answer.expect("change");
        assert_eq!(on_disk(&directory), content);
        assert!(!whole.diff.truncated, "{change:?}: the whole diff");
        let needed = json_bytes(&whole);

        let mut at_the_bound = whole.clone();
        at_the_bound.fit_json_bytes(needed);
        assert_eq!(at_the_bound, whole, "{change:?}: the whole result fits");

        let mut one_less = whole.clone();
        one_less.fit_json_bytes(needed - 1);
        assert!(one_less.diff.truncated, "{change:?}: a cut diff says so");
        assert!(whole.diff.text.starts_with(&one_less.diff.text));
        assert!(one_less.diff.text.len() < whole.diff.text.len());
        assert!(json_bytes(&one_less) < needed);
    }
}

/// The diff text a change answers is within the length its result schema publishes, for two
/// files at the file limit.
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
        )
        .expect("replacement");
    assert_eq!(on_disk(&directory), after.as_bytes());
    assert_eq!(result.diff.text.chars().count(), MAX_FILE_DIFF_TEXT_CHARS);
    assert!(result.diff.truncated);
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
            .edit_cas(WORKSPACE, WORKSPACE, PATH, &input)
            .expect_err("a text over the published length");
        assert_eq!(refusal.code, "request.schema-invalid");
        assert_eq!(on_disk(&directory), SEED.as_bytes());
    }
    let at_the_bound = "\u{e9}".repeat(MAX_FILE_TEXT_CHARS);
    assert!(at_the_bound.len() > MAX_FILE_TEXT_CHARS);
    let (directory, filesystem) = workspace_holding(SEED.as_bytes());
    let refusal = filesystem
        .edit_cas(WORKSPACE, WORKSPACE, PATH, &edit(&at_the_bound, "changed"))
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
        )
        .expect_err("a text over the published length");
    assert_eq!(refusal.code, "request.schema-invalid");
    assert_eq!(on_disk(&directory), SEED.as_bytes());
}

/// A replacement's `content.data` outside the two lengths its schema publishes is
/// `request.schema-invalid`, and the parents it asked for are not created: data longer than the
/// longest form, and data of the longest form's length that does not end `==`.
#[test]
fn a_replacement_content_over_its_published_length_is_refused_before_anything_is_written() {
    for data in [
        "A".repeat(MAX_FILE_CONTENT_BASE64_CHARS + 4),
        "A".repeat(MAX_FILE_CONTENT_BASE64_CHARS),
        format!("{}=", "A".repeat(MAX_FILE_CONTENT_BASE64_CHARS - 1)),
    ] {
        let (directory, filesystem) = workspace_holding(SEED.as_bytes());
        let input = FileReplaceInput {
            content: Base64Content {
                encoding: Base64Encoding::Base64,
                data,
            },
            expected: ExpectedFileState::Absent,
            create_parents: true,
        };
        let refusal = filesystem
            .replace_cas(WORKSPACE, WORKSPACE, "new/dir/file.txt", &input)
            .expect_err("content outside the published length");
        assert_eq!(refusal.code, "request.schema-invalid");
        assert!(!directory.path().join(WORKSPACE).join("new").exists());
        assert_eq!(on_disk(&directory), SEED.as_bytes());
    }
}

/// A replacement that is admitted creates the parents it asked for and writes the file beneath
/// them: the directories are made directly before the write, after every refusal.
#[test]
fn an_admitted_replacement_creates_the_parents_it_asked_for() {
    let (directory, filesystem) = workspace_holding(SEED.as_bytes());
    let mut input = replacement(b"changed", ExpectedFileState::Absent);
    input.create_parents = true;
    let result = filesystem
        .replace_cas(WORKSPACE, WORKSPACE, "new/dir/file.txt", &input)
        .expect("an admitted replacement");
    assert_eq!(result.before_sha256, None);
    assert_eq!(
        std::fs::read(directory.path().join(WORKSPACE).join("new/dir/file.txt")).expect("file"),
        b"changed"
    );
    input.create_parents = false;
    let refusal = filesystem
        .replace_cas(WORKSPACE, WORKSPACE, "other/file.txt", &input)
        .expect_err("no parent, and none asked for");
    assert_eq!(refusal.code, "resource.not-found");
    assert!(!directory.path().join(WORKSPACE).join("other").exists());
}

mod review;
