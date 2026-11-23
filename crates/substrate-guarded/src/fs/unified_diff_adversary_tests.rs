//! Adversarial cases for the rule that the diff a file change carries is a unified diff Git
//! applies (story:file-change-diff-is-a-unified-diff-git-applies).
//!
//! The rule under attack: a `diff.text` that is not empty, not `truncated` and not `binary`
//! applies with `git apply` to the file as it was before the change and gives the file as it is
//! after, byte for byte. Each case here builds the diff with the code under test, applies it
//! with the `git` executable to the former bytes and compares every byte.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use base64::Engine as _;
use sha2::{Digest as _, Sha256};
use substrate_wire::{Base64Content, Base64Encoding, ExpectedFileState, FileReplaceInput};
use tempfile::{TempDir, tempdir};

use super::{GuardedFilesystem, bounded_diff};

const WORKSPACE: &str = "ws";

/// A new Git repository in a temporary directory, with no configuration but Git's defaults.
struct Repository {
    directory: TempDir,
}

impl Repository {
    fn new() -> Self {
        let directory = tempdir().expect("tempdir");
        std::fs::create_dir(directory.path().join("repository")).expect("repository directory");
        let repository = Self { directory };
        let initialized = repository.git(&["init", "--quiet"]);
        assert!(initialized.status.success(), "git init: {initialized:?}");
        repository
    }

    fn root(&self) -> PathBuf {
        self.directory.path().join("repository")
    }

    fn git(&self, arguments: &[&str]) -> Output {
        Command::new("git")
            .args(arguments)
            .current_dir(self.root())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .expect("this case runs the git executable on PATH and fails without it")
    }

    /// Puts `before` at `path` (or makes it absent), applies `diff` with `git apply --check`
    /// and `git apply`, and answers the bytes `path` holds then, `None` for an absent file, or
    /// what Git said when it refused.
    fn apply(
        &self,
        path: &str,
        before: Option<&[u8]>,
        diff: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        let file = self.root().join(path);
        let _ = std::fs::remove_file(&file);
        if let Some(before) = before {
            std::fs::create_dir_all(file.parent().expect("parent")).expect("parents");
            std::fs::write(&file, before).expect("former bytes");
        }
        std::fs::write(self.directory.path().join("change.diff"), diff).expect("diff file");
        for arguments in [
            &["apply", "--check", "../change.diff"][..],
            &["apply", "../change.diff"][..],
        ] {
            let answer = self.git(arguments);
            if !answer.status.success() {
                return Err(format!(
                    "git {} answered {}: {}",
                    arguments.join(" "),
                    answer.status,
                    String::from_utf8_lossy(&answer.stderr).trim()
                ));
            }
        }
        Ok(std::fs::read(&file).ok())
    }
}

/// Holds one change to the rule: a diff that claims to apply must apply and give `after`.
/// Answers `None` when the rule holds or does not claim this diff, and the failure otherwise.
fn broken(
    repository: &Repository,
    path: &str,
    before: Option<&[u8]>,
    after: &[u8],
) -> Option<String> {
    let diff = bounded_diff(path, before, after);
    if diff.binary || diff.truncated || diff.text.is_empty() {
        return None;
    }
    let shown =
        |bytes: Option<&[u8]>| bytes.map(|bytes| String::from_utf8_lossy(bytes).into_owned());
    match repository.apply(path, before, &diff.text) {
        Ok(applied) if applied.as_deref() == Some(after) => None,
        Ok(applied) => Some(format!(
            "git apply gave {:?}, the change wrote {:?}, from {:?} with the diff {:?}",
            shown(applied.as_deref()),
            shown(Some(after)),
            shown(before),
            diff.text
        )),
        Err(refusal) => Some(format!(
            "{refusal}; from {:?} to {:?} with the diff {:?}",
            shown(before),
            shown(Some(after)),
            diff.text
        )),
    }
}

fn assert_none_broken(failures: &[String], cases: usize) {
    assert!(
        failures.is_empty(),
        "{} of {cases} diffs that claim to apply do not; the first:\n{}",
        failures.len(),
        failures[..failures.len().min(3)].join("\n")
    );
}

/// A fixed-seed generator, so a red case is the same case on every run.
struct Generator(u64);

