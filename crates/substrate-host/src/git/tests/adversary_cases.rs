//! Cases of an adversary pass over the Git changes reader. Each case states what must hold for
//! the change-set observation of a workspace whose writer also writes its repository metadata.

use std::path::Path;
use std::time::{Duration, SystemTime};

use git2::Repository;
use substrate_wire::{GitChangeStatus, GitChangesQuery, MAX_IO_BYTES};

use super::super::COMPARISON_BUDGET;
use super::{
    baseline_object, baseline_root, changes, changes_within, commit_paths, observe, paths,
    remove_store, source_repository, store_of, target,
};
use crate::git_baselines::DEFAULT_BYTE_LIMIT;

/// An instant long before any index of a test is written.
fn earlier() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_hours(271_752)
}

fn set_modified(path: &Path, time: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("file")
        .set_modified(time)
        .expect("modification time");
}

fn write(root: &Path, path: &str, content: &[u8]) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().expect("parent directory")).expect("directory");
    std::fs::write(file, content).expect("workspace file");
}

/// Stages `README.md` again, so the index records the file as it is now.
fn stage_readme(workspace: &Path) {
    let repository = Repository::open(workspace).expect("repository");
    let mut index = repository.index().expect("index");
    index.add_path(Path::new("README.md")).expect("stage file");
    index.write().expect("write index");
}

/// A tracked file whose bytes differ from the baseline is reported, whatever modification time
/// the file carries.
#[test]
fn a_tracked_file_whose_bytes_differ_from_the_baseline_is_reported_whatever_time_it_carries() {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    let file = workspace.path().join("README.md");
    set_modified(&file, earlier());
    stage_readme(workspace.path());
    let unchanged = observe(workspace.path(), baselines.path(), 1024).expect("change set");
    assert!(paths(&unchanged).is_empty(), "{unchanged:#?}");

    std::fs::write(&file, b"BOUNDED GIT WORKSPACE\n").expect("other bytes of the same length");
    set_modified(&file, earlier());

    let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
    assert_eq!(paths(&observed), ["README.md"], "{observed:#?}");
    assert_eq!(observed.items[0].status, GitChangeStatus::Modified);
}

/// A tracked file that carries the time the index itself was written is read and compared.
#[test]
fn a_tracked_file_as_old_as_the_index_is_read_and_compared() {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    let file = workspace.path().join("README.md");
    set_modified(&file, earlier());
    stage_readme(workspace.path());
    std::fs::write(&file, b"BOUNDED GIT WORKSPACE\n").expect("other bytes of the same length");
    set_modified(&file, earlier());
    set_modified(&workspace.path().join(".git/index"), earlier());

    let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
    assert_eq!(paths(&observed), ["README.md"], "{observed:#?}");
}

/// A changed tracked file is reported whatever flags its index entry carries.
#[test]
fn a_changed_tracked_file_is_reported_whatever_flags_its_index_entry_carries() {
    const ASSUMED_UNCHANGED: u16 = 0x8000;
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    {
        let repository = Repository::open(workspace.path()).expect("repository");
        let mut index = repository.index().expect("index");
        let mut entry = index
            .get_path(Path::new("README.md"), 0)
            .expect("staged entry");
        entry.flags |= ASSUMED_UNCHANGED;
        index.add(&entry).expect("flagged entry");
        index.write().expect("write index");
    }
    let flags = Repository::open(workspace.path())
        .expect("repository")
        .index()
        .expect("index")
        .get_path(Path::new("README.md"), 0)
        .expect("staged entry")
        .flags;
    assert_ne!(flags & ASSUMED_UNCHANGED, 0, "the index holds the flag");
    std::fs::write(workspace.path().join("README.md"), b"changed workspace\n")
        .expect("modify worktree");

    let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
    assert_eq!(paths(&observed), ["README.md"], "{observed:#?}");
}

