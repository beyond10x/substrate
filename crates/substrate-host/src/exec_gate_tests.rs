//! The executable gate's decisions (design 35 D19 to D21, D24 to D27).
//!
//! The first half decides on what it is given: what the kernel reported for a path, a mount
//! table as text, metadata and a protocol line, as `tmp_size_served_by` decides on help text.
//! It needs no sandbox, no mount and no file system of a particular type.
//!
//! The second half asks the same of real files. A place is established only on the listed
//! file-system types, so on a host whose temporary directory or `/usr` is on another type each
//! of those cases prints that it is absent and why, and asserts nothing.

use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};
use substrate_exec_gate::place::{Cause, MountTable, MountTree, Opened, Unestablished};
use substrate_exec_gate::{Class, Invocation, Lists, Located};
use substrate_guarded::DriverErrorClass;
use substrate_wire::ReadOnlyRoot;

use super::{
    EXEC_GATE_INVALID, EXEC_GATE_UNSERVED, ExecGate, FileFacts, Location, Mounts, OpenDecision,
    OpenPlacement, OpenPlaces, OpenSightings, OpenUnplaced, Opening, PLACE_UNSERVED, Shown,
    Sighted, Source, Unfollowed, View, check_file, check_length, check_linkage, check_links,
    check_location, check_openable, check_protocol, class_together, classed, decide_open,
    denied_unplaced, judge_denied, judge_link, judge_passed, open, place_open, plan,
    shown_location, start_unplaced, withheld_facts, writable_source,
};
use crate::confinement::{Fact, FactWithheld};
use crate::seatbelt::{EXEC_SUBPATH_UNDECLARED, PATH_UNREPRESENTABLE};
use crate::{DriverError, HostConfig};

/// A host: the workspace root is a directory of the root file system, and the host mounts it a
/// second time at `/mnt/second` (K6). `/tmp` is a tmpfs, and `/data` a file system of a type
/// outside the list.
const HOST: &str = "\
31 1 259:2 / / rw,noatime shared:1 - ext4 /dev/nvme0n1p2 rw
60 31 259:2 /work/u/workspaces /mnt/second rw,noatime shared:1 - ext4 /dev/nvme0n1p2 rw
61 31 0:40 / /tmp rw,nosuid,nodev shared:30 - tmpfs tmpfs rw
62 31 8:17 / /data rw,noatime shared:40 - xfs /dev/sdb1 rw
";

fn host() -> MountTree {
    tree(HOST)
}

/// The table `text` as a tree in which no mount is hidden.
fn tree(text: &str) -> MountTree {
    MountTable::parse(text)
        .expect("the table parses")
        .into_tree_as_written()
        .expect("every row of the table is attached")
}

/// What the kernel would report for `path`, opened on the mount with `mount_id`.
fn seen(path: &str, mount_id: u64) -> Opened {
    Opened {
        path: path.to_owned(),
        mount_id: Some(mount_id),
        descriptor_path: Some(path.to_owned()),
        remaining: Vec::new(),
    }
}

/// The open of a host whose helper is `gate`: the workspace root, one declared writable root,
/// `/usr` and the probes' temporary directory.
fn sightings(gate: Opened) -> OpenSightings {
    OpenSightings {
        gate,
        workspace_root: seen("/work/u/workspaces", 31),
        writable_roots: vec![seen("/srv/out", 31)],
        base: vec![seen("/usr", 31)],
        temporary: seen("/tmp", 61),
    }
}

/// The locations a run can write on that host, placed.
fn writable() -> Vec<Location> {
    ["/work/u/workspaces", "/srv/out"]
        .iter()
        .map(|path| {
            let place = host().table().place_of(&seen(path, 31)).expect("placed");
            Location::reaching(path, &place, &host()).expect("reached")
        })
        .collect()
}

/// The places of an open on which every place is established.
fn established(sightings: &OpenSightings, table: &MountTree) -> OpenPlaces {
    match place_open(sightings, table) {
        OpenPlacement::Placed(places) => places,
        other => panic!("a place of the open was not established: {other:?}"),
    }
}

fn gate() -> &'static Path {
    Path::new("/opt/substrate/substrate-exec-gate")
}

fn assert_invalid(error: &DriverError, holds: &str) {
    assert_eq!(error.code, EXEC_GATE_INVALID, "{error}");
    assert_eq!(error.class, DriverErrorClass::Refused, "{error}");
    assert_eq!(error.address.as_deref(), Some("config.exec_gate"));
    assert!(error.message.contains(holds), "{error}");
}

fn assert_unserved(error: &DriverError, holds: &str) {
    assert_eq!(error.code, EXEC_GATE_UNSERVED, "{error}");
    assert_eq!(error.class, DriverErrorClass::Unserved, "{error}");
    assert_eq!(error.address.as_deref(), Some("config.exec_gate"));
    assert!(error.message.contains(holds), "{error}");
}

/// D21: the helper's file is refused where a run can write, by its place. The second mount of
/// the workspace root is the case the text of the two paths gets wrong (K6).
#[test]
fn a_gate_in_a_location_a_run_can_write_is_invalid_by_place() {
    let table = host();
    let outside = established(
        &sightings(seen("/opt/substrate/substrate-exec-gate", 31)),
        &table,
    );
    check_location(gate(), &outside).expect("a helper outside every writable location");
    // On a tmpfs, which is a listed type and another device: outside too.
    let temporary = established(&sightings(seen("/tmp/gate", 61)), &table);
    check_location(gate(), &temporary).expect("a helper on another file system");
    for (opened, location) in [
        (
            seen("/work/u/workspaces/ws_1/gate", 31),
            "/work/u/workspaces",
        ),
        (seen("/srv/out/tools/gate", 31), "/srv/out"),
        // Not beneath the workspace root as text, and inside it.
        (seen("/mnt/second/ws_1/gate", 60), "/work/u/workspaces"),
    ] {
        let places = established(&sightings(opened), &table);
        let error = check_location(gate(), &places).expect_err("a helper a run could have written");
        assert_invalid(
            &error,
            &format!("lies in {location}, which a run can write"),
        );
        assert!(
            error.message.contains("/opt/substrate/substrate-exec-gate"),
            "the message names the configured path: {error}"
        );
    }
}

/// A host with mounts beneath what a run can write, and with a writable location on a mount
/// beneath a directory that is not one: a file system mounted in a workspace, another one
/// mounted in that, and one mounted in `/srv/tools`, on which the declared writable root lies.
const HOST_WITH_MOUNTS_BENEATH: &str = "\
31 1 259:2 / / rw,noatime shared:1 - ext4 /dev/nvme0n1p2 rw
61 31 0:40 / /tmp rw,nosuid,nodev shared:30 - tmpfs tmpfs rw
70 31 0:50 / /work/u/workspaces/ws_1/media rw - tmpfs tmpfs rw
71 70 0:51 / /work/u/workspaces/ws_1/media/deep rw - tmpfs tmpfs rw
72 31 0:52 / /srv/tools/media rw - tmpfs tmpfs rw
";

/// The open of that host: the declared writable root lies on the mount in `/srv/tools`.
fn open_with_mounts_beneath(gate: Opened) -> OpenSightings {
    OpenSightings {
        writable_roots: vec![seen("/srv/tools/media/out", 72)],
        ..sightings(gate)
    }
}

/// The locations a run can write on that host, each with what it reaches.
fn writable_with_mounts_beneath(table: &MountTree) -> Vec<Location> {
    established(
        &open_with_mounts_beneath(seen("/opt/substrate/substrate-exec-gate", 31)),
        table,
    )
    .writable
}

/// D30 on story 1's checks (design 35 § 6, *Through the view's mounts*): what is mounted
/// beneath a location a run can write lies in that location, although its place is on another
/// device. **The helper's file** there is refused (D21): on the mount in a workspace, and on
/// the one mounted in that.
#[test]
fn what_is_mounted_beneath_a_writable_location_lies_in_it_for_the_helpers_file() {
    let table = tree(HOST_WITH_MOUNTS_BENEATH);
    for (path, mount) in [
        ("/work/u/workspaces/ws_1/media/gate", 70),
        ("/work/u/workspaces/ws_1/media/deep/gate", 71),
    ] {
        let places = established(&open_with_mounts_beneath(seen(path, mount)), &table);
        let error = check_location(gate(), &places)
            .expect_err("a helper in a mount beneath the workspace root");
        assert_invalid(&error, "lies in /work/u/workspaces, which a run can write");
    }
    // Beside them: on the root file system, and on a mount beneath no writable location.
    for (path, mount) in [
        ("/opt/substrate/substrate-exec-gate", 31),
        ("/tmp/gate", 61),
    ] {
        let places = established(&open_with_mounts_beneath(seen(path, mount)), &table);
        check_location(gate(), &places).expect("a helper outside what a run can write");
    }
    // On the mount the declared writable root lies on, beside that root: the root reaches
    // nothing above itself.
    let beside = established(
        &open_with_mounts_beneath(seen("/srv/tools/media/gate", 72)),
        &table,
    );
    check_location(gate(), &beside).expect("a helper beside the writable root");
}

/// D30 on **the class of a mount** (D24): a read-only root whose host source lies on a mount
/// beneath a writable location is classed `beneath`, and so is one that holds a mount on which
/// a writable location lies.
#[test]
fn what_is_mounted_beneath_a_writable_location_lies_in_it_for_the_class() {
    let table = tree(HOST_WITH_MOUNTS_BENEATH);
    let writable = writable_with_mounts_beneath(&table);
    for (path, mount, class) in [
        // On a mount beneath the workspace root, and on one beneath that.
        ("/work/u/workspaces/ws_1/media/tools", 70, Class::Beneath),
        (
            "/work/u/workspaces/ws_1/media/deep/tools",
            71,
            Class::Beneath,
        ),
        // Holds a mount on which the declared writable root lies.
        ("/srv/tools", 31, Class::Beneath),
        ("/srv", 31, Class::Beneath),
        // Beside every writable location and every mount beneath one.
        ("/srv/other", 31, Class::Follow),
        ("/srv/tools/media/other", 72, Class::Follow),
        ("/tmp/tools", 61, Class::Follow),
    ] {
        assert_eq!(
            classed("read_only_roots", &seen(path, mount), &writable, &table).expect("classed"),
            class,
            "{path}"
        );
    }
}

/// D30 on **a followed entry** (D24): a path that ends on a mount beneath a writable location
/// names a file a run can replace, on the mount in a workspace and on the one mounted in that.
#[test]
fn what_is_mounted_beneath_a_writable_location_lies_in_it_for_a_followed_object() {
    let table = tree(HOST_WITH_MOUNTS_BENEATH);
    let writable = writable_with_mounts_beneath(&table);
    let ends_in = |path: &str, mount: u64| {
        let place = table.table().place_of(&seen(path, mount)).expect("placed");
        shown_location(&place, &writable).map(|location| location.path.clone())
    };
    assert_eq!(
        ends_in("/work/u/workspaces/ws_1/media/x", 70).as_deref(),
        Some("/work/u/workspaces"),
        "a followed path that ends on a mount beneath the workspace root"
    );
    assert_eq!(
        ends_in("/work/u/workspaces/ws_1/media/deep/x", 71).as_deref(),
        Some("/work/u/workspaces"),
        "and on a mount mounted in that one"
    );
    assert_eq!(
        ends_in("/srv/tools/media/out/x", 72).as_deref(),
        Some("/srv/tools/media/out")
    );
    assert_eq!(ends_in("/srv/tools/media/other", 72), None);
    assert_eq!(ends_in("/tmp/x", 61), None);
}

/// D30's causes at the open. A mount beneath a writable location whose root cannot be
/// compared refuses it `config.exec-gate-unserved`, with the location's own field as the
/// address and the mount point in the text.
#[test]
fn a_mount_beneath_a_writable_location_that_cannot_be_compared_refuses_the_open() {
    use substrate_exec_gate::place::Asked;
    let helper = || sightings(seen("/opt/substrate/substrate-exec-gate", 31));
    let stacked = format!(
        "{HOST}\
80 31 0:60 / /work/u/workspaces/ws_1/stack rw - tmpfs tmpfs rw
81 80 0:61 / /work/u/workspaces/ws_1/stack rw - tmpfs tmpfs rw
"
    );
    let deleted =
        format!("{HOST}82 31 259:2 /gone//deleted /srv/out/removed rw - ext4 /dev/nvme0n1p2 rw\n");
    for (table, address, mount_point, cause) in [
        (
            stacked.as_str(),
            "config.workspace_root",
            "/work/u/workspaces/ws_1/stack",
            Cause::MountCovered,
        ),
        (
            deleted.as_str(),
            "config.writable_roots",
            "/srv/out/removed",
            Cause::MountRootDeleted,
        ),
    ] {
        let OpenPlacement::Refused(unplaced) = place_open(&helper(), &tree(table)) else {
            panic!("{cause} beneath a writable location did not refuse the open");
        };
        assert_eq!(unplaced.address, address, "{cause}");
        assert_eq!(
            unplaced.why,
            Unestablished {
                path: mount_point.to_owned(),
                cause: cause.clone(),
                asked: Asked::Attachment,
            }
        );
        assert!(!unplaced.withholds(), "{cause} refuses the open");
        let error = unplaced.refusal(gate());
        assert_eq!(error.code, EXEC_GATE_UNSERVED, "{error}");
        assert_eq!(error.class, DriverErrorClass::Unserved, "{error}");
        assert_eq!(error.address.as_deref(), Some(address));
        assert!(
            error.message.contains(&format!(
                "cannot establish where {mount_point} is attached: {cause}"
            )),
            "{error}"
        );
    }
}