impl Generator {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(bound).expect("bound")).expect("below bound")
    }

    fn line(&mut self) -> String {
        const TEXTS: [&str; 22] = [
            "alpha",
            "bravo",
            "",
            "-minus",
            "+plus",
            "\\ No newline at end of file",
            "@@ -1,2 +1,2 @@",
            "diff --git a/f.txt b/f.txt",
            "--- a/f.txt",
            "+++ b/f.txt",
            "--- /dev/null",
            "\ttab and trailing spaces  ",
            " leading space",
            "carriage\rreturn inside",
            "\r",
            "caf\u{e9} \u{1f600}",
            "index 0000000..1111111 100644",
            "new file mode 100644",
            "deleted file mode 100644",
            "Binary files a/f.txt and b/f.txt differ",
            "GIT binary patch",
            "rename from f.txt",
        ];
        let text = TEXTS[self.below(TEXTS.len())];
        let ending = match self.below(10) {
            0..=5 => "\n",
            6..=8 => "\r\n",
            _ => "\r",
        };
        format!("{text}{ending}")
    }

    fn file(&mut self) -> Vec<String> {
        let count = self.below(10);
        (0..count).map(|_| self.line()).collect()
    }

    /// The bytes of `lines`, one time in three without the line ending of the last line.
    fn bytes(&mut self, lines: &[String]) -> String {
        let mut text = lines.concat();
        if self.below(3) == 0 {
            while text.ends_with(['\n', '\r']) {
                text.pop();
            }
        }
        text
    }
}

/// The property over generated files: lines ended by LF, CRLF or a lone carriage return, with
/// and without a line ending at the end of the file on either side, lines that look like parts
/// of a diff, and a former file that is absent, empty, or changed by up to three line edits.
#[test]
fn git_applies_the_diff_between_any_two_generated_files() {
    let repository = Repository::new();
    let mut generator = Generator(0x9e37_79b9_7f4a_7c15);
    let mut failures = Vec::new();
    let mut claimed = 0;
    let cases = 400;
    for _ in 0..cases {
        let former = generator.file();
        let mut new = if generator.below(6) == 0 {
            generator.file()
        } else {
            former.clone()
        };
        for _ in 0..=generator.below(3) {
            let at = generator.below(new.len() + 1);
            match generator.below(3) {
                0 => new.insert(at, generator.line()),
                1 if at < new.len() => {
                    new.remove(at);
                }
                _ if at < new.len() => new[at] = generator.line(),
                _ => new.push(generator.line()),
            }
        }
        let before = generator.bytes(&former);
        let after = generator.bytes(&new);
        let before = (generator.below(8) != 0).then_some(before.as_bytes());
        let diff = bounded_diff("f.txt", before, after.as_bytes());
        if !(diff.binary || diff.truncated || diff.text.is_empty()) {
            claimed += 1;
        }
        failures.extend(broken(&repository, "f.txt", before, after.as_bytes()));
    }
    assert!(
        claimed > 300,
        "only {claimed} generated diffs claim to apply"
    );
    assert_none_broken(&failures, claimed);
}

/// The ends of a file, one by one: a file that is only a line break, a last line added or
/// removed with and without a line ending on either side, and a change of nothing but the
/// last line's ending.
#[test]
fn git_applies_the_diff_of_every_change_at_the_end_of_a_file() {
    let repository = Repository::new();
    let files = [
        "",
        "\n",
        "\n\n",
        "a",
        "a\n",
        "a\r\n",
        "a\r",
        "a\nb",
        "a\nb\n",
        "a\n\n",
        "a\nb\r\n",
        "\na",
        "a\n\nb",
        "1\n2\n3\n4\n5",
        "1\n2\n3\n4\n5\n",
        "1\n2\n3\n4\n5\n6",
    ];
    let mut failures = Vec::new();
    let mut cases = 0;
    for before in files {
        for after in files {
            cases += 1;
            failures.extend(broken(
                &repository,
                "f.txt",
                Some(before.as_bytes()),
                after.as_bytes(),
            ));
        }
    }
    for after in files {
        cases += 1;
        failures.extend(broken(&repository, "f.txt", None, after.as_bytes()));
    }
    assert_none_broken(&failures, cases);
}

/// A diff of exactly the line bound is whole and applies; one line more is `truncated`. The
/// bound is 200 lines of diff text, the three header lines and the "no newline" line included.
#[test]
fn git_applies_a_diff_of_exactly_the_line_bound_and_one_line_more_is_truncated() {
    use std::fmt::Write as _;

    let repository = Repository::new();
    let lines = |prefix: &str, count: usize, ended: bool| {
        let mut text = String::new();
        for line in 1..=count {
            writeln!(text, "{prefix}{line}").expect("a String takes every write");
        }
        if !ended {
            text.pop();
        }
        text
    };
    // 3 header lines + 98 removed + 99 added = 200.
    let before = lines("old ", 98, true);
    let after = lines("new ", 99, true);
    let diff = bounded_diff("f.txt", Some(before.as_bytes()), after.as_bytes());
    assert_eq!(diff.text.lines().count(), 200);
    assert!(!diff.truncated, "200 lines are within the bound");
    assert_eq!(
        broken(
            &repository,
            "f.txt",
            Some(before.as_bytes()),
            after.as_bytes()
        ),
        None
    );
    // The same change with an unterminated last line needs line 201 for the marker.
    let after = lines("new ", 99, false);
    let diff = bounded_diff("f.txt", Some(before.as_bytes()), after.as_bytes());
    assert!(
        diff.truncated,
        "a diff whose \"no newline\" line is cut says so: {:?}",
        &diff.text[diff.text.len() - 40..]
    );
    // A megabyte with a change on its first and on its last line is one hunk over the bound.
    let mut big = lines("line ", 150_000, true);
    assert!(big.len() > 1024 * 1024);
    let former = big.clone();
    big.replace_range(0..6, "first!");
    big.push_str("last\n");
    let diff = bounded_diff("f.txt", Some(former.as_bytes()), big.as_bytes());
    assert!(diff.truncated && !diff.binary);
    assert!(diff.text.lines().count() <= 200);
}