/// A tracked file over the per-file bound that nothing touched since it was staged is not
/// reported.
#[test]
fn an_untouched_tracked_file_over_the_per_file_bound_is_not_reported() {
    let (workspace, first) = source_repository();
    write(
        workspace.path(),
        "big.txt",
        "wide line\n".repeat(512).as_bytes(),
    );
    let commit = commit_paths(workspace.path(), first, &["big.txt"]);
    let baselines = baseline_root(workspace.path(), commit);

    let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
    assert!(paths(&observed).is_empty(), "{observed:#?}");
}

/// A path the index records only in conflict stages is reported as conflicted.
#[test]
fn a_path_the_index_records_only_in_conflict_stages_is_reported_as_conflicted() {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    {
        let repository = Repository::open(workspace.path()).expect("repository");
        let mut index = repository.index().expect("index");
        let mut entry = index
            .get_path(Path::new("README.md"), 0)
            .expect("staged entry");
        index
            .remove_path(Path::new("README.md"))
            .expect("merged entry removed");
        for stage in [1_u16, 2, 3] {
            entry.flags = stage << 12;
            index.add(&entry).expect("conflict stage");
        }
        index.write().expect("write index");
    }

    let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
    assert_eq!(paths(&observed), ["README.md"], "{observed:#?}");
    assert_eq!(observed.items[0].status, GitChangeStatus::Conflicted);
}

/// The ignore rules of the workspace decide as Git's do: negation, anchored and directory-only
/// rules, `**`, character classes, trailing spaces, an escaped `#`, a nested ignore file and the
/// repository's own exclude file. The expected list is what `git status --untracked-files=all`
/// of Git 2.56 lists for the same tree.
#[test]
fn ignore_rules_decide_as_git_decides() {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    let root = workspace.path();
    write(
        root,
        ".gitignore",
        b"# comment\n*.log\n!keep.log\n/anchored.txt\nbuild/\n!build/kept.txt\ndoc/*.md\n\
          **/deep.tmp\ncache/**\n[ab].dat\ntrailing.txt   \n\\#hash\n/keep/*\n!/keep/this.txt\n",
    );
    write(
        root,
        "sub/.gitignore",
        b"/only-here.txt\n!important.log\n*.gen\n",
    );
    write(root, ".git/info/exclude", b"excluded.txt\n");
    for path in [
        "a.log",
        "keep.log",
        "anchored.txt",
        "sub/anchored.txt",
        "build/x",
        "build/kept.txt",
        "doc/a.md",
        "doc/sub/b.md",
        "x/y/deep.tmp",
        "deep.tmp",
        "cache/a/b",
        "a.dat",
        "c.dat",
        "trailing.txt",
        "#hash",
        "sub/only-here.txt",
        "only-here.txt",
        "sub/important.log",
        "sub/x.gen",
        "x.gen",
        "excluded.txt",
        "sub/excluded.txt",
        "keep/this.txt",
        "keep/other.txt",
    ] {
        write(root, path, b"x\n");
    }

    let observed = observe(root, baselines.path(), 1024).expect("change set");
    assert_eq!(
        paths(&observed),
        [
            ".gitignore",
            "c.dat",
            "doc/sub/b.md",
            "keep.log",
            "keep/this.txt",
            "only-here.txt",
            "sub/.gitignore",
            "sub/anchored.txt",
            "sub/important.log",
            "x.gen",
        ],
        "{observed:#?}"
    );
}

/// An ignore file that starts with a byte-order mark gives every rule it holds, the first one
/// included.
#[test]
fn an_ignore_file_with_a_byte_order_mark_gives_every_rule_it_holds() {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    let root = workspace.path();
    write(root, ".gitignore", b"\xef\xbb\xbffirst.tmp\nsecond.tmp\n");
    for path in ["first.tmp", "second.tmp", "third.tmp"] {
        write(root, path, b"x\n");
    }

    let observed = observe(root, baselines.path(), 1024).expect("change set");
    assert_eq!(
        paths(&observed),
        [".gitignore", "third.tmp"],
        "{observed:#?}"
    );
}

/// Directories that hold no file do not decide whether the observation answers.
#[test]
fn directories_that_hold_no_file_do_not_fail_the_observation() {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    let root = workspace.path();
    std::fs::create_dir_all(root.join(["d"; 70].join("/"))).expect("nested directories");
    write(root, "own.txt", b"own\n");

    let observed = observe(root, baselines.path(), 1024).expect("change set");
    assert_eq!(paths(&observed), ["own.txt"], "{observed:#?}");
}

