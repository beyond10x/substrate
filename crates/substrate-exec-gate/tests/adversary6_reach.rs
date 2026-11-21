//! Adversary round on `d36436a0f` (design 35 § 18, *Two decisions after the first review of
//! that branch*): **what a directory reaches by the table alone**
//! (`MountTree::reaches_as_written`), against the sentence the decision rests on, that counting
//! a mount the kernel was not asked about "can only make a class stricter".
//!
//! Every table here is text. A class is `beneath` when what a source reaches overlaps what a
//! writable location reaches (`Reach::overlaps`), so "stricter" is: the overlap is found where
//! the kernel's answer would not have found it, and never the other way round.

use substrate_exec_gate::place::{Cause, MountTable, MountTree, Place, Reach, Unlisted};

fn row(id: u64, parent: u64, device: &str, root: &str, mount_point: &str, kind: &str) -> String {
    format!("{id} {parent} {device} {root} {mount_point} rw - {kind} source rw\n")
}

fn tree(text: &str) -> MountTree {
    MountTable::parse(text)
        .expect("the rows parse")
        .into_tree_as_written()
        .expect("a tree")
}

fn at(tree: &MountTree, id: u64, path: &str) -> Place {
    tree.table()
        .place(Some(id), Some(path))
        .unwrap_or_else(|cause| panic!("{path} on mount {id}: {cause}"))
}

fn writable(tree: &MountTree, id: u64, path: &str) -> Reach {
    tree.reaches(&at(tree, id, path), Unlisted::Skipped)
        .expect("the writable side is established")
}

const ROOT: &str = "1 0 8:1 / / rw - ext4 /dev/root rw\n";

/// **A hidden mount, with the mount that hides it shown.** `/w/ws` is bound at `/s/m` and a
/// tmpfs is stacked on it. A run sees the tmpfs through `/s`; the bind is under it.
///
/// - the kernel's answer for `/s` refuses, `mount-covered`: the answer before the decision;
/// - the table's answer counts the hidden bind by its row's root, `/w/ws`, and overlaps the
///   writable `/w`: the class is `beneath`;
/// - the same table without the hidden row is what a run sees, and does not overlap.
///
/// So the count changes the class, from `follow` to `beneath`. That is the direction the
/// sentence allows.
#[test]
fn adversary6_a_hidden_mount_is_counted_and_classes_the_source_beneath() {
    let hidden = format!(
        "{ROOT}{}{}",
        row(2, 1, "8:1", "/w/ws", "/s/m", "ext4"),
        row(3, 2, "0:50", "/", "/s/m", "tmpfs")
    );
    let tree_hidden = tree(&hidden);
    let source = at(&tree_hidden, 1, "/s");
    let refused = tree_hidden
        .reaches(&source, Unlisted::Skipped)
        .expect_err("the kernel shows the tmpfs at /s/m, not the bind");
    assert_eq!(refused.cause, Cause::MountCovered);
    let location = writable(&tree_hidden, 1, "/w");
    assert!(tree_hidden.reaches_as_written(&source).overlaps(&location));

    let seen = tree(&format!(
        "{ROOT}{}",
        row(3, 1, "0:50", "/", "/s/m", "tmpfs")
    ));
    let shown = seen
        .reaches(&at(&seen, 1, "/s"), Unlisted::Skipped)
        .expect("nothing is covered");
    assert!(!shown.overlaps(&writable(&seen, 1, "/w")));
    assert!(
        !seen
            .reaches_as_written(&at(&seen, 1, "/s"))
            .overlaps(&writable(&seen, 1, "/w"))
    );
}

/// A small generator: the same sequence on every run.
struct Sequence(u64);

impl Sequence {
    fn next(&mut self, below: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % below
    }
}

