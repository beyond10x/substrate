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
    EXEC_GATE_INVALID, EXEC_GATE_UNSERVED, ExecGate, FileFacts, Location, OpenDecision,
    OpenPlacement, OpenPlaces, OpenSightings, OpenUnplaced, Opening, PLACE_UNSERVED, Unfollowed,
    View, check_file, check_length, check_linkage, check_links, check_location, check_openable,
    check_protocol, classed, decide_open, judge_link, open, place_open, plan, shown_location,
    start_unplaced, withheld_facts,
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
/// declared mount refuses no start**, whatever the kernel would say of it. A stack, a removed
/// root and a mount ID two rows have are counted by the root each row names, and the class is
/// decided: `follow` where none of them shows a writable place, and `beneath` where one does,
/// although a later mount covers it. The fail-closed causes belong to the writable side
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
    // is `beneath`. Never the looser answer for want of the kernel's.
    let covered = "80 31 259:2 /work/u/workspaces/ws_1/media /opt/tools/stack rw - ext4 /dev/a rw\n\
                   81 80 0:61 / /opt/tools/stack rw - tmpfs tmpfs rw\n";
    assert_eq!(class(covered, "/opt/tools"), Class::Beneath);
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
            Err(Unfollowed::Refused(why)) => panic!("{cause}: refused for the path: {why}"),
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
            // One entry per fact the helper would prove; this story has the allow-list fact.
            assert_eq!(
                withheld_facts(&unplaced),
                [entry(address, path, Some(file_system))],
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
        [entry("config.workspace_root", "/work/u/workspaces", None)]
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
        [
            entry("config.exec_gate", "/data/substrate-exec-gate", Some("xfs")),
            entry("config.exec_gate", "/data/tmp", Some("xfs")),
        ]
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
            [entry(
                "config.workspace_root",
                "/data/workspaces",
                Some("xfs")
            )]
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
            Opening::Withheld(entries) => assert_eq!(entries, expected, "{links} links"),
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

    /// [`Self::plan`], for a helper that read `loader` as the host's ELF loader at open.
    fn plan_with_loader(
        &self,
        roots: &[(&Path, &str)],
        entries: &[&str],
        program: &str,
        loader: Option<&str>,
    ) -> Result<Lists, DriverError> {
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
