//! Cases of an adversary pass over the Git changes reader. Each case states what must hold for
//! the change-set observation of a workspace whose writer also writes its repository metadata.

use std::path::Path;
use std::time::{Duration, SystemTime};

use git2::Repository;
use substrate_wire::{GitChangeStatus, GitChangesQuery};

use super::super::changes;
use super::{baseline_root, commit_paths, observe, paths, source_repository, workspaces_root};

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
    let baselines = baseline_root(commit);
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
    let baselines = baseline_root(commit);
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
    let baselines = baseline_root(commit);
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
    let baselines = baseline_root(commit);

    let observed = observe(workspace.path(), baselines.path(), 1024).expect("change set");
    assert!(paths(&observed).is_empty(), "{observed:#?}");
}

/// A path the index records only in conflict stages is reported as conflicted.
#[test]
fn a_path_the_index_records_only_in_conflict_stages_is_reported_as_conflicted() {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(commit);
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
    let baselines = baseline_root(commit);
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
    let baselines = baseline_root(commit);
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
    let baselines = baseline_root(commit);
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
    let baselines = baseline_root(commit);
    let root = workspace.path();
    for path in ["c.txt", "a.txt", "b.txt"] {
        write(root, path, "line\n".repeat(100).as_bytes());
    }

    let observed = changes(
        "ws_git",
        &workspaces_root(root),
        root,
        baselines.path(),
        "ws_git",
        GitChangesQuery {
            max_files: 2,
            max_file_bytes: 1024,
            max_total_bytes: 1000,
        },
    )
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
    let baselines = baseline_root(commit);
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