/// **Never fewer.** Over 4 000 tables of up to seven mounts, with stacks, hidden rows, removed
/// roots, repeated mount IDs and two types outside the list: wherever the kernel's answer for a
/// source can be had, the table's answer reaches every place of it and overlaps every writable
/// location it overlaps; and no pair of a source and a writable location overlaps for the
/// kernel's answer and not for the table's. The case prints how often the table's answer found
/// an overlap where the kernel's answer refused.
#[test]
fn adversary6_the_tables_answer_never_reaches_less_than_the_kernels() {
    const DEVICES: [(&str, &str); 4] = [
        ("8:1", "ext4"),
        ("0:50", "tmpfs"),
        ("0:51", "ramfs"),
        ("0:52", "nfs"),
    ];
    const ROOTS: [&str; 5] = ["/", "/a", "/a/b", "/c", "/a//deleted"];
    const NAMES: [&str; 3] = ["a", "b", "c"];
    let mut sequence = Sequence(35);
    let (mut tables, mut pairs, mut both, mut stricter) = (0_u32, 0_u32, 0_u32, 0_u32);
    for _ in 0..4_000 {
        let mut rows: Vec<(u64, u64, String)> = vec![(1, 0, "/".to_owned())];
        let mut text = ROOT.to_owned();
        let count = 2 + sequence.next(6);
        for index in 0..count {
            let (parent, _, above) =
                rows[usize::try_from(sequence.next(rows.len() as u64)).expect("fits")].clone();
            let mount_point = if sequence.next(4) == 0 && above != "/" {
                above.clone()
            } else {
                let name = NAMES[usize::try_from(sequence.next(3)).expect("fits")];
                format!("{}/{name}", above.trim_end_matches('/'))
            };
            let id = if sequence.next(10) == 0 && rows.len() > 1 {
                rows[1 + usize::try_from(sequence.next(rows.len() as u64 - 1)).expect("fits")].0
            } else {
                10 + index
            };
            let (device, kind) = DEVICES[usize::try_from(sequence.next(4)).expect("fits")];
            let root = ROOTS[usize::try_from(sequence.next(5)).expect("fits")];
            text.push_str(&row(id, parent, device, root, &mount_point, kind));
            rows.push((id, parent, mount_point));
        }
        let Ok(tree) = MountTable::parse(&text)
            .expect("parses")
            .into_tree_as_written()
        else {
            continue;
        };
        tables += 1;
        let mut directories = Vec::new();
        for (id, _, mount_point) in &rows {
            for below in ["", "/a", "/a/b", "/c"] {
                let path = format!("{}{below}", mount_point.trim_end_matches('/'));
                let path = if path.is_empty() {
                    "/".to_owned()
                } else {
                    path
                };
                if let Ok(place) = tree.table().place(Some(*id), Some(&path)) {
                    directories.push(place);
                }
            }
        }
        for source in &directories {
            let written = tree.reaches_as_written(source);
            let asked = tree.reaches(source, Unlisted::Skipped);
            if let Ok(asked) = &asked {
                for place in asked.places() {
                    assert!(
                        written.places().contains(place),
                        "{text}the table's answer for {source:?} lacks {place:?}"
                    );
                }
            }
            for location in &directories {
                let Ok(location) = tree.reaches(location, Unlisted::Skipped) else {
                    continue;
                };
                pairs += 1;
                match &asked {
                    Ok(asked) => {
                        both += 1;
                        assert_eq!(
                            asked.overlaps(&location),
                            written.overlaps(&location),
                            "{text}{source:?} against {location:?}"
                        );
                    }
                    Err(_) if written.overlaps(&location) => stricter += 1,
                    Err(_) => {}
                }
            }
        }
    }
    println!(
        "adversary6 reach: {tables} tables, {pairs} pairs of a source and a writable location; \
         in {both} the kernel's answer could be had and the two agreed; in {stricter} it refused \
         and the table's answer overlapped"
    );
    assert!(tables > 1_000 && both > 1_000 && stricter > 0);
}

/// **Row against row, for types outside the list.** The comparison is by the device the rows
/// name and their roots:
///
/// - one file system mounted beneath both, same device, roots nested: they overlap;
/// - a `btrfs` names one device for all its subvolumes: two sibling subvolumes do not overlap,
///   a subvolume and one nested in it do;
/// - **one export mounted twice with a superblock each** (an NFS export under different mount
///   options, two FUSE mounts of one directory) names two devices for the same files: no
///   overlap is found, and the source is classed `follow`. What then keeps a planted link out
///   is the other rule alone: a link, or a path's end, on such a type has no place.
#[test]
fn adversary6_unlisted_mounts_are_compared_by_the_device_their_rows_name() {
    let overlap = |writable_row: String, source_row: String| {
        let tree = tree(&format!("{ROOT}{writable_row}{source_row}"));
        tree.reaches_as_written(&at(&tree, 1, "/s"))
            .overlaps(&writable(&tree, 1, "/w"))
    };
    assert!(overlap(
        row(2, 1, "0:60", "/", "/w/vol", "ramfs"),
        row(3, 1, "0:60", "/sub", "/s/vol", "ramfs")
    ));
    assert!(!overlap(
        row(2, 1, "0:60", "/@a", "/w/vol", "btrfs"),
        row(3, 1, "0:60", "/@b", "/s/vol", "btrfs")
    ));
    assert!(overlap(
        row(2, 1, "0:60", "/@a", "/w/vol", "btrfs"),
        row(3, 1, "0:60", "/@a/nested", "/s/vol", "btrfs")
    ));
    assert!(
        !overlap(
            row(2, 1, "0:60", "/", "/w/vol", "nfs4"),
            row(3, 1, "0:61", "/", "/s/vol", "nfs4")
        ),
        "two superblocks of one export: the rows name two devices"
    );
}

/// **A removed root is counted by the names it had**, and a mount inside the removed directory
/// is found through its parent. The source `/s` holds a mount whose root was removed; the
/// names it had lie in the writable `/w`. The kernel's answer refuses (`mount-root-deleted`);
/// the table's answer overlaps.
#[test]
fn adversary6_a_removed_root_is_counted_by_the_names_it_had() {
    let tree = tree(&format!(
        "{ROOT}{}{}",
        row(2, 1, "8:1", "/w/gone//deleted", "/s/m", "ext4"),
        row(3, 2, "0:50", "/", "/s/m/inner", "tmpfs")
    ));
    let source = at(&tree, 1, "/s");
    assert_eq!(
        tree.reaches(&source, Unlisted::Skipped)
            .expect_err("a removed root")
            .cause,
        Cause::MountRootDeleted
    );
    let written = tree.reaches_as_written(&source);
    assert!(written.overlaps(&writable(&tree, 1, "/w")));
    assert_eq!(written.places().len(), 3, "{written:?}");
}
