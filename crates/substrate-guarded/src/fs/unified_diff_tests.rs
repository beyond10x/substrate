//! The diff a file change carries is a unified diff that Git applies
//! (story:file-change-diff-is-a-unified-diff-git-applies).
//!
//! The rule: a `diff.text` that is neither `truncated` nor `binary` and is not empty applies
//! with `git apply` to the file as it was before the change and gives the file as it is after.
//! Each case makes a change through the guarded file system, applies the diff the result
//! carries with the `git` executable in a new repository that holds the former bytes, and
//! compares every byte with what the change wrote. Without `git` on `PATH` every such case
//! fails; none is skipped.

use std::path::Path;
use std::process::{Command, Output};

use base64::Engine as _;
use sha2::{Digest as _, Sha256};
use substrate_wire::{
    Base64Content, Base64Encoding, ExpectedFileState, FileEditInput, FileMutationResult,
    FilePatchInput, FileReplaceInput, LinePatchEdit, TextMatchPolicy, UnifiedDiff,
};
use tempfile::tempdir;

use super::{GuardedFilesystem, bounded_diff};

const WORKSPACE: &str = "ws";

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Runs `git` in `directory` with no configuration but its built-in defaults.
fn git(directory: &Path, arguments: &[&str]) -> Output {
    Command::new("git")
        .args(arguments)
        .current_dir(directory)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("this case runs the git executable on PATH and fails without it")
}

/// What `git apply` makes of `path` in a new repository where it holds `before`, or is absent:
/// the bytes it holds afterwards, or `None` when it is absent then.
fn applied_by_git(path: &str, before: Option<&[u8]>, diff: &UnifiedDiff) -> Option<Vec<u8>> {
    assert!(!diff.binary, "a text change");
    assert!(!diff.truncated, "the whole diff: {}", diff.text);
    let directory = tempdir().expect("tempdir");
    let repository = directory.path().join("repository");
    std::fs::create_dir(&repository).expect("repository directory");
    let initialized = git(&repository, &["init", "--quiet"]);
    assert!(initialized.status.success(), "git init: {initialized:?}");
    if let Some(before) = before {
        let file = repository.join(path);
        std::fs::create_dir_all(file.parent().expect("parent")).expect("parents");
        std::fs::write(file, before).expect("former bytes");
    }
    std::fs::write(directory.path().join("change.diff"), &diff.text).expect("diff file");
    let applied = git(&repository, &["apply", "../change.diff"]);
    assert!(
        applied.status.success(),
        "git apply answered {} for this diff:\n{}\n{}",
        applied.status,
        diff.text,
        String::from_utf8_lossy(&applied.stderr)
    );
    std::fs::read(repository.join(path)).ok()
}

/// Makes `change` to `path` in a workspace where it holds `before`, or is absent, and asserts
/// that Git makes the same bytes of the former file with the diff the result carries. Returns
/// that diff's text.
fn assert_git_applies_the_diff_of(
    path: &str,
    before: Option<&str>,
    change: impl FnOnce(&GuardedFilesystem) -> FileMutationResult,
) -> String {
    let directory = tempdir().expect("tempdir");
    let filesystem = GuardedFilesystem::open(directory.path(), 1024 * 1024, 1024 * 1024, 100)
        .expect("guarded filesystem");
    assert!(
        filesystem.openat2_available(),
        "this case needs openat2; absent, it proves nothing and must not read as passed"
    );
    let file = directory.path().join(WORKSPACE).join(path);
    std::fs::create_dir(directory.path().join(WORKSPACE)).expect("workspace root");
    if let Some(before) = before {
        std::fs::create_dir_all(file.parent().expect("parent")).expect("parents");
        std::fs::write(&file, before).expect("former bytes");
    }
    let result = change(&filesystem);
    let written = std::fs::read(&file).expect("the changed file");
    assert_eq!(result.after_sha256, sha256(&written));
    let applied = applied_by_git(path, before.map(str::as_bytes), &result.diff);
    assert_eq!(
        applied.as_deref().map(String::from_utf8_lossy),
        Some(String::from_utf8_lossy(&written)),
        "git apply gave other bytes than the change wrote, with this diff:\n{}",
        result.diff.text
    );
    result.diff.text
}

fn replacement(content: &str, before: Option<&str>) -> FileReplaceInput {
    FileReplaceInput {
        content: Base64Content {
            encoding: Base64Encoding::Base64,
            data: base64::engine::general_purpose::STANDARD.encode(content),
        },
        expected: before.map_or(ExpectedFileState::Absent, |before| {
            ExpectedFileState::Sha256 {
                sha256: sha256(before.as_bytes()),
            }
        }),
        create_parents: true,
    }
}

/// Replaces the content of `path`, which held `before` or was absent, and holds the diff to Git.
fn assert_git_applies_a_replacement(path: &str, before: Option<&str>, after: &str) -> String {
    assert_git_applies_the_diff_of(path, before, |filesystem| {
        filesystem
            .replace_cas(WORKSPACE, WORKSPACE, path, &replacement(after, before))
            .expect("replacement")
    })
}

