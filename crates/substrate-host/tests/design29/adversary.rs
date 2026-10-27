//! Adversary pass on design 29 (`docs/design/29-scoped-workspace-write-and-output-tail.md`,
//! commit b7d677ef): boundary and conformance cases for output retention and for a scoped
//! workspace write, driven from the design note rather than from the unit's own tests.
//!
//! The portable cases run the `local` adapter. The delegated cases run the `linux` adapter and
//! need `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2 subtree this process is
//! inside, and bubblewrap; without them each prints `<name>: ABSENT (<reason>)` and is **absent,
//! never reported as passed** (invariant 3). Each delegated case that ran prints
//! `<name>: executed`. `bash scripts/delegated-lane.sh` provides the environment.
//!
//! Every expected value here is computed by an oracle written in this file from the design's
//! § 6 table and from the base commit's `drain_capped`, never by the code under test.

#![cfg(target_os = "linux")]
#![allow(clippy::too_many_lines)]

use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile,
    IoBounds, KeptOutput, OutputRetention, Retention, RunOutput, RunState, WorkspaceAccess,
    linux::LinuxRequest, local::LocalRequest,
};

const REASON: &str = "the adversary runs a host process on purpose";
/// The host driver's truncation marker, as the base commit wrote it.
const MARKER: &[u8] = b"\n[substrate: output truncated]\n";
const MODES: [Retention; 3] = [Retention::Head, Retention::Tail, Retention::HeadAndTail];

// ---------------------------------------------------------------------------------------------
// Oracles
// ---------------------------------------------------------------------------------------------

/// `len` bytes that do not repeat with any short period, so a head, a tail and a gap are each
/// distinguishable. A different `seed` gives an unrelated stream.
fn pattern(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33).to_le_bytes()[0]
        })
        .collect()
}

/// What the base commit (b7d677ef~1) stored for one stream: the first `bound` bytes and, on the
/// host driver over the bound, the marker over the last 31 of them (`drain_capped` before the
/// change, `mark_truncation = true`); `local`'s `drain` kept the first `bound` bytes and no marker.
fn previous_head(stream: &[u8], bound: usize, linux: bool) -> (Vec<u8>, bool) {
    let mut stored = stream[..stream.len().min(bound)].to_vec();
    let truncated = stream.len() > bound;
    if truncated && linux {
        if bound >= MARKER.len() {
            stored.truncate(bound - MARKER.len());
            stored.extend_from_slice(MARKER);
        } else {
            stored.truncate(bound);
        }
    }
    (stored, truncated)
}

/// Asserts one stream of a run against design 29 §§ 5–6, with `linux` choosing whether `Head`
/// over the bound carries the driver's marker.
#[allow(clippy::too_many_arguments)]
fn check_stream(
    name: &str,
    stream: &[u8],
    retained: &[u8],
    truncated: bool,
    kept: &KeptOutput,
    mode: Retention,
    bound: usize,
    linux: bool,
) {
    let len = stream.len();
    let head = usize::try_from(kept.head_bytes).expect("head");
    let tail = usize::try_from(kept.tail_bytes).expect("tail");
    assert_eq!(kept.retention, mode, "{name}: the record's mode");
    assert_eq!(
        kept.observed_bytes,
        Some(len as u64),
        "{name}: observed_bytes is every byte the stream produced"
    );
    assert_eq!(
        truncated,
        len > bound,
        "{name}: *_truncated is true exactly when bytes were dropped"
    );
    assert!(
        retained.len() <= bound,
        "{name}: {} bytes held over the bound {bound}",
        retained.len()
    );
    if len <= bound {
        assert_eq!(
            retained, stream,
            "{name}: at or under the bound the whole stream"
        );
        assert_eq!(head + tail, len, "{name}: nothing omitted");
        let expected_head = match mode {
            Retention::Head => len,
            Retention::Tail => 0,
            Retention::HeadAndTail => len.min(bound / 2),
        };
        assert_eq!(
            (head, tail),
            (expected_head, len - expected_head),
            "{name}: the split under the bound"
        );
    } else {
        let (expected, expected_head, expected_tail) = match mode {
            Retention::Head => {
                let (bytes, _) = previous_head(stream, bound, linux);
                let marked = linux && bound >= MARKER.len();
                let head = if marked { bound - MARKER.len() } else { bound };
                (bytes, head, 0)
            }
            Retention::Tail => (stream[len - bound..].to_vec(), 0, bound),
            Retention::HeadAndTail => {
                let (head, tail) = (bound / 2, bound - bound / 2);
                let mut bytes = stream[..head].to_vec();
                bytes.extend_from_slice(&stream[len - tail..]);
                (bytes, head, tail)
            }
        };
        assert_eq!(
            (head, tail),
            (expected_head, expected_tail),
            "{name}: the split over the bound"
        );
        assert!(retained == expected.as_slice(), "{name}: the kept bytes");
        assert_eq!(
            len as u64 - kept.head_bytes - kept.tail_bytes,
            (len - expected_head - expected_tail) as u64,
            "{name}: the omitted count"
        );
    }
    if mode == Retention::Head {
        let (previous, previous_truncated) = previous_head(stream, bound, linux);
        assert!(
            retained == previous.as_slice() && truncated == previous_truncated,
            "{name}: Head is byte for byte what b7d677ef~1 kept"
        );
    }
}