/// The observation lists the first paths in byte order within the file bound, renders patches
/// within the total byte bound, and says when it left something out.
#[test]
fn the_file_and_total_byte_bounds_cut_the_answer_and_mark_it_truncated() {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    let root = workspace.path();
    for path in ["c.txt", "a.txt", "b.txt"] {
        write(root, path, "line\n".repeat(100).as_bytes());
    }

    let observed = target(root, baselines.path(), DEFAULT_BYTE_LIMIT, |held| {
        changes(
            held,
            GitChangesQuery {
                max_files: 2,
                max_file_bytes: 1024,
                max_total_bytes: 1000,
            },
        )
    })
    .expect("change set");
    assert_eq!(paths(&observed), ["a.txt", "b.txt"], "{observed:#?}");
    assert!(observed.truncated, "{observed:#?}");
    let (first, second) = (&observed.items[0].patch, &observed.items[1].patch);
    assert!(
        first.text.contains("+line\n") && !first.truncated,
        "{first:#?}"
    );
    assert!(second.text.is_empty() && second.truncated, "{second:#?}");
    assert_eq!(
        observed.returned_bytes,
        u64::try_from(first.text.len()).expect("patch length")
    );
}

/// The reader opens no entry of the worktree, and no ignore file, in a way that waits: the
/// observation answers when such an entry is not a regular file, and reports a tracked path
/// that is no longer a file as deleted.
#[test]
fn the_observation_answers_when_worktree_entries_are_not_regular_files() {
    use std::os::unix::fs::OpenOptionsExt as _;
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    let root = workspace.path().to_owned();
    let entries = [
        root.join("README.md"),
        root.join("pipe"),
        root.join(".gitignore"),
        root.join(".git/info/exclude"),
    ];
    std::fs::create_dir_all(root.join(".git/info")).expect("directory");
    for entry in &entries {
        let _ = std::fs::remove_file(entry);
        nix::unistd::mkfifo(entry, nix::sys::stat::Mode::from_bits_truncate(0o600))
            .expect("entry that is not a regular file");
    }

    let (answer, answered) = std::sync::mpsc::channel();
    let (observed_root, baseline) = (root.clone(), baselines.path().to_owned());
    std::thread::spawn(move || {
        let outcome = observe(&observed_root, &baseline, 1024)
            .map(|set| {
                set.items
                    .iter()
                    .map(|item| {
                        let side = item.current.as_ref().or(item.baseline.as_ref());
                        (side.expect("one side").path.clone(), item.status)
                    })
                    .collect::<Vec<_>>()
            })
            .map_err(|error| error.code.to_owned());
        let _ = answer.send(outcome);
    });
    let outcome = answered.recv_timeout(Duration::from_secs(3));
    // Lets a reader that waits return, so the test process keeps no thread.
    for entry in &entries {
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(entry);
    }
    assert_eq!(
        outcome.expect("the observation waited on an entry that is not a regular file"),
        Ok(vec![("README.md".to_owned(), GitChangeStatus::Deleted)])
    );
}

/// A size the reader reports for a baseline entry is the size of an object that was read whole
/// and checked against its id: the object in the driver's store, whatever regular file the
/// workspace's object store holds in its place. No store is built from an object that is not
/// what its id names.
#[test]
fn a_baseline_size_comes_only_from_a_checked_object_of_the_store_the_driver_holds() {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    let object = baseline_object(workspace.path(), commit);
    std::fs::remove_file(&object).expect("object removed");
    std::fs::write(&object, [0xb7, 0x55, 0x00, 0x00]).expect("other bytes in the object's place");
    std::fs::remove_file(workspace.path().join("README.md")).expect("tracked file removed");

    let observed = observe(workspace.path(), baselines.path(), 16).expect("change set");
    let sizes: Vec<u64> = observed
        .items
        .iter()
        .filter_map(|item| item.baseline.as_ref())
        .map(|side| side.size)
        .collect();
    assert_eq!(sizes, [22], "{observed:#?}");

    remove_store(baselines.path());
    let error = observe(workspace.path(), baselines.path(), 16)
        .expect_err("an object that is not what its id names");
    assert_eq!(error.code, "workspace.git-baseline-missing");
    assert!(!store_of(baselines.path()).exists());
}

