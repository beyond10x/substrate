//! A line patch changes only the lines it names
//! (story:line-patch-changes-only-the-lines-it-names).
//!
//! Every line the patch does not name keeps its bytes, its own line ending included. A line the
//! patch writes ends as the line its edit names ends. No line ending is written for a line that
//! is gone: a file whose every line is deleted is empty. Each case compares every byte of the
//! result with a literal.

use sha2::{Digest as _, Sha256};
use substrate_wire::{FilePatchInput, LinePatchEdit};
use tempfile::tempdir;

use super::{GuardedFilesystem, apply_line_patch};

fn before(line: u32, text: &str) -> LinePatchEdit {
    LinePatchEdit::InsertBefore {
        line,
        text: text.to_owned(),
    }
}

fn after(line: u32, text: &str) -> LinePatchEdit {
    LinePatchEdit::InsertAfter {
        line,
        text: text.to_owned(),
    }
}

fn replace(start_line: u32, end_line: u32, text: &str) -> LinePatchEdit {
    LinePatchEdit::ReplaceRange {
        start_line,
        end_line,
        text: text.to_owned(),
    }
}

fn delete(start_line: u32, end_line: u32) -> LinePatchEdit {
    LinePatchEdit::DeleteRange {
        start_line,
        end_line,
    }
}

/// Asserts the bytes `edits` make of `source`, shown escaped so a wrong line ending is readable.
fn assert_patched(source: &str, edits: &[LinePatchEdit], expected: &str) {
    let patched = apply_line_patch(source, edits).expect("the patch applies");
    assert_eq!(
        patched, expected,
        "{edits:?} on {source:?} gave {patched:?}"
    );
}

/// The bytes the patch route leaves in a file that held `source`.
fn patched_on_disk(source: &str, edits: Vec<LinePatchEdit>) -> Vec<u8> {
    let directory = tempdir().expect("tempdir");
    let filesystem = GuardedFilesystem::open(directory.path(), 1024 * 1024, 1024 * 1024, 100)
        .expect("guarded filesystem");
    assert!(
        filesystem.openat2_available(),
        "this case needs openat2; absent, it proves nothing and must not read as passed"
    );
    std::fs::create_dir(directory.path().join("ws")).expect("workspace root");
    std::fs::write(directory.path().join("ws/notes.txt"), source).expect("seed file");
    let result = filesystem
        .patch_cas(
            "ws",
            "ws",
            "notes.txt",
            &FilePatchInput {
                expected_sha256: hex::encode(Sha256::digest(source.as_bytes())),
                edits,
            },
        )
        .expect("the patch applies");
    let written = std::fs::read(directory.path().join("ws/notes.txt")).expect("patched file");
    assert_eq!(result.after_sha256, hex::encode(Sha256::digest(&written)));
    assert_eq!(result.size, u64::try_from(written.len()).expect("u64"));
    written
}

/// A file of one LF-terminated line whose line is deleted holds no byte: the line's ending goes
/// with the line.
#[test]
fn deleting_the_only_line_of_a_file_leaves_it_empty() {
    assert_patched("alpha\n", &[delete(1, 1)], "");
    assert_eq!(patched_on_disk("alpha\n", vec![delete(1, 1)]), b"");
}

/// The same for a file of several lines, for CRLF endings and for a last line without an ending.
#[test]
fn deleting_every_line_of_a_file_leaves_it_empty() {
    for source in ["one\ntwo\n", "one\r\ntwo\r\n", "one\ntwo", "one\r\n"] {
        let lines = u32::try_from(source.lines().count()).expect("u32");
        assert_patched(source, &[delete(1, lines)], "");
    }
}

/// In a file whose lines end differently, replacing line three leaves lines one, two and four
/// with the endings they had; the written line ends as the line it replaces did.
#[test]
fn replacing_one_line_of_a_file_with_mixed_endings_keeps_every_other_line_ending() {
    let source = "one\r\ntwo\nthree\nfour\r\n";
    let expected = "one\r\ntwo\nTHREE\nfour\r\n";
    assert_patched(source, &[replace(3, 3, "THREE")], expected);
    assert_eq!(
        patched_on_disk(source, vec![replace(3, 3, "THREE")]),
        expected.as_bytes()
    );
    assert_patched(
        "one\ntwo\r\nthree\n",
        &[replace(2, 2, "TWO")],
        "one\nTWO\r\nthree\n",
    );
}

/// Inserted lines end as the line the insert names ends, each of them, and a replacement of
/// several lines by several lines ends each written line as the first line of its range ends.
#[test]
fn written_lines_end_as_the_line_their_edit_names() {
    let source = "one\r\ntwo\nthree\r\nfour\n";
    assert_patched(
        source,
        &[before(1, "A\nB")],
        "A\r\nB\r\none\r\ntwo\nthree\r\nfour\n",
    );
    assert_patched(
        source,
        &[after(2, "A\nB")],
        "one\r\ntwo\nA\nB\nthree\r\nfour\n",
    );
    assert_patched(
        source,
        &[replace(3, 4, "X\nY\nZ")],
        "one\r\ntwo\nX\r\nY\r\nZ\r\n",
    );
    // The line breaks inside an edit's own text only separate its lines: CRLF there writes the
    // same bytes as LF.
    assert_patched(
        source,
        &[after(2, "A\r\nB\r\n")],
        "one\r\ntwo\nA\nB\nthree\r\nfour\n",
    );
}

/// A file whose last line has no ending still ends without one after a patch, and the lines
/// above keep theirs, also where they differ.
#[test]
fn a_file_without_a_final_newline_keeps_that_and_every_untouched_ending() {
    assert_patched(
        "one\r\ntwo\nthree",
        &[replace(3, 3, "THREE")],
        "one\r\ntwo\nTHREE",
    );
    assert_patched(
        "one\r\ntwo\nthree",
        &[replace(1, 1, "ONE")],
        "ONE\r\ntwo\nthree",
    );
    // A line written after the last line: the former last line takes the ending of the line
    // above it, because a line now follows it, and the new last line has none.
    assert_patched(
        "one\r\ntwo\nthree",
        &[after(3, "four\nfive")],
        "one\r\ntwo\nthree\nfour\nfive",
    );
    assert_patched("one", &[after(1, "two")], "one\ntwo");
    assert_patched("one", &[before(1, "zero")], "zero\none");
    assert_patched("one", &[replace(1, 1, "ONE\nTWO")], "ONE\nTWO");
}

/// A file with only CRLF endings has only CRLF endings after a patch, and nothing else changes.
#[test]
fn a_crlf_only_file_keeps_crlf_on_every_line() {
    let source = "one\r\ntwo\r\nthree\r\n";
    assert_patched(
        source,
        &[before(1, "zero"), replace(2, 2, "TWO\nTWO-B"), delete(3, 3)],
        "zero\r\none\r\nTWO\r\nTWO-B\r\n",
    );
    assert_eq!(
        patched_on_disk(source, vec![delete(1, 3)]),
        b"",
        "no line is left, so no ending is"
    );
    assert_patched(
        "one\r\ntwo\r\nthree",
        &[replace(2, 2, "TWO")],
        "one\r\nTWO\r\nthree",
    );
}

/// A carriage return that is not part of a line ending is content: an untouched line keeps it.
#[test]
fn a_carriage_return_inside_a_line_is_kept() {
    assert_patched(
        "a\rb\r\r\nlast\n",
        &[replace(2, 2, "LAST")],
        "a\rb\r\r\nLAST\n",
    );
}
