//! Adversary pass on the fix `f402165ad` (design 35 D30, *Through the view's mounts*): the mount
//! table as a tree, against tables written to break it. Every table here is text; nothing is
//! mounted and no helper runs.

use std::time::Instant;

use substrate_exec_gate::place::{Asked, Cause, MountTable, Place, Unestablished, Unlisted};

/// One row of a table: `<id> <parent> <device> <root> <mount point>` and the file-system type.
fn row(id: u64, parent: u64, device: &str, root: &str, mount_point: &str, kind: &str) -> String {
    format!("{id} {parent} {device} {root} {mount_point} rw - {kind} source rw\n")
}

/// The place of `path` on the mount `id` of `table`.
fn at(table: &MountTable, id: u64, path: &str) -> Place {
    table
        .place(Some(id), Some(path))
        .unwrap_or_else(|cause| panic!("{path} on mount {id}: {cause}"))
}

/// The refusal of a table that is no tree.
fn unattached(text: &str) -> Unestablished {
    MountTable::parse(text)
        .expect("the rows parse")
        .into_tree_as_written()
        .expect_err("the table is no tree")
}

const ROOT: &str = "1 0 8:1 / / rw - ext4 /dev/root rw\n";

/// **Rows that name each other, and rows that name themselves, refuse the whole table.** A
/// circle at one mount point passes the mount-point comparison of every row, so only the walk
/// to the root can find it.
#[test]
fn adversary5_rows_that_name_each_other_refuse_the_table() {
    let circles = [
        // Two rows, each the other's parent.
        format!(
            "{ROOT}{}{}",
            row(2, 3, "0:50", "/", "/a", "tmpfs"),
            row(3, 2, "0:51", "/", "/a", "tmpfs")
        ),
        // Three.
        format!(
            "{ROOT}{}{}{}",
            row(2, 4, "0:50", "/", "/a", "tmpfs"),
            row(3, 2, "0:51", "/", "/a", "tmpfs"),
            row(4, 3, "0:52", "/", "/a", "tmpfs")
        ),
        // A row that is its own parent and is not at the root.
        format!("{ROOT}{}", row(2, 2, "0:50", "/", "/a", "tmpfs")),
        // Two rows at `/` without a parent row: one names itself.
        format!("{ROOT}{}", row(2, 2, "0:50", "/", "/", "tmpfs")),
        // No root at all: the one row at `/` hangs beneath a circle.
        format!(
            "{}{}{}",
            row(1, 3, "8:1", "/", "/", "ext4"),
            row(2, 1, "0:50", "/", "/", "tmpfs"),
            row(3, 2, "0:51", "/", "/", "tmpfs")
        ),
    ];
    for table in &circles {
        let refused = unattached(table);
        assert_eq!(
            (refused.cause.clone(), refused.asked),
            (Cause::MountRowMissing, Asked::Attachment),
            "{table}"
        );
    }
}

/// **A parent that no row has, and a mount point that only looks beneath its parent's.**
/// `/a/bc` is not beneath `/a/b`: the comparison is by name, never by the text of the path.
#[test]
fn adversary5_a_mount_point_is_compared_with_its_parents_by_name() {
    let absent = unattached(&format!("{ROOT}{}", row(2, 9, "0:50", "/", "/a", "tmpfs")));
    assert_eq!(absent.cause, Cause::MountRowMissing);

    let prefix = unattached(&format!(
        "{ROOT}{}{}",
        row(2, 1, "0:50", "/", "/a/b", "tmpfs"),
        row(3, 2, "0:51", "/", "/a/bc", "tmpfs")
    ));
    assert_eq!(
        (prefix.cause, prefix.path.as_str()),
        (Cause::MountPointMismatch, "/a/bc")
    );

    // The directory `/a/b` of the root file system does not reach a mount at `/a/bc`, and
    // does reach one at `/a/b/c`.
    let tree = MountTable::parse(&format!(
        "{ROOT}{}{}",
        row(2, 1, "0:50", "/", "/a/bc", "tmpfs"),
        row(3, 1, "0:51", "/", "/a/b/c", "tmpfs")
    ))
    .expect("parses")
    .into_tree_as_written()
    .expect("a tree");
    let reached = tree
        .reaches(&at(tree.table(), 1, "/a/b"), Unlisted::Refused)
        .expect("reached");
    assert_eq!(
        reached.places(),
        [at(tree.table(), 1, "/a/b"), at(tree.table(), 3, "/a/b/c")]
    );
}

/// **A mount ID two rows have, beneath a directory.** D26 refuses a path whose reported mount
/// ID two rows have (`mount-row-repeated`): which row is the path's mount cannot be said. For
/// a mount beneath a directory the same holds: the kernel reports ID 3 at `/w/a`, and two rows
/// have it. Neither row is a parent, so the check of the whole table does not see them.
#[test]
fn adversary5_a_repeated_mount_id_beneath_a_directory_refuses() {
    let table = format!(
        "{ROOT}{}{}{}",
        row(2, 1, "0:50", "/", "/w", "tmpfs"),
        row(3, 2, "0:51", "/", "/w/a", "tmpfs"),
        row(3, 1, "0:52", "/", "/other", "tmpfs")
    );
    let answer = MountTable::parse(&table)
        .expect("parses")
        // As written: the kernel is taken to report 3 at `/w/a` and at `/other`, 2 at `/w`.
        .into_tree_as_written()
        .and_then(|tree| {
            let directory = at(tree.table(), 2, "/w");
            tree.reaches(&directory, Unlisted::Skipped)
        });
    let refused = answer.expect_err(
        "two rows have the mount ID the kernel reports at /w/a: which one is mounted there \
         cannot be established",
    );
    assert_eq!(refused.cause, Cause::MountRowRepeated);
}