fn assert_git_applies_an_edit(path: &str, before: &str, old_text: &str, new_text: &str) -> String {
    assert_git_applies_the_diff_of(path, Some(before), |filesystem| {
        filesystem
            .edit_cas(
                WORKSPACE,
                WORKSPACE,
                path,
                &FileEditInput {
                    expected_sha256: sha256(before.as_bytes()),
                    old_text: old_text.to_owned(),
                    new_text: new_text.to_owned(),
                    match_policy: TextMatchPolicy::Exact,
                },
            )
            .expect("edit")
    })
}

fn assert_git_applies_a_patch(path: &str, before: &str, edits: Vec<LinePatchEdit>) -> String {
    assert_git_applies_the_diff_of(path, Some(before), |filesystem| {
        filesystem
            .patch_cas(
                WORKSPACE,
                WORKSPACE,
                path,
                &FilePatchInput {
                    expected_sha256: sha256(before.as_bytes()),
                    edits,
                },
            )
            .expect("patch")
    })
}

/// `count` lines, each `prefix` and its number from one, each ended by LF.
fn numbered_lines(prefix: &str, count: usize) -> String {
    use std::fmt::Write as _;

    let mut text = String::new();
    for line in 1..=count {
        writeln!(text, "{prefix}{line}").expect("a String takes every write");
    }
    text
}

/// Twelve lines, `l1` to `l12`, each ended by LF.
fn twelve_lines() -> String {
    numbered_lines("l", 12)
}

/// The change the defect was seen with: one line `alpha` replaced by `bravo`. The hunk header
/// names both line ranges and no line stands between the removed and the added one.
#[test]
fn git_applies_the_diff_of_a_replacement_of_one_line() {
    let text = assert_git_applies_a_replacement("main.txt", Some("alpha\n"), "bravo\n");
    assert_eq!(
        text,
        "--- a/main.txt\n+++ b/main.txt\n@@ -1,1 +1,1 @@\n-alpha\n+bravo\n"
    );
}

/// An edit in the middle of a file: the hunk holds the changed line between three unchanged
/// lines on either side, and its ranges count them.
#[test]
fn git_applies_the_diff_of_an_edit_and_the_hunk_names_its_line_ranges() {
    let text = assert_git_applies_an_edit("src/main.txt", &twelve_lines(), "l6\n", "L6\nL6b\n");
    assert_eq!(
        text,
        "--- a/src/main.txt\n+++ b/src/main.txt\n@@ -3,7 +3,8 @@\n l3\n l4\n l5\n-l6\n+L6\n+L6b\n \
         l7\n l8\n l9\n"
    );
}

/// A patch of several edits, at the first line, in the middle and at the last line.
#[test]
fn git_applies_the_diff_of_a_line_patch() {
    assert_git_applies_a_patch(
        "main.txt",
        &twelve_lines(),
        vec![
            LinePatchEdit::InsertBefore {
                line: 1,
                text: "head".to_owned(),
            },
            LinePatchEdit::DeleteRange {
                start_line: 5,
                end_line: 6,
            },
            LinePatchEdit::ReplaceRange {
                start_line: 12,
                end_line: 12,
                text: "tail\nend".to_owned(),
            },
        ],
    );
    assert_git_applies_a_patch(
        "main.txt",
        &twelve_lines(),
        vec![LinePatchEdit::DeleteRange {
            start_line: 2,
            end_line: 2,
        }],
    );
}

/// A last line without a line ending is followed by the marker Git reads for it, on the side or
/// sides that lack the ending: a change of that line, a change above it that shows it as
/// context, a change that removes the ending and one that adds it.
#[test]
fn git_applies_the_diff_of_a_file_without_a_final_newline() {
    let text = assert_git_applies_an_edit("main.txt", "one\ntwo", "two", "TWO");
    assert_eq!(
        text,
        "--- a/main.txt\n+++ b/main.txt\n@@ -1,2 +1,2 @@\n one\n-two\n\\ No newline at end of \
         file\n+TWO\n\\ No newline at end of file\n"
    );
    let text = assert_git_applies_an_edit("main.txt", "one\ntwo", "one", "ONE");
    assert_eq!(
        text,
        "--- a/main.txt\n+++ b/main.txt\n@@ -1,2 +1,2 @@\n-one\n+ONE\n two\n\\ No newline at end \
         of file\n"
    );
    assert_git_applies_a_replacement("main.txt", Some("one\ntwo\n"), "one\ntwo");
    assert_git_applies_a_replacement("main.txt", Some("one\ntwo"), "one\ntwo\n");
    assert_git_applies_a_patch(
        "main.txt",
        "one\ntwo\nthree",
        vec![LinePatchEdit::InsertAfter {
            line: 3,
            text: "four".to_owned(),
        }],
    );
}