/// D30 on the side nothing is written through: **a mount beneath the host source of a
/// declared mount refuses no start**, whatever the kernel would say of it, where the source
/// overlaps nothing a run can write. A stack, a removed root and a mount ID two rows have are
/// counted by the root each row names, and the class is `follow` where none of them shows a
/// writable place. Where one does, although a later mount covers it, the source is a writable
/// location from then on and is asked as one
/// (`a_source_that_overlaps_a_writable_location_is_asked_as_one`). The fail-closed causes
/// belong to the writable side
/// (`a_mount_beneath_a_writable_location_that_cannot_be_compared_refuses_the_open`).
#[test]
fn a_mount_beneath_a_source_that_cannot_be_compared_refuses_no_start() {
    let helper = || sightings(seen("/opt/substrate/substrate-exec-gate", 31));
    let class = |rows: &str, source: &str| {
        let table = tree(&format!("{HOST}{rows}"));
        let writable = established(&helper(), &table).writable;
        classed("read_only_roots", &seen(source, 31), &writable, &table)
            .unwrap_or_else(|error| panic!("{source} with {rows}: {error}"))
    };
    for rows in [
        // Two mounts on one mount point: the kernel does not show the lower one.
        "80 31 0:60 / /opt/tools/stack rw - tmpfs tmpfs rw\n\
         81 80 0:61 / /opt/tools/stack rw - tmpfs tmpfs rw\n",
        // A mount whose directory was removed.
        "82 31 259:2 /gone//deleted /opt/tools/removed rw - ext4 /dev/nvme0n1p2 rw\n",
        // A mount ID two rows have.
        "83 31 0:62 / /opt/tools/twice rw - tmpfs tmpfs rw\n\
         83 31 0:63 / /opt/tools/again rw - tmpfs tmpfs rw\n",
    ] {
        assert_eq!(class(rows, "/opt/tools"), Class::Follow, "{rows}");
        assert_eq!(class(rows, "/usr"), Class::Follow, "{rows}");
    }
    // Counted, not skipped: the covered mount shows a directory of a workspace, so the source
    // is never `follow` for want of the kernel's answer. It is a location a run can write, and
    // the stack beneath it cannot be compared: refused here, before anything is dispatched.
    let covered = "80 31 259:2 /work/u/workspaces/ws_1/media /opt/tools/stack rw - ext4 /dev/a rw\n\
                   81 80 0:61 / /opt/tools/stack rw - tmpfs tmpfs rw\n";
    let table = tree(&format!("{HOST}{covered}"));
    let writable = established(&helper(), &table).writable;
    let error = classed(
        "read_only_roots",
        &seen("/opt/tools", 31),
        &writable,
        &table,
    )
    .expect_err("a source that shows a workspace through a covered mount");
    assert_eq!(error.code, PLACE_UNSERVED, "{error}");
    assert_eq!(error.address.as_deref(), Some("read_only_roots"));
    assert!(
        error
            .message
            .contains("cannot establish where /opt/tools/stack is attached: mount-covered"),
        "{error}"
    );
    assert_eq!(class(covered, "/opt/other"), Class::Follow);
    // The writable side still fails closed on the same stack.
    let stacked = tree(&format!(
        "{HOST}\
80 31 0:60 / /srv/out/stack rw - tmpfs tmpfs rw
81 80 0:61 / /srv/out/stack rw - tmpfs tmpfs rw
"
    ));
    assert!(matches!(
        place_open(&helper(), &stacked),
        OpenPlacement::Refused(OpenUnplaced {
            address: "config.writable_roots",
            ..
        })
    ));
    // A row attached nowhere refuses the table, with the helper's field at the open and the
    // list's at a start.
    let unattached = MountTable::parse(&format!(
        "{HOST}90 999 0:70 / /srv/lost rw - tmpfs tmpfs rw\n"
    ))
    .expect("the table parses")
    .into_tree_as_written()
    .expect_err("a row whose parent no row has");
    assert_eq!(unattached.cause, Cause::MountRowMissing);
    let at_open = OpenUnplaced::table(unattached.clone());
    assert!(!at_open.withholds());
    let error = at_open.refusal(gate());
    assert_eq!(
        (error.code, error.address.as_deref()),
        (EXEC_GATE_UNSERVED, Some("config.exec_gate"))
    );
    let attached = "cannot establish where /srv/lost is attached: mount-row-missing";
    assert!(error.message.contains(attached), "{error}");
    let error = start_unplaced("exec_subpaths", &unattached);
    assert_eq!(error.code, PLACE_UNSERVED);
    assert!(error.message.contains(attached), "{error}");
}

/// D24 and D30, **a source that overlaps a location a run can write is one from then on**:
/// the helper is told the run can write its mount and asks the kernel about every mount
/// beneath such a mount, so the driver asks the same before dispatch. Where every mount
/// beneath the source can be compared it is a location with all it reaches; where one cannot,
/// the start is refused `exec.sandbox-place-unserved` with the cause and the field the mount
/// was declared in, in place of a run the helper ends with status 125.
#[test]
fn a_source_that_overlaps_a_writable_location_is_asked_as_one() {
    // A mount beneath the source that shows a directory of a workspace.
    const SHOWS: &str =
        "80 31 259:2 /work/u/workspaces/ws_1/media /opt/tools/shown rw - ext4 /dev/a rw\n";
    let asked = |rows: &str, address: &str| {
        let table = tree(&format!("{HOST}{SHOWS}{rows}"));
        let writable = established(
            &sightings(seen("/opt/substrate/substrate-exec-gate", 31)),
            &table,
        )
        .writable;
        let source = writable_source(address, &seen("/opt/tools", 31), &writable, &table);
        (table, source)
    };
    // Every mount beneath it can be compared: a location, with what it reaches.
    let (table, location) = asked(
        "84 31 0:64 / /opt/tools/cache rw - tmpfs tmpfs rw\n",
        "read_only_roots",
    );
    let location = location
        .expect("every mount beneath the source is compared")
        .expect("a source that shows a workspace");
    assert_eq!(location.path, "/opt/tools");
    for (path, mount) in [
        ("/opt/tools/bin", 31),
        ("/opt/tools/shown/x", 80),
        ("/opt/tools/cache/x", 84),
    ] {
        let place = table.table().place_of(&seen(path, mount)).expect("placed");
        assert!(location.reaches.holds(&place), "{path}");
    }
    // One cannot: each cause the writable side fails closed on, with the declaring field.
    for (rows, address, holds) in [
        (
            "81 31 0:60 / /opt/tools/stack rw - tmpfs tmpfs rw\n\
             82 81 0:61 / /opt/tools/stack rw - tmpfs tmpfs rw\n",
            "read_only_roots",
            "cannot establish where /opt/tools/stack is attached: mount-covered",
        ),
        (
            "82 31 259:2 /gone//deleted /opt/tools/removed rw - ext4 /dev/nvme0n1p2 rw\n",
            "read_only_files",
            "cannot establish where /opt/tools/removed is attached: mount-root-deleted",
        ),
        (
            "83 31 0:62 / /opt/tools/twice rw - tmpfs tmpfs rw\n\
             83 31 0:63 / /opt/tools/again rw - tmpfs tmpfs rw\n",
            "exec_subpaths",
            "mount-row-repeated",
        ),
    ] {
        let error = asked(rows, address)
            .1
            .expect_err("a mount beneath a writable source that cannot be compared");
        assert_eq!(error.code, PLACE_UNSERVED, "{error}");
        assert_eq!(error.class, DriverErrorClass::Unserved, "{error}");
        assert_eq!(error.address.as_deref(), Some(address), "{error}");
        assert!(error.message.contains(holds), "{error}");
    }
}

/// D24, **the classes of the shown mounts are decided together.** A source classed `beneath`
/// is a location a run can write, and a source that overlaps it is `beneath` too, although it
/// overlaps no workspace and no declared writable root. The answer is the same in every order
/// of declaration, and what the helper will be told the run can write is what every path is
/// judged against.
#[test]
fn the_classes_of_the_shown_mounts_do_not_depend_on_their_order() {
    // `/opt/a` shows a directory of a workspace and holds a tmpfs that `/opt/b` holds too.
    const ROWS: &str = "\
80 31 259:2 /work/u/workspaces/ws_1/media /opt/a/shown rw - ext4 /dev/a rw
84 31 0:64 / /opt/a/shared rw - tmpfs tmpfs rw
85 31 0:64 / /opt/b/shared rw - tmpfs tmpfs rw
";
    let shown = |address: &'static str, path: &str| Shown {
        address,
        path: path.to_owned(),
        source: Source::Opaque,
        file: false,
        eligible: true,
        opened: seen(path, 31),
    };
    let together = |rows: &str, order: [&str; 3]| {
        let table = tree(&format!("{HOST}{ROWS}{rows}"));
        let mut locations = established(
            &sightings(seen("/opt/substrate/substrate-exec-gate", 31)),
            &table,
        )
        .writable;
        let shown: Vec<Shown<'_>> = order
            .iter()
            .map(|path| match *path {
                "/opt/b" => shown("read_only_files", path),
                _ => shown("read_only_roots", path),
            })
            .collect();
        class_together(&shown, &mut locations, &table).map(|classes| {
            let joined: Vec<String> = locations[2..]
                .iter()
                .map(|location| location.path.clone())
                .collect();
            (classes, joined)
        })
    };
    let (classes, joined) = together("", ["/opt/a", "/opt/b", "/opt/c"]).expect("classed");
    assert_eq!(classes, [Class::Beneath, Class::Beneath, Class::Follow]);
    assert_eq!(joined, ["/opt/a", "/opt/b"]);
    // `/opt/b` first: `follow` against the workspace alone, `beneath` once `/opt/a` joined.
    let (classes, joined) = together("", ["/opt/b", "/opt/c", "/opt/a"]).expect("classed");
    assert_eq!(classes, [Class::Beneath, Class::Follow, Class::Beneath]);
    assert_eq!(joined, ["/opt/a", "/opt/b"]);
    // And it is asked as a writable location: a stack beneath it refuses, with its own field.
    let stack = "86 31 0:65 / /opt/b/stack rw - tmpfs tmpfs rw\n\
                 87 86 0:66 / /opt/b/stack rw - tmpfs tmpfs rw\n";
    for order in [
        ["/opt/a", "/opt/b", "/opt/c"],
        ["/opt/b", "/opt/c", "/opt/a"],
    ] {
        let error = together(stack, order).expect_err("a stack beneath a writable source");
        assert_eq!(error.code, PLACE_UNSERVED, "{error}");
        assert_eq!(error.address.as_deref(), Some("read_only_files"), "{error}");
        assert!(
            error
                .message
                .contains("cannot establish where /opt/b/stack is attached: mount-covered"),
            "{error}"
        );
    }
}

/// D30 and a file-system type outside the list. Such a mount is compared as a place nowhere:
/// beneath a workspace (the device nodes bubblewrap binds are such mounts) and beneath a
/// declared root's host source it refuses nothing. Row against row it is compared: the same
/// file system mounted beneath a workspace and beneath the source, whole or in part, is one
/// a run can write, and the source is `beneath`.
#[test]
fn a_mount_of_an_unlisted_type_is_compared_row_against_row_only() {
    let helper = || sightings(seen("/opt/substrate/substrate-exec-gate", 31));
    let class = |rows: &str| {
        let table = tree(&format!("{HOST}{rows}"));
        let writable = established(&helper(), &table).writable;
        classed(
            "read_only_roots",
            &seen("/opt/tools", 31),
            &writable,
            &table,
        )
        .unwrap_or_else(|error| panic!("{rows}: {error}"))
    };
    for (rows, expected) in [
        // Beneath the workspace only, and beneath the source only.
        (
            "83 31 0:5 /null /work/u/workspaces/ws_1/null rw - devtmpfs devtmpfs rw\n\
             84 31 8:33 / /opt/tools/data rw - xfs /dev/sdc1 rw\n",
            Class::Follow,
        ),
        // One ramfs beneath both.
        (
            "85 31 0:70 / /work/u/workspaces/ws_1/media rw - ramfs ramfs rw\n\
             86 31 0:70 / /opt/tools/media rw - ramfs ramfs rw\n",
            Class::Beneath,
        ),
        // A part of it beneath the source, and the whole beneath a part of the workspace.
        (
            "85 31 0:70 / /work/u/workspaces/ws_1/media rw - ramfs ramfs rw\n\
             86 31 0:70 /sub /opt/tools/media rw - ramfs ramfs rw\n",
            Class::Beneath,
        ),
        (
            "85 31 0:70 /sub/deep /work/u/workspaces/ws_1/media rw - ramfs ramfs rw\n\
             86 31 0:70 /sub /opt/tools/media rw - ramfs ramfs rw\n",
            Class::Beneath,
        ),
        // Two file systems of the type, and two parts of one that are apart.
        (
            "85 31 0:70 / /work/u/workspaces/ws_1/media rw - ramfs ramfs rw\n\
             86 31 0:71 / /opt/tools/media rw - ramfs ramfs rw\n",
            Class::Follow,
        ),
        (
            "85 31 0:70 /one /work/u/workspaces/ws_1/media rw - ramfs ramfs rw\n\
             86 31 0:70 /two /opt/tools/media rw - ramfs ramfs rw\n",
            Class::Follow,
        ),
    ] {
        assert_eq!(class(rows), expected, "{rows}");
    }
}

/// D30, **every symbolic link the walk of a followed entry reads is placed.** A link on a
/// mount of a file-system type outside the list has no place: no rule compares such a mount,
/// so whether a run can write the link cannot be said, and the start is refused
/// `exec.sandbox-place-unserved` with the cause and the link's path in the view. A link with a
/// place is judged by it: on a mount beneath a workspace a run can replace it.
#[test]
fn a_symbolic_link_the_walk_reads_is_placed() {
    let table = tree(&format!(
        "{HOST}\
70 31 0:50 / /work/u/workspaces/ws_1/media rw - tmpfs tmpfs rw
85 31 0:70 / /opt/tools/media rw - ramfs ramfs rw
"
    ));
    let writable = established(
        &sightings(seen("/opt/substrate/substrate-exec-gate", 31)),
        &table,
    )
    .writable;
    // The link as the walk sees it: named by its view path, reported at its host path.
    let link = |view: &str, host: &str, mount_id: Option<u64>| Opened {
        path: view.to_owned(),
        mount_id,
        descriptor_path: Some(host.to_owned()),
        remaining: Vec::new(),
    };
    let judged = |link: &Opened| judge_link(link, &table, &writable);
    for (mount_id, cause) in [
        (Some(85), "file-system-unlisted ramfs"),
        (None, "mount-id-unreported"),
        (Some(999), "mount-row-missing"),
        (Some(61), "mount-point-mismatch"),
    ] {
        let unplaced = match judged(&link(
            "/opt/k/media/link",
            "/opt/tools/media/link",
            mount_id,
        )) {
            Err(why @ Unfollowed::Unplaced(_)) => why,
            Err(Unfollowed::Refused(why) | Unfollowed::Absent(why)) => {
                panic!("{cause}: refused for the path: {why}")
            }
            Ok(()) => panic!("{cause}: a link with no place was followed"),
        };
        let error = unplaced.refusal("/opt/k/media/link", "exec_subpaths");
        assert_eq!(error.code, PLACE_UNSERVED, "{error}");
        assert_eq!(error.class, DriverErrorClass::Unserved, "{error}");
        assert_eq!(error.address.as_deref(), Some("exec_subpaths"));
        assert!(
            error.message.contains(&format!(
                "cannot establish the place of /opt/k/media/link: {cause}"
            )),
            "{error}"
        );
    }
    // A link a run can replace: on the mount beneath a workspace.
    match judged(&link(
        "/opt/j/link",
        "/work/u/workspaces/ws_1/media/link",
        Some(70),
    )) {
        Err(Unfollowed::Refused(why)) => {
            assert_eq!(
                why,
                "resolves through the symbolic link /opt/j/link, which lies in \
                 /work/u/workspaces, a location a run can write"
            );
            let error = Unfollowed::Refused(why).refusal("/opt/j/link", "exec_subpaths");
            assert_eq!(error.code, PATH_UNREPRESENTABLE, "{error}");
        }
        Err(Unfollowed::Unplaced(why)) => panic!("{why}"),
        Err(Unfollowed::Absent(why)) => panic!("read as absent: {why}"),
        Ok(()) => panic!("a link on a mount beneath a workspace was followed"),
    }
    // A link nothing writable reaches is followed.
    for (view, host, mount_id) in [
        ("/usr/bin/sh", "/usr/bin/sh", 31),
        ("/opt/k/link", "/opt/tools/link", 31),
        ("/tmp/link", "/tmp/link", 61),
    ] {
        assert!(
            judged(&link(view, host, Some(mount_id))).is_ok(),
            "{view} was not followed"
        );
    }
}