/// Runs `work` on a thread with the 2 MiB stack a runtime's worker has, and returns what it
/// returned.
fn on_a_worker_stack<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(work)
        .expect("a thread")
        .join()
        .expect("the work ended")
}

/// **A long table.** 10 000 mounts side by side beneath one directory: the tree is linked, and
/// the directory reaches every one. The case prints what the link and the question cost; the
/// bound it asserts is far above both and only catches a table that is never finished.
#[test]
fn adversary5_a_table_of_ten_thousand_rows_is_linked_and_asked() {
    const ROWS: u64 = 10_000;
    let mut text = format!("{ROOT}{}", row(2, 1, "0:50", "/", "/w", "tmpfs"));
    for index in 0..ROWS {
        let id = 100 + index;
        let mount_point = format!("/w/m{index}");
        text.push_str(&row(id, 2, &format!("1:{id}"), "/", &mount_point, "tmpfs"));
    }
    let (linked, asked, reached) = on_a_worker_stack(move || {
        let table = MountTable::parse(&text).expect("parses");
        let start = Instant::now();
        // As written: each mount is shown at its own mount point.
        let tree = table.into_tree_as_written().expect("a tree");
        let linked = start.elapsed();
        let directory = at(tree.table(), 2, "/w");
        let start = Instant::now();
        let reached = tree
            .reaches(&directory, Unlisted::Skipped)
            .expect("reached");
        (linked, start.elapsed(), reached.places().len())
    });
    println!(
        "adversary5 mount tree {ROWS} rows side by side: linked in {linked:?}, one directory \
         asked in {asked:?}"
    );
    assert_eq!(reached, usize::try_from(ROWS).expect("fits") + 1);
    assert!(linked.as_secs() < 60 && asked.as_secs() < 60);
}

/// **A deep table.** `depth` mounts stacked on one mount point beneath a directory, each the
/// parent of the next, written deepest first so that the first row asked is the deepest. Only
/// the top one is of a listed type and shown. The directory reaches it, on a worker's stack.
fn stacked(depth: u64) -> (std::time::Duration, std::time::Duration, usize) {
    let top = 100 + depth - 1;
    let mut rows = Vec::new();
    for level in (0..depth).rev() {
        let id = 100 + level;
        let parent = if level == 0 { 2 } else { id - 1 };
        let kind = if id == top { "tmpfs" } else { "devpts" };
        rows.push(row(id, parent, &format!("1:{id}"), "/", "/w/s", kind));
    }
    let text = format!(
        "{ROOT}{}{}",
        row(2, 1, "0:50", "/", "/w", "tmpfs"),
        rows.concat()
    );
    on_a_worker_stack(move || {
        let table = MountTable::parse(&text).expect("parses");
        let start = Instant::now();
        // As written: the top of the stack at `/w/s`, by the rows' parent links, is shown.
        let tree = table.into_tree_as_written().expect("a tree");
        let linked = start.elapsed();
        let directory = at(tree.table(), 2, "/w");
        let start = Instant::now();
        let reached = tree
            .reaches(&directory, Unlisted::Skipped)
            .expect("reached");
        (linked, start.elapsed(), reached.places().len())
    })
}

#[test]
fn adversary5_a_chain_of_two_thousand_mounts_is_walked_on_a_worker_stack() {
    let (linked, asked, reached) = stacked(2_000);
    println!("adversary5 mount tree 2000 rows deep: linked in {linked:?}, asked in {asked:?}");
    assert_eq!(reached, 2);
}

/// The same 10 000 deep, which is a tenth of the kernel's default bound on the mounts of one
/// namespace (`fs.mount-max`). The walk runs in a process of its own, because a walk that
/// exhausts its stack ends the whole process and not one case.
#[test]
fn adversary5_a_chain_of_ten_thousand_mounts_is_walked_on_a_worker_stack() {
    const CASE: &str = "adversary5_a_chain_of_ten_thousand_mounts_is_walked_on_a_worker_stack";
    const INNER: &str = "ADVERSARY5_MOUNT_TREE_DEEP";
    if std::env::var_os(INNER).is_some() {
        let (linked, asked, reached) = stacked(10_000);
        println!("adversary5 mount tree 10000 rows deep: linked in {linked:?}, asked in {asked:?}");
        assert_eq!(reached, 2);
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", CASE, "--nocapture", "--test-threads=1"])
        .env(INNER, "1")
        .output()
        .expect("the walk starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "the process that asked what a directory reaches ended {:?}: {}",
        output.status,
        stderr.trim()
    );
    for line in stdout
        .lines()
        .filter(|line| line.contains("adversary5 mount tree"))
    {
        println!("{line}");
    }
}