/// A rule that the bound on an ignore file cuts does not apply in part.
#[test]
fn a_rule_the_ignore_bound_cuts_does_not_apply() {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    let root = workspace.path();
    // The bound falls directly after the first five bytes of the last line.
    let before_rule = usize::try_from(MAX_IO_BYTES).expect("bound fits usize") - 5;
    let mut rules = "# filler line\n".repeat(before_rule / 14);
    match before_rule % 14 {
        0 => {}
        1 => rules.push('\n'),
        rest => {
            rules.push('#');
            rules.push_str(&"x".repeat(rest - 2));
            rules.push('\n');
        }
    }
    assert_eq!(rules.len(), before_rule);
    rules.push_str("*.tmp-not-this\n");
    write(root, ".gitignore", rules.as_bytes());
    write(root, "x.tmp", b"x\n");

    let observed = observe(root, baselines.path(), 1024).expect("change set");
    assert_eq!(paths(&observed), [".gitignore", "x.tmp"], "{observed:#?}");
}

/// The answer ends at a changed path whose baseline object the hashing budget no longer covers,
/// and says so.
#[test]
fn a_baseline_object_the_budget_does_not_cover_ends_the_answer_and_marks_it() {
    let (workspace, first) = source_repository();
    let root = workspace.path();
    write(root, "a.txt", "line\n".repeat(20).as_bytes());
    let commit = commit_paths(root, first, &["a.txt"]);
    let baselines = baseline_root(workspace.path(), commit);
    write(root, "a.txt", "LINE\n".repeat(20).as_bytes());
    let within = |budget: u64| {
        target(root, baselines.path(), DEFAULT_BYTE_LIMIT, |held| {
            changes_within(
                held,
                GitChangesQuery {
                    max_files: 100,
                    max_file_bytes: 1024,
                    max_total_bytes: 1024 * 1024,
                },
                budget,
            )
        })
        .expect("change set")
    };

    // `README.md` and `a.txt` are hashed; the baseline object of `a.txt` is 100 bytes.
    let covered = within(22 + 100 + 100);
    assert_eq!(paths(&covered), ["a.txt"], "{covered:#?}");
    assert!(!covered.truncated);
    let cut = within(22 + 100 + 99);
    assert!(paths(&cut).is_empty(), "{cut:#?}");
    assert!(cut.truncated);
}

/// One tracked file that is too large to compare is reported as changed, the answer is marked,
/// and the paths after it are still compared.
#[test]
fn a_tracked_file_too_large_to_compare_does_not_hide_the_paths_after_it() {
    let (workspace, first) = source_repository();
    let root = workspace.path();
    write(root, "b.txt", b"tracked\n");
    let commit = commit_paths(root, first, &["b.txt"]);
    let baselines = baseline_root(workspace.path(), commit);
    std::fs::File::options()
        .write(true)
        .open(root.join("README.md"))
        .expect("tracked file")
        .set_len(COMPARISON_BUDGET + 1)
        .expect("a length beyond what one observation compares");
    write(root, "b.txt", b"changed\n");

    let observed = observe(root, baselines.path(), 1024).expect("change set");
    assert!(observed.truncated, "{observed:#?}");
    assert_eq!(paths(&observed), ["README.md", "b.txt"], "{observed:#?}");
    let (large, after) = (&observed.items[0], &observed.items[1]);
    assert_eq!(large.status, GitChangeStatus::Modified);
    assert!(
        large.patch.binary && large.patch.text.is_empty(),
        "{large:#?}"
    );
    assert_eq!(large.current.as_ref().expect("current side").oid, None);
    assert_eq!(after.status, GitChangeStatus::Modified);
    assert!(after.patch.text.contains("+changed\n"), "{after:#?}");
}
