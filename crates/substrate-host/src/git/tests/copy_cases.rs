//! Cases of what a build takes of a workspace's object store (design 21, § *The baseline
//! store*, steps 1 and 2): it copies the pack files and the loose objects and nothing else, and
//! indexes each copied pack itself. A pack whose objects do not all resolve ends the attempt.

use std::collections::BTreeSet;
use std::os::fd::OwnedFd;
use std::path::Path;

use git2::{Oid, Repository};
use tempfile::{TempDir, tempdir};

use super::super::worktree;
use super::store_cases::{leftovers, store_objects};
use super::store_failure_cases::{both, codes, commit_id_file_alone};
use super::watch::{Kind, Watch};
use super::{baseline_root, observe, remove_store, source_repository, store_of};

/// Every entry beneath `directory`, by its path relative to it; a directory ends in `/`.
fn entries(directory: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut pending = vec![directory.to_owned()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).expect("directory") {
            let entry = entry.expect("entry");
            let path = entry.path();
            let relative = path
                .strip_prefix(directory)
                .expect("an entry beneath the directory")
                .to_string_lossy()
                .into_owned();
            if entry.file_type().expect("entry type").is_dir() {
                found.insert(format!("{relative}/"));
                pending.push(path);
            } else {
                found.insert(relative);
            }
        }
    }
    found
}

/// A build copies the pack files and the loose objects of the workspace's object store, and
/// nothing else of it: no pack index, no reverse index, no bitmap, no multi-pack index, no
/// commit graph, nothing under `info/`, and no file whose name is not a pack file's or a loose
/// object's.
#[test]
fn a_build_copies_pack_files_and_loose_objects_and_nothing_else() {
    let (workspace, _) = source_repository();
    let objects = workspace.path().join(".git/objects");
    let loose_objects: BTreeSet<String> = entries(&objects)
        .into_iter()
        .filter(|entry| !entry.starts_with("info") && !entry.starts_with("pack"))
        .collect();
    assert!(loose_objects.len() >= 6, "{loose_objects:?}");
    let fanout = loose_objects
        .iter()
        .find(|entry| entry.ends_with('/'))
        .expect("a directory of loose objects")
        .clone();
    std::fs::create_dir_all(objects.join("info/commit-graphs")).expect("directory");
    std::fs::create_dir(objects.join("zz")).expect("directory");
    for other in [
        "pack/pack-a.pack".to_owned(),
        "pack/pack-a.idx".to_owned(),
        "pack/pack-a.rev".to_owned(),
        "pack/pack-a.bitmap".to_owned(),
        "pack/pack-a.keep".to_owned(),
        "pack/multi-pack-index".to_owned(),
        "pack/.pack".to_owned(),
        "info/packs".to_owned(),
        "info/commit-graph".to_owned(),
        "info/commit-graphs/graph-1.graph".to_owned(),
        format!("zz/{}", "0".repeat(38)),
        format!("{fanout}{}", "0".repeat(37)),
        format!("{fanout}{}", "A".repeat(38)),
        format!("{fanout}tmp_obj_1"),
        "0000000000000000000000000000000000000000".to_owned(),
    ] {
        std::fs::write(objects.join(&other), b"another file\n").expect("file");
    }
    let copy = tempdir().expect("copy");
    let (loose, packs) = (copy.path().join("objects"), copy.path().join("packs"));
    for directory in [&loose, &packs] {
        std::fs::create_dir(directory).expect("directory");
    }
    let pinned: OwnedFd = std::fs::File::open(&objects).expect("object store").into();

    worktree::copy_store(&pinned, &loose, &packs, usize::MAX, u64::MAX).expect("copy");

    assert_eq!(entries(&loose), loose_objects);
    assert_eq!(entries(&packs), BTreeSet::from(["pack-a.pack".to_owned()]));
    assert_eq!(
        std::fs::read(packs.join("pack-a.pack")).expect("pack"),
        b"another file\n"
    );
}

/// A workspace materialized before the store existed whose baseline objects are in one pack,
/// with the index the workspace holds for it, and in no loose object.
fn packed_workspace() -> (TempDir, TempDir, Oid) {
    let (workspace, commit) = source_repository();
    let baselines = baseline_root(workspace.path(), commit);
    remove_store(baselines.path());
    {
        let repository = Repository::open(workspace.path()).expect("repository");
        let mut builder = repository.packbuilder().expect("pack builder");
        builder.insert_commit(commit).expect("baseline objects");
        let mut pack = git2::Buf::new();
        builder.write_buf(&mut pack).expect("pack");
        let objects = repository.odb().expect("object store");
        let mut writer = objects.packwriter().expect("pack writer");
        std::io::Write::write_all(&mut writer, &pack).expect("pack bytes");
        writer.commit().expect("pack index");
    }
    for entry in std::fs::read_dir(workspace.path().join(".git/objects")).expect("objects") {
        let entry = entry.expect("entry");
        if entry.file_name().len() == 2 {
            std::fs::remove_dir_all(entry.path()).expect("loose objects removed");
        }
    }
    (workspace, baselines, commit)
}

