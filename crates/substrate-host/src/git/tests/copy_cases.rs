//! Cases of what a build takes of a workspace's object store (design 21, § *The baseline
//! store*, steps 1 and 2): it copies the pack files and the loose objects and nothing else, and
//! indexes each copied pack itself. A pack whose objects do not all resolve ends the attempt.

use std::collections::BTreeSet;
use std::os::fd::OwnedFd;
use std::path::Path;

use git2::{ObjectType, Oid, Repository};
use tempfile::{TempDir, tempdir};

use super::super::{store, worktree};
use super::store_cases::{leftovers, store_objects};
use super::store_failure_cases::{both, codes, commit_id_file_alone};
use super::watch::{Kind, Watch};
use super::{baseline_root, observe, remove_store, source_repository, store_of, target};
use crate::git_baselines::{BuildTurns, DEFAULT_BYTE_LIMIT};

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
/// The processor time the builds of [`a_build_refuses_an_object_its_copy_stores_against_itself`]
/// may take together, in seconds.
const PROCESSOR_SECONDS: u64 = 3;

/// The bytes of `data` as one zlib stream of one stored block.
fn stored(data: &[u8]) -> Vec<u8> {
    let length = u16::try_from(data.len()).expect("at most one stored block");
    let (mut low, mut high) = (1_u32, 0_u32);
    for byte in data {
        low = (low + u32::from(*byte)) % 65_521;
        high = (high + low) % 65_521;
    }
    let mut stream = vec![0x78, 0x01, 0x01];
    stream.extend(length.to_le_bytes());
    stream.extend((!length).to_le_bytes());
    stream.extend(data);
    stream.extend(((high << 16) | low).to_be_bytes());
    stream
}

/// Ends `pack` with the hash of its bytes and answers that hash.
fn trailer_of(pack: &mut Vec<u8>) -> gix::hash::ObjectId {
    let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
    hasher.update(pack);
    let trailer = hasher.try_finalize().expect("hash of the pack");
    pack.extend(trailer.as_bytes());
    trailer
}

/// Writes one pack into the object store at `objects`, and an index of it that names `id`.
/// The pack holds one object, stored against the object `base`.
fn plant_pack_of_an_object_stored_against(objects: &Path, id: Oid, base: Oid) {
    let mut pack = b"PACK".to_vec();
    pack.extend(2_u32.to_be_bytes());
    pack.extend(1_u32.to_be_bytes());
    pack.push(0x74);
    pack.extend(base.as_bytes());
    pack.extend(stored(&[1, 1, 1, b'x']));
    let trailer = trailer_of(&mut pack);
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

/// A pack whose objects do not all resolve ends the build `workspace.git-baseline-missing`
/// for both observations, with no store and no copy left. Each workspace here holds its commit
/// in no loose object, and a pack of one object that is stored against another: in the first
/// against the commit's own id, in the second against an id no object has. The builds run in a
/// process of their own, whose processor time is bounded.
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

/// `value` in groups of seven bits, the lowest first, as a delta states a size.
fn size_of_delta(mut value: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    loop {
        let group = u8::try_from(value & 0x7f).expect("seven bits");
        value >>= 7;
        if value == 0 {
            bytes.push(group);
            return bytes;
        }
        bytes.push(group | 0x80);
    }
}

/// The header of a pack entry of the type `kind` that states `size` bytes.
fn entry_header(kind: u8, mut size: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut byte = (kind << 4) | u8::try_from(size & 0x0f).expect("four bits");
    size >>= 4;
    while size > 0 {
        bytes.push(byte | 0x80);
        byte = u8::try_from(size & 0x7f).expect("seven bits");
        size >>= 7;
    }
    bytes.push(byte);
    bytes
}

/// The bytes one object of [`plant_pack_that_holds_an_object`] resolves to.
const RESOLVED_BYTES: usize = 128 * 65_535;

/// Writes one pack file of 66 KiB, and no index, into the object store at `objects`. It holds
/// a file object of 65,535 bytes and, `times` times, the same object stored against it, of
/// [`RESOLVED_BYTES`] bytes, about 8 MiB.
fn plant_pack_that_holds_an_object(objects: &Path, times: u32) {
    let base = vec![b'a'; 65_535];
    let id = Oid::hash_object(ObjectType::Blob, &base).expect("id of the base");
    let mut delta = size_of_delta(base.len());
    delta.extend(size_of_delta(RESOLVED_BYTES));
    for _ in 0..RESOLVED_BYTES / base.len() {
        // Copy 65,535 bytes of the base from its start.
        delta.extend([0xb0, 0xff, 0xff]);
    }
    let mut pack = b"PACK".to_vec();
    pack.extend(2_u32.to_be_bytes());
    pack.extend((1 + times).to_be_bytes());
    pack.extend(entry_header(3, base.len()));
    pack.extend(stored(&base));
    for _ in 0..times {
        pack.extend(entry_header(7, delta.len()));
        pack.extend(id.as_bytes());
        pack.extend(stored(&delta));
    }
    let trailer = trailer_of(&mut pack);
    let directory = objects.join("pack");
    std::fs::create_dir_all(&directory).expect("pack directory");
    std::fs::write(directory.join(format!("pack-{trailer}.pack")), pack).expect("pack");
}

/// The memory this process holds, resident and swapped, in KiB.
fn held_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("process status");
    ["VmRSS:", "VmSwap:"]
        .iter()
        .map(|field| {
            status
                .lines()
                .find_map(|line| line.strip_prefix(field))
                .and_then(|rest| {
                    rest.trim()
                        .trim_end_matches("kB")
                        .trim()
                        .parse::<u64>()
                        .ok()
                })
                .expect("a count of KiB")
        })
        .sum()
}

