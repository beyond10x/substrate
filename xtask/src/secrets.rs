//! `cargo xtask check-secrets`: Gitleaks over the whole reachable history, with the scanned
//! count held to Git's own. Merges are diffed with an explicit `--diff-merges=separate` so caller
//! config (`log.diffMerges`) cannot change the format.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};
use sha2::{Digest as _, Sha256};

use crate::report::Report;

const VERSION: &str = "8.30.1";
const ARCHIVE: &str = "gitleaks_8.30.1_linux_x64.tar.gz";
const SHA256: &str = "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb";

/// Set on every Git process this check starts, Gitleaks included, so local state that is never
/// pushed cannot change which history both sides read. `GIT_NO_REPLACE_OBJECTS`: `refs/replace/*`
/// would make Git print another commit's content under a replaced commit's hash.
/// `GIT_GRAFT_FILE`: a `.git/info/grafts` line can cut a commit's parents, so every commit
/// behind it would drop out of the count and of Gitleaks' log alike; Git reads the named file
/// instead. The name is beneath a device, so it can never exist and Git reads no graft, silently:
/// any file that does exist, `/dev/null` and an empty one included, makes Git 2.55 print its
/// graft-deprecation hint on stderr, and Gitleaks then reports no scanned commits.
const WHOLE_HISTORY: [(&str, &str); 2] = [
    ("GIT_NO_REPLACE_OBJECTS", "1"),
    ("GIT_GRAFT_FILE", "/dev/null/grafts"),
];

pub fn check(root: &Path) -> Result<Report> {
    require_success(root, "git", &["fsck", "--full"])?;
    if let Some(refusal) = shallow(root)? {
        return Ok(Report::failed(vec![refusal]));
    }
    let expected = all_commits(root)?.len();
    if expected == 0 {
        return Ok(Report::failed(vec![
            "secret scan refused an empty history".to_owned(),
        ]));
    }

    let directory = tempfile::tempdir().context("creating Gitleaks tool directory")?;
    let archive = directory.path().join(ARCHIVE);
    let url =
        format!("https://github.com/gitleaks/gitleaks/releases/download/v{VERSION}/{ARCHIVE}");
    require_success_at(
        root,
        "curl",
        &["-fsSL", &url, "-o", archive.to_str().unwrap_or("")],
    )?;
    let bytes = fs::read(&archive)?;
    if hex::encode(Sha256::digest(bytes)) != SHA256 {
        bail!("downloaded Gitleaks archive checksum disagrees with the repository pin");
    }
    require_success_at(
        root,
        "tar",
        &[
            "-xzf",
            archive.to_str().unwrap_or(""),
            "-C",
            directory.path().to_str().unwrap_or(""),
            "gitleaks",
        ],
    )?;
    let output = Command::new(directory.path().join("gitleaks"))
        .envs(WHOLE_HISTORY)
        .args([
            "git",
            "--redact=100",
            "--no-banner",
            "--no-color",
            "--log-level=info",
            "--log-opts=--diff-merges=separate --root --all --no-textconv",
        ])
        .current_dir(root)
        .output()
        .context("running Gitleaks")?;
    let log = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Ok(Report::failed(vec![format!(
            "Gitleaks refused the history:\n{log}"
        )]));
    }
    let roots = root_commits(root)?;
    let scanned = log.lines().find_map(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        let at = words.iter().position(|word| *word == "commits")?;
        words.get(at.checked_sub(1)?)?.parse::<u64>().ok()
    });
    // Gitleaks counts the commits that yield at least one fragment from the same
    // `git log -p -U0 --diff-merges=separate --root --all`; the rest carry no added bytes and are
    // named instead.
    let Expectation { scannable, hidden } = expectation(root)?;
    let refusals = refusals(scanned, &scannable, hidden);
    if !refusals.is_empty() {
        return Ok(Report::failed(refusals));
    }
    let expected_diffs = scannable.len();
    for commit in &roots {
        scan_root_commit(root, directory.path(), commit)?;
    }
    let mut empty: Vec<String> = all_commits(root)?
        .into_iter()
        .filter(|commit| !scannable.contains(commit))
        .collect();
    empty.sort();
    let unchanged = if empty.is_empty() {
        String::new()
    } else {
        format!(
            "; {} commits add nothing to scan: {}",
            empty.len(),
            empty.join(", ")
        )
    };
    Ok(Report::passed(format!(
        "Gitleaks scanned all {expected} reachable commits ({expected_diffs} adding content and {} root trees{unchanged})",
        roots.len()
    )))
}