/// D30, **every directory the walk of a followed entry passes through is placed.** One with no
/// place refuses the start like a link with none, and one in a place a writable location
/// reaches refuses the path: a run can rename what the directory holds while the helper
/// resolves the same path. Either answer is handed back for the walk to hold until the
/// path's end was judged.
#[test]
fn a_directory_the_walk_passes_through_is_placed() {
    let table = tree(&format!(
        "{HOST}\
70 31 0:50 / /work/u/workspaces/ws_1/media rw - tmpfs tmpfs rw
85 31 0:70 / /opt/tools/media rw - ramfs ramfs rw
"
    ));
    let writable = established(
        &sightings(seen("/opt/substrate/substrate-exec-gate", 31)),
        &table,
    )
    .writable;
    let passed = |view: &str, host: &str, mount_id: u64| {
        let directory = Opened {
            path: view.to_owned(),
            mount_id: Some(mount_id),
            descriptor_path: Some(host.to_owned()),
            remaining: Vec::new(),
        };
        judge_passed(&directory, &table, &writable)
    };
    match passed("/opt/k/media/dir", "/opt/tools/media/dir", 85) {
        Some(why @ Unfollowed::Unplaced(_)) => {
            let error = why.refusal("/opt/k/media/dir/tool", "exec_subpaths");
            assert_eq!(error.code, PLACE_UNSERVED, "{error}");
            assert_eq!(error.address.as_deref(), Some("exec_subpaths"));
            assert!(
                error.message.contains(
                    "cannot establish the place of /opt/k/media/dir: file-system-unlisted ramfs"
                ),
                "{error}"
            );
        }
        Some(Unfollowed::Refused(why) | Unfollowed::Absent(why)) => {
            panic!("refused for the path: {why}")
        }
        None => panic!("a directory with no place was passed"),
    }
    match passed("/opt/j/dir", "/work/u/workspaces/ws_1/media/dir", 70) {
        Some(Unfollowed::Refused(why)) => {
            assert_eq!(
                why,
                "passes through /opt/j/dir, which lies in /work/u/workspaces, a location a run \
                 can write"
            );
            let error = Unfollowed::Refused(why).refusal("/opt/j/dir/../tool", "exec_subpaths");
            assert_eq!(error.code, PATH_UNREPRESENTABLE, "{error}");
        }
        Some(Unfollowed::Unplaced(why)) => panic!("{why}"),
        Some(Unfollowed::Absent(why)) => panic!("read as absent: {why}"),
        None => panic!("a directory a run can write was passed"),
    }
    for (view, host, mount_id) in [
        ("/usr/lib", "/usr/lib", 31),
        ("/opt/k/dir", "/opt/tools/dir", 31),
        ("/tmp/dir", "/tmp/dir", 61),
    ] {
        assert!(
            passed(view, host, mount_id).is_none(),
            "{view} is in no place a run can write"
        );
    }
}

/// The entries of an open that withholds; any other placement fails the case.
fn withheld(placement: OpenPlacement) -> Vec<OpenUnplaced> {
    match placement {
        OpenPlacement::Withheld(unplaced) => unplaced,
        OpenPlacement::Placed(_) => panic!("every place was established"),
        OpenPlacement::Refused(unplaced) => panic!("the open is refused: {unplaced:?}"),
    }
}

fn entry(address: &'static str, path: &str, file_system: Option<&str>) -> FactWithheld {
    FactWithheld {
        fact: Fact::ExecSubpaths,
        cause: if file_system.is_some() {
            "file-system-unlisted"
        } else {
            "mount-id-unreported"
        },
        address,
        path: path.to_owned(),
        file_system: file_system.map(str::to_owned),
    }
}

/// What an open records for `paths`: for each, in order, one entry per fact the helper would
/// prove, the allow-list's and then the deny-list's.
fn per_fact(paths: impl IntoIterator<Item = FactWithheld>) -> Vec<FactWithheld> {
    paths
        .into_iter()
        .flat_map(|allowed| {
            let denied = FactWithheld {
                fact: Fact::ExecDeniedPaths,
                ..allowed.clone()
            };
            [allowed, denied]
        })
        .collect()
}

/// D27: a path of the open on a file-system type outside the list withholds the facts. The
/// open's decision is "withheld", not an error, and each entry names the cause, the address of
/// the configuration field, the path and the type, once per fact the helper would prove.
#[test]
fn an_unlisted_file_system_at_the_open_withholds_the_facts() {
    let base = || sightings(seen("/opt/substrate/substrate-exec-gate", 31));
    for file_system in ["xfs", "btrfs", "nfs4", "fuse.sshfs"] {
        let table = tree(&HOST.replace("xfs", file_system));
        // The workspace root, the helper's own file, a declared writable root, a base
        // directory and the probes' temporary directory, each with its address.
        let mut workspace_root = base();
        workspace_root.workspace_root = seen("/data/workspaces", 62);
        let mut helper = base();
        helper.gate = seen("/data/substrate-exec-gate", 62);
        let mut writable_root = base();
        writable_root.writable_roots = vec![seen("/data/out", 62)];
        let mut base_directory = base();
        base_directory.base = vec![seen("/data/usr", 62)];
        let mut temporary = base();
        temporary.temporary = seen("/data/tmp", 62);
        for (sightings, address, path) in [
            (workspace_root, "config.workspace_root", "/data/workspaces"),
            (helper, "config.exec_gate", "/data/substrate-exec-gate"),
            (writable_root, "config.writable_roots", "/data/out"),
            (base_directory, "config.exec_gate", "/data/usr"),
            (temporary, "config.exec_gate", "/data/tmp"),
        ] {
            let unplaced = withheld(place_open(&sightings, &table));
            assert_eq!(
                unplaced,
                [OpenUnplaced {
                    address,
                    why: Unestablished {
                        path: path.to_owned(),
                        cause: Cause::FileSystemUnlisted(file_system.to_owned()),
                        asked: substrate_exec_gate::place::Asked::Place,
                    },
                }],
                "{file_system} at {address}"
            );
            // One entry per fact the helper would prove: the two list facts.
            assert_eq!(
                withheld_facts(&unplaced),
                per_fact([entry(address, path, Some(file_system))]),
                "{file_system} at {address}"
            );
        }
    }
}

/// D27: a kernel that reports no mount ID withholds too, with that cause and no type.
#[test]
fn an_unreported_mount_id_at_the_open_withholds_the_facts() {
    let mut unreported = sightings(seen("/opt/substrate/substrate-exec-gate", 31));
    unreported.workspace_root.mount_id = None;
    let unplaced = withheld(place_open(&unreported, &host()));
    assert_eq!(
        withheld_facts(&unplaced),
        per_fact([entry("config.workspace_root", "/work/u/workspaces", None)])
    );
    assert_eq!(unplaced[0].why.cause, Cause::MountIdUnreported);
}

/// D27: every path is placed before the answer. Two paths with a withholding cause are both
/// recorded, in the open's order; one refusing cause on any path refuses the open, wherever it
/// stands among the withholding ones.
#[test]
fn every_path_of_the_open_is_placed_before_the_answer() {
    let table = host();
    let mut two = sightings(seen("/data/substrate-exec-gate", 62));
    two.temporary = seen("/data/tmp", 62);
    assert_eq!(
        withheld_facts(&withheld(place_open(&two, &table))),
        per_fact([
            entry("config.exec_gate", "/data/substrate-exec-gate", Some("xfs")),
            entry("config.exec_gate", "/data/tmp", Some("xfs")),
        ])
    );
    // The refusing cause is on the last path the open places, after a withholding one.
    let mut mixed = sightings(seen("/data/substrate-exec-gate", 62));
    mixed.temporary = seen("/tmp", 999);
    match place_open(&mixed, &table) {
        OpenPlacement::Refused(unplaced) => {
            assert_eq!(unplaced.address, "config.exec_gate");
            assert_eq!(unplaced.why.cause, Cause::MountRowMissing);
            assert_eq!(unplaced.why.path, "/tmp");
        }
        OpenPlacement::Placed(_) | OpenPlacement::Withheld(_) => {
            panic!("a refusing cause beside a withholding one did not refuse the open")
        }
    }
}

/// D26 and D27: each of the seven other causes refuses the open `config.exec-gate-unserved`,
/// 501, with the address of whose path it was.
#[test]
fn each_refusing_cause_refuses_the_open() {
    let deleted = "\
31 1 259:2 / / rw - ext4 /dev/a rw
63 31 259:2 /gone//deleted /work/u/workspaces rw - ext4 /dev/a rw
61 31 0:40 / /tmp rw - tmpfs tmpfs rw
";
    let repeated = "\
31 1 259:2 / / rw - ext4 /dev/a rw
63 31 259:2 /a /work/u/workspaces rw - ext4 /dev/a rw
63 31 259:2 /b /work/u/workspaces rw - ext4 /dev/a rw
61 31 0:40 / /tmp rw - tmpfs tmpfs rw
";
    let base = || sightings(seen("/opt/substrate/substrate-exec-gate", 31));
    let on = |mount_id: u64| {
        let mut sightings = base();
        sightings.workspace_root = seen("/work/u/workspaces", mount_id);
        sightings
    };
    let mut unread = base();
    unread.workspace_root.descriptor_path = None;
    for (table, sightings, cause) in [
        (deleted, on(63), Cause::MountRootDeleted),
        (repeated, on(63), Cause::MountRowRepeated),
        (HOST, on(999), Cause::MountRowMissing),
        (HOST, on(61), Cause::MountPointMismatch),
        (HOST, unread, Cause::MountPointMismatch),
    ] {
        let table = tree(table);
        let OpenPlacement::Refused(unplaced) = place_open(&sightings, &table) else {
            panic!("{cause} did not refuse the open");
        };
        assert_eq!(unplaced.address, "config.workspace_root", "{cause}");
        assert_eq!(unplaced.why.cause, cause);
        let error = unplaced.refusal(gate());
        assert_eq!(error.code, EXEC_GATE_UNSERVED, "{error}");
        assert_eq!(error.class, DriverErrorClass::Unserved, "{error}");
        assert_eq!(error.address.as_deref(), Some("config.workspace_root"));
        assert!(error.message.contains(cause.name()), "{error}");
    }
    // The three causes met before a place is asked refuse through the same refusal.
    for cause in [
        Cause::MountTableUnreadable,
        Cause::MountTableUnparsable,
        Cause::PathUnopenable,
    ] {
        let unplaced = OpenUnplaced {
            address: "config.exec_gate",
            why: Unestablished {
                path: "/opt/substrate/substrate-exec-gate".to_owned(),
                cause: cause.clone(),
                asked: substrate_exec_gate::place::Asked::Place,
            },
        };
        assert!(!unplaced.withholds(), "{cause}");
        assert_eq!(unplaced.refusal(gate()).code, EXEC_GATE_UNSERVED);
    }
}

/// D27, the order of the open: the file's own checks, then the places, then the trust check.
/// A withheld open never reaches the trust check, because the helper is never used: a helper
/// with two links on a table that withholds is "withheld", not `config.exec-gate-invalid`.
#[test]
fn a_withheld_open_never_reaches_the_trust_check() {
    let table = host();
    let two_links = FileFacts {
        regular: true,
        bytes: 1 << 20,
        links: 2,
    };
    let mut on_xfs = sightings(seen("/opt/substrate/substrate-exec-gate", 31));
    on_xfs.workspace_root = seen("/data/workspaces", 62);
    match decide_open(gate(), two_links, place_open(&on_xfs, &table))
        .expect("withheld, not refused")
    {
        OpenDecision::Withheld(entries) => assert_eq!(
            entries,
            per_fact([entry(
                "config.workspace_root",
                "/data/workspaces",
                Some("xfs")
            )])
        ),
        OpenDecision::Use => panic!("a helper on a host that withholds is used"),
    }
    // Where every place is established, the same helper is refused.
    let placed = sightings(seen("/opt/substrate/substrate-exec-gate", 31));
    assert_invalid(
        &decide_open(gate(), two_links, place_open(&placed, &table)).expect_err("two links"),
        "has 2 links",
    );
    // And a helper inside a workspace, with one link.
    let one_link = FileFacts {
        links: 1,
        ..two_links
    };
    let inside = sightings(seen("/work/u/workspaces/ws_1/gate", 31));
    assert_invalid(
        &decide_open(gate(), one_link, place_open(&inside, &table)).expect_err("in a workspace"),
        "which a run can write",
    );
    assert!(matches!(
        decide_open(gate(), one_link, place_open(&placed, &table)),
        Ok(OpenDecision::Use)
    ));
    // The file's own checks come first: a directory is unserved whatever the table says.
    let directory = FileFacts {
        regular: false,
        ..one_link
    };
    assert_unserved(
        &decide_open(gate(), directory, place_open(&on_xfs, &table)).expect_err("a directory"),
        "is not a regular file",
    );
}