/// A build indexes each copied pack itself: the pack file of the workspace is read, and the
/// index the workspace holds for it is neither opened nor read. The store is built from the
/// pack all the same.
#[test]
fn a_build_reads_the_pack_files_of_a_workspace_and_no_index_of_them() {
    let (workspace, baselines, commit) = packed_workspace();
    let pack = workspace.path().join(".git/objects/pack");
    let held: BTreeSet<String> = entries(&pack);
    assert!(
        held.iter().any(|name| name.strip_suffix(".idx").is_some()),
        "the workspace holds no index: {held:?}"
    );
    let mut watch = Watch::new();
    watch.add("pack", &pack);

    observe(workspace.path(), baselines.path(), 1024).expect("change set");

    let read: BTreeSet<String> = watch
        .events()
        .into_iter()
        .filter(|event| event.kind == Kind::Read && !event.name.is_empty())
        .map(|event| event.name)
        .collect();
    let packs: BTreeSet<String> = held
        .into_iter()
        .filter(|name| name.strip_suffix(".pack").is_some())
        .collect();
    assert_eq!(read, packs, "what a build opened or read of the packs");
    assert!(store_objects(baselines.path()).contains(&commit));
    assert_eq!(leftovers(baselines.path()), Vec::<String>::new());
}

const CHILD: &str = "SUBSTRATE_TEST_BUILD_IN_A_PROCESS_OF_ITS_OWN";
/// The processor time the builds of this file's small stores may take together, in seconds.
const PROCESSOR_SECONDS: u64 = 3;

/// Writes one pack, and an index for it, into the object store at `objects`. The pack holds
/// one object, stored against the object `base`, and its trailer is the hash of its bytes.
/// The index names that object `id`.
fn plant_pack_of_an_object_stored_against(objects: &Path, id: Oid, base: Oid) {
    let delta = [1_u8, 1, 1, b'x'];
    let (mut low, mut high) = (1_u32, 0_u32);
    for byte in delta {
        low = (low + u32::from(byte)) % 65_521;
        high = (high + low) % 65_521;
    }
    let mut pack = b"PACK".to_vec();
    pack.extend(2_u32.to_be_bytes());
    pack.extend(1_u32.to_be_bytes());
    pack.push(0x74);
    pack.extend(base.as_bytes());
    pack.extend([0x78, 0x01, 0x01, 0x04, 0x00, 0xfb, 0xff]);
    pack.extend(delta);
    pack.extend(((high << 16) | low).to_be_bytes());
    let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
    hasher.update(&pack);
    let trailer = hasher.try_finalize().expect("hash of the pack");
    pack.extend(trailer.as_bytes());
    let mut index = vec![0xff, b't', b'O', b'c'];
    index.extend(2_u32.to_be_bytes());
    for first in 0..=255_u8 {
        index.extend(u32::from(first >= id.as_bytes()[0]).to_be_bytes());
    }
    index.extend(id.as_bytes());
    index.extend(0_u32.to_be_bytes());
    index.extend(12_u32.to_be_bytes());
    index.extend(trailer.as_bytes());
    index.extend(trailer.as_bytes());
    let directory = objects.join("pack");
    std::fs::create_dir_all(&directory).expect("pack directory");
    let name = format!("pack-{trailer}");
    std::fs::write(directory.join(format!("{name}.pack")), pack).expect("pack");
    std::fs::write(directory.join(format!("{name}.idx")), index).expect("index");
}

/// A build ends, and a pack whose objects do not all resolve ends it
/// `workspace.git-baseline-missing` for both observations, with no store and no copy left. The
/// commit of each workspace here is in no loose object; its object store holds a pack of one
/// object stored against another, and an index that names that object by the commit's id. In
/// the first workspace the other object is the commit's id itself, in the second an id no
/// object has. The builds run in a process of their own with a bound on its processor time,
/// which no load on the machine changes.
#[test]
fn a_build_refuses_an_object_its_copy_stores_against_itself() {
    if std::env::var_os(CHILD).is_none() {
        let module = module_path!().split_once("::").expect("module path").1;
        let case = format!("{module}::a_build_refuses_an_object_its_copy_stores_against_itself");
        let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args([case.as_str(), "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .status()
            .expect("the builds' process");
        assert!(
            status.success(),
            "the builds did not end in their refusal within {PROCESSOR_SECONDS} s of processor \
             time: {status}"
        );
        return;
    }
    for (resource, bound) in [
        (libc::RLIMIT_CORE, 0),
        (libc::RLIMIT_CPU, PROCESSOR_SECONDS),
    ] {
        let limit = libc::rlimit {
            rlim_cur: bound,
            rlim_max: bound,
        };
        // SAFETY: `limit` is a valid `rlimit` that outlives the call.
        assert_eq!(unsafe { libc::setrlimit(resource, &raw const limit) }, 0);
    }
    let absent = Oid::from_bytes(&[0x22; 20]).expect("an id");
    for base in [None, Some(absent)] {
        let (workspace, commit) = source_repository();
        let baselines = commit_id_file_alone(commit);
        let objects = workspace.path().join(".git/objects");
        let commit_id = commit.to_string();
        std::fs::remove_file(objects.join(&commit_id[..2]).join(&commit_id[2..]))
            .expect("the commit's own object");
        plant_pack_of_an_object_stored_against(&objects, commit, base.unwrap_or(commit));

        assert_eq!(
            codes(both(workspace.path(), baselines.path())),
            ["workspace.git-baseline-missing"; 2],
            "stored against {}",
            base.unwrap_or(commit)
        );
        assert!(!store_of(baselines.path()).exists(), "a store is kept");
        assert_eq!(leftovers(baselines.path()), Vec::<String>::new());
    }
}
