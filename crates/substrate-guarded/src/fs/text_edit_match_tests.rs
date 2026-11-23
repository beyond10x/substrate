//! A text edit replaces `old_text` only where the file holds it at exactly one offset
//! (story:exact-text-edit-refuses-overlapping-matches).
//!
//! Every offset at which `old_text` starts counts, also one inside another occurrence: `ana`
//! starts at offsets 1 and 3 of `banana`. Two offsets are `workspace.edit-ambiguous`, under both
//! match policies, and the file keeps its bytes.

use sha2::{Digest as _, Sha256};
use substrate_wire::{FileEditInput, TextMatchPolicy};
use tempfile::{TempDir, tempdir};

use super::GuardedFilesystem;
use crate::DriverError;

const POLICIES: [TextMatchPolicy; 2] = [
    TextMatchPolicy::Exact,
    TextMatchPolicy::LineWhitespaceNormalized,
];

/// A workspace `ws` whose `main.txt` holds `source`.
fn workspace_holding(source: &str) -> (TempDir, GuardedFilesystem) {
    let directory = tempdir().expect("tempdir");
    let filesystem = GuardedFilesystem::open(directory.path(), 1024 * 1024, 1024 * 1024, 100)
        .expect("guarded filesystem");
    assert!(
        filesystem.openat2_available(),
        "this case needs openat2; absent, it proves nothing and must not read as passed"
    );
    std::fs::create_dir(directory.path().join("ws")).expect("workspace root");
    std::fs::write(directory.path().join("ws/main.txt"), source).expect("seed file");
    (directory, filesystem)
}

/// What an edit of `old_text` to `new_text` answers for a file holding `source`, and the bytes
/// the file holds afterwards.
fn edit(
    source: &str,
    old_text: &str,
    new_text: &str,
    match_policy: TextMatchPolicy,
) -> (Result<(), DriverError>, Vec<u8>) {
    let (directory, filesystem) = workspace_holding(source);
    let answer = filesystem
        .edit_cas(
            "ws",
            "ws",
            "main.txt",
            &FileEditInput {
                expected_sha256: hex::encode(Sha256::digest(source.as_bytes())),
                old_text: old_text.to_owned(),
                new_text: new_text.to_owned(),
                match_policy,
            },
        )
        .map(|_| ());
    let held = std::fs::read(directory.path().join("ws/main.txt")).expect("file");
    (answer, held)
}

/// Asserts that the edit is refused as ambiguous under both policies and changes no byte.
fn assert_ambiguous(source: &str, old_text: &str) {
    for policy in POLICIES {
        let (answer, held) = edit(source, old_text, "X", policy);
        let refusal = match answer {
            Ok(()) => panic!(
                "{policy:?}: {old_text:?} starts at two offsets of {source:?} and was replaced: \
                 the file holds {:?}",
                String::from_utf8_lossy(&held)
            ),
            Err(refusal) => refusal,
        };
        assert_eq!(refusal.code, "workspace.edit-ambiguous", "{policy:?}");
        assert_eq!(held, source.as_bytes(), "{policy:?}: the file is unchanged");
    }
}

#[test]
fn ana_in_banana_starts_at_two_offsets_and_is_refused() {
    assert_ambiguous("banana", "ana");
}

#[test]
fn aa_in_aaa_starts_at_two_offsets_and_is_refused() {
    assert_ambiguous("aaa", "aa");
}

/// Overlap across line ends and in characters of several bytes is counted the same way.
#[test]
fn overlapping_occurrences_of_several_lines_or_wide_characters_are_refused() {
    assert_ambiguous("x\nx\nx\n", "x\nx\n");
    assert_ambiguous("\u{e9}\u{e9}\u{e9}", "\u{e9}\u{e9}");
    assert_ambiguous("ababab", "abab");
}

/// Under the whitespace-normalized policy the exact occurrences are counted first and by the
/// same rule: two overlapping ones are refused there too, and no normalized match is looked for.
#[test]
fn the_normalized_policy_refuses_two_overlapping_exact_occurrences() {
    for (source, old_text) in [("banana", "ana"), ("  x\n  x\n  x\n", "  x\n  x\n")] {
        let (answer, held) = edit(
            source,
            old_text,
            "X",
            TextMatchPolicy::LineWhitespaceNormalized,
        );
        let refusal = answer.err().unwrap_or_else(|| {
            panic!(
                "{old_text:?} starts at two offsets of {source:?} and was replaced: the file \
                 holds {:?}",
                String::from_utf8_lossy(&held)
            )
        });
        assert_eq!(refusal.code, "workspace.edit-ambiguous");
        assert_eq!(held, source.as_bytes());
    }
}

/// The whitespace-normalized match reads overlap the same way: `old_text` of two lines matches
/// lines one to two and lines two to three of a file of three such lines, which no exact match
/// finds, and the edit is refused with the file unchanged.
#[test]
fn a_normalized_match_at_two_overlapping_line_ranges_is_refused() {
    let source = "  x\n\tx\n  x\n";
    let (answer, held) = edit(
        source,
        "x\nx",
        "X",
        TextMatchPolicy::LineWhitespaceNormalized,
    );
    assert_eq!(
        answer.expect_err("two overlapping ranges").code,
        "workspace.edit-ambiguous"
    );
    assert_eq!(held, source.as_bytes());
    let (answer, held) = edit(source, "x\nx", "X", TextMatchPolicy::Exact);
    assert_eq!(
        answer.expect_err("no exact occurrence").code,
        "workspace.edit-not-found"
    );
    assert_eq!(held, source.as_bytes());
}

/// One occurrence is still replaced, where it is, when the text around it only resembles it.
#[test]
fn a_single_occurrence_is_replaced_at_its_offset() {
    for policy in POLICIES {
        let (answer, held) = edit("banana", "nan", "X", policy);
        answer.expect("one occurrence");
        assert_eq!(held, b"baXa", "{policy:?}");
        let (answer, held) = edit("aab", "ab", "X", policy);
        answer.expect("one occurrence");
        assert_eq!(held, b"aX", "{policy:?}");
    }
}