/// D26 at a start: a declared root whose row has the type `xfs` has no class. The start is
/// `exec.sandbox-place-unserved`, 501, with the declaring field as its address, and the mount
/// is never classed `follow` for want of an answer.
#[test]
fn a_mount_whose_place_cannot_be_established_refuses_the_start() {
    let error = classed(
        "read_only_roots",
        &seen("/data/tools", 62),
        &writable(),
        &host(),
    )
    .expect_err("a root on a file system outside the list");
    assert_eq!(error.code, PLACE_UNSERVED, "{error}");
    assert_eq!(error.code, "exec.sandbox-place-unserved");
    assert_eq!(error.class, DriverErrorClass::Unserved, "{error}");
    assert_eq!(error.address.as_deref(), Some("read_only_roots"));
    for holds in [
        "cannot establish the place of /data/tools: file-system-unlisted xfs",
        "ext4, overlay, tmpfs",
    ] {
        assert!(error.message.contains(holds), "{error}");
    }
    // Each other cause refuses the same way, and names itself.
    let mut unreported = seen("/opt/tools", 31);
    unreported.mount_id = None;
    let mut unread = seen("/opt/tools", 31);
    unread.descriptor_path = None;
    for (source, cause) in [
        (unreported, "mount-id-unreported"),
        (seen("/opt/tools", 999), "mount-row-missing"),
        (unread, "mount-point-mismatch"),
        (seen("/opt/tools", 61), "mount-point-mismatch"),
    ] {
        let error = classed("read_only_files", &source, &writable(), &host())
            .expect_err("an unestablished place");
        assert_eq!(error.code, PLACE_UNSERVED, "{error}");
        assert_eq!(error.address.as_deref(), Some("read_only_files"));
        assert!(error.message.contains(cause), "{error}");
    }
    let why = Unestablished {
        path: "/opt/tool".to_owned(),
        cause: Cause::MountTableUnparsable,
        asked: substrate_exec_gate::place::Asked::Place,
    };
    let error = start_unplaced("argv", &why);
    assert_eq!(error.code, PLACE_UNSERVED);
    assert_eq!(error.address.as_deref(), Some("argv"));
    assert!(
        error
            .message
            .contains("cannot establish the place of /opt/tool: mount-table-unparsable"),
        "{error}"
    );
}

/// D19 and D21 on metadata: what the file is, how large, and how many names it has.
#[test]
fn the_file_checks_decide_on_metadata() {
    check_file(gate(), true, 1 << 20).expect("a regular file within the bound");
    assert_unserved(
        &check_file(gate(), false, 0).expect_err("a directory"),
        "is not a regular file",
    );
    assert_unserved(
        &check_file(gate(), true, crate::probe::MAX_BACKEND_BINARY_BYTES + 1)
            .expect_err("over the bound"),
        "over the bound",
    );
    check_links(gate(), 1).expect("one link");
    // K4: a second name may lie in a workspace, and no path check sees it.
    assert_invalid(
        &check_links(gate(), 2).expect_err("a second hard link"),
        "has 2 links",
    );
}

/// D19: the protocol line, exactly, from a helper that exited 0.
#[test]
fn the_protocol_check_decides_on_the_line() {
    check_protocol(gate(), true, b"substrate-exec-gate/1\n").expect("the protocol line");
    for (succeeded, stdout) in [
        (true, &b"substrate-exec-gate/2\n"[..]),
        (true, &b"substrate-exec-gate/1"[..]),
        (true, &b""[..]),
        (true, &b"hello\nsubstrate-exec-gate/1\n"[..]),
        (false, &b"substrate-exec-gate/1\n"[..]),
    ] {
        let error = check_protocol(gate(), succeeded, stdout).expect_err("another answer");
        assert_unserved(&error, "does not answer --version with the protocol line");
        assert!(
            error.message.contains("/opt/substrate/substrate-exec-gate"),
            "{error}"
        );
    }
}

/// D24: the class of a mount comes from its host source, by place and in both directions.
#[test]
fn the_class_of_a_mount_follows_its_host_source() {
    let table = host();
    let class = |path: &str, mount_id: u64, writable: &[Location]| {
        classed("read_only_roots", &seen(path, mount_id), writable, &table)
            .unwrap_or_else(|error| panic!("{path}: {error}"))
    };
    // Beside every writable location.
    assert_eq!(class("/usr", 31, &writable()), Class::Follow);
    assert_eq!(
        class("/opt/toolchains/stable", 31, &writable()),
        Class::Follow
    );
    // On another device, a listed type.
    assert_eq!(class("/tmp/tools", 61, &writable()), Class::Follow);
    // Inside the workspace root: writable as a whole (K10).
    assert_eq!(
        class("/work/u/workspaces/ws_1/tools", 31, &writable()),
        Class::Beneath
    );
    // Holds the workspace root: the mount shows it (K9).
    assert_eq!(class("/work/u", 31, &writable()), Class::Beneath);
    assert_eq!(class("/", 31, &writable()), Class::Beneath);
    // Inside it under the host's second mount, where the text is beside it.
    assert_eq!(
        class("/mnt/second/ws_1/tools", 60, &writable()),
        Class::Beneath
    );
    // No writable location: nothing to hold or lie in.
    assert_eq!(class("/work/u", 31, &[]), Class::Follow);
}

/// D20: the bytes are sealed against writing, resizing and any change of the seals, the hash is
/// of the sealed bytes, and every reopen reads them from their start.
#[test]
fn the_sealed_bytes_cannot_change_and_every_reopen_reads_them_whole() {
    use std::io::Read as _;
    let bytes = b"\x7fELF not really".repeat(100);
    let gate = ExecGate::seal(&bytes, Some("/lib64/ld.so".to_owned())).expect("sealed");
    let seals = gate.seals();
    for seal in [
        libc::F_SEAL_WRITE,
        libc::F_SEAL_GROW,
        libc::F_SEAL_SHRINK,
        libc::F_SEAL_SEAL,
    ] {
        assert_eq!(seals & seal, seal, "seal {seal:#x} of {seals:#x}");
    }
    assert_eq!(gate.sha256(), hex::encode(Sha256::digest(&bytes)));
    assert_eq!(gate.loader(), Some("/lib64/ld.so"));
    // K3: a second start reads the bytes again, because its descriptor is its own.
    for _ in 0..2 {
        let mut read = Vec::new();
        std::fs::File::from(gate.reopen().expect("reopened"))
            .read_to_end(&mut read)
            .expect("read");
        assert_eq!(read, bytes);
    }
    // The write seal refuses the write itself, whichever descriptor carries it: a descriptor
    // opened for writing through `/proc` exists, and nothing it is given reaches the bytes.
    let descriptor = gate.reopen().expect("reopened");
    let link = format!(
        "/proc/self/fd/{}",
        std::os::fd::AsRawFd::as_raw_fd(&descriptor)
    );
    if let Ok(mut writing) = std::fs::OpenOptions::new().write(true).open(&link) {
        use std::io::Write as _;
        let refused = writing.write(b"changed").expect_err("a sealed write");
        assert_eq!(refused.raw_os_error(), Some(libc::EPERM), "{refused}");
    }
    let mut read = Vec::new();
    std::fs::File::from(gate.reopen().expect("reopened"))
        .read_to_end(&mut read)
        .expect("read");
    assert_eq!(read, bytes, "the sealed bytes changed");
}

/// A temporary directory, as the repository's other cases stage their workspace roots.
fn staged() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("exec-gate-")
        .tempdir()
        .expect("a temporary directory")
}

/// Whether the place of `directory` and of every base directory can be established here;
/// prints why a case is absent when it cannot.
fn placed(case: &str, directory: &Path) -> bool {
    let place = |table: &MountTable, path: &str| {
        table
            .place_of(&Opened::of_path(path).map_err(|why| why.to_string())?)
            .map(drop)
            .map_err(|why| why.to_string())
    };
    let established = MountTable::read()
        .map_err(|cause| cause.to_string())
        .and_then(|table| {
            place(&table, directory.to_str().expect("a UTF-8 path"))?;
            ["/usr", "/bin", "/lib", "/lib64"]
                .iter()
                .filter(|path| Path::new(path).exists())
                .try_for_each(|path| place(&table, path))
        });
    match established {
        Ok(()) => true,
        Err(why) => {
            println!("exec gate {case}: ABSENT ({why})");
            false
        }
    }
}

fn config(root: &Path, gate: &Path) -> HostConfig {
    let mut config = HostConfig::minimum(root.join("workspaces"));
    config.exec_gate = Some(gate.to_owned());
    config
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory");
    std::fs::write(path, bytes).expect("the file");
}

/// D19: no helper configured is no gate, and the open asks nothing.
#[test]
fn an_unset_gate_opens_as_none() {
    let config = HostConfig::minimum("/nonexistent/workspaces");
    assert!(matches!(open(&config).expect("opens"), Opening::Unset));
}

/// D21: a relative path is refused before anything is opened.
#[test]
fn a_relative_gate_is_invalid() {
    let config = config(
        Path::new("/nonexistent"),
        Path::new("bin/substrate-exec-gate"),
    );
    assert_invalid(
        &open(&config).expect_err("a relative path"),
        "is not an absolute path",
    );
}

/// D19: a set helper that cannot be used refuses the open and names the path.
#[test]
fn a_gate_that_cannot_be_used_is_unserved() {
    let root = staged();
    if !placed("unusable helper", root.path()) {
        return;
    }
    std::fs::create_dir_all(root.path().join("workspaces")).expect("the workspace root");
    let missing = root.path().join("install/missing");
    let error = open(&config(root.path(), &missing)).expect_err("a missing file");
    assert_unserved(&error, "cannot be opened");
    assert!(error.message.contains(missing.to_str().unwrap()), "{error}");

    let directory = root.path().join("install/directory");
    std::fs::create_dir_all(&directory).expect("a directory");
    let error = open(&config(root.path(), &directory)).expect_err("a directory");
    assert_unserved(&error, "is not a regular file");
    assert!(
        error.message.contains(directory.to_str().unwrap()),
        "{error}"
    );

    let unreadable = root.path().join("install/unreadable");
    write(&unreadable, b"gate");
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000))
        .expect("mode 000");
    if std::fs::File::open(&unreadable).is_ok() {
        println!("exec gate unreadable helper: ABSENT (this uid reads a file of mode 000)");
    } else {
        let error = open(&config(root.path(), &unreadable)).expect_err("an unreadable file");
        assert_unserved(&error, "cannot be opened");
        assert!(
            error.message.contains(unreadable.to_str().unwrap()),
            "{error}"
        );
    }
    println!("exec gate unusable helper: executed");
}

/// D21 on real files: each open is `config.exec-gate-invalid`, and a helper outside every
/// writable location with one link opens.
#[test]
fn a_gate_a_run_could_have_written_is_invalid() {
    let root = staged();
    if !placed("helper a run could have written", root.path()) {
        return;
    }
    let workspaces = root.path().join("workspaces");
    let declared = root.path().join("declared");
    std::fs::create_dir_all(&workspaces).expect("the workspace root");
    std::fs::create_dir_all(&declared).expect("a writable root");
    let with_root = |gate: &Path| {
        let mut config = config(root.path(), gate);
        config.writable_roots = vec![crate::DeclaredWritableRoot {
            name: "out".to_owned(),
            path: declared.clone(),
        }];
        config
    };

    // In a workspace.
    let inside = workspaces.join("ws_1/bin/gate");
    write(&inside, b"gate");
    assert_invalid(
        &open(&with_root(&inside)).expect_err("a helper in a workspace"),
        "which a run can write",
    );
    // Beneath a declared writable root.
    let beneath = declared.join("gate");
    write(&beneath, b"gate");
    assert_invalid(
        &open(&with_root(&beneath)).expect_err("a helper beneath a writable root"),
        "which a run can write",
    );
    // A symbolic link outside both that points into a workspace.
    let link = root.path().join("install/link");
    std::fs::create_dir_all(link.parent().unwrap()).expect("install");
    std::os::unix::fs::symlink(&inside, &link).expect("a link");
    assert_invalid(
        &open(&with_root(&link)).expect_err("a link into a workspace"),
        "which a run can write",
    );
    // The workspace root configured through a symbolic link, the helper by its resolved path.
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&workspaces, &alias).expect("a link to the workspace root");
    let mut through_link = with_root(&inside);
    through_link.workspace_root = alias;
    assert_invalid(
        &open(&through_link).expect_err("a workspace root named through a link"),
        "which a run can write",
    );
    // K4: outside every workspace, with a second hard link inside one.
    let outside = root.path().join("install/gate");
    let image = elf_image(ET_EXEC, &[PT_LOAD, PT_LOAD]);
    write(&outside, &image);
    let second = workspaces.join("ws_1/second-name");
    std::fs::hard_link(&outside, &second).expect("a second link");
    assert_invalid(
        &open(&with_root(&outside)).expect_err("a helper with two links"),
        "has 2 links",
    );
    // With that name gone the same file opens, and what is held is its bytes.
    std::fs::remove_file(&second).expect("the second link removed");
    let Opening::Gate(gate) =
        open(&with_root(&outside)).expect("a helper outside every writable location")
    else {
        panic!("the helper is not used on a host where every place is established");
    };
    assert_eq!(gate.sha256(), hex::encode(Sha256::digest(&image)));
    let env = std::fs::read("/usr/bin/env").expect("the host's env");
    assert_eq!(
        gate.loader().map(str::to_owned),
        substrate_exec_gate::elf_interpreter(&env),
        "the loader is the host's own, read from /usr/bin/env (D7)"
    );
    println!("exec gate helper a run could have written: executed");
}

/// D27 on a real host, with no privilege: a declared writable root at `/proc/sys` lies on the
/// type `proc`, which is outside the list. The open answers "withheld" and names the cause, the
/// field, the path and the type; the helper's bytes are never read, so a helper with two links
/// is not refused here, and one that is a directory still is.
#[test]
fn an_open_with_a_path_on_an_unlisted_file_system_withholds_on_a_real_host() {
    let root = staged();
    if !placed("withheld open", root.path()) {
        return;
    }
    std::fs::create_dir_all(root.path().join("workspaces")).expect("the workspace root");
    let on_proc = |gate: &Path| {
        let mut config = config(root.path(), gate);
        config.writable_roots = vec![crate::DeclaredWritableRoot {
            name: "kernel".to_owned(),
            path: PathBuf::from("/proc/sys"),
        }];
        config
    };
    let expected = [FactWithheld {
        fact: Fact::ExecSubpaths,
        cause: "file-system-unlisted",
        address: "config.writable_roots",
        path: "/proc/sys".to_owned(),
        file_system: Some("proc".to_owned()),
    }];
    let helper = root.path().join("install/gate");
    write(&helper, b"gate");
    for links in [1, 2] {
        if links == 2 {
            std::fs::hard_link(&helper, root.path().join("install/second-name"))
                .expect("a second link");
        }
        match open(&on_proc(&helper)).expect("the adapter opens") {
            Opening::Withheld(entries) => {
                assert_eq!(entries, per_fact(expected.clone()), "{links} links");
            }
            Opening::Unset | Opening::Gate(_) => {
                panic!("a writable root on proc did not withhold the facts ({links} links)")
            }
        }
    }
    // The file's own checks still come first.
    let directory = root.path().join("install/directory");
    std::fs::create_dir_all(&directory).expect("a directory");
    assert_unserved(
        &open(&on_proc(&directory)).expect_err("a directory"),
        "is not a regular file",
    );
    assert_unserved(
        &open(&on_proc(&root.path().join("install/missing"))).expect_err("a missing file"),
        "cannot be opened",
    );
    println!("exec gate withheld open: executed");
}