/// A file that holds a NUL character is valid UTF-8, so its diff is text. The rule still
/// binds it: marked `binary`, or a diff Git applies.
#[test]
fn git_applies_the_diff_of_a_file_that_holds_a_nul_character_or_it_is_binary() {
    let repository = Repository::new();
    let mut failures = Vec::new();
    for (before, after) in [
        (&b"one\ntw\0o\nthree\n"[..], &b"one\ntw\0o\nTHREE\n"[..]),
        (b"one\ntwo\n", b"one\n\0\ntwo\n"),
        (b"\0", b"\0\0"),
        (b"a\0b\n", b"a\n"),
    ] {
        failures.extend(broken(&repository, "f.txt", Some(before), after));
    }
    assert_none_broken(&failures, 4);
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Replaces `path` through the guarded file system, which is what decides that the path is one
/// a caller can change, and holds the diff of the result to Git. `before` absent creates it.
fn replacement_broken(path: &str, before: Option<&str>, after: &str) -> Option<String> {
    let directory = tempdir().expect("tempdir");
    let filesystem = GuardedFilesystem::open(directory.path(), 1024 * 1024, 1024 * 1024, 100)
        .expect("guarded filesystem");
    assert!(filesystem.openat2_available(), "this case needs openat2");
    let root: &Path = directory.path();
    let file = root.join(WORKSPACE).join(path);
    std::fs::create_dir(root.join(WORKSPACE)).expect("workspace root");
    if let Some(before) = before {
        std::fs::create_dir_all(file.parent().expect("parent")).expect("parents");
        std::fs::write(&file, before).expect("former bytes");
    }
    let input = FileReplaceInput {
        content: Base64Content {
            encoding: Base64Encoding::Base64,
            data: base64::engine::general_purpose::STANDARD.encode(after),
        },
        expected: before.map_or(ExpectedFileState::Absent, |before| {
            ExpectedFileState::Sha256 {
                sha256: sha256(before.as_bytes()),
            }
        }),
        create_parents: true,
    };
    // A path the file system does not change carries no diff, so the rule does not bind it.
    let Ok(result) = filesystem.replace_cas(WORKSPACE, WORKSPACE, path, &input) else {
        return None;
    };
    assert_eq!(
        std::fs::read(&file).expect("the changed file"),
        after.as_bytes()
    );
    let diff = result.diff;
    if diff.binary || diff.truncated || diff.text.is_empty() {
        return None;
    }
    let repository = Repository::new();
    match repository.apply(path, before.map(str::as_bytes), &diff.text) {
        Ok(applied) if applied.as_deref() == Some(after.as_bytes()) => None,
        Ok(applied) => Some(format!(
            "path {path:?}: git apply gave {applied:?} with the diff {:?}",
            diff.text
        )),
        Err(refusal) => Some(format!(
            "path {path:?}: {refusal}; the diff {:?}",
            diff.text
        )),
    }
}

/// Path names Git reads back as the same name: a leading dash, a backslash, a leading and a
/// trailing space, characters outside ASCII and control characters outside ASCII.
#[test]
fn git_applies_the_diff_of_a_path_with_a_dash_a_backslash_or_edge_spaces() {
    let mut failures = Vec::new();
    let paths = [
        "-dash.txt",
        "--",
        "dir/-dash.txt",
        "back\\slash.txt",
        "ends with backslash\\",
        "trailing space ",
        " leading space",
        "dir /file ",
        "a\u{85}b\u{2028}c.txt",
        "b/a/b.txt",
        "dev/null",
        "x\t1970-01-01 00:00:00 +0000",
        "x 1970-01-01 00:00:00.000000000 +0000",
        "\"",
        "\u{1}\u{1f}",
    ];
    for path in paths {
        failures.extend(replacement_broken(path, Some("alpha\n"), "bravo\n"));
        failures.extend(replacement_broken(path, None, "bravo"));
    }
    assert_none_broken(&failures, paths.len() * 2);
}

/// Replaces `path` through the guarded file system and answers the diff of the result, or the
/// code of the refusal for a path the file system does not change. `before` absent creates it.
fn replacement_diff(
    path: &str,
    before: Option<&str>,
    after: &str,
) -> Result<super::UnifiedDiff, &'static str> {
    let directory = tempdir().expect("tempdir");
    let filesystem = GuardedFilesystem::open(directory.path(), 1024 * 1024, 1024 * 1024, 100)
        .expect("guarded filesystem");
    assert!(filesystem.openat2_available(), "this case needs openat2");
    let file = directory.path().join(WORKSPACE).join(path);
    std::fs::create_dir(directory.path().join(WORKSPACE)).expect("workspace root");
    if let Some(before) = before {
        std::fs::create_dir_all(file.parent().expect("parent")).expect("parents");
        std::fs::write(&file, before).expect("former bytes");
    }
    let input = FileReplaceInput {
        content: Base64Content {
            encoding: Base64Encoding::Base64,
            data: base64::engine::general_purpose::STANDARD.encode(after),
        },
        expected: before.map_or(ExpectedFileState::Absent, |before| {
            ExpectedFileState::Sha256 {
                sha256: sha256(before.as_bytes()),
            }
        }),
        create_parents: true,
    };
    let result = filesystem
        .replace_cas(WORKSPACE, WORKSPACE, path, &input)
        .map_err(|refusal| refusal.code)?;
    assert_eq!(
        std::fs::read(&file).expect("the changed file"),
        after.as_bytes()
    );
    Ok(result.diff)
}