/// Runs `/bin/sh -c 'cat <dir>/o; cat <dir>/e >&2'` and checks both streams.
struct Pair<'a> {
    stdout: (&'a [u8], Retention),
    stderr: (&'a [u8], Retention),
}

fn check_pair(name: &str, pair: &Pair<'_>, output: &RunOutput, bound: usize, linux: bool) {
    check_stream(
        &format!("{name} stdout {:?} {}B", pair.stdout.1, pair.stdout.0.len()),
        pair.stdout.0,
        &output.stdout,
        output.stdout_truncated,
        &output.stdout_kept,
        pair.stdout.1,
        bound,
        linux,
    );
    check_stream(
        &format!("{name} stderr {:?} {}B", pair.stderr.1, pair.stderr.0.len()),
        pair.stderr.0,
        &output.stderr,
        output.stderr_truncated,
        &output.stderr_kept,
        pair.stderr.1,
        bound,
        linux,
    );
}

/// For each size, three runs, each stream taking a different mode than the other, so every mode
/// meets every size on both streams and no run has the two streams in the same mode.
fn pairs_for(sizes: &[usize]) -> Vec<(usize, Retention, Retention)> {
    let mut runs = Vec::new();
    for &size in sizes {
        for index in 0..MODES.len() {
            runs.push((size, MODES[index], MODES[(index + 1) % MODES.len()]));
        }
    }
    runs
}

fn sh(script: &str) -> Command {
    Command {
        argv: vec!["/bin/sh".into(), "-c".into(), script.to_owned()],
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
    }
}

async fn run(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    command: &Command,
) -> RunOutput {
    let admitted = adapter.admit(profile).expect("admitted");
    let run = adapter.start(&admitted, command).await.expect("started");
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    assert_eq!(ended.exit.as_ref().and_then(|exit| exit.code), Some(0));
    adapter.output(&run).await.expect("output")
}

// ---------------------------------------------------------------------------------------------
// Case 1, portable: `local` at B - 1, B and B + 1
// ---------------------------------------------------------------------------------------------

fn local() -> Arc<dyn ConfinementAdapter> {
    confinement::open(AdapterRequest::Local(LocalRequest::default())).expect("local opens")
}

async fn local_at(sizes: &[usize]) {
    let bound = confinement::local::LOCAL_OUTPUT_LIMIT_BYTES;
    let adapter = local();
    let workspace = tempfile::tempdir().expect("workspace");
    for (index, (size, out_mode, err_mode)) in pairs_for(sizes).into_iter().enumerate() {
        let seed = index as u64;
        let stdout = pattern(size, 2 * seed + 1);
        let stderr = pattern(size, 2 * seed + 2);
        std::fs::write(workspace.path().join("o"), &stdout).expect("o");
        std::fs::write(workspace.path().join("e"), &stderr).expect("e");
        let output = run(
            &adapter,
            &ConfinementProfile {
                local_reason: Some(REASON.into()),
                workspace: Some(workspace.path().to_path_buf()),
                output_retention: OutputRetention {
                    stdout: out_mode,
                    stderr: err_mode,
                },
                ..ConfinementProfile::default()
            },
            &sh("cat o; cat e >&2"),
        )
        .await;
        check_pair(
            "local",
            &Pair {
                stdout: (&stdout, out_mode),
                stderr: (&stderr, err_mode),
            },
            &output,
            bound,
            false,
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_local_retention_one_under_the_bound() {
    local_at(&[confinement::local::LOCAL_OUTPUT_LIMIT_BYTES - 1]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_local_retention_exactly_the_bound() {
    local_at(&[confinement::local::LOCAL_OUTPUT_LIMIT_BYTES]).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_local_retention_one_over_the_bound() {
    local_at(&[confinement::local::LOCAL_OUTPUT_LIMIT_BYTES + 1]).await;
}

/// The default profile keeps the head of both streams and reports a `Head` record, so a
/// consumer that never sets `output_retention` reads what it read before.
#[tokio::test(flavor = "multi_thread")]
async fn adv_local_default_profile_is_head_on_both_streams() {
    let bound = confinement::local::LOCAL_OUTPUT_LIMIT_BYTES;
    let adapter = local();
    let workspace = tempfile::tempdir().expect("workspace");
    let stdout = pattern(bound + 7, 91);
    let stderr = pattern(5, 92);
    std::fs::write(workspace.path().join("o"), &stdout).expect("o");
    std::fs::write(workspace.path().join("e"), &stderr).expect("e");
    let output = run(
        &adapter,
        &ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            ..ConfinementProfile::default()
        },
        &sh("cat o; cat e >&2"),
    )
    .await;
    check_pair(
        "local default",
        &Pair {
            stdout: (&stdout, Retention::Head),
            stderr: (&stderr, Retention::Head),
        },
        &output,
        bound,
        false,
    );
}

// ---------------------------------------------------------------------------------------------
// Case 2, portable: workspace_access admission on `local`
// ---------------------------------------------------------------------------------------------

fn scoped(subtrees: &[&str]) -> WorkspaceAccess {
    WorkspaceAccess::Scoped {
        writable_subtrees: subtrees.iter().map(|path| (*path).to_owned()).collect(),
    }
}

/// Every spelling the brief names, and the neighbours that separate a component rule from a
/// substring rule. `local` serves no narrower workspace, so a list that passes both shared rules
/// is `exec.workspace-scoped-write-unserved` (501) there: that answer proves it was not refused
/// by shape or by `.git`.
fn admission_table() -> Vec<(Vec<&'static str>, &'static str, u16)> {
    let invalid = substrate_wire::EXEC_WORKSPACE_ACCESS_INVALID;
    let git = confinement::WRITABLE_SUBTREE_GIT;
    let unserved = substrate_wire::EXEC_WORKSPACE_SCOPED_WRITE_UNSERVED;
    vec![
        // Shape: each spelling is refused by the wire's rule before `.git` is looked at.
        (vec!["./.git"], invalid, 422),
        (vec![".git/"], invalid, 422),
        (vec!["a/../.git"], invalid, 422),
        (vec!["/.git"], invalid, 422),
        (vec![".git\\x"], invalid, 422),
        (vec![".git\0"], invalid, 422),
        (vec!["out", "out"], invalid, 422),
        (vec![".git", ".git"], invalid, 422),
        (vec!["a/.git", "a/.git"], invalid, 422),
        (vec![".git", ".git/hooks"], invalid, 422),
        // `.git` as a component, any ASCII case, even where the filesystem is case-sensitive.
        (vec![".git"], git, 422),
        (vec![".GIT"], git, 422),
        (vec![".gIt/objects"], git, 422),
        (vec!["vendor/.Git"], git, 422),
        // Not `.git`: the rule is a component rule, not a substring rule.
        (vec![".github"], unserved, 501),
        (vec![".git.d", "x.git"], unserved, 501),
        (vec![".gitmodules"], unserved, 501),
        (vec!["git"], unserved, 501),
    ]
}

fn assert_admission(
    adapter: &Arc<dyn ConfinementAdapter>,
    label: &str,
    local_answers: bool,
    workspace: Option<&Path>,
) {
    for (subtrees, code, status) in admission_table() {
        let profile = ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace: workspace.map(Path::to_path_buf),
            workspace_access: scoped(&subtrees),
            ..ConfinementProfile::default()
        };
        let answer = adapter.admit(&profile);
        if !local_answers && status == 501 {
            // `linux` on a host that proved scoped write admits what passes the shared rules.
            assert!(
                answer.is_ok(),
                "{label} {subtrees:?}: {:?}",
                answer.err().map(|error| error.code())
            );
            continue;
        }
        let error = answer
            .err()
            .unwrap_or_else(|| panic!("{label} {subtrees:?} was admitted"));
        assert_eq!(
            (error.code(), error.status()),
            (code, status),
            "{label} {subtrees:?}: {error}"
        );
    }
}

#[test]
fn adv_workspace_access_admission_spellings_on_local() {
    assert_admission(&local(), "local", true, None);
}

// ---------------------------------------------------------------------------------------------
// Delegated: the `linux` adapter
// ---------------------------------------------------------------------------------------------

fn delegated() -> Result<PathBuf, &'static str> {
    let root = PathBuf::from(
        std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")
            .ok_or("SUBSTRATE_VECTORS_CGROUP_ROOT is unset")?,
    );
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids")
        .map_err(|_| "the delegated cgroup root does not accept controllers")?;
    if !Path::new("/usr/bin/bwrap").is_file() {
        return Err("no bubblewrap at /usr/bin/bwrap");
    }
    Ok(root)
}

/// A `linux` adapter on the delegated root with one workspace, or `None` after printing ABSENT.
fn linux(name: &str) -> Option<(tempfile::TempDir, Arc<dyn ConfinementAdapter>, PathBuf)> {
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("{name}: ABSENT ({reason})");
            return None;
        }
    };
    let host = tempfile::tempdir().expect("host root");
    let config = HostConfig::minimum(host.path().join("workspaces"));
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .expect("linux opens");
    let workspace = host.path().join("workspaces").join("ws_adv29");
    std::fs::create_dir_all(&workspace).expect("workspace");
    Some((host, adapter, workspace))
}

async fn linux_at(name: &str, bound: usize, sizes: &[usize]) {
    let Some((_host, adapter, workspace)) = linux(name) else {
        return;
    };
    for (index, (size, out_mode, err_mode)) in pairs_for(sizes).into_iter().enumerate() {
        let seed = 100 + index as u64;
        let stdout = pattern(size, 2 * seed + 1);
        let stderr = pattern(size, 2 * seed + 2);
        std::fs::write(workspace.join("o"), &stdout).expect("o");
        std::fs::write(workspace.join("e"), &stderr).expect("e");
        let output = run(
            &adapter,
            &ConfinementProfile {
                workspace: Some(workspace.clone()),
                io_bounds: Some(IoBounds {
                    output_bytes: bound as u64,
                }),
                output_retention: OutputRetention {
                    stdout: out_mode,
                    stderr: err_mode,
                },
                ..ConfinementProfile::default()
            },
            &sh("cat /workspace/o; cat /workspace/e >&2"),
        )
        .await;
        check_pair(
            &format!("{name} bound {bound}"),
            &Pair {
                stdout: (&stdout, out_mode),
                stderr: (&stderr, err_mode),
            },
            &output,
            bound,
            true,
        );
    }
    println!("{name}: executed");
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_linux_retention_bound_one() {
    linux_at("adversary design29 linux bound 1", 1, &[0, 1, 2, 9000]).await;
}

/// 31 is odd and is the marker's own length: `Head` over it is the marker alone.
#[tokio::test(flavor = "multi_thread")]
async fn adv_linux_retention_odd_bound_at_the_marker_length() {
    linux_at("adversary design29 linux bound 31", 31, &[30, 31, 32]).await;
}

/// An odd bound over several pipe reads.
#[tokio::test(flavor = "multi_thread")]
async fn adv_linux_retention_odd_bound_4097() {
    linux_at(
        "adversary design29 linux bound 4097",
        4097,
        &[4096, 4097, 4098, 30_001],
    )
    .await;
}

/// Case 2 on `linux`: the same shared answers, and what passes them is admitted.
#[tokio::test(flavor = "multi_thread")]
async fn adv_linux_workspace_access_admission_spellings() {
    const NAME: &str = "adversary design29 linux admission";
    let Some((_host, adapter, workspace)) = linux(NAME) else {
        return;
    };
    assert_admission(&adapter, "linux", false, Some(&workspace));
    println!("{NAME}: executed");
}

/// Case 2 at start: a subtree the host cannot bind is refused by its documented
/// `exec.workspace-subtree-*` name (422), audited, before anything runs.
#[tokio::test(flavor = "multi_thread")]
async fn adv_linux_subtree_refusals_at_start() {
    const NAME: &str = "adversary design29 linux subtree refusals";
    let Some((host, adapter, workspace)) = linux(NAME) else {
        return;
    };
    let outside = host.path().join("outside");
    std::fs::create_dir_all(&outside).expect("outside");
    std::fs::create_dir_all(workspace.join("real/x")).expect("real/x");
    std::fs::create_dir_all(workspace.join("a")).expect("a");
    std::fs::write(workspace.join("file"), "f").expect("file");
    std::os::unix::fs::symlink("real", workspace.join("lnk")).expect("lnk");
    std::os::unix::fs::symlink(&outside, workspace.join("abs")).expect("abs");
    std::os::unix::fs::symlink("missing-target", workspace.join("dangling")).expect("dangling");
    let absent = substrate_wire::EXEC_WORKSPACE_SUBTREE_ABSENT;
    let symlink = substrate_wire::EXEC_WORKSPACE_SUBTREE_SYMLINK;
    let not_directory = substrate_wire::EXEC_WORKSPACE_SUBTREE_NOT_DIRECTORY;
    let cases: [(&str, &str); 9] = [
        ("missing", absent),
        ("a/missing", absent),
        ("missing/deeper", absent),
        ("file", not_directory),
        ("lnk", symlink),
        ("lnk/x", symlink),
        ("abs", symlink),
        ("dangling", symlink),
        // A regular file in the middle of the path: not a directory, and no symlink anywhere.
        ("file/x", not_directory),
    ];
    let before = snapshot(&workspace, &[]);
    let outside_before = snapshot(&outside, &[]);
    let mut wrong = Vec::new();
    for (subtree, code) in cases {
        let records = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = Arc::clone(&records);
        let admitted = adapter
            .admit(&ConfinementProfile {
                workspace: Some(workspace.clone()),
                workspace_access: scoped(&[subtree]),
                audit: Some(confinement::AuditSink::new(
                    move |record: &confinement::AuditRecord| {
                        sink.lock()
                            .push(serde_json::to_value(record).expect("record"));
                    },
                )),
                ..ConfinementProfile::default()
            })
            .expect("admission opens no workspace");
        let error = adapter
            .start(
                &admitted,
                &sh(&format!("printf x > /workspace/{subtree}/w")),
            )
            .await
            .expect_err("refused at start");
        let message = match &error {
            ConfinementError::Driver(driver) => driver.message.clone(),
            other => other.to_string(),
        };
        if error.code() != code || error.status() != 422 {
            wrong.push(format!(
                "{subtree}: expected {code} 422, got {} {} ({message})",
                error.code(),
                error.status()
            ));
            continue;
        }
        let records = records.lock();
        assert_eq!(records.len(), 2, "{subtree}: {records:?}");
        assert_eq!(records[1]["event"], "profile_refused", "{subtree}");
        assert_eq!(records[1]["code"], code, "{subtree}");
    }
    assert_eq!(snapshot(&workspace, &[]), before, "the workspace");
    assert_eq!(snapshot(&outside, &[]), outside_before, "outside");
    assert!(wrong.is_empty(), "wrong refusals:\n{}", wrong.join("\n"));
    println!("{NAME}: executed");
}

// ---------------------------------------------------------------------------------------------
// Case 3, delegated: scoped write conformance
// ---------------------------------------------------------------------------------------------

/// One entry of a host tree: kind, permission bits, size, mtime, and the bytes or link target.
type Entry = (char, u32, u64, i64, i64, Vec<u8>);

/// Every entry beneath `root` except those at or beneath a path in `skip`, read without
/// following any link.
fn snapshot(root: &Path, skip: &[&str]) -> BTreeMap<PathBuf, Entry> {
    fn walk(root: &Path, dir: &Path, skip: &[&str], out: &mut BTreeMap<PathBuf, Entry>) {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .expect("read_dir")
            .map(|entry| entry.expect("entry").path())
            .collect();
        names.sort();
        for path in names {
            let relative = path.strip_prefix(root).expect("beneath").to_path_buf();
            if skip.iter().any(|prefix| relative.starts_with(prefix)) {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path).expect("metadata");
            let kind = meta.file_type();
            let (tag, bytes) = if kind.is_dir() {
                ('d', Vec::new())
            } else if kind.is_symlink() {
                (
                    'l',
                    std::fs::read_link(&path)
                        .expect("link")
                        .into_os_string()
                        .into_encoded_bytes(),
                )
            } else if kind.is_file() {
                ('f', std::fs::read(&path).expect("file"))
            } else {
                ('o', Vec::new())
            };
            out.insert(
                relative,
                (
                    tag,
                    meta.permissions().mode(),
                    if tag == 'd' { 0 } else { meta.size() },
                    meta.mtime(),
                    meta.mtime_nsec(),
                    bytes,
                ),
            );
            if tag == 'd' {
                walk(root, &path, skip, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, skip, &mut out);
    out
}

/// The layout every conformance run starts from: two declared subtrees (`a/b`, `c`), their
/// undeclared neighbours, a file at the top, the workspace's own `.git`, and links made on the
/// host inside `c` that lead out of it.
fn conformance_workspace(workspace: &Path, outside: &Path) {
    for dir in ["a/b", "a/x", "c", "d", ".git"] {
        std::fs::create_dir_all(workspace.join(dir)).expect("dir");
    }
    std::fs::write(workspace.join("top"), "top").expect("top");
    std::fs::write(workspace.join("a/keep"), "keep").expect("keep");
    std::fs::write(workspace.join("a/b/mv_src"), "m").expect("mv_src");
    std::fs::write(workspace.join(".git/config"), "[core]\n").expect("config");
    std::fs::create_dir_all(outside).expect("outside");
    std::fs::write(outside.join("o"), "o").expect("outside/o");
    std::os::unix::fs::symlink("../d", workspace.join("c/pre_rel")).expect("pre_rel");
    std::os::unix::fs::symlink("/workspace/d", workspace.join("c/pre_abs")).expect("pre_abs");
    std::os::unix::fs::symlink("..", workspace.join("c/pre_dotdot")).expect("pre_dotdot");
    std::os::unix::fs::symlink(outside, workspace.join("c/pre_host")).expect("pre_host");
}

/// Each attempt prints `<label>: done` or `<label>: refused`.
const CONFORMANCE_SCRIPT: &str = r#"cd /workspace
t() { l="$1"; shift; if "$@" >/dev/null 2>&1; then echo "$l: done"; else echo "$l: refused"; fi; }
t in_b sh -c 'printf ok > a/b/f'
t in_c sh -c 'printf ok > c/f'
t a_f sh -c 'printf x > a/f'
t a_x_f sh -c 'printf x > a/x/f'
t d_f sh -c 'printf x > d/f'
t top_new sh -c 'printf x > f'
t top_append sh -c 'printf x >> top'
t top_trunc sh -c ': > top'
t top_chmod chmod 0777 top
t top_touch touch -d @0 top
t top_rm rm -f top
t a_keep_rm rm -f a/keep
t a_mkdir mkdir a/new
t a_fifo mkfifo a/fifo
t a_symlink ln -s /workspace/c a/sl
t mv_b_to_d mv a/b/mv_src d/moved
t mv_b_to_a mv a/b/mv_src a/moved
t mv_c_to_top mv c/f moved_top
t mv_subtree_b mv a/b a/b2
t mv_subtree_c mv c c2
t ln_b_to_d ln a/b/f d/hard
t run_link sh -c 'ln -s /workspace/d c/run_link && printf x > c/run_link/f'
t pre_rel sh -c 'printf x > c/pre_rel/f'
t pre_abs sh -c 'printf x > c/pre_abs/f'
t pre_dotdot_top sh -c 'printf x >> c/pre_dotdot/top'
t pre_dotdot_new sh -c 'printf x > c/pre_dotdot/g'
t pre_host sh -c 'printf x > c/pre_host/o'
t git sh -c 'printf x >> .git/config'
"#;

const CONFORMANCE_LABELS: [&str; 28] = [
    "in_b",
    "in_c",
    "a_f",
    "a_x_f",
    "d_f",
    "top_new",
    "top_append",
    "top_trunc",
    "top_chmod",
    "top_touch",
    "top_rm",
    "a_keep_rm",
    "a_mkdir",
    "a_fifo",
    "a_symlink",
    "mv_b_to_d",
    "mv_b_to_a",
    "mv_c_to_top",
    "mv_subtree_b",
    "mv_subtree_c",
    "ln_b_to_d",
    "run_link",
    "pre_rel",
    "pre_abs",
    "pre_dotdot_top",
    "pre_dotdot_new",
    "pre_host",
    "git",
];

fn expected_lines(done: &[&str]) -> String {
    let mut lines = String::new();
    for label in CONFORMANCE_LABELS {
        let verdict = if done.contains(&label) {
            "done"
        } else {
            "refused"
        };
        lines.push_str(label);
        lines.push_str(": ");
        lines.push_str(verdict);
        lines.push('\n');
    }
    lines
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_linux_scoped_write_conformance() {
    const NAME: &str = "adversary design29 linux scoped conformance";
    let Some((host, adapter, workspace)) = linux(NAME) else {
        return;
    };
    let outside = host.path().join("outside");
    conformance_workspace(&workspace, &outside);
    let before = snapshot(&workspace, &["a/b", "c"]);
    let c_links_before = snapshot(&workspace.join("c"), &[]);
    let outside_before = snapshot(&outside, &[]);
    let output = run(
        &adapter,
        &ConfinementProfile {
            workspace: Some(workspace.clone()),
            workspace_access: scoped(&["a/b", "c"]),
            ..ConfinementProfile::default()
        },
        &sh(CONFORMANCE_SCRIPT),
    )
    .await;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected_lines(&["in_b", "in_c"]),
        "{NAME}: only the two declared subtrees take a write; stderr {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        snapshot(&workspace, &["a/b", "c"]),
        before,
        "{NAME}: the host workspace outside the subtrees"
    );
    assert_eq!(
        snapshot(&outside, &[]),
        outside_before,
        "{NAME}: the host directory a link inside c names"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("a/b/f")).expect("a/b/f"),
        "ok"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("a/b/mv_src")).expect("mv_src kept"),
        "m"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("c/f")).expect("c/f"),
        "ok"
    );
    let mut c_links_after = snapshot(&workspace.join("c"), &[]);
    c_links_after.remove(Path::new("f"));
    let run_link = c_links_after
        .remove(Path::new("run_link"))
        .expect("the run's own link inside c");
    assert_eq!(run_link.0, 'l');
    assert_eq!(run_link.5, b"/workspace/d");
    assert_eq!(
        c_links_after, c_links_before,
        "{NAME}: the host's links in c"
    );
    println!("{NAME}: executed");
}

/// `ReadOnly`: nothing in the workspace takes a write, subtree or not.
#[tokio::test(flavor = "multi_thread")]
async fn adv_linux_read_only_workspace_conformance() {
    const NAME: &str = "adversary design29 linux read-only conformance";
    let Some((host, adapter, workspace)) = linux(NAME) else {
        return;
    };
    let outside = host.path().join("outside");
    conformance_workspace(&workspace, &outside);
    let before = snapshot(&workspace, &[]);
    let output = run(
        &adapter,
        &ConfinementProfile {
            workspace: Some(workspace.clone()),
            workspace_access: WorkspaceAccess::ReadOnly,
            ..ConfinementProfile::default()
        },
        &sh(CONFORMANCE_SCRIPT),
    )
    .await;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected_lines(&[]),
        "{NAME}: stderr {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(snapshot(&workspace, &[]), before, "{NAME}: the workspace");
    println!("{NAME}: executed");
}

/// Design 29 § 6, as corrected after this pass: `linux` reports its output when the run ends, so
/// `output()` while it runs holds no bytes. The run prints, then says so through a host file, then
/// waits for the host's go-ahead, so `output()` is read while the run is alive and after its bytes
/// were written, with no timing guess.
#[tokio::test(flavor = "multi_thread")]
async fn adv_linux_output_while_running_is_the_current_snapshot() {
    const NAME: &str = "adversary design29 linux output while running";
    let Some((_host, adapter, workspace)) = linux(NAME) else {
        return;
    };
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.clone()),
            output_retention: OutputRetention {
                stdout: Retention::Tail,
                stderr: Retention::Head,
            },
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let run_id = adapter
        .start(
            &admitted,
            &sh("printf abc; : > /workspace/printed; \
                 i=0; while [ ! -e /workspace/go ] && [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done"),
        )
        .await
        .expect("started");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !workspace.join("printed").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "{NAME}: never printed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let during = adapter.output(&run_id).await.expect("output while running");
    std::fs::write(workspace.join("go"), "").expect("go");
    let ended = adapter.wait(&run_id).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    let after = adapter.output(&run_id).await.expect("output");
    assert_eq!(after.stdout, b"abc", "{NAME}: after the run");
    // story:output-keeps-tail-within-bound — design 29 § 6 was corrected after this pass: `linux`
    // reports its output when the run ends, as before this change, so mid-run it holds no bytes.
    assert_eq!(
        during.stdout, b"",
        "{NAME}: linux reports output when the run ends; record {:?}",
        during.stdout_kept
    );
    println!("{NAME}: executed");
}

// ---------------------------------------------------------------------------------------------
// Render parity for a scoped workspace
// ---------------------------------------------------------------------------------------------

/// `ConfinementAdapter::render` is "the program and arguments this adapter would run" and the
/// driver's `render` says the workspace is "pinned and refused exactly as a start pins and
/// refuses" it. Before design 29 the `linux` adapter always sent `ReadWrite`, so no caller reached
/// the render of a subtree; now `render` and `start` must answer the same subtree with the same
/// documented name (design 29 § 1: `exec.workspace-subtree-absent`, `-symlink`,
/// `-not-directory`).
#[tokio::test(flavor = "multi_thread")]
async fn adv_linux_render_refuses_a_subtree_as_start_does() {
    const NAME: &str = "adversary design29 linux render parity";
    let Some((_host, adapter, workspace)) = linux(NAME) else {
        return;
    };
    std::fs::create_dir_all(workspace.join("real")).expect("real");
    std::fs::create_dir_all(workspace.join("out")).expect("out");
    std::fs::write(workspace.join("file"), "f").expect("file");
    std::os::unix::fs::symlink("real", workspace.join("lnk")).expect("lnk");
    let command = sh("true");
    let mut differences = Vec::new();
    for subtree in ["missing", "file", "lnk"] {
        let admitted = adapter
            .admit(&ConfinementProfile {
                workspace: Some(workspace.clone()),
                workspace_access: scoped(&[subtree]),
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        let started = adapter
            .start(&admitted, &command)
            .await
            .expect_err("start refuses");
        let rendered = adapter
            .render(&admitted, &command)
            .expect_err("render refuses");
        if (rendered.code(), rendered.status()) != (started.code(), started.status()) {
            differences.push(format!(
                "{subtree}: start {} {}, render {} {} ({rendered})",
                started.code(),
                started.status(),
                rendered.code(),
                rendered.status()
            ));
        }
    }
    // The served shape renders the binds the start makes: the root read-only, the subtree over it.
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace.clone()),
            workspace_access: scoped(&["out"]),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let rendered = adapter.render(&admitted, &command).expect("rendered");
    let args = &rendered.args;
    let bind = |flag: &str, target: &str| {
        args.windows(3)
            .any(|window| window[0] == flag && window[2] == target)
    };
    assert!(
        bind("--ro-bind-fd", "/workspace") && bind("--bind-fd", "/workspace/out"),
        "{NAME}: {args:?}"
    );
    assert!(!bind("--bind-fd", "/workspace"), "{NAME}: {args:?}");
    assert!(
        differences.is_empty(),
        "{NAME}: render and start disagree:\n{}",
        differences.join("\n")
    );
    println!("{NAME}: executed");
}