/// One start's view on real directories: a workspace in the workspace root, and read-only
/// roots the case declares.
struct Staged {
    root: tempfile::TempDir,
    workspace: PathBuf,
}

impl Staged {
    fn new() -> Self {
        let root = staged();
        let workspace = root.path().join("workspaces/ws_1");
        std::fs::create_dir_all(&workspace).expect("a workspace");
        Self { root, workspace }
    }

    fn plan(
        &self,
        roots: &[(&Path, &str)],
        entries: &[&str],
        program: &str,
    ) -> Result<Lists, DriverError> {
        self.plan_with_loader(roots, entries, program, None)
    }

    /// Asks `ask` of this start's mounts, built on the host's own table and then given a
    /// table in which **the mount the staged directories lie on has the type `ramfs`**: what
    /// is placed once the view stands (an entry's end, a link and a directory of a walk) has
    /// no place there. So a case holds that the question is asked at all, with no mount of
    /// its own and no privilege.
    fn on_an_unlisted_type<T>(
        &self,
        roots: &[(&Path, &str)],
        ask: impl FnOnce(&Mounts<'_>) -> T,
    ) -> T {
        self.asked(roots, |mounts| mounts.tree = self.retyped(), ask)
    }

    /// Asks `ask` of this start's mounts, built on the host's own table and then changed by
    /// `change`: what a walk is judged against once the view stands.
    fn asked<T>(
        &self,
        roots: &[(&Path, &str)],
        change: impl FnOnce(&mut Mounts<'_>),
        ask: impl FnOnce(&Mounts<'_>) -> T,
    ) -> T {
        let workspace = OwnedFd::from(std::fs::File::open(&self.workspace).expect("workspace"));
        let declared: Vec<ReadOnlyRoot> = roots
            .iter()
            .map(|(host, mount)| ReadOnlyRoot {
                host_path: host.to_str().expect("UTF-8").to_owned(),
                mount: (*mount).to_owned(),
            })
            .collect();
        let pinned = crate::read_only_roots::pin(&declared).expect("pinned roots");
        let view = View {
            workspace: &workspace,
            read_only_roots: &pinned,
            read_only_files: &[],
            writable_roots: &[],
            scratch: false,
        };
        let config = HostConfig::minimum(self.root.path().join("workspaces"));
        let mut mounts = Mounts::of(&view, &config).expect("the view on the host's table");
        change(&mut mounts);
        ask(&mounts)
    }

    /// This process's mount table, with the type of the mount the staged directories lie on
    /// written as `ramfs`.
    fn retyped(&self) -> MountTree {
        let staged = Opened::of_path(self.root.path().to_str().expect("UTF-8"))
            .expect("the staged directory opens")
            .mount_id
            .expect("the kernel reports its mount")
            .to_string();
        let mut retyped = String::new();
        for row in std::fs::read_to_string("/proc/self/mountinfo")
            .expect("the mount table")
            .lines()
        {
            if row.split(' ').next() == Some(staged.as_str()) {
                let (shared, own) = row.split_once(" - ").expect("the separator of a row");
                let (_, own) = own.split_once(' ').expect("a file-system type");
                retyped.push_str(shared);
                retyped.push_str(" - ramfs ");
                retyped.push_str(own);
                retyped.push('\n');
            } else {
                retyped.push_str(row);
                retyped.push('\n');
            }
        }
        tree(&retyped)
    }

    /// [`Self::plan`], for a helper that read `loader` as the host's ELF loader at open.
    fn plan_with_loader(
        &self,
        roots: &[(&Path, &str)],
        entries: &[&str],
        program: &str,
        loader: Option<&str>,
    ) -> Result<Lists, DriverError> {
        self.plan_all(roots, entries, &[], program, loader)
    }

    /// [`Self::plan`], with `denied` as the deny-list.
    fn plan_denying(
        &self,
        roots: &[(&Path, &str)],
        entries: &[&str],
        denied: &[&str],
        program: &str,
    ) -> Result<Lists, DriverError> {
        self.plan_all(roots, entries, denied, program, None)
    }

    fn plan_all(
        &self,
        roots: &[(&Path, &str)],
        entries: &[&str],
        denied: &[&str],
        program: &str,
        loader: Option<&str>,
    ) -> Result<Lists, DriverError> {
        let denied: Vec<String> = denied.iter().map(|entry| (*entry).to_owned()).collect();
        let workspace = OwnedFd::from(std::fs::File::open(&self.workspace).expect("workspace"));
        let declared: Vec<ReadOnlyRoot> = roots
            .iter()
            .map(|(host, mount)| ReadOnlyRoot {
                host_path: host.to_str().expect("UTF-8").to_owned(),
                mount: (*mount).to_owned(),
            })
            .collect();
        let pinned = crate::read_only_roots::pin(&declared).expect("pinned roots");
        let entries: Vec<String> = entries.iter().map(|entry| (*entry).to_owned()).collect();
        plan(
            &ExecGate::seal(b"gate", loader.map(str::to_owned)).expect("sealed"),
            &entries,
            &denied,
            program,
            &View {
                workspace: &workspace,
                read_only_roots: &pinned,
                read_only_files: &[],
                writable_roots: &[],
                scratch: false,
            },
            &HostConfig::minimum(self.root.path().join("workspaces")),
        )
    }
}

fn assert_refused(planned: Result<Lists, DriverError>, code: &str, address: &str, holds: &str) {
    let error = planned.expect_err("a refused list");
    assert_eq!(error.code, code, "{error}");
    assert_eq!(error.class, DriverErrorClass::Refused, "{error}");
    assert_eq!(error.address.as_deref(), Some(address), "{error}");
    assert!(error.message.contains(holds), "{error}");
}

/// D24: each path is located in a mount and classed, and the helper reads back what the driver
/// wrote: one `--allow` per entry, the program, and the mounts the run can write.
#[test]
fn a_list_becomes_the_arguments_the_helper_parses() {
    let staged = Staged::new();
    if !placed("planned list", staged.root.path()) {
        return;
    }
    std::fs::create_dir_all(staged.workspace.join("bin")).expect("bin");
    let tools = staged.root.path().join("tools");
    std::fs::create_dir_all(tools.join("real")).expect("a tool directory");
    std::os::unix::fs::symlink("real", tools.join("link")).expect("a link within the root");
    let lists = staged
        .plan(
            &[(&tools, "/opt/t")],
            &["bin", "/workspace/bin", "/opt/t/link", "/opt/t", "/usr/bin"],
            "/usr/bin/env",
        )
        .expect("a list the view carries");
    let located: Vec<(Class, &str, &str)> = lists
        .allow
        .iter()
        .map(|entry| (entry.class(), entry.root(), entry.rel()))
        .collect();
    assert_eq!(
        located,
        [
            (Class::Beneath, "/workspace", "bin"),
            (Class::Beneath, "/workspace", "bin"),
            (Class::Follow, "/opt/t", "link"),
            (Class::Follow, "/opt/t", "."),
            (Class::Follow, "/usr", "bin"),
        ]
    );
    assert_eq!(lists.program.path(), "/usr/bin/env");
    assert_eq!(lists.program.class(), Class::Follow);
    // The workspace first, then the view's own file systems; no read-only mount beside them.
    assert_eq!(lists.writable, ["/workspace", "/tmp", "/dev"]);
    let mut arguments: Vec<std::ffi::OsString> =
        lists.options().into_iter().map(Into::into).collect();
    arguments.push("/usr/bin/env".into());
    arguments.push("true".into());
    match substrate_exec_gate::parse(arguments).expect("the helper parses the options") {
        Invocation::Run {
            lists: parsed,
            environment,
            command,
        } => {
            assert_eq!(parsed, lists);
            assert!(environment.is_empty(), "the lists alone carry no variable");
            assert_eq!(command, ["/usr/bin/env", "true"]);
        }
        other @ Invocation::Version => panic!("parsed as {other:?}"),
    }
    println!("exec gate planned list: executed");
}

/// Design 35 § 5: what the view cannot carry is refused by name before anything is dispatched.
#[test]
fn an_entry_the_view_cannot_carry_is_refused_by_name() {
    let staged = Staged::new();
    if !placed("refused entries", staged.root.path()) {
        return;
    }
    std::fs::create_dir_all(staged.workspace.join("bin")).expect("bin");
    std::os::unix::fs::symlink("/usr/bin", staged.workspace.join("planted")).expect("a link");
    let plan = |entries: &[&str], program: &str| staged.plan(&[], entries, program);
    assert_refused(
        plan(&["absent"], "/usr/bin/env"),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "does not exist at the start",
    );
    assert_refused(
        plan(&["/usr/bin/no-such-program"], "/usr/bin/env"),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "does not exist at the start",
    );
    // A link in the workspace: an earlier run could have left it.
    assert_refused(
        plan(&["planted"], "/usr/bin/env"),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "has a symbolic link in it",
    );
    for outside in [
        "/tmp/tools",
        "/proc/self",
        "/dev/shm",
        "/etc/alternatives",
        "/opt/none",
    ] {
        assert_refused(
            plan(&[outside], "/usr/bin/env"),
            EXEC_SUBPATH_UNDECLARED,
            "exec_subpaths",
            "lies outside",
        );
    }
    assert_refused(
        plan(&["bin/../bin"], "/usr/bin/env"),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "",
    );
    // The program is an absolute view path when the list is not empty.
    assert_refused(
        plan(&["bin"], "env"),
        PATH_UNREPRESENTABLE,
        "argv",
        "is not an absolute path",
    );
    assert_refused(
        plan(&["bin"], "/tmp/program"),
        PATH_UNREPRESENTABLE,
        "argv",
        "lies in no mount",
    );
    assert_refused(
        plan(&["bin"], "/workspace/planted/env"),
        PATH_UNREPRESENTABLE,
        "argv",
        "has a symbolic link in it",
    );
    println!("exec gate refused entries: executed");
}

/// D24, the class comes from the host source: a read-only root whose host directory lies in the
/// workspace root is opened `beneath`, so a link a run planted in it is refused and the helper
/// is told the run can write the mount (K10).
#[test]
fn a_read_only_root_in_the_workspace_root_is_classed_beneath() {
    let staged = Staged::new();
    if !placed("root in the workspace root", staged.root.path()) {
        return;
    }
    let tools = staged.root.path().join("workspaces/ws_2/tools");
    std::fs::create_dir_all(tools.join("real")).expect("a tool directory");
    std::os::unix::fs::symlink("/usr/bin", tools.join("link")).expect("a planted link");
    assert_refused(
        staged.plan(&[(&tools, "/opt/k")], &["/opt/k/link"], "/usr/bin/env"),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "has a symbolic link in it",
    );
    let lists = staged
        .plan(&[(&tools, "/opt/k")], &["/opt/k/real"], "/usr/bin/env")
        .expect("a path of the root without a link");
    assert_eq!(lists.allow[0].class(), Class::Beneath);
    assert_eq!(lists.writable, ["/workspace", "/opt/k", "/tmp", "/dev"]);
    println!("exec gate root in the workspace root: executed");
}

/// D24, a followed path into a writable mount: a link in a read-only root outside every
/// writable location names a file the run can replace when it ends in `/workspace`, or passes
/// through a link there (K9).
#[test]
fn a_followed_entry_that_resolves_into_a_writable_mount_is_refused() {
    let staged = Staged::new();
    if !placed("followed entry into a writable mount", staged.root.path()) {
        return;
    }
    std::fs::create_dir_all(staged.workspace.join("tools/bin")).expect("tools");
    std::os::unix::fs::symlink("/usr/bin", staged.workspace.join("forward")).expect("a link");
    let tools = staged.root.path().join("tools");
    std::fs::create_dir_all(&tools).expect("a root");
    std::os::unix::fs::symlink("/workspace/tools/bin", tools.join("link")).expect("a link");
    std::os::unix::fs::symlink("/tmp/built", tools.join("temporary")).expect("a link");
    std::os::unix::fs::symlink("/workspace/forward", tools.join("hop")).expect("a link");
    std::os::unix::fs::symlink("/proc/self/exe", tools.join("magic")).expect("a link");
    std::os::unix::fs::symlink("/usr/bin", tools.join("system")).expect("a link");
    std::os::unix::fs::symlink("/etc/passwd", tools.join("unbound")).expect("a link");
    let plan = |entry: &str| staged.plan(&[(&tools, "/opt/ro")], &[entry], "/usr/bin/env");
    assert_refused(
        plan("/opt/ro/link"),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "resolves to /workspace/tools/bin, in the writable mount /workspace",
    );
    assert_refused(
        plan("/opt/ro/temporary"),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "in the writable mount /tmp",
    );
    // Ends in `/usr/bin`, through a link the run can replace.
    assert_refused(
        plan("/opt/ro/hop"),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "resolves through a symbolic link in the writable mount /workspace",
    );
    assert_refused(
        plan("/opt/ro/magic"),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "in /proc",
    );
    // An absolute target is the view's, not the host's: the view has no `/etc/passwd`.
    assert_refused(
        plan("/opt/ro/unbound"),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "which the view does not have",
    );
    // A link that stays in mounts the run cannot write is followed.
    let lists = plan("/opt/ro/system").expect("a link into the base system");
    assert_eq!(lists.allow[0].class(), Class::Follow);
    println!("exec gate followed entry into a writable mount: executed");
}

/// D24 and D30, **a followed path that passes through a directory a run can write is
/// refused**, although no link lies there and the path ends where no run can write: the run
/// can rename what the directory holds while the helper resolves the same path. The mount is
/// named. A path that ends in a writable mount keeps the answer it had, and `..` in a target
/// that never visits a writable place is resolved from the text.
#[test]
fn a_followed_path_through_a_writable_mount_is_refused() {
    let staged = Staged::new();
    if !placed("followed path through a writable mount", staged.root.path()) {
        return;
    }
    std::fs::create_dir_all(staged.workspace.join("tools/bin")).expect("tools");
    let shown = staged.root.path().join("workspaces/ws_2/shown");
    std::fs::create_dir_all(shown.join("real")).expect("a directory in the workspace root");
    let tools = staged.root.path().join("tools");
    std::fs::create_dir_all(tools.join("dir")).expect("a root");
    for (name, target) in [
        ("around", "/workspace/../usr/bin"),
        ("deeper", "/workspace/tools/bin/../../../usr/bin"),
        ("beneath", "/opt/k/real/../../../usr/bin"),
        ("ends", "/usr/../workspace/tools/bin"),
        ("system", "/usr/bin"),
        ("back", "dir/../system"),
    ] {
        std::os::unix::fs::symlink(target, tools.join(name)).expect("a link");
    }
    let plan = |entry: &str| {
        staged.plan(
            &[(&tools, "/opt/ro"), (&shown, "/opt/k")],
            &[entry],
            "/usr/bin/env",
        )
    };
    for (entry, holds) in [
        (
            "/opt/ro/around",
            "passes through /workspace, in the writable mount /workspace",
        ),
        (
            "/opt/ro/deeper",
            "passes through /workspace, in the writable mount /workspace",
        ),
        // A read-only root classed `beneath` is a mount the run can write.
        (
            "/opt/ro/beneath",
            "passes through /opt/k, in the writable mount /opt/k",
        ),
        // The end is judged first.
        (
            "/opt/ro/ends",
            "resolves to /workspace/tools/bin, in the writable mount /workspace",
        ),
    ] {
        assert_refused(plan(entry), PATH_UNREPRESENTABLE, "exec_subpaths", holds);
    }
    // The host's loader is a followed path too.
    assert_refused(
        staged.plan_with_loader(
            &[(&tools, "/opt/ro")],
            &["/usr/bin"],
            "/usr/bin/env",
            Some("/opt/ro/around/env"),
        ),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "passes through /workspace, in the writable mount /workspace",
    );
    for entry in ["/opt/ro/system", "/opt/ro/back"] {
        let lists = plan(entry).expect("a path that visits no writable place");
        assert_eq!(lists.allow[0].class(), Class::Follow, "{entry}");
    }
    println!("exec gate followed path through a writable mount: executed");
}

/// D26, **every entry is placed where it ends, whatever its class.** An entry and a program
/// in a mount classed `beneath` are opened with no link followed and then placed: where the
/// end has no place the start is refused `exec.sandbox-place-unserved` with the cause, as it
/// is under a `follow` mount.
#[test]
fn an_entry_in_a_mount_classed_beneath_is_placed_where_it_ends() {
    let staged = Staged::new();
    if !placed("beneath entry placed", staged.root.path()) {
        return;
    }
    write(&staged.workspace.join("bin/tool"), b"tool");
    // On the host's own table each of them has a place.
    staged
        .plan(&[], &["bin", "/workspace"], "/workspace/bin/tool")
        .expect("entries with a place");
    for (text, address, named) in [
        ("bin", "exec_subpaths", "/workspace/bin"),
        (
            "/workspace/bin/tool",
            "exec_subpaths",
            "/workspace/bin/tool",
        ),
        ("/workspace", "exec_subpaths", "/workspace"),
        ("/workspace/bin/tool", "argv", "/workspace/bin/tool"),
    ] {
        let error = staged
            .on_an_unlisted_type(&[], |mounts| mounts.located(text, address))
            .expect_err("an end with no place");
        assert_eq!(error.code, PLACE_UNSERVED, "{error}");
        assert_eq!(error.class, DriverErrorClass::Unserved, "{error}");
        assert_eq!(error.address.as_deref(), Some(address), "{error}");
        assert!(
            error.message.contains(&format!(
                "cannot establish the place of {named}: file-system-unlisted ramfs"
            )),
            "{error}"
        );
    }
    println!("exec gate beneath entry placed: executed");
}

/// D30, **the walk asks**: `follow()` hands every symbolic link it reads to `judge_link`,
/// before it takes the target, and every directory it passes through to `judge_passed`. Held
/// on the driver's own walk of real files, through two changes made once the view stands:
///
/// - the files' own mount has no place. A link refuses where it is read and is named by its
///   own view path, where the path's end would be named by the entry; a directory with no
///   place before it is held, so the link is what the refusal names;
/// - one directory of the root is a location a run can write. A path through it that ends
///   beside it is refused for the directory, once the end was judged; a path that ends in it
///   keeps the answer of the end.
#[test]
fn the_walk_asks_for_the_place_of_every_link_and_directory() {
    let staged = Staged::new();
    if !placed("walk asks for every place", staged.root.path()) {
        return;
    }
    let tools = staged.root.path().join("tools");
    for name in ["dir/inner", "beside"] {
        std::fs::create_dir_all(tools.join(name)).expect("a directory of the root");
    }
    std::os::unix::fs::symlink("/usr/bin", tools.join("system")).expect("a link");
    std::os::unix::fs::symlink("system", tools.join("hop")).expect("a link");
    let roots: [(&Path, &str); 1] = [(&tools, "/opt/ro")];
    // On the host's own table every path here is followed.
    for path in ["/opt/ro/hop", "/opt/ro/dir/inner", "/opt/ro/beside"] {
        staged
            .plan(&roots, &[path], "/usr/bin/env")
            .unwrap_or_else(|error| panic!("{path}: {error}"));
    }
    for path in ["/opt/ro/hop", "/opt/ro/dir/../hop"] {
        let error = staged
            .on_an_unlisted_type(&roots, |mounts| {
                mounts.check_followed("the entry", path, "exec_subpaths")
            })
            .expect_err("a walk that reads a link with no place");
        assert_eq!(error.code, PLACE_UNSERVED, "{error}");
        assert_eq!(error.address.as_deref(), Some("exec_subpaths"), "{error}");
        assert!(
            error
                .message
                .contains("cannot establish the place of /opt/ro/hop: file-system-unlisted ramfs"),
            "{path}: {error}"
        );
    }
    // `tools/dir` as a location a run can write, added once the root was classed `follow`.
    let writable = |mounts: &mut Mounts<'_>| {
        let directory = Opened::of_path(tools.join("dir").to_str().expect("UTF-8"))
            .expect("the directory opens");
        let place = mounts.tree.table().place_of(&directory).expect("placed");
        let location = Location::reaching("the staged directory", &place, &mounts.tree)
            .expect("nothing is mounted beneath it");
        mounts.locations.push(location);
    };
    for (path, holds) in [
        (
            "/opt/ro/dir/../beside",
            "passes through /opt/ro/dir, which lies in the staged directory, a location a run \
             can write",
        ),
        (
            "/opt/ro/dir/inner",
            "resolves to /opt/ro/dir/inner, which shows the staged directory, a location a run \
             can write",
        ),
    ] {
        let followed = staged.asked(&roots, writable, |mounts| {
            mounts.check_followed("the entry", path, "exec_subpaths")
        });
        let error = followed.expect_err("a walk through a directory a run can write");
        assert_eq!(error.code, PATH_UNREPRESENTABLE, "{error}");
        assert_eq!(error.address.as_deref(), Some("exec_subpaths"), "{error}");
        assert!(error.message.contains(holds), "{path}: {error}");
    }
    println!("exec gate walk asks for every place: executed");
}

/// D26 on a real mount: the host's `/proc/sys` declared as a read-only root has a row of type
/// `proc`, outside the list, so the mount has no class and the plan is refused
/// `exec.sandbox-place-unserved` with the declaring field as its address (U3).
#[test]
fn a_declared_root_on_an_unlisted_file_system_refuses_the_plan() {
    let staged = Staged::new();
    if !placed("root on an unlisted file system", staged.root.path()) {
        return;
    }
    let error = staged
        .plan(
            &[(Path::new("/proc/sys"), "/opt/p")],
            &["/opt/p"],
            "/usr/bin/env",
        )
        .expect_err("a root on procfs");
    assert_eq!(error.code, PLACE_UNSERVED, "{error}");
    assert_eq!(error.class, DriverErrorClass::Unserved, "{error}");
    assert_eq!(error.address.as_deref(), Some("read_only_roots"), "{error}");
    for holds in [
        "cannot establish the place of /opt/p: file-system-unlisted proc",
        "ext4, overlay, tmpfs",
    ] {
        assert!(error.message.contains(holds), "{error}");
    }
    println!("exec gate root on an unlisted file system: executed");
}

/// D24 and D25, where a followed path ends on the host: the location a run can write that it
/// lies in is found by place. The text of a path under the host's second mount of the
/// workspace root is beside that root, and its place is inside it; a path beside every
/// location, and one on another device, lie in none.
#[test]
fn a_followed_object_in_a_writable_location_is_found_by_place() {
    let table = host();
    let locations = writable();
    let shown = |path: &str, mount_id: u64| {
        let place = table
            .table()
            .place_of(&seen(path, mount_id))
            .expect("placed");
        shown_location(&place, &locations).map(|location| location.path.clone())
    };
    assert_eq!(
        shown("/work/u/workspaces/ws_1/tools/bin", 31).as_deref(),
        Some("/work/u/workspaces")
    );
    assert_eq!(
        shown("/mnt/second/ws_1/tools/bin", 60).as_deref(),
        Some("/work/u/workspaces"),
        "the second mount of the workspace root"
    );
    assert_eq!(shown("/srv/out", 31).as_deref(), Some("/srv/out"));
    assert_eq!(shown("/srv/out/a/b", 31).as_deref(), Some("/srv/out"));
    assert_eq!(shown("/usr/bin", 31), None);
    assert_eq!(
        shown("/srv/outside", 31),
        None,
        "a name that only begins alike"
    );
    assert_eq!(
        shown("/work/u", 31),
        None,
        "what holds a location is not in it"
    );
    assert_eq!(shown("/tmp/tools", 61), None, "another device");
    // With no writable location there is nothing to lie in.
    let place = table
        .table()
        .place_of(&seen("/work/u/workspaces/ws_1", 31))
        .expect("placed");
    assert!(shown_location(&place, &[]).is_none());
}

/// Design 35 § 5, the kernel's path limit: an entry whose text `<root>/<rel>` in the view is
/// longer than 4095 bytes is refused by the driver, whatever its class, and one of 4095 is not:
/// no bound below the kernel's.
#[test]
fn an_entry_the_helper_could_not_open_for_its_length_is_refused() {
    // A first name that fills the length, then names of 255 bytes with their separators.
    let names = |bytes: usize| {
        let first = bytes % 256;
        assert!((1..=255).contains(&first), "a length this case can spell");
        let rel = format!(
            "{}{}",
            "e".repeat(first),
            format!("/{}", "d".repeat(255)).repeat(bytes / 256)
        );
        assert_eq!(rel.len(), bytes);
        rel
    };
    let follow = |rel: &str| Located::new(Class::Follow, "/opt/t", rel).expect("a path");
    let beneath = |rel: &str| Located::new(Class::Beneath, "/workspace", rel).expect("a path");
    // "/opt/t/" is seven bytes: 4088 more are the limit, and one more is over it.
    check_openable(&follow(&names(4088))).expect("4095 bytes of path");
    let refused = check_openable(&follow(&names(4089))).expect_err("4096 bytes of path");
    assert!(
        refused.contains("is 4096 bytes long") && refused.contains("4095"),
        "{refused}"
    );
    // "/workspace/" is eleven bytes: the same measure for a path opened beneath its root.
    check_openable(&beneath(&names(4084))).expect("4095 bytes of path");
    let refused = check_openable(&beneath(&names(4085))).expect_err("4096 bytes of path");
    assert!(refused.contains("is 4096 bytes long"), "{refused}");
    check_openable(&follow("bin")).expect("a short path");
    check_openable(&Located::new(Class::Follow, "/usr", ".").expect("a root")).expect("a root");
    // The loader and a mount the run can write are measured as the text the helper is given.
    check_length(&format!("/{}", "l".repeat(4094))).expect("4095 bytes");
    check_length(&format!("/{}", "l".repeat(4095))).expect_err("4096 bytes");
}

/// D7 and D24, the loader before dispatch: the helper opens the host's loader as a `follow`
/// path and refuses one that resolves into a mount the run can write. The driver gives the same
/// answer before anything is dispatched, and not exit status 125 after it.
#[test]
fn a_loader_that_resolves_into_a_writable_mount_is_refused_before_dispatch() {
    let staged = Staged::new();
    if !placed("loader into a writable mount", staged.root.path()) {
        return;
    }
    std::fs::create_dir_all(staged.workspace.join("lib")).expect("lib");
    std::fs::write(staged.workspace.join("lib/ld.so"), b"loader").expect("a file");
    let tools = staged.root.path().join("tools");
    std::fs::create_dir_all(&tools).expect("a root");
    std::os::unix::fs::symlink("/workspace/lib/ld.so", tools.join("ld.so")).expect("a link");
    let plan = |loader: &str| {
        staged.plan_with_loader(
            &[(&tools, "/opt/ro")],
            &["/usr/bin"],
            "/usr/bin/env",
            Some(loader),
        )
    };
    assert_refused(
        plan("/opt/ro/ld.so"),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "resolves to /workspace/lib/ld.so, in the writable mount /workspace",
    );
    assert_refused(
        plan("/workspace/lib/ld.so"),
        PATH_UNREPRESENTABLE,
        "exec_subpaths",
        "in the writable mount /workspace",
    );
    // The control: a loader of the base system is carried to the helper as it was read.
    let lists = plan("/usr/bin/env").expect("a loader in the base system");
    assert_eq!(lists.loader.as_deref(), Some("/usr/bin/env"));
    println!("exec gate loader into a writable mount: executed");
}

/// D25, sixth revision, and § 16 *The root's spelling*: a workspace root written through a name
/// that does not exist yet has no place. `<ws>` holds a workspace with a file in it and the
/// helper names that file; with the root written `<ws>/new/..` or `<ws>/new/.` the open is
/// `config.exec-gate-unserved`, address `config.workspace_root`, cause `path-unopenable`, and
/// creates nothing. The control, the root written `<ws>`, is `config.exec-gate-invalid`. The
/// same holds for a declared writable root written that way.
#[test]
fn a_root_written_through_a_missing_name_and_dot_dot_refuses_the_open() {
    let root = staged();
    if !placed("root spelling", root.path()) {
        return;
    }
    let workspaces = root.path().join("workspaces");
    let inside = workspaces.join("ws_earlier/gate");
    write(&inside, b"what an earlier run left");
    let rooted = |workspace_root: PathBuf| {
        let mut config = config(root.path(), &inside);
        config.workspace_root = workspace_root;
        config
    };
    assert_invalid(
        &open(&rooted(workspaces.clone())).expect_err("the root as it is"),
        "which a run can write",
    );
    for last in ["..", "."] {
        let spelled = workspaces.join("new").join(last);
        let error = open(&rooted(spelled.clone())).expect_err("a root with no place");
        assert_eq!(
            (error.code, error.address.as_deref()),
            (EXEC_GATE_UNSERVED, Some("config.workspace_root")),
            "{error}"
        );
        assert!(
            error.message.contains("path-unopenable")
                && error.message.contains(spelled.to_str().expect("UTF-8")),
            "{error}"
        );
        assert!(
            !workspaces.join("new").exists(),
            "the open created {last:?}"
        );
    }
    // A declared writable root written the same way is not compared either.
    let outside = root.path().join("install/gate");
    write(&outside, b"gate");
    let mut declared = config(root.path(), &outside);
    declared.writable_roots = vec![crate::DeclaredWritableRoot {
        name: "out".to_owned(),
        path: root.path().join("install/new/.."),
    }];
    let error = open(&declared).expect_err("a writable root with no place");
    assert_eq!(
        (error.code, error.address.as_deref()),
        (EXEC_GATE_UNSERVED, Some("config.writable_roots")),
        "{error}"
    );
    assert!(error.message.contains("path-unopenable"), "{error}");
    // The control: the same roots written through names that exist, or through none, open.
    std::fs::create_dir_all(workspaces.join("new")).expect("new");
    assert_invalid(
        &open(&rooted(workspaces.join("new/.."))).expect_err("the root, once it resolves"),
        "which a run can write",
    );
    println!("exec gate root spelling: executed");
}

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_GNU_STACK: u32 = 0x6474_e551;

/// A synthetic 64-bit little-endian ELF header for this host's machine, of the type `kind`,
/// with one empty program header of each type in `segments`: what the linkage check reads, and
/// nothing a kernel would run.
fn elf_image(kind: u16, segments: &[u32]) -> Vec<u8> {
    let native = substrate_exec_gate::ElfTarget::NATIVE.expect("a target the driver runs on");
    assert_eq!(
        (native.class, native.data),
        (2, 1),
        "a 64-bit little-endian host"
    );
    elf_image_for(native.machine, kind, segments)
}

fn elf_image_for(machine: u16, kind: u16, segments: &[u32]) -> Vec<u8> {
    let mut image = vec![0_u8; 64];
    image[..4].copy_from_slice(b"\x7fELF");
    image[4] = 2;
    image[5] = 1;
    image[16..18].copy_from_slice(&kind.to_le_bytes());
    image[18..20].copy_from_slice(&machine.to_le_bytes());
    image[32..40].copy_from_slice(&64_u64.to_le_bytes());
    image[54..56].copy_from_slice(&56_u16.to_le_bytes());
    image[56..58].copy_from_slice(&u16::try_from(segments.len()).expect("few").to_le_bytes());
    for segment in segments {
        let mut header = [0_u8; 56];
        header[..4].copy_from_slice(&segment.to_le_bytes());
        image.extend(header);
    }
    image
}

/// D29, step 5 of the open, on synthetic headers: a static image that is not position
/// independent and a static-pie image, which has `PT_DYNAMIC`, are taken. An image with
/// `PT_INTERP`, an image of another machine, a `#!` script and a truncated image are each
/// `config.exec-gate-invalid`, and the message says which test failed.
#[test]
fn a_helper_a_loader_would_start_is_invalid() {
    let path = Path::new("/opt/substrate/substrate-exec-gate");
    check_linkage(path, &elf_image(ET_EXEC, &[PT_LOAD, PT_LOAD, PT_GNU_STACK]))
        .expect("a static image");
    check_linkage(
        path,
        &elf_image(ET_DYN, &[PT_LOAD, PT_LOAD, PT_DYNAMIC, PT_GNU_STACK]),
    )
    .expect("a static-pie image: PT_DYNAMIC is not asked");
    let dynamic = elf_image(
        ET_DYN,
        &[PT_INTERP, PT_LOAD, PT_LOAD, PT_DYNAMIC, PT_GNU_STACK],
    );
    assert_invalid(
        &check_linkage(path, &dynamic).expect_err("a dynamically linked image"),
        "its program headers hold PT_INTERP",
    );
    // Another machine: the one of the two the driver runs on that this host is not.
    let native = substrate_exec_gate::ElfTarget::NATIVE.expect("a target the driver runs on");
    let other = if native.machine == 62 { 183 } else { 62 };
    for (what, image) in [
        (
            "a static image of another machine",
            elf_image_for(other, ET_EXEC, &[PT_LOAD, PT_GNU_STACK]),
        ),
        (
            "a dynamic image of another machine",
            elf_image_for(other, ET_DYN, &[PT_INTERP, PT_LOAD]),
        ),
    ] {
        assert_invalid(
            &check_linkage(path, &image).expect_err(what),
            "is not an ELF image of the driver's class, byte order and machine",
        );
    }
    // A `#!` script, and what is no image at all.
    for (what, bytes) in [
        ("a script", &b"#!/bin/sh\nexec \"$@\"\n"[..]),
        ("an empty file", b""),
        ("text", b"gate"),
    ] {
        assert_invalid(
            &check_linkage(path, bytes).expect_err(what),
            "is not an ELF image of the driver's class, byte order and machine",
        );
    }
    // A truncated image: the table ends outside the bytes, whatever the part read holds.
    let mut truncated = elf_image(ET_EXEC, &[PT_LOAD, PT_GNU_STACK]);
    truncated.pop();
    let mut hidden = elf_image(ET_EXEC, &[PT_LOAD, PT_INTERP]);
    hidden.truncate(hidden.len() - 8);
    for (what, image) in [("truncated", truncated), ("its PT_INTERP cut off", hidden)] {
        assert_invalid(
            &check_linkage(path, &image).expect_err(what),
            "program header table cannot be read whole",
        );
    }
}

/// D29, step 5 of the open: an image of the driver's class, byte order and machine that the
/// kernel answers `ENOEXEC` for is `config.exec-gate-invalid` at the byte test, with or without
/// a `PT_INTERP`: a core file's type, a program header size that is not the class's, more
/// headers than the kernel reads, and none.
#[test]
fn a_helper_the_kernel_does_not_start_is_invalid() {
    const ET_CORE: u16 = 4;
    const ET_REL: u16 = 1;
    let path = Path::new("/opt/substrate/substrate-exec-gate");
    let mut entry_size = elf_image(ET_EXEC, &[PT_LOAD, PT_GNU_STACK]);
    entry_size[54..56].copy_from_slice(&64_u16.to_le_bytes());
    entry_size.extend([0_u8; 64]);
    for (what, image) in [
        ("a core file", elf_image(ET_CORE, &[PT_LOAD, PT_GNU_STACK])),
        (
            "a core file with a loader",
            elf_image(ET_CORE, &[PT_INTERP, PT_LOAD]),
        ),
        ("a relocatable object", elf_image(ET_REL, &[PT_LOAD])),
        ("a larger program header size", entry_size),
        ("74 program headers", elf_image(ET_EXEC, &[PT_LOAD; 74])),
        ("no program header", elf_image(ET_EXEC, &[])),
    ] {
        assert_invalid(
            &check_linkage(path, &image).expect_err(what),
            "is an ELF image the kernel does not start",
        );
    }
    check_linkage(path, &elf_image(ET_EXEC, &[PT_LOAD; 73])).expect("one page of headers");
}

/// D29 through the open, on real files: the check reads the bytes the open then seals, after
/// the trust checks. A dynamically linked file outside every writable location, with one link,
/// is refused for its `PT_INTERP`; the host's own `env`, copied, is one where it is dynamic. A
/// static image in the same place opens, and one in a workspace is refused for where it lies.
#[test]
fn the_open_refuses_a_dynamically_linked_helper_and_takes_a_static_one() {
    let root = staged();
    if !placed("helper linkage", root.path()) {
        return;
    }
    std::fs::create_dir_all(root.path().join("workspaces/ws_1")).expect("a workspace");
    let dynamic = root.path().join("install/dynamic");
    write(
        &dynamic,
        &elf_image(ET_DYN, &[PT_INTERP, PT_LOAD, PT_DYNAMIC]),
    );
    let error = open(&config(root.path(), &dynamic)).expect_err("a dynamic helper");
    assert_invalid(&error, "its program headers hold PT_INTERP");
    assert!(
        error.message.contains(dynamic.to_str().expect("UTF-8")),
        "{error}"
    );

    let env = std::fs::read("/usr/bin/env").expect("the host's env");
    if substrate_exec_gate::elf_linkage(&env) == substrate_exec_gate::Linkage::Interpreted {
        let copied = root.path().join("install/env");
        write(&copied, &env);
        assert_invalid(
            &open(&config(root.path(), &copied)).expect_err("a copy of the host's env"),
            "its program headers hold PT_INTERP",
        );
    }

    let script = root.path().join("install/script");
    write(&script, b"#!/bin/sh\nexec \"$@\"\n");
    assert_invalid(
        &open(&config(root.path(), &script)).expect_err("a script"),
        "is not an ELF image of the driver's class, byte order and machine",
    );

    for (kind, segments) in [
        (ET_EXEC, &[PT_LOAD, PT_GNU_STACK][..]),
        (ET_DYN, &[PT_LOAD, PT_DYNAMIC, PT_GNU_STACK]),
    ] {
        let fixed = root.path().join(format!("install/static-{kind}"));
        write(&fixed, &elf_image(kind, segments));
        assert!(
            matches!(
                open(&config(root.path(), &fixed)).expect("a static helper opens"),
                Opening::Gate(_)
            ),
            "type {kind}"
        );
    }
    // Where it lies is asked first: a dynamic file in a workspace is refused for that.
    let inside = root.path().join("workspaces/ws_1/gate");
    write(&inside, &elf_image(ET_DYN, &[PT_INTERP, PT_LOAD]));
    assert_invalid(
        &open(&config(root.path(), &inside)).expect_err("in a workspace"),
        "which a run can write",
    );
    println!("exec gate helper linkage: executed");
}

/// Story 2's acceptance, D26 at a start: where the place of a deny entry cannot be established
/// the start is `exec.sandbox-place-unserved`, 501, address `exec_denied_paths`, and no code
/// path compares the text of two paths in its stead. One case per cause, each from synthetic
/// mount-table text or from what the kernel would report for the path.
#[test]
fn a_deny_entry_without_a_place_refuses_the_start_for_each_of_the_nine_causes() {
    const REPEATED: &str = "70 31 259:2 /opt /opt rw - ext4 /dev/sda rw\n\
                            70 31 259:2 /opt /opt rw - ext4 /dev/sda rw\n";
    const DELETED: &str = "71 31 259:2 /gone//deleted /opt/gone rw - ext4 /dev/sda rw\n";
    let table = |more: &str| MountTable::parse(&format!("{HOST}{more}")).expect("the table");
    let on = |table: &MountTable, opened: &Opened| table.place_of(opened).expect_err("no place");
    let of_table = |cause: Cause| Unestablished {
        path: "/opt/t/tool".to_owned(),
        cause,
        asked: substrate_exec_gate::place::Asked::Place,
    };
    let mut unreported = seen("/opt/t/tool", 31);
    unreported.mount_id = None;
    let cases = [
        (
            // What the reader answers for a table it cannot read; its own cases hold that.
            of_table(Cause::MountTableUnreadable),
            "mount-table-unreadable",
        ),
        (
            of_table(MountTable::parse("31 1 259:2 / / rw\n").expect_err("no separator")),
            "mount-table-unparsable",
        ),
        (
            Opened::of_path("relative/tool").expect_err("not absolute"),
            "path-unopenable",
        ),
        (on(host().table(), &unreported), "mount-id-unreported"),
        (
            on(host().table(), &seen("/opt/t/tool", 999)),
            "mount-row-missing",
        ),
        (
            on(&table(REPEATED), &seen("/opt/t/tool", 70)),
            "mount-row-repeated",
        ),
        (
            on(&table(DELETED), &seen("/opt/gone/tool", 71)),
            "mount-root-deleted",
        ),
        (
            on(host().table(), &seen("/opt/t/tool", 61)),
            "mount-point-mismatch",
        ),
        (
            on(host().table(), &seen("/data/tool", 62)),
            "file-system-unlisted xfs",
        ),
    ];
    assert_eq!(cases.len(), 9, "D26 names nine causes");
    for (why, cause) in cases {
        let error = denied_unplaced(&why);
        assert_eq!(
            error.code, "exec.sandbox-place-unserved",
            "{cause}: {error}"
        );
        assert_eq!(error.class, DriverErrorClass::Unserved, "{cause}: {error}");
        assert_eq!(
            error.address.as_deref(),
            Some("exec_denied_paths"),
            "{cause}"
        );
        assert!(
            error.message.contains(&format!(": {cause}")),
            "{cause}: {error}"
        );
    }
}

/// A read-only root with `bin/real`, `bin/other`, the link `bin/sh` to `real` and the hard link
/// `bin/again` to it: what the deny cases of the plan declare.
fn deny_tools(staged: &Staged) -> PathBuf {
    let tools = staged.root.path().join("tools");
    std::fs::create_dir_all(tools.join("bin")).expect("bin");
    for name in ["real", "other"] {
        write(&tools.join("bin").join(name), b"a program");
    }
    std::os::unix::fs::symlink("real", tools.join("bin/sh")).expect("a link");
    std::fs::hard_link(tools.join("bin/real"), tools.join("bin/again")).expect("a hard link");
    tools
}

/// Design 35 D10, D23: a denied program is refused before dispatch,
/// `exec.sandbox-program-unlisted`, address `argv`. The driver asks by place on its own mount
/// table (D25) and by device and inode: the program itself, a symbolic link to it (`sh` to a
/// denied `bash`, B1), a hard link beside it, a program in a denied directory (K8), and a
/// program denied only through a second mount of a part of the listed directory (K2). A
/// program the deny-list does not reach is planned, with one `--deny` per entry.
#[test]
fn a_denied_program_is_refused_before_dispatch_by_place() {
    let staged = Staged::new();
    if !placed("denied program", staged.root.path()) {
        return;
    }
    let tools = deny_tools(&staged);
    let bin = tools.join("bin");
    let one = [(tools.as_path(), "/opt/t")];
    let two = [(tools.as_path(), "/opt/t"), (bin.as_path(), "/opt/x")];
    let denied = |roots: &[(&Path, &str)], entries: &[&str], deny: &[&str], program: &str| {
        assert_refused(
            staged.plan_denying(roots, entries, deny, program),
            "exec.sandbox-program-unlisted",
            "argv",
            "is denied by the deny-list entry",
        );
    };
    denied(&one, &["/opt/t"], &["/opt/t/bin/real"], "/opt/t/bin/real");
    denied(&one, &["/opt/t"], &["/opt/t/bin/real"], "/opt/t/bin/sh");
    denied(&one, &["/opt/t"], &["/opt/t/bin/real"], "/opt/t/bin/again");
    denied(&one, &["/opt/t/bin"], &["/opt/t"], "/opt/t/bin/other");
    denied(&two, &["/opt/t"], &["/opt/x/real"], "/opt/t/bin/real");
    denied(&two, &["/opt/t"], &["/opt/x"], "/opt/t/bin/other");
    denied(&two, &["/opt/x"], &["/opt/t/bin/real"], "/opt/x/sh");

    let lists = staged
        .plan_denying(
            &two,
            &["/opt/t"],
            &["/opt/x/real", "/opt/t/bin/later"],
            "/opt/t/bin/other",
        )
        .expect("a program the deny-list does not reach");
    assert_eq!(
        lists.deny,
        [
            Located::new(Class::Follow, "/opt/x", "real").expect("a path"),
            Located::new(Class::Follow, "/opt/t", "bin/later").expect("a path"),
        ]
    );
    let options = lists.options();
    assert_eq!(options.iter().filter(|part| *part == "--deny").count(), 2);
    println!("exec gate denied program: executed");
}

/// Design 35 § 8.1, § 12: an entry of the deny-list has the form of an allow entry and is held
/// to its rules, each refusal with the address `exec_denied_paths`; but it need not exist at
/// the start, in a mount classed `follow` or `beneath`, and is then kept as a name.
#[test]
fn a_deny_entry_is_held_to_the_rules_of_an_entry_and_need_not_exist() {
    let staged = Staged::new();
    if !placed("deny entries", staged.root.path()) {
        return;
    }
    let tools = deny_tools(&staged);
    std::fs::create_dir_all(staged.workspace.join("bin")).expect("bin");
    std::os::unix::fs::symlink("/usr/bin", staged.workspace.join("planted")).expect("a link");
    let roots = [(tools.as_path(), "/opt/t")];
    let plan = |deny: &[&str]| staged.plan_denying(&roots, &["bin"], deny, "/opt/t/bin/other");
    let lists = plan(&[
        "later/tool",
        "bin/later",
        "/opt/t/absent/tool",
        "/opt/t/bin/real",
    ])
    .expect("entries that do not exist yet");
    assert_eq!(
        lists
            .deny
            .iter()
            .map(|entry| (entry.class(), entry.path()))
            .collect::<Vec<_>>(),
        [
            (Class::Beneath, "/workspace/later/tool".to_owned()),
            (Class::Beneath, "/workspace/bin/later".to_owned()),
            (Class::Follow, "/opt/t/absent/tool".to_owned()),
            (Class::Follow, "/opt/t/bin/real".to_owned()),
        ]
    );
    assert_refused(
        plan(&["/tmp/tool"]),
        EXEC_SUBPATH_UNDECLARED,
        "exec_denied_paths",
        "lies outside",
    );
    assert_refused(
        plan(&["planted/sh"]),
        PATH_UNREPRESENTABLE,
        "exec_denied_paths",
        "has a symbolic link in it",
    );
    assert_refused(
        plan(&["bin/../bin"]),
        PATH_UNREPRESENTABLE,
        "exec_denied_paths",
        "",
    );
    println!("exec gate deny entries: executed");
}

/// A host with what the reviews of the deny-list staged (design 35 § 3.13, E19 to E22). A
/// second file system at `/usr/local/src` (E20), mounted again at `/srv/again`. `/opt/t/bin`
/// bound a second time at `/opt/one/deep` (E21 (a)). Beneath `/boot` a `vfat` mount, beneath
/// `/stack` two mounts on one mount point, as beneath `/srv/again/x`, and beneath `/gone` a
/// mount whose root was removed.
const HOST_WITH_DENIED_MOUNTS: &str = "\
31 1 259:2 / / rw - ext4 /dev/nvme0n1p2 rw
70 31 8:33 / /usr/local/src rw - ext4 /dev/sdc1 rw
71 31 8:33 / /srv/again rw - ext4 /dev/sdc1 rw
72 31 259:2 /opt/t/bin /opt/one/deep rw - ext4 /dev/nvme0n1p2 rw
73 31 0:50 / /boot/efi rw - vfat /dev/sda1 rw
74 31 8:49 / /stack/m rw - ext4 /dev/sdd1 rw
75 74 8:50 / /stack/m rw - ext4 /dev/sdd2 rw
76 31 259:2 /lost//deleted /gone/m rw - ext4 /dev/nvme0n1p2 rw
77 71 8:65 / /srv/again/x/m rw - ext4 /dev/sde1 rw
78 77 8:66 / /srv/again/x/m rw - ext4 /dev/sde2 rw
";

/// What the driver sighted at `path`, on the mount `mount_id` of `tree`: a directory, or the
/// file `file`.
fn sighted_at(
    tree: &MountTree,
    mount_id: u64,
    path: &str,
    file: Option<(libc::dev_t, libc::ino_t)>,
) -> Sighted {
    Sighted {
        place: tree
            .table()
            .place_of(&seen(path, mount_id))
            .expect("the place of a path of the table"),
        file,
    }
}

/// **D23 with D30, the program against the deny entries, on a table written as text**
/// (design 35 § 16, *the six checks*): a program in a host mount beneath a denied directory
/// is `exec.sandbox-program-unlisted`, address `argv`, and so is one in a second mount of
/// that file system elsewhere, a program reached through a second mount of a denied file's
/// directory, and another name of a denied file. A comparison of places alone refuses none
/// of the first two: the mount at `/usr/local/src` is on another device than `/usr/local`.
#[test]
fn a_program_in_a_mount_beneath_a_denied_directory_is_refused_by_what_the_entry_reaches() {
    let tree = tree(HOST_WITH_DENIED_MOUNTS);
    let at = |mount_id: u64, path: &str| sighted_at(&tree, mount_id, path, None);
    let file = |mount_id: u64, path: &str, inode: libc::ino_t| {
        sighted_at(&tree, mount_id, path, Some((7, inode)))
    };
    let judged = |program: &Sighted, denied: Sighted| {
        judge_denied(&tree, "/program", program, &[("the entry", denied)], &[])
    };
    let refused = |program: &Sighted, denied: Sighted, what: &str| {
        let error = judged(program, denied).expect_err(what);
        assert_eq!(
            error.code, "exec.sandbox-program-unlisted",
            "{what}: {error}"
        );
        assert_eq!(error.class, DriverErrorClass::Refused, "{what}: {error}");
        assert_eq!(error.address.as_deref(), Some("argv"), "{what}");
        assert!(
            error
                .message
                .contains("\"/program\" is denied by the deny-list entry"),
            "{what}: {error}"
        );
    };

    // E20: `/usr/local` denied, the program in the host mount at `/usr/local/src`.
    let tool = file(70, "/usr/local/src/tool", 1);
    refused(&tool, at(31, "/usr/local"), "a program in a mount beneath");
    refused(
        &file(71, "/srv/again/tool", 1),
        at(31, "/usr/local"),
        "the same file system mounted a second time elsewhere (D23)",
    );
    // The entry's own place, as before D30, and a directory above the mount.
    refused(
        &file(70, "/usr/local/src/sub/tool", 1),
        at(70, "/usr/local/src/sub"),
        "a directory of the mount",
    );
    refused(&tool, at(31, "/usr"), "a directory above the mount");
    // The mount itself, denied: by place it holds what is attached in its file system
    // wherever that shows, the stack beneath `/srv/again/x` among it, and the stack cannot
    // be compared. The start is refused for that, and the program is not looked at.
    let error = judged(&tool, at(70, "/usr/local/src")).expect_err("the stack");
    assert_eq!(error.code, PLACE_UNSERVED, "{error}");
    assert!(
        error
            .message
            .contains("cannot establish where /srv/again/x/m is attached: mount-covered"),
        "{error}"
    );
    // E21 (a): `/opt/t/bin/tool` denied, the program its name through the second mount.
    refused(
        &file(72, "/opt/one/deep/tool", 2),
        file(31, "/opt/t/bin/tool", 2),
        "a second mount of the denied file's directory",
    );
    // Another name of the denied file, by device and inode, wherever it lies.
    refused(
        &file(31, "/bin/alias", 3),
        file(70, "/usr/local/src/tool", 3),
        "another name",
    );

    // What no deny entry reaches is planned.
    judged(&tool, at(31, "/usr/local/bin")).expect("a directory beside the mount");
    judged(
        &file(31, "/usr/local/bin/tool", 4),
        at(70, "/usr/local/src/sub"),
    )
    .expect("a directory of the mount denied, a program beside the mount");
    judged(
        &file(31, "/usr/bin/sh", 5),
        file(70, "/usr/local/src/tool", 6),
    )
    .expect("another file on another file system");
    // A directory is no file: two of them are not "the same file".
    judged(&at(31, "/usr/bin"), at(31, "/usr/local")).expect("two directories");
}

/// **Failing closed before dispatch, by D30's table.** What a denied directory reaches and
/// what a listed directory of a start with a deny-list reaches must be established on the
/// driver's table: the start is `exec.sandbox-place-unserved`, 501, with the cause and the
/// line of an attachment, address `exec_denied_paths` for the first and `exec_subpaths` for
/// the second. A type outside the list refuses beneath a denied directory only. A listed
/// file, and a listed directory that lies in a denied place, are asked nothing: the helper
/// writes no rule for the second and asks nothing of the first.
#[test]
fn a_reach_the_deny_list_needs_and_cannot_have_refuses_the_start() {
    let tree = tree(HOST_WITH_DENIED_MOUNTS);
    let at = |path: &str| sighted_at(&tree, 31, path, None);
    let program = sighted_at(&tree, 31, "/usr/bin/sh", Some((7, 5)));
    let unserved = |denied: &[(&str, Sighted)], listed: &[Sighted], address: &str, line: &str| {
        let error = judge_denied(&tree, "/usr/bin/sh", &program, denied, listed)
            .expect_err("a reach that cannot be established");
        assert_eq!(error.code, PLACE_UNSERVED, "{line}: {error}");
        assert_eq!(error.class, DriverErrorClass::Unserved, "{line}: {error}");
        assert_eq!(error.address.as_deref(), Some(address), "{line}: {error}");
        assert!(error.message.contains(line), "{line}: {error}");
    };
    for (directory, line) in [
        (
            "/boot",
            "cannot establish where /boot/efi is attached: file-system-unlisted vfat",
        ),
        (
            "/stack",
            "cannot establish where /stack/m is attached: mount-covered",
        ),
        (
            "/gone",
            "cannot establish where /gone/m is attached: mount-root-deleted",
        ),
    ] {
        unserved(&[("denied", at(directory))], &[], "exec_denied_paths", line);
    }
    let denied = || [("denied", at("/opt/t/bin"))];
    for (directory, line) in [
        (
            "/stack",
            "cannot establish where /stack/m is attached: mount-covered",
        ),
        (
            "/gone",
            "cannot establish where /gone/m is attached: mount-root-deleted",
        ),
    ] {
        unserved(&denied(), &[at(directory)], "exec_subpaths", line);
    }
    let planned = |denied: &[(&str, Sighted)], listed: &[Sighted]| {
        judge_denied(&tree, "/usr/bin/sh", &program, denied, listed)
    };
    planned(&denied(), &[at("/boot")])
        .expect("a type outside the list beneath a listed directory is compared with nothing");
    planned(
        &denied(),
        &[sighted_at(&tree, 31, "/stack/file", Some((7, 8)))],
    )
    .expect("a listed file reaches nothing");
    // `/srv/again/x` shows a directory of the file system at `/usr/local/src`, and holds a
    // stack. Listed beside a denied `/usr/local` it lies in a denied place: no rule, and
    // nothing asked. Beside another deny entry it is asked, and refuses.
    let again = || [sighted_at(&tree, 71, "/srv/again/x", None)];
    planned(&[("denied", at("/usr/local"))], &again())
        .expect("a listed directory in a denied place gets no rule and is asked nothing");
    unserved(
        &denied(),
        &again(),
        "exec_subpaths",
        "cannot establish where /srv/again/x/m is attached: mount-covered",
    );
    planned(
        &denied(),
        &[at("/usr/local"), at("/opt/one"), at("/usr/bin")],
    )
    .expect("listed directories whose mounts beneath can be compared");
}

/// The three guards of a deny entry's sighting, each on real directories.
///
/// - **Only an absent name is absent.** A deny entry whose path fails for another reason, a
///   link that never ends or one that leaves the view, refuses the start; it is not kept as
///   a name beneath the directory above it. Read as absent it would deny nothing.
/// - **An entry that does not exist is sighted nowhere.** Its nearest ancestor is placed,
///   and the ancestor is not what the entry denies: a program beside the absent name, in a
///   mount classed `beneath` and in the workspace itself, is planned.
/// - **A deny entry is measured like an entry**: one whose path in the view is longer than
///   the kernel takes is refused by name, because the helper could not open it.
#[test]
fn a_deny_entry_is_absent_only_where_its_name_is_and_then_denies_no_ancestor() {
    let staged = Staged::new();
    if !placed("deny entry guards", staged.root.path()) {
        return;
    }
    let tools = deny_tools(&staged);
    std::os::unix::fs::symlink("loop", tools.join("bin/loop")).expect("a link to itself");
    std::os::unix::fs::symlink("/nowhere/tool", tools.join("bin/out")).expect("a link out");
    std::fs::create_dir_all(staged.workspace.join("bin")).expect("bin");
    write(&staged.workspace.join("bin/tool"), b"a program");
    write(&staged.workspace.join("top"), b"a program");
    let roots = [(tools.as_path(), "/opt/t")];
    let plan = |deny: &[&str], program: &str| {
        staged.plan_denying(&roots, &["bin", "/opt/t"], deny, program)
    };

    assert_refused(
        plan(&["/opt/t/bin/loop"], "/opt/t/bin/other"),
        PATH_UNREPRESENTABLE,
        "exec_denied_paths",
        "resolves through more than 40 symbolic links",
    );
    assert_refused(
        plan(&["/opt/t/bin/out"], "/opt/t/bin/other"),
        PATH_UNREPRESENTABLE,
        "exec_denied_paths",
        "which the view does not have",
    );

    let lists = plan(
        &["bin/later", "later", "bin/tool/beneath"],
        "/workspace/bin/tool",
    )
    .expect("a program beside names that do not exist");
    assert_eq!(lists.deny.len(), 3);
    plan(&["later"], "/workspace/top").expect("a program of the workspace beside an absent name");
    assert_refused(
        plan(&["bin"], "/workspace/bin/tool"),
        "exec.sandbox-program-unlisted",
        "argv",
        "is denied by the deny-list entry",
    );

    // "/workspace/" is eleven bytes; sixteen names of 255 bytes with their separators more.
    let long = vec!["d".repeat(255); 16].join("/");
    assert_eq!(format!("/workspace/{long}").len(), 4106);
    assert_refused(
        plan(&[long.as_str()], "/opt/t/bin/other"),
        PATH_UNREPRESENTABLE,
        "exec_denied_paths",
        "is 4106 bytes long",
    );
    println!("exec gate deny entry guards: executed");
}