/// The refusal for a shallow repository, naming its shallow file: the commits past that
/// boundary are not in the object store, so no environment variable can bring them back to scan.
fn shallow(root: &Path) -> Result<Option<String>> {
    let output = Command::new("git")
        .envs(WHOLE_HISTORY)
        .args([
            "rev-parse",
            "--is-shallow-repository",
            "--path-format=absolute",
            "--git-path",
            "shallow",
        ])
        .current_dir(root)
        .output()
        .context("asking Git whether the repository is shallow")?;
    if !output.status.success() {
        bail!(
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8(output.stdout)?;
    let mut lines = stdout.lines();
    match (lines.next(), lines.next()) {
        (Some("false"), Some(_)) => Ok(None),
        (Some("true"), Some(file)) => Ok(Some(format!(
            "secret scan refused a shallow repository: {file} cuts the history, so the commits behind it cannot be scanned; fetch with --unshallow first"
        ))),
        _ => bail!("git rev-parse --is-shallow-repository answered {stdout:?}"),
    }
}

fn root_commits(root: &Path) -> Result<Vec<String>> {
    rev_list(root, &["--max-parents=0"], "listing reachable root commits")
}

fn all_commits(root: &Path) -> Result<Vec<String>> {
    rev_list(root, &[], "listing reachable commits")
}

fn rev_list(root: &Path, extra: &[&str], purpose: &'static str) -> Result<Vec<String>> {
    let output = Command::new("git")
        .envs(WHOLE_HISTORY)
        .args(["rev-list", "--all"])
        .args(extra)
        .current_dir(root)
        .output()
        .context(purpose)?;
    if !output.status.success() {
        bail!(
            "git rev-list failed {purpose}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// The refusal for a Gitleaks run whose scanned-commit count is not the number of commits that
/// add something to scan, or `None` when the two agree. Equality is required both ways: fewer
/// means a commit carrying added content went unscanned, more means this model of Gitleaks is wrong.
fn completeness(scanned: Option<u64>, scannable: usize) -> Option<String> {
    if scanned == Some(scannable as u64) {
        None
    } else {
        Some(format!(
            "Gitleaks reported {scanned:?} scanned commits; Git reports {scannable} adding content to scan"
        ))
    }
}

/// Every reason the scan is incomplete: each text blob Git hid from Gitleaks as binary, by
/// commit and path, then a scanned-commit count that disagrees with the expectation.
fn refusals(
    scanned: Option<u64>,
    scannable: &BTreeSet<String>,
    hidden: Vec<String>,
) -> Vec<String> {
    let mut refusals: Vec<String> = hidden
        .into_iter()
        .map(|diff| {
            format!(
                "Git printed this diff as binary although the new blob has no NUL in its first 8000 bytes, so Gitleaks did not scan it: {diff}"
            )
        })
        .collect();
    if refusals.is_empty()
        && let Some(refusal) = completeness(scanned, scannable.len())
    {
        refusals.push(refusal);
    }
    refusals
}

/// What the patch Gitleaks reads says about the history.
struct Expectation {
    /// The commits Gitleaks 8.30.1 counts as scanned, plus every commit whose added text Git
    /// printed as binary.
    scannable: BTreeSet<String>,
    /// `<commit> <path>` for each text blob Git printed as binary, which Gitleaks therefore skips.
    hidden: Vec<String>,
}

/// The commits Gitleaks 8.30.1 counts as scanned (see [`expectation`]).
#[cfg(test)]
fn scannable_commits(root: &Path) -> Result<BTreeSet<String>> {
    Ok(expectation(root)?.scannable)
}

/// Reads the patch Gitleaks reads itself (`git log -p -U0` with `--log-opts=--diff-merges=separate
/// --root --all --no-textconv`, so a textconv driver from the checking machine's config cannot empty it;
/// `sources/git.go`; `--full-index` only lengthens the `index` line). A diff Git prints as binary
/// is checked against Git's own heuristic, a NUL in the first 8000 bytes of the post-image: one
/// without (a diff attribute, or a merge parent whose version had a NUL) went unscanned, so its
/// commit stays expected and is named. This fails closed on the merge case by decision.
fn expectation(root: &Path) -> Result<Expectation> {
    let output = Command::new("git")
        .envs(WHOLE_HISTORY)
        .args([
            "log",
            "-p",
            "-U0",
            "--diff-merges=separate",
            "--root",
            "--all",
            "--no-color",
            "--full-index",
            "--no-textconv",
            "--format=commit %H",
        ])
        .current_dir(root)
        .output()
        .context("listing the patches Gitleaks scans")?;
    if !output.status.success() {
        bail!(
            "git log -p failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let patch = parse_log(&String::from_utf8_lossy(&output.stdout));
    let blobs: BTreeSet<&str> = patch.binaries.iter().map(|b| b.blob.as_str()).collect();
    let has_nul = blobs_with_nul(root, &blobs)?;
    let mut scannable = patch.scannable;
    let mut hidden = BTreeSet::new();
    for binary in &patch.binaries {
        if !has_nul.contains(binary.blob.as_str()) {
            scannable.insert(binary.commit.clone());
            hidden.insert(format!("{} {}", binary.commit, binary.path));
        }
    }
    Ok(Expectation {
        scannable,
        hidden: hidden.into_iter().collect(),
    })
}

/// The blobs among `blobs` that Git's binary heuristic calls binary: a NUL byte in the first
/// 8000 bytes (`xdiff-interface.c`, `buffer_is_binary`). A blob Git cannot read is an error.
fn blobs_with_nul<'a>(root: &Path, blobs: &BTreeSet<&'a str>) -> Result<BTreeSet<&'a str>> {
    const FIRST_FEW_BYTES: usize = 8000;
    let mut binary = BTreeSet::new();
    if blobs.is_empty() {
        return Ok(binary);
    }
    let mut child = Command::new("git")
        .envs(WHOLE_HISTORY)
        .args(["cat-file", "--batch"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("reading blobs Git prints as binary")?;
    let mut stdin = child.stdin.take().context("git cat-file stdin")?;
    let request = blobs.iter().fold(String::new(), |mut request, blob| {
        request.push_str(blob);
        request.push('\n');
        request
    });
    let writer = std::thread::spawn(move || stdin.write_all(request.as_bytes()));
    let output = child.wait_with_output().context("reading git cat-file")?;
    writer
        .join()
        .map_err(|_| anyhow::anyhow!("git cat-file writer panicked"))?
        .context("writing to git cat-file")?;
    if !output.status.success() {
        bail!(
            "git cat-file failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut rest = output.stdout.as_slice();
    for blob in blobs {
        let end = rest
            .iter()
            .position(|byte| *byte == b'\n')
            .context("git cat-file ended early")?;
        let header = std::str::from_utf8(&rest[..end])?;
        let words: Vec<&str> = header.split(' ').collect();
        let [oid, "blob", size] = words.as_slice() else {
            bail!("git cat-file could not read blob {blob}: {header}");
        };
        if oid != blob {
            bail!("git cat-file answered {oid} for blob {blob}");
        }
        let size: usize = size.parse()?;
        let content = rest
            .get(end + 1..end + 1 + size)
            .context("git cat-file blob truncated")?;
        if content[..size.min(FIRST_FEW_BYTES)].contains(&0) {
            binary.insert(*blob);
        }
        rest = rest.get(end + 2 + size..).unwrap_or_default();
    }
    Ok(binary)
}

/// A file diff Git printed as binary, with its post-image blob.
struct BinaryDiff {
    commit: String,
    path: String,
    blob: String,
}

/// One file diff of a commit's patch, as far as Gitleaks' skip rules read it.
#[derive(Default)]
struct FileDiff {
    deleted: bool,
    binary: Option<String>,
    blob: Option<String>,
    hunk: bool,
}

impl FileDiff {
    /// Gitleaks skips a deleted file (`IsDelete`), a binary that is not an archive, and an
    /// archive too, because `secrets.rs` passes no `--max-archive-depth` and its default of 0
    /// makes `File::Fragments` yield nothing (`cmd/root.go:92`, `sources/file.go:59`). A text
    /// diff yields one fragment per hunk, so a diff with no hunk (an exact rename or copy, a mode
    /// change, an empty new file) yields none. A commit is counted once any fragment carries it
    /// (`detect/detect.go`, `addCommit`).
    fn yields_fragment(&self) -> bool {
        !self.deleted && self.binary.is_none() && self.hunk
    }
}

/// The parse of a `git log -p --full-index --format='commit %H'` stream.
#[derive(Default)]
struct Patch {
    /// Commits with at least one file diff that yields a Gitleaks fragment. Under `--diff-merges=separate` a merge
    /// appears once per parent; any parent's diff counts.
    scannable: BTreeSet<String>,
    /// Every non-deleted file diff Git printed as binary.
    binaries: Vec<BinaryDiff>,
}

#[cfg(test)]
fn scannable_in_log(log: &str) -> BTreeSet<String> {
    parse_log(log).scannable
}

fn parse_log(log: &str) -> Patch {
    let mut patch = Patch::default();
    let mut commit: Option<&str> = None;
    let mut file: Option<FileDiff> = None;
    let mut settle = |commit: Option<&str>, file: Option<FileDiff>| {
        let (Some(commit), Some(file)) = (commit, file) else {
            return;
        };
        if file.yields_fragment() {
            patch.scannable.insert(commit.to_owned());
        }
        if let (false, Some(path), Some(blob)) = (file.deleted, &file.binary, &file.blob) {
            patch.binaries.push(BinaryDiff {
                commit: commit.to_owned(),
                path: path.clone(),
                blob: blob.clone(),
            });
        }
    };
    for line in log.lines() {
        // Hunk lines start with `+`, `-`, ` ` or `\`, and the format emits only the hash, so a
        // line starting with either header word is always a header.
        if let Some(sha) = line.strip_prefix("commit ") {
            settle(commit, file.take());
            commit = Some(sha.trim());
        } else if line.starts_with("diff --git ") {
            settle(commit, file.take());
            file = Some(FileDiff::default());
        } else if let Some(diff) = file.as_mut() {
            if line.starts_with("@@") {
                diff.hunk = true;
            } else if diff.hunk {
                // A hunk line; the extended header ends at the first `@@`.
            } else if line.starts_with("deleted file mode ") {
                diff.deleted = true;
            } else if let Some(range) = line.strip_prefix("index ") {
                let range = range.split(' ').next().unwrap_or_default();
                diff.blob = range.split_once("..").map(|(_, new)| new.to_owned());
            } else if let Some(names) = line.strip_prefix("Binary files ") {
                let names = names.strip_suffix(" differ").unwrap_or(names);
                let new = names.rsplit_once(" and ").map_or(names, |(_, new)| new);
                let new = new.trim_matches('"');
                diff.binary = Some(new.strip_prefix("b/").unwrap_or(new).to_owned());
            } else if line == "GIT binary patch" {
                diff.binary = Some(String::new());
            }
        }
    }
    settle(commit, file.take());
    patch
}

fn scan_root_commit(root: &Path, tool_directory: &Path, commit: &str) -> Result<()> {
    let tree = tempfile::tempdir().context("creating root-commit scan directory")?;
    let archive = tree.path().join("root.tar");
    require_success_at(
        root,
        "git",
        &[
            "archive",
            "--format=tar",
            &format!("--output={}", archive.display()),
            commit,
        ],
    )?;
    let extracted = tree.path().join("tree");
    fs::create_dir(&extracted)?;
    require_success_at(
        root,
        "tar",
        &[
            "-xf",
            archive.to_str().unwrap_or(""),
            "-C",
            extracted.to_str().unwrap_or(""),
        ],
    )?;
    let output = Command::new(tool_directory.join("gitleaks"))
        .envs(WHOLE_HISTORY)
        .args([
            "dir",
            "--redact=100",
            "--no-banner",
            "--no-color",
            "--log-level=info",
            "--config",
        ])
        .arg(root.join(".gitleaks.toml"))
        .arg(&extracted)
        .output()
        .context("running Gitleaks on a root commit tree")?;
    if !output.status.success() {
        bail!(
            "Gitleaks refused root commit {commit}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn require_success(root: &Path, program: &str, args: &[&str]) -> Result<()> {
    require_success_at(root, program, args)
}

fn require_success_at(root: &Path, program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .envs(WHOLE_HISTORY)
        .args(args)
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use super::{all_commits, completeness, expectation, scannable_commits, scannable_in_log};

    /// A Git repository whose commits do not depend on the caller's Git configuration.
    struct Repo {
        directory: tempfile::TempDir,
        config: tempfile::NamedTempFile,
    }

    impl Repo {
        fn new() -> Self {
            let repo = Self {
                directory: tempfile::tempdir().expect("temporary repository"),
                config: tempfile::NamedTempFile::new().expect("empty Git config"),
            };
            repo.git(&["init", "--quiet", "--initial-branch=main", "."]);
            repo
        }

        fn path(&self) -> &Path {
            self.directory.path()
        }

        fn git(&self, args: &[&str]) -> String {
            let output = Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", self.config.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .current_dir(self.path())
                .output()
                .expect("running git");
            assert!(
                output.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout)
                .expect("git output is UTF-8")
                .trim()
                .to_owned()
        }

        fn write(&self, name: &str, bytes: &[u8]) {
            fs::write(self.path().join(name), bytes).expect("writing a work-tree file");
        }

        /// Stages everything and commits it, returning the new commit.
        fn commit(&self, message: &str) -> String {
            self.git(&["add", "--all"]);
            self.git(&["commit", "--quiet", "--allow-empty", "--message", message]);
            self.git(&["rev-parse", "HEAD"])
        }
    }

    fn set(commits: &[&String]) -> BTreeSet<String> {
        commits.iter().map(|commit| (*commit).clone()).collect()
    }

    #[test]
    fn a_delete_only_commit_is_not_expected_and_added_content_still_is() {
        let repo = Repo::new();
        repo.write("a.txt", b"alpha\n");
        let add_a = repo.commit("add a");
        repo.write("b.txt", b"beta\n");
        let add_b = repo.commit("add b");
        repo.git(&["rm", "--quiet", "a.txt"]);
        let delete = repo.commit("delete a");

        let scannable = scannable_commits(repo.path()).expect("counting scannable commits");
        assert_eq!(scannable, set(&[&add_a, &add_b]));
        assert!(!scannable.contains(&delete));

        assert_eq!(completeness(Some(2), scannable.len()), None);
        // Gitleaks scanning fewer commits than carry added content is still a refusal.
        assert!(completeness(Some(1), scannable.len()).is_some());
        assert!(completeness(Some(3), scannable.len()).is_some());
        assert!(completeness(None, scannable.len()).is_some());
    }

    #[test]
    fn a_commit_that_only_changes_a_non_archive_binary_is_not_expected() {
        let repo = Repo::new();
        repo.write("a.txt", b"alpha\n");
        let add_a = repo.commit("add a");
        repo.write("blob.bin", b"x\0y");
        let add_binary = repo.commit("add binary");
        repo.write("blob.bin", b"x\0z");
        let change_binary = repo.commit("change binary");
        repo.write("b.txt", b"beta\n");
        let add_b = repo.commit("add b");

        let expected = expectation(repo.path()).expect("counting scannable commits");
        assert_eq!(expected.scannable, set(&[&add_a, &add_b]));
        assert!(!expected.scannable.contains(&add_binary));
        assert!(!expected.scannable.contains(&change_binary));
        assert!(expected.hidden.is_empty(), "{:?}", expected.hidden);
        assert_eq!(completeness(Some(2), expected.scannable.len()), None);
        assert!(completeness(Some(1), expected.scannable.len()).is_some());
    }

    #[test]
    fn text_a_diff_attribute_prints_as_binary_is_named_by_commit_and_path() {
        let repo = Repo::new();
        repo.write("blob.bin", b"x\0y");
        repo.write("a.txt", b"alpha\n");
        let add = repo.commit("add both");
        repo.write(".gitattributes", b"a.txt -diff\n");
        repo.commit("hide a.txt");

        let expected = expectation(repo.path()).expect("counting scannable commits");
        assert_eq!(expected.hidden, vec![format!("{add} a.txt")]);
        assert!(expected.scannable.contains(&add));
    }

    #[test]
    fn a_merge_counts_when_a_parent_diff_adds_content_and_every_branch_counts() {
        let repo = Repo::new();
        repo.write("a.txt", b"alpha\n");
        let base = repo.commit("base");
        repo.git(&["checkout", "--quiet", "-b", "side"]);
        repo.write("s.txt", b"side\n");
        let side = repo.commit("side");
        repo.git(&["rm", "--quiet", "s.txt"]);
        let side_delete = repo.commit("side delete");
        repo.git(&["checkout", "--quiet", "main"]);
        repo.write("m.txt", b"main\n");
        let main = repo.commit("main");
        repo.git(&["merge", "--quiet", "--no-edit", "side~1"]);
        let merge = repo.git(&["rev-parse", "HEAD"]);

        let scannable = scannable_commits(repo.path()).expect("counting scannable commits");
        assert_eq!(scannable, set(&[&base, &side, &main, &merge]));
        assert!(!scannable.contains(&side_delete));
    }

    #[test]
    fn renames_mode_changes_and_empty_files_carry_no_fragment() {
        let repo = Repo::new();
        repo.write("a.txt", b"alpha\n");
        let add_a = repo.commit("add a");
        repo.git(&["mv", "a.txt", "b.txt"]);
        let rename = repo.commit("rename");
        repo.git(&["update-index", "--chmod=+x", "b.txt"]);
        let mode = repo.commit("mode");
        repo.write("empty", b"");
        let empty = repo.commit("empty file");
        let nothing = repo.commit("no change");

        let scannable = scannable_commits(repo.path()).expect("counting scannable commits");
        assert_eq!(scannable, set(&[&add_a]));
        for commit in [&rename, &mode, &empty, &nothing] {
            assert!(!scannable.contains(commit), "{commit} has nothing to scan");
        }
    }

    #[test]
    fn the_log_parser_reads_each_file_diff_on_its_own() {
        let sha = "0123456789abcdef0123456789abcdef01234567".to_owned();
        let other = "89abcdef0123456789abcdef0123456789abcdef";
        let third = "fedcba9876543210fedcba9876543210fedcba98";
        let log = format!(
            "commit {sha}\n\ndiff --git a/x b/x\ndeleted file mode 100644\nindex 1..0\n--- a/x\n\
             +++ /dev/null\n@@ -1 +0,0 @@\n-x\ndiff --git a/w b/w\nindex 3..4 100644\n--- a/w\n\
             +++ b/w\n@@ -1 +1 @@\n-v\n+w\n\
             commit {other}\n\ndiff --git a/y.tar.gz b/y.tar.gz\n\
             new file mode 100644\nindex 0..1\nBinary files /dev/null and b/y.tar.gz differ\n\
             diff --git a/z b/z\nindex 1..2 100644\nBinary files a/z and b/z differ\n\
             commit {third}\n\ndiff --git a/d b/d\ndeleted file mode 100644\nindex 5..0\n\
             Binary files a/d and /dev/null differ\n"
        );
        assert_eq!(scannable_in_log(&log), set(&[&sha]));
        let binaries: Vec<(String, String, String)> = super::parse_log(&log)
            .binaries
            .into_iter()
            .map(|b| (b.commit, b.path, b.blob))
            .collect();
        let named = |path: &str, blob: &str| (other.to_owned(), path.to_owned(), blob.to_owned());
        assert_eq!(binaries, vec![named("y.tar.gz", "1"), named("z", "2")]);
    }

    /// Gitleaks 8.30.1 defaults `--max-archive-depth` to 0 (`cmd/root.go:92`), and
    /// `File::Fragments` returns before yielding anything when `archiveDepth+1 > MaxArchiveDepth`
    /// (`sources/file.go:59`), so an archive diff yields no fragment and its commit is never
    /// counted. Observed with the pinned binary: base + an archive-only commit reports
    /// `1 commits scanned.` against `rev-list` 2.
    #[test]
    fn adversary_an_archive_only_commit_is_not_counted_at_archive_depth_zero() {
        let repo = Repo::new();
        repo.write("a.txt", b"alpha\n");
        let add_a = repo.commit("add a");
        repo.write("bundle.zip", b"PK\x03\x04\0\0");
        let add_archive = repo.commit("add archive");

        let scannable = scannable_commits(repo.path()).expect("counting scannable commits");
        assert!(
            !scannable.contains(&add_archive),
            "Gitleaks at --max-archive-depth=0 yields no fragment for {add_archive}"
        );
        assert_eq!(scannable, set(&[&add_a]));
    }

    /// The work tree's `.gitattributes` decides what `git log -p` prints as binary, for every
    /// commit in history. One `* -diff` line turns each text diff into `Binary files … differ`,
    /// Gitleaks skips all of them, and the expected count drops to match. The commit below adds
    /// text, so the acceptance statement requires it to be expected.
    #[test]
    fn adversary_a_diff_attribute_cannot_remove_added_text_from_the_expected_count() {
        let repo = Repo::new();
        repo.write("a.txt", b"alpha\n");
        let add_a = repo.commit("add a");
        repo.write("creds.txt", b"token = not-a-real-value\n");
        let add_text = repo.commit("add text");
        repo.write(".gitattributes", b"* -diff\n");
        let add_attributes = repo.commit("disable textual diffs");

        let scannable = scannable_commits(repo.path()).expect("counting scannable commits");
        assert_eq!(
            scannable,
            set(&[&add_a, &add_text, &add_attributes]),
            "every one of these commits adds text"
        );
    }

    fn write_default_gitleaks_config(repo: &Repo) {
        repo.write(".gitleaks.toml", b"[extend]\nuseDefault = true\n");
    }

    /// End to end with the pinned Gitleaks (downloads it, so ignored by default): a history that
    /// adds a secret and then `* -diff` must not pass. Run with
    /// `cargo test -p xtask -- --ignored adversary_e2e`.
    #[test]
    #[ignore = "downloads the pinned Gitleaks release"]
    fn adversary_e2e_a_diff_attribute_does_not_let_a_secret_commit_pass() {
        let repo = Repo::new();
        write_default_gitleaks_config(&repo);
        repo.write("a.txt", b"alpha\n");
        repo.commit("base");
        let tail: String = "2MAZPFHHH5NMPLYQ".chars().rev().collect();
        repo.write(
            "creds.txt",
            format!("aws_access_key_id = {}{tail}\n", "AKIA").as_bytes(),
        );
        repo.commit("add a credential");
        repo.write(".gitattributes", b"* -diff\n");
        repo.commit("disable textual diffs");

        let report = super::check(repo.path()).expect("running the check");
        assert!(
            !report.failures().is_empty(),
            "check-secrets passed a history holding an unscanned credential: {}",
            report.summary()
        );
    }

    /// End to end with the pinned Gitleaks: a history that only adds an archive is legitimate
    /// and must pass.
    #[test]
    #[ignore = "downloads the pinned Gitleaks release"]
    fn adversary_e2e_an_archive_only_commit_passes() {
        let repo = Repo::new();
        write_default_gitleaks_config(&repo);
        repo.write("a.txt", b"alpha\n");
        repo.commit("base");
        repo.write("bundle.zip", b"PK\x03\x04\0\0");
        repo.commit("add archive");

        let report = super::check(repo.path()).expect("running the check");
        assert!(
            report.failures().is_empty(),
            "check-secrets refused a legitimate history: {}",
            report.failure_text()
        );
    }

    /// An AWS-shaped access key id, assembled so no credential-shaped literal sits in the source.
    fn adversary2_key_line() -> String {
        let tail: String = "2MAZPFHHH5NMPLYQ".chars().rev().collect();
        format!("aws_access_key_id = {}{tail}\n", "AKIA")
    }

    /// A `diff=<driver>` attribute is committable; when the checking machine's Git config defines
    /// `diff.<driver>.textconv`, `git log -p` prints the converted text, not the blob. A converter
    /// that prints nothing leaves a header with no hunk and no `Binary files` line: Gitleaks sees
    /// no fragment and the expectation agrees, so the commit drops out of both sides.
    #[test]
    fn adversary2_a_textconv_driver_cannot_remove_added_text_from_the_expected_count() {
        let repo = Repo::new();
        repo.write("a.txt", b"alpha\n");
        repo.commit("base");
        repo.write(".gitattributes", b"*.txt diff=hide\n");
        repo.commit("route text through a driver");
        repo.write("creds.txt", b"token = not-a-real-value\n");
        let add_text = repo.commit("add text");
        repo.git(&["config", "diff.hide.textconv", "true"]);

        let expected = expectation(repo.path()).expect("counting scannable commits");
        assert!(
            expected.scannable.contains(&add_text) || !expected.hidden.is_empty(),
            "{add_text} adds text that a textconv driver hid; scannable {:?}, hidden {:?}",
            expected.scannable,
            expected.hidden
        );
    }

    /// End to end: the same textconv history holding a credential must not pass.
    #[test]
    #[ignore = "downloads the pinned Gitleaks release"]
    fn adversary2_e2e_a_textconv_driver_does_not_let_a_secret_commit_pass() {
        let repo = Repo::new();
        write_default_gitleaks_config(&repo);
        repo.write("a.txt", b"alpha\n");
        repo.commit("base");
        repo.write(".gitattributes", b"*.txt diff=hide\n");
        repo.commit("route text through a driver");
        repo.write("creds.txt", adversary2_key_line().as_bytes());
        repo.commit("add a credential");
        repo.git(&["config", "diff.hide.textconv", "true"]);

        let report = super::check(repo.path()).expect("running the check");
        assert!(
            !report.failures().is_empty(),
            "check-secrets passed a history holding an unscanned credential: {}",
            report.summary()
        );
    }

    /// End to end: `refs/replace/*` makes `git log` print another commit's content under the
    /// replaced commit's hash, for Gitleaks and for the expectation alike.
    #[test]
    #[ignore = "downloads the pinned Gitleaks release"]
    fn adversary2_e2e_a_replace_ref_does_not_let_a_secret_commit_pass() {
        let repo = Repo::new();
        write_default_gitleaks_config(&repo);
        repo.write("a.txt", b"alpha\n");
        repo.commit("base");
        repo.write("creds.txt", adversary2_key_line().as_bytes());
        let leaking = repo.commit("add a credential");
        repo.git(&["checkout", "--quiet", "--detach", "HEAD~1"]);
        repo.write("creds.txt", b"nothing here\n");
        let clean = repo.commit("add a clean file");
        repo.git(&["checkout", "--quiet", "main"]);
        repo.git(&["replace", &leaking, &clean]);

        let report = super::check(repo.path()).expect("running the check");
        assert!(
            !report.failures().is_empty(),
            "check-secrets passed a history holding an unscanned credential: {}",
            report.summary()
        );
    }

    /// `base`, then a commit adding a credential, then one deleting it again, with a local
    /// `.git/info/grafts` line that makes the last commit parentless. Git honours that file
    /// unless `GIT_GRAFT_FILE` points elsewhere, so without it the credential commit is not
    /// reachable to `git rev-list`, to `git log` or to Gitleaks. Returns the credential commit.
    fn history_hidden_behind_a_graft(repo: &Repo) -> String {
        repo.write("a.txt", b"alpha\n");
        repo.commit("base");
        repo.write("creds.txt", adversary2_key_line().as_bytes());
        let leaking = repo.commit("add a credential");
        fs::remove_file(repo.path().join("creds.txt")).expect("deleting the credential file");
        repo.write("b.txt", b"beta\n");
        let tip = repo.commit("delete the credential");
        fs::write(
            repo.path().join(".git/info/grafts"),
            format!("{tip}\n").as_bytes(),
        )
        .expect("writing a graft file");
        leaking
    }

    /// Story check-secrets-ignores-a-local-graft-file: a local graft file cannot cut a commit out
    /// of the history this check lists and expects Gitleaks to scan.
    #[test]
    fn a_graft_file_cannot_hide_a_commit_from_the_expected_count() {
        let repo = Repo::new();
        let leaking = history_hidden_behind_a_graft(&repo);

        let all = all_commits(repo.path()).expect("listing reachable commits");
        assert!(
            all.contains(&leaking),
            "{leaking} is hidden behind a graft; listed {all:?}"
        );
        let expected = expectation(repo.path()).expect("counting scannable commits");
        assert!(
            expected.scannable.contains(&leaking),
            "{leaking} adds a credential behind a graft; scannable {:?}",
            expected.scannable
        );
    }

    /// A shallow repository has no history past its boundary at all, so the scan cannot be
    /// complete; the check refuses it by name before downloading anything.
    #[test]
    fn a_shallow_repository_is_refused_by_name() {
        let origin = Repo::new();
        origin.write("a.txt", b"alpha\n");
        origin.commit("base");
        origin.write("b.txt", b"beta\n");
        origin.commit("tip");
        let shallow = Repo::new();
        let url = format!("file://{}", origin.path().display());
        shallow.git(&["fetch", "--quiet", "--depth=1", &url, "main"]);
        shallow.git(&["checkout", "--quiet", "FETCH_HEAD"]);

        let report = super::check(shallow.path()).expect("running the check");
        let text = report.failure_text();
        assert!(
            text.contains("shallow"),
            "check-secrets did not refuse a shallow repository by name: {} {text}",
            report.summary()
        );
    }

    /// End to end with the pinned Gitleaks: the credential commit behind the graft is scanned,
    /// and the check refuses it with Gitleaks' own finding.
    #[test]
    #[ignore = "downloads the pinned Gitleaks release"]
    fn adversary_e2e_a_graft_file_does_not_let_a_secret_commit_pass() {
        let repo = Repo::new();
        write_default_gitleaks_config(&repo);
        history_hidden_behind_a_graft(&repo);

        let report = super::check(repo.path()).expect("running the check");
        let text = report.failure_text();
        assert!(
            text.contains("Gitleaks refused the history") && text.contains("leaks found: 1"),
            "check-secrets did not refuse the credential behind a graft: {} {text}",
            report.summary()
        );
    }

    /// Accepted behaviour (story acceptance, binary line): a file carrying a NUL in its first
    /// 8000 bytes is a binary Gitleaks skips, so a commit adding only such a file is not expected
    /// and is not named as hidden text. UTF-16 text is such a file.
    #[test]
    fn a_nul_carrying_file_is_uncounted_even_when_it_is_utf16_text() {
        let repo = Repo::new();
        repo.write("a.txt", b"alpha\n");
        let base = repo.commit("base");
        let utf16: Vec<u8> = "token = not-a-real-value\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        repo.write("wide.txt", &utf16);
        let wide = repo.commit("add a UTF-16 file");

        let expected = expectation(repo.path()).expect("counting scannable commits");
        assert_eq!(expected.scannable, set(&[&base]));
        assert!(!expected.scannable.contains(&wide));
        assert!(expected.hidden.is_empty(), "{:?}", expected.hidden);
    }

    /// `base`, a side branch adding `s.txt`, a main commit adding `m.txt`, and a merge whose tree
    /// also adds `evil.txt` (in neither parent) carrying `content`. Returns the merge commit.
    fn adversary3_evil_merge(repo: &Repo, content: &[u8]) -> String {
        repo.write("a.txt", b"alpha\n");
        repo.commit("base");
        repo.git(&["checkout", "--quiet", "-b", "side"]);
        repo.write("s.txt", b"side\n");
        repo.commit("side");
        repo.git(&["checkout", "--quiet", "main"]);
        repo.write("m.txt", b"main\n");
        repo.commit("main");
        repo.git(&["merge", "--quiet", "--no-commit", "side"]);
        repo.write("evil.txt", content);
        repo.commit("merge")
    }

    /// `-m` is `--diff-merges=on`, which shows a merge in the format the local `log.diffMerges`
    /// names. That config is never pushed, and `off` (or `none`) makes `git log -p -m` print no
    /// patch for any merge, for the expectation and for Gitleaks' own log alike, so content a
    /// merge adds drops out of both counts and they still agree.
    #[test]
    fn adversary3_log_diff_merges_off_cannot_remove_a_merge_from_the_expected_count() {
        let repo = Repo::new();
        let merge = adversary3_evil_merge(&repo, b"token = not-a-real-value\n");
        repo.git(&["config", "log.diffMerges", "off"]);

        let expected = expectation(repo.path()).expect("counting scannable commits");
        assert!(
            expected.scannable.contains(&merge),
            "{merge} adds evil.txt in neither parent; scannable {:?}",
            expected.scannable
        );
    }

    /// The same with `log.diffMerges=dense-combined`: Git prints `diff --cc`, which the
    /// expectation's parser does not read as a file diff.
    #[test]
    fn adversary3_log_diff_merges_combined_cannot_remove_a_merge_from_the_expected_count() {
        let repo = Repo::new();
        let merge = adversary3_evil_merge(&repo, b"token = not-a-real-value\n");
        repo.git(&["config", "log.diffMerges", "dense-combined"]);

        let expected = expectation(repo.path()).expect("counting scannable commits");
        assert!(
            expected.scannable.contains(&merge),
            "{merge} adds evil.txt in neither parent; scannable {:?}",
            expected.scannable
        );
    }

    fn adversary3_e2e_evil_merge_with(diff_merges: &str) {
        let repo = Repo::new();
        write_default_gitleaks_config(&repo);
        adversary3_evil_merge(&repo, adversary2_key_line().as_bytes());
        repo.git(&["config", "log.diffMerges", diff_merges]);

        let report = super::check(repo.path()).expect("running the check");
        assert!(
            !report.failures().is_empty(),
            "check-secrets passed a merge holding an unscanned credential under log.diffMerges={diff_merges}: {}",
            report.summary()
        );
    }

    /// End to end with the pinned Gitleaks: a credential added by a merge must not pass under a
    /// local `log.diffMerges=off`.
    #[test]
    #[ignore = "downloads the pinned Gitleaks release"]
    fn adversary3_e2e_log_diff_merges_off_does_not_let_a_secret_merge_pass() {
        adversary3_e2e_evil_merge_with("off");
    }

    /// End to end with the pinned Gitleaks, `log.diffMerges=dense-combined`.
    #[test]
    #[ignore = "downloads the pinned Gitleaks release"]
    fn adversary3_e2e_log_diff_merges_combined_does_not_let_a_secret_merge_pass() {
        adversary3_e2e_evil_merge_with("dense-combined");
    }

    /// End to end with the pinned Gitleaks: with no local config the same merge is refused
    /// (control for the two cases above).
    #[test]
    #[ignore = "downloads the pinned Gitleaks release"]
    fn adversary3_e2e_a_secret_merge_is_refused_with_default_config() {
        adversary3_e2e_evil_merge_with("separate");
    }

    /// End to end with the pinned Gitleaks: `color.diff=always` colours only Gitleaks' log (the
    /// expectation passes `--no-color`), so the check must refuse, not pass.
    #[test]
    #[ignore = "downloads the pinned Gitleaks release"]
    fn adversary3_e2e_color_diff_always_does_not_let_a_secret_commit_pass() {
        let repo = Repo::new();
        write_default_gitleaks_config(&repo);
        repo.write("a.txt", b"alpha\n");
        repo.commit("base");
        repo.write("creds.txt", adversary2_key_line().as_bytes());
        repo.commit("add a credential");
        repo.git(&["config", "color.diff", "always"]);

        let report = super::check(repo.path()).expect("running the check");
        assert!(
            !report.failures().is_empty(),
            "check-secrets passed a history holding an unscanned credential: {}",
            report.summary()
        );
    }
}