/// A file the change creates: the diff names no former file, and Git creates the file and the
/// directories above it.
#[test]
fn git_applies_the_diff_of_a_new_file() {
    let text = assert_git_applies_a_replacement("new/dir/file.txt", None, "one\ntwo\n");
    assert_eq!(
        text,
        "--- /dev/null\n+++ b/new/dir/file.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n"
    );
    assert_git_applies_a_replacement("file.txt", None, "one\ntwo");
}

/// A file that held nothing, and a file that holds nothing afterwards and still exists.
#[test]
fn git_applies_the_diff_of_a_change_from_and_to_an_empty_file() {
    let text = assert_git_applies_a_replacement("main.txt", Some(""), "one\n");
    assert_eq!(
        text,
        "--- a/main.txt\n+++ b/main.txt\n@@ -0,0 +1,1 @@\n+one\n"
    );
    let text = assert_git_applies_a_replacement("main.txt", Some("one\ntwo\n"), "");
    assert_eq!(
        text,
        "--- a/main.txt\n+++ b/main.txt\n@@ -1,2 +0,0 @@\n-one\n-two\n"
    );
}

/// Lines ended by CRLF are diff lines like any other: the carriage return is content.
#[test]
fn git_applies_the_diff_of_a_file_with_crlf_endings() {
    assert_git_applies_an_edit("main.txt", "one\r\ntwo\r\nthree\r\n", "two", "TWO");
    assert_git_applies_an_edit("main.txt", "one\r\ntwo\nthree", "three", "THREE\r\n");
}

/// Content that looks like a part of a diff is still content.
#[test]
fn git_applies_the_diff_of_lines_that_look_like_diff_lines() {
    assert_git_applies_a_replacement(
        "main.txt",
        Some("--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n\\ No newline at end of file\n"),
        "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n+b\n\n\\ No newline at end of file",
    );
}

/// A path is written so that Git reads the same name: with a space in it, and quoted when it
/// holds a quote or a control character. The last two paths are of files the change creates.
#[test]
fn git_applies_the_diff_of_a_path_with_spaces_quotes_and_control_characters() {
    for path in [
        "my notes/a file.txt",
        "x 2026-01-01 00:00:00",
        "say \"hi\".txt",
        "tab\there.txt",
        "line\nbreak.txt",
        "bell\u{7}\u{7f}.txt",
        "caf\u{e9}/\u{1f600}.txt",
    ] {
        assert_git_applies_a_replacement(path, Some("alpha\n"), "bravo\n");
    }
    let text = assert_git_applies_a_replacement("new \"file\"\t.txt", None, "bravo\n");
    assert!(
        text.starts_with("--- /dev/null\n+++ \"b/new \\\"file\\\"\\t.txt\"\n"),
        "{text}"
    );
    let text = assert_git_applies_a_replacement("new file.txt", None, "bravo\n");
    assert!(
        text.starts_with("--- /dev/null\n+++ b/new file.txt\t\n"),
        "{text}"
    );
}

/// A change that changes no line has no hunk, and a unified diff without a hunk is no diff: the
/// text is empty and is not marked `truncated`. That is the answer for the same bytes written
/// again and for an empty file created.
#[test]
fn a_change_without_a_changed_line_carries_an_empty_text() {
    let unchanged = UnifiedDiff {
        text: String::new(),
        truncated: false,
        binary: false,
    };
    assert_eq!(
        bounded_diff("main.txt", Some(b"one\ntwo"), b"one\ntwo"),
        unchanged
    );
    assert_eq!(bounded_diff("main.txt", Some(b""), b""), unchanged);
    assert_eq!(bounded_diff("main.txt", None, b""), unchanged);
}

/// A change far from both ends of a long file is one hunk of the changed lines and their
/// context, within the line bound, so it is whole and Git applies it.
#[test]
fn git_applies_the_diff_of_one_changed_line_in_a_long_file() {
    let before = numbered_lines("line ", 5_000);
    let text = assert_git_applies_an_edit("main.txt", &before, "line 2500\n", "changed\n");
    assert_eq!(text.lines().count(), 11, "{text}");
    assert!(text.contains("\n@@ -2497,7 +2497,7 @@\n"), "{text}");
}

/// A diff over the line bound is cut to it, marked `truncated`, and not claimed to apply.
#[test]
fn a_diff_over_the_line_bound_is_cut_and_says_so() {
    let before = numbered_lines("old ", 300);
    let after = numbered_lines("new ", 300);
    let diff = bounded_diff("main.txt", Some(before.as_bytes()), after.as_bytes());
    assert!(diff.truncated);
    assert!(!diff.binary);
    assert_eq!(diff.text.lines().count(), 200);
    assert!(
        diff.text
            .starts_with("--- a/main.txt\n+++ b/main.txt\n@@ -1,300 +1,300 @@\n-old 1\n")
    );
    assert!(diff.text.ends_with("-old 197\n"));
}