/// A build that is refused leaves nothing: no store, no copy, and none of the memory its
/// indexer took. The workspace's object store here holds the pack of
/// [`plant_pack_that_holds_an_object`] with its object twice, and each build is refused
/// `workspace.git-baseline-missing`. After one pair of observations, four more pairs, eight
/// builds, leave the process holding less than 48 MiB more than before them. The same pack
/// with that object once is indexed, and the store is built beside it.
#[test]
#[ignore = "waits for the decision on the build of a store for a workspace that predates it"]
fn a_refused_build_keeps_none_of_the_memory_its_indexer_took() {
    let (sound, commit) = source_repository();
    plant_pack_that_holds_an_object(&sound.path().join(".git/objects"), 1);
    assert_eq!(
        codes(both(sound.path(), commit_id_file_alone(commit).path())),
        ["answered"; 2],
        "the pack that holds the object once"
    );

    let (workspace, commit) = source_repository();
    let baselines = commit_id_file_alone(commit);
    plant_pack_that_holds_an_object(&workspace.path().join(".git/objects"), 2);
    let observed = || codes(both(workspace.path(), baselines.path()));

    assert_eq!(observed(), ["workspace.git-baseline-missing"; 2]);
    let before = held_kib();
    for _ in 0..4 {
        assert_eq!(observed(), ["workspace.git-baseline-missing"; 2]);
    }
    let kept = held_kib().saturating_sub(before);

    assert!(
        kept < 48 * 1024,
        "eight refused builds of a pack of 66 KiB left the process holding {kept} KiB more"
    );
}

/// An entry of the workspace's object store counts toward the entry bound whether or not a
/// build copies it: with eight files a build does not copy in the pack directory, a bound of
/// one entry fewer than the object store holds ends the attempt
/// `workspace.git-repository-failed`, and a bound of exactly as many builds the store.
#[test]
fn an_entry_a_build_does_not_copy_counts_toward_its_entry_bound() {
    let (workspace, commit) = source_repository();
    let objects = workspace.path().join(".git/objects");
    let pack = objects.join("pack");
    std::fs::create_dir_all(&pack).expect("pack directory");
    for number in 0..8 {
        std::fs::write(pack.join(format!("other-{number}.idx")), b"another file\n")
            .expect("a file a build does not copy");
    }
    let held = entries(&objects).len();
    let build = |max_entries: usize| {
        let baselines = commit_id_file_alone(commit);
        let turn = BuildTurns::default().taken_blocking();
        target(
            workspace.path(),
            baselines.path(),
            DEFAULT_BYTE_LIMIT,
            |workspace| store::built(workspace, commit, max_entries, &turn),
        )
        .map_or_else(|error| error.code, |()| "built")
    };

    assert_eq!(build(held - 1), "workspace.git-repository-failed");
    assert_eq!(build(held), "built");
}