/// `diff` with only the paths of its two header lines replaced by `name`; a former file that is
/// `/dev/null` stays that.
fn under_the_name(diff: &str, name: &str) -> String {
    let mut parts = diff.splitn(3, '\n');
    let former = parts.next().expect("the former file's line");
    let new = parts.next().expect("the new file's line");
    let hunk = parts.next().expect("the hunk");
    assert!(
        former.starts_with("--- ") && new.starts_with("+++ b/"),
        "{diff:?}"
    );
    assert!(hunk.starts_with("@@ -"), "{diff:?}");
    let former = if former == "--- /dev/null" {
        former.to_owned()
    } else {
        format!("--- a/{name}")
    };
    format!("{former}\n+++ b/{name}\n{hunk}")
}

/// The limit of the rule, held from both sides. The guarded file system changes a path one of
/// whose components Git reads as its own `.git` directory: without regard to case, as `git~1`,
/// and with dots, spaces or a colon after it. Git refuses such a path in a working tree by its
/// own path rule, whatever the patch. For each such path, changed and created:
///
/// - the diff of the change is whole text, and `git apply --check` refuses it by the path's name
///   (`invalid path '<path>'`), so the refusal is Git's path rule and nothing in the diff;
/// - the same diff with only the paths of its two header lines replaced applies and gives the
///   new file byte for byte, so the form is the one the rule states.
///
/// A path of this list that Git accepts fails the case: the list names what Git refuses today.
#[test]
fn git_refuses_a_path_it_reserves_by_name_and_applies_the_same_diff_under_another_name() {
    let reserved = [
        ".GIT/notes.txt",
        "sub/.Git/notes.txt",
        "git~1",
        "sub/GIT~1/notes.txt",
        ".git /notes.txt",
        ".git./notes.txt",
        ".git:stream.txt",
    ];
    let repository = Repository::new();
    let after = "bravo\n";
    let mut failures = Vec::new();
    for path in reserved {
        for before in [Some("alpha\n"), None] {
            let diff = replacement_diff(path, before, after)
                .unwrap_or_else(|code| panic!("the file system changes {path:?}: {code}"));
            assert!(
                !diff.binary && !diff.truncated && !diff.text.is_empty(),
                "path {path:?}: a whole text diff: {diff:?}"
            );
            let former = before.map(str::as_bytes);
            match repository.apply(path, former, &diff.text) {
                Err(refusal)
                    if refusal.starts_with("git apply --check ")
                        && refusal.contains(&format!("invalid path '{path}'")) => {}
                answer => failures.push(format!(
                    "path {path:?}: Git does not refuse it as an invalid path: {answer:?}"
                )),
            }
            let renamed = under_the_name(&diff.text, "neutral.txt");
            match repository.apply("neutral.txt", former, &renamed) {
                Ok(Some(applied)) if applied == after.as_bytes() => {}
                answer => failures.push(format!(
                    "path {path:?}: under another name git apply gave {answer:?} with the diff \
                     {renamed:?}"
                )),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} answers of Git are not the ones stated:\n{}",
        failures.len(),
        reserved.len() * 4,
        failures.join("\n")
    );
    // A backslash is no part of a path the file system changes, so no diff is made for one.
    assert_eq!(
        replacement_diff("sub\\.git\\notes.txt", Some("alpha\n"), "bravo\n").err(),
        Some("workspace.path-escape")
    );
}
