//! Security review of `b10x-substrate confine run` (`story:sandbox-repository-parity`).
//!
//! Each case spawns the shipped binary and asserts only on what it prints, the status it exits
//! with, and what it left on the host. The delegated cases need the delegated cgroup v2 root
//! `SUBSTRATE_VECTORS_CGROUP_ROOT` names (`scripts/delegated-lane.sh`); without it they print
//! that they are **absent** and assert nothing.

#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const PINNED: &str =
    "registry.invalid/run@sha256:0000000000000000000000000000000000000000000000000000000000000000";
const TAGGED: &str = "docker.io/library/alpine:3.20";
const REASON: &str = "the security review runs a host process on purpose";
/// `story:sandbox-repository-parity`, security F1: the verb reads its own cgroup root variable and
/// never the daemon's `SUBSTRATE_CGROUP_ROOT`, so the delegated root is handed over under this
/// name.
const ROOT_ENV: &str = "SUBSTRATE_CONFINE_CGROUP_ROOT";

/// Delegated cases share one cgroup root; each opens its own host driver on it.
static DELEGATED: Mutex<()> = Mutex::new(());

struct Host {
    _directory: tempfile::TempDir,
    root: PathBuf,
}

impl Host {
    fn workspace(&self, name: &str) -> PathBuf {
        let workspace = self.root.join(name);
        std::fs::create_dir(&workspace).expect("workspace");
        workspace
    }

    /// A directory this host owns, for `TMPDIR`.
    fn temp(&self) -> PathBuf {
        let temp = self.root.join("tmpdir");
        std::fs::create_dir(&temp).expect("tmpdir");
        temp
    }
}

fn host() -> Host {
    let directory = tempfile::Builder::new()
        .prefix("confine-review-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("host directory");
    let root = std::fs::canonicalize(directory.path()).expect("canonical");
    Host {
        _directory: directory,
        root,
    }
}

fn cli() -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_b10x-substrate"));
    command
        .env_remove("SUBSTRATE_SOCKET")
        .env_remove("SUBSTRATE_CGROUP_ROOT")
        .env_remove(ROOT_ENV)
        .env_remove("SUBSTRATE_CONFINEMENT_LOCAL")
        .stdin(Stdio::null());
    command
}

fn run(command: &mut std::process::Command) -> Output {
    command.output().expect("the CLI runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn path(path: &Path) -> &str {
    path.to_str().expect("UTF-8")
}

fn state_dirs(temp: &Path) -> Vec<String> {
    std::fs::read_dir(temp)
        .expect("TMPDIR")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("b10x-substrate-confine-"))
        .collect()
}

fn delegated() -> Option<PathBuf> {
    std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT").map(PathBuf::from)
}

// --- `--image`: a tagged image is refused before anything runs, whatever the backend --------

/// The brief and CHANGELOG: "`--image` takes a sha256 digest only". On `--backend linux` a tag is
/// accepted and the image silently dropped: the render is the host's bubblewrap argv.
#[test]
fn review_a_tagged_image_on_the_linux_backend_exits_2_unpinned() {
    let host = host();
    let ws = host.workspace("ws_review");
    let output = run(cli().args([
        "confine",
        "run",
        "--backend",
        "linux",
        "--image",
        TAGGED,
        "--dry-run",
        "--dir",
        path(&ws),
        "--",
        "/bin/true",
    ]));
    let stderr = text(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(2),
        "a tagged --image on linux was not refused; stdout: {} stderr: {stderr}",
        text(&output.stdout)
    );
    // `story:sandbox-repository-parity`, security F2: only `oci` runs an image, so any `--image`
    // on linux is refused as the fact linux does not prove, before the tag is looked at.
    assert!(
        stderr.contains("confinement.fact-unserved: image_digest_pinned"),
        "stderr: {stderr}"
    );
}

/// `--local` skips the CLI's own digest check (`confine.rs:155`), and `local` proves no
/// `image_digest_pinned`, so a tagged image runs the command on the host.
#[test]
fn review_a_tagged_image_with_local_exits_2_before_anything_runs() {
    let host = host();
    let ws = host.workspace("ws_review");
    let output = run(cli().args([
        "confine",
        "run",
        "--net",
        "--local",
        REASON,
        "--image",
        TAGGED,
        "--dir",
        path(&ws),
        "--",
        "/bin/sh",
        "-c",
        "echo ran > ran",
    ]));
    let stderr = text(&output.stderr);
    assert!(
        !ws.join("ran").exists(),
        "a tagged --image under --local ran the command; exit {:?} stderr: {stderr}",
        output.status.code()
    );
    assert_eq!(output.status.code(), Some(2), "stderr: {stderr}");
}

/// A digest-pinned image on `--backend linux` is not run, and nothing says so: the run would use
/// the host's own `/usr`. A field the adapter cannot honour is a named refusal (invariant 3).
#[test]
fn review_a_pinned_image_on_the_linux_backend_is_refused_not_dropped() {
    let host = host();
    let ws = host.workspace("ws_review");
    let output = run(cli().args([
        "confine",
        "run",
        "--backend",
        "linux",
        "--image",
        PINNED,
        "--dry-run",
        "--dir",
        path(&ws),
        "--",
        "/bin/true",
    ]));
    assert_eq!(
        output.status.code(),
        Some(2),
        "--image was dropped on linux; stdout: {} stderr: {}",
        text(&output.stdout),
        text(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
}

// --- `--local`: the only unconfined path, reasoned and policy-bound ---------------------------

#[test]
fn review_local_under_a_host_policy_that_forbids_it_exits_2_before_anything_runs() {
    let host = host();
    let ws = host.workspace("ws_review");
    for policy in ["deny", "", "Permit"] {
        let output = run(cli().env("SUBSTRATE_CONFINEMENT_LOCAL", policy).args([
            "confine",
            "run",
            "--net",
            "--local",
            REASON,
            "--dir",
            path(&ws),
            "--",
            "/bin/sh",
            "-c",
            "echo ran > ran",
        ]));
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{policy:?}: {stderr}");
        assert!(
            stderr.contains("confinement.local-refused"),
            "{policy:?}: {stderr}"
        );
        assert!(!ws.join("ran").exists(), "{policy:?}: the command ran");
    }
}

#[test]
fn review_local_reasons_without_a_visible_character_exit_2() {
    let host = host();
    let ws = host.workspace("ws_review");
    for reason in ["", "\t", "\u{200b}", "\u{3164}", "\n\n", "-"] {
        let output = run(cli().args([
            "confine",
            "run",
            "--net",
            "--local",
            reason,
            "--dir",
            path(&ws),
            "--",
            "/bin/sh",
            "-c",
            "echo ran > ran",
        ]));
        let stderr = text(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{reason:?}: {stderr}");
        assert!(
            stderr.contains("confinement.local-reason-missing"),
            "{reason:?}: {stderr}"
        );
        assert!(!ws.join("ran").exists(), "{reason:?}: the command ran");
    }
}

#[test]
fn review_local_reason_is_in_the_json_result() {
    let host = host();
    let ws = host.workspace("ws_review");
    for dry_run in [false, true] {
        let mut args = vec![
            "--output", "json", "confine", "run", "--net", "--local", REASON,
        ];
        if dry_run {
            args.push("--dry-run");
        }
        args.extend(["--dir", path(&ws), "--", "/bin/true"]);
        let output = run(cli().args(&args));
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
        let envelope: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("one JSON result");
        assert_eq!(envelope["result"]["adapter"], "local", "{envelope}");
        assert_eq!(
            envelope["result"]["declared_insecure"], REASON,
            "{envelope}"
        );
    }
}

// --- The child's environment ------------------------------------------------------------------

const CREDENTIALS: &[(&str, &str)] = &[
    ("GITHUB_TOKEN", "review-sekrit-1"),
    ("AWS_SECRET_ACCESS_KEY", "review-sekrit-2"),
    ("SSH_AUTH_SOCK", "/review-sekrit-3"),
    ("ANTHROPIC_API_KEY", "review-sekrit-4"),
    ("HOME", "/review-sekrit-6"),
];

fn assert_clean_environment(output: &Output) {
    let stdout = text(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {stdout} stderr: {}",
        text(&output.stderr)
    );
    for (name, value) in CREDENTIALS {
        assert!(
            !stdout.contains(value),
            "{name}'s value reached the child:\n{stdout}"
        );
    }
    assert!(stdout.contains("PATH="), "{stdout}");
}

#[test]
fn review_local_child_sees_no_variable_of_the_cli() {
    let host = host();
    let ws = host.workspace("ws_review");
    let mut command = cli();
    command.envs(CREDENTIALS.iter().copied()).args([
        "confine",
        "run",
        "--net",
        "--local",
        REASON,
        "--dir",
        path(&ws),
        "--",
        "/usr/bin/env",
    ]);
    assert_clean_environment(&run(&mut command));
}

// --- The private state directory --------------------------------------------------------------

#[test]
fn review_the_private_state_directory_is_owner_private_and_removed() {
    let host = host();
    let ws = host.workspace("ws_review");
    let temp = host.temp();
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o777)).expect("mode");
    let probe = format!("stat -c %a {}/b10x-substrate-confine-*", path(&temp));
    let output = run(cli().env("TMPDIR", &temp).args([
        "confine",
        "run",
        "--net",
        "--local",
        REASON,
        "--dir",
        path(&ws),
        "--",
        "/bin/sh",
        "-c",
        &probe,
    ]));
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout).trim(), "700");
    assert!(state_dirs(&temp).is_empty(), "{:?}", state_dirs(&temp));
    // A refusal leaves nothing either.
    let output = run(cli().env("TMPDIR", &temp).args([
        "confine",
        "run",
        "--backend",
        "linux",
        "--net",
        "--dir",
        path(&ws),
        "--",
        "/bin/true",
    ]));
    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    assert!(state_dirs(&temp).is_empty(), "{:?}", state_dirs(&temp));
}

/// `confine.rs:165` relies on `TempDir`'s `Drop`, which does not run when the verb is ended by
/// a signal, so an interrupted run leaves its state directory in `TMPDIR`.
#[test]
fn review_a_terminated_run_removes_its_private_state_directory() {
    let host = host();
    let ws = host.workspace("ws_review");
    let temp = host.temp();
    let mut child = cli()
        .env("TMPDIR", &temp)
        .args([
            "confine",
            "run",
            "--net",
            "--local",
            REASON,
            "--dir",
            path(&ws),
            "--",
            "/bin/sleep",
            "2",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let deadline = Instant::now() + Duration::from_secs(10);
    while state_dirs(&temp).is_empty() {
        assert!(Instant::now() < deadline, "no state directory appeared");
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(status.success());
    let _ = child.wait();
    std::thread::sleep(Duration::from_millis(2_500));
    assert!(
        state_dirs(&temp).is_empty(),
        "an interrupted run left {:?} in TMPDIR",
        state_dirs(&temp)
    );
}

// --- Delegated: SUBSTRATE_CGROUP_ROOT ---------------------------------------------------------

/// Every `substrate-ex_*` group beneath `root`, at any depth, by path relative to `root`.
/// `story:sandbox-repository-parity`, security F1: each invocation's run groups sit in its own
/// private group beneath the root, no longer directly in it.
fn exec_cgroups(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, found: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.filter_map(Result::ok) {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let path = entry.path();
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("substrate-ex_")
            {
                found.push(
                    path.strip_prefix(root)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            walk(root, &path, found);
        }
    }
    let mut found = Vec::new();
    walk(root, root, &mut found);
    found
}

/// Every `confine run` opens its own host driver on `SUBSTRATE_CGROUP_ROOT`, and
/// `ProcessRuntime::new_with_quotas` kills every `substrate-ex_*` cgroup beneath it as an
/// orphan (`crates/substrate-host/src/process.rs:483`). A second `confine run` sharing the root,
/// or one sharing the daemon's own `SUBSTRATE_CGROUP_ROOT`, kills runs it does not own.
#[test]
fn review_delegated_a_second_run_does_not_kill_a_first_in_the_same_cgroup_root() {
    let Some(root) = delegated() else {
        println!("security review confine concurrent cgroup root: absent");
        return;
    };
    let _guard = DELEGATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let host = host();
    let ws_a = host.workspace("ws_first");
    let ws_b = host.workspace("ws_second");
    let before = exec_cgroups(&root);
    let first = cli()
        .env(ROOT_ENV, &root)
        .args([
            "confine",
            "run",
            "--dir",
            path(&ws_a),
            "--",
            "/bin/sh",
            "-c",
            "sleep 3; echo survived > survived",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the first run");
    let mut first = Some(first);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if exec_cgroups(&root)
            .iter()
            .any(|name| !before.contains(name))
        {
            break;
        }
        if let Some(status) = first
            .as_mut()
            .and_then(|child| child.try_wait().ok().flatten())
        {
            let output = first
                .take()
                .expect("child")
                .wait_with_output()
                .expect("output");
            panic!(
                "the first run ended before its cgroup appeared: {status:?} stderr: {}",
                text(&output.stderr)
            );
        }
        assert!(Instant::now() < deadline, "the first run never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    let second = run(cli().env(ROOT_ENV, &root).args([
        "confine",
        "run",
        "--dir",
        path(&ws_b),
        "--",
        "/bin/true",
    ]));
    let first = first
        .take()
        .expect("child")
        .wait_with_output()
        .expect("output");
    let survived = ws_a.join("survived").exists();
    assert!(
        first.status.code() == Some(0) && survived && second.status.code() == Some(0),
        "two confine runs on one SUBSTRATE_CGROUP_ROOT:\n first: exit {:?}, survived {survived}, \
         stderr {:?}\n second: exit {:?}, stderr {:?}",
        first.status.code(),
        text(&first.stderr),
        second.status.code(),
        text(&second.stderr)
    );
    println!("security review confine concurrent cgroup root: executed");
}

/// The binding's envelope (`lib.rs:7-10`, `spec/client/cli.yaml`): under `--output json` a
/// failure is one `{"ok":false,…}` on stderr. On a host where `linux` proves its families, the
/// host driver's open-time probe (`crates/substrate-host/src/probe.rs:259`) writes its shell's
/// own errors to the inherited stderr first.
#[test]
fn review_delegated_a_json_refusal_is_the_only_thing_on_stderr() {
    let Some(root) = delegated() else {
        println!("security review confine json stderr: absent");
        return;
    };
    let _guard = DELEGATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let host = host();
    let ws = host.workspace("ws_review");
    let output = run(cli().env(ROOT_ENV, &root).args([
        "--output",
        "json",
        "confine",
        "run",
        "--net",
        "--dir",
        path(&ws),
        "--",
        "/bin/true",
    ]));
    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    let envelope: Result<serde_json::Value, _> = serde_json::from_slice(&output.stderr);
    assert!(
        envelope.is_ok(),
        "stderr is not one JSON envelope: {:?}",
        text(&output.stderr)
    );
    println!("security review confine json stderr: executed");
}

#[test]
fn review_delegated_linux_child_sees_no_variable_of_the_cli() {
    let Some(root) = delegated() else {
        println!("security review confine linux env: absent");
        return;
    };
    let _guard = DELEGATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let host = host();
    let ws = host.workspace("ws_review");
    let mut command = cli();
    command
        .env(ROOT_ENV, &root)
        .envs(CREDENTIALS.iter().copied())
        .args(["confine", "run", "--dir", path(&ws), "--", "/usr/bin/env"]);
    assert_clean_environment(&run(&mut command));
    println!("security review confine linux env: executed");
}

/// `--ro` reaches only the declared path, read-only, and an argv that looks like a bubblewrap
/// option stays the command.
#[test]
fn review_delegated_ro_and_argv_stay_inside_the_declared_set() {
    let Some(root) = delegated() else {
        println!("security review confine ro and argv: absent");
        return;
    };
    let _guard = DELEGATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let host = host();
    let ws = host.workspace("ws_review");
    let declared = host.root.join("declared");
    std::fs::create_dir(&declared).expect("declared");
    std::fs::write(declared.join("in"), "in").expect("in");
    std::fs::write(host.root.join("beside"), "beside").expect("beside");
    let script = format!(
        "cat {d}/in; echo x > {d}/in 2>/dev/null && echo WROTE; \
         test -e {r}/beside && echo BESIDE; test -e {r}/ws_review && echo PARENT; true",
        d = path(&declared),
        r = path(&host.root)
    );
    let output = run(cli().env(ROOT_ENV, &root).args([
        "confine",
        "run",
        "--ro",
        path(&declared),
        "--dir",
        path(&ws),
        "--",
        "/bin/sh",
        "-c",
        &script,
    ]));
    let stdout = text(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(stdout.trim(), "in", "{stdout}");
    assert_eq!(
        std::fs::read_to_string(declared.join("in")).expect("in"),
        "in"
    );
    // An argv that reads as a bubblewrap option is the program bubblewrap is asked to run.
    let output = run(cli().env(ROOT_ENV, &root).args([
        "confine",
        "run",
        "--dir",
        path(&ws),
        "--",
        "--bind",
        "/",
        "/host",
        "/bin/sh",
        "-c",
        "test -e /host/etc/passwd && echo HOST",
    ]));
    assert!(
        !text(&output.stdout).contains("HOST") && output.status.code() != Some(0),
        "argv became a bubblewrap option: {:?} {}",
        output.status.code(),
        text(&output.stdout)
    );
    println!("security review confine ro and argv: executed");
}

// --- The reaper after findings F1 and R1 (`story:confine-run-reaps-killed-groups`) -----------

/// Security review of the reaper after F1 and R1: a zombie owner counts as gone, and what the
/// reaper leaves is named, on stderr under text output and under `--output json` inside the verb's
/// one document as `left_groups`.
///
/// The change is safe because of one fact: counting a zombie owner as gone only lets the reaper
/// reach its `holds_process` check, and a group that still holds a live process is never removed.
/// The first case measures that fact for a zombie owner. The other two measure the one-document
/// rule on the paths the change's own case does not take, a refusal after the reaper ran and an
/// interrupted run, where `left_groups` belongs in `error.data`.
mod reaper_review {
    use std::path::{Path, PathBuf};
    use std::process::Output;
    use std::time::{Duration, Instant};

    use super::{DELEGATED, ROOT_ENV, cli, host, path, text};

    fn delegated(case: &str) -> Option<PathBuf> {
        let root = super::delegated();
        if root.is_none() {
            println!(
                "security review confine reaper {case}: ABSENT (SUBSTRATE_VECTORS_CGROUP_ROOT is \
                 unset)"
            );
        }
        root
    }

    /// The state (field 3) and start time (field 22) of `/proc/<pid>/stat`.
    fn stat_fields(pid: u32) -> Option<(char, u64)> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
        let state = fields.next()?.chars().next()?;
        Some((state, fields.nth(18)?.parse().ok()?))
    }

    fn procs(group: &Path) -> Vec<String> {
        std::fs::read_to_string(group.join("cgroup.procs"))
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// A `confine-<pid>-<start time>` group whose owner is a zombie this case has not waited for,
    /// holding a second process this case started and moved into it.
    struct ZombieOwnersGroup {
        group: PathBuf,
        owner: std::process::Child,
        holder: std::process::Child,
    }

    impl ZombieOwnersGroup {
        fn new(root: &Path) -> Self {
            let mut owner = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("the owner");
            let pid = owner.id();
            let (_, start) = stat_fields(pid).expect("the owner's start time");
            // `Child::kill` sends SIGKILL and does not wait: the owner stays a zombie.
            owner.kill().expect("end the owner");
            let deadline = Instant::now() + Duration::from_secs(5);
            while stat_fields(pid).map(|(state, _)| state) != Some('Z') {
                assert!(Instant::now() < deadline, "the owner never became a zombie");
                std::thread::sleep(Duration::from_millis(5));
            }
            let group = root.join(format!("confine-{pid}-{start}"));
            std::fs::create_dir(&group).expect("the zombie owner's group");
            let holder = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("the held process");
            std::fs::write(group.join("cgroup.procs"), holder.id().to_string())
                .expect("move the held process into the group");
            Self {
                group,
                owner,
                holder,
            }
        }

        fn named_in(&self, groups: &serde_json::Value) -> bool {
            groups.as_array().is_some_and(|groups| {
                groups.iter().any(|group| {
                    group["path"].as_str() == self.group.to_str() && group["reason"].is_string()
                })
            })
        }
    }

    impl Drop for ZombieOwnersGroup {
        fn drop(&mut self) {
            let _ = self.holder.kill();
            let _ = self.holder.wait();
            let _ = self.owner.wait();
            let deadline = Instant::now() + Duration::from_secs(2);
            while std::fs::remove_dir(&self.group).is_err()
                && self.group.exists()
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// The one JSON document a failed `--output json` invocation writes on stderr, asserting it is
    /// the only line there and that stdout is empty.
    fn the_one_error_document(output: &Output) -> serde_json::Value {
        let stderr = text(&output.stderr);
        assert_eq!(
            text(&output.stdout),
            "",
            "a failed json invocation wrote on stdout"
        );
        let lines: Vec<&str> = stderr.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "under --output json stderr carries exactly one document: {stderr:?}"
        );
        serde_json::from_str(lines[0]).expect("the line is one JSON document")
    }

    /// R1 meets F1: the owner is a zombie, so it counts as gone, and its group still holds a live
    /// process. The group is not removed, its process runs on, and the json document names it.
    #[test]
    fn review_delegated_a_zombie_owners_group_holding_a_process_is_kept_and_named() {
        let _lock = DELEGATED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(root) = delegated("zombie owner") else {
            return;
        };
        let host = host();
        let workspace = host.workspace("ws_zombie");
        let mut zombie = ZombieOwnersGroup::new(&root);
        let output = cli()
            .env(ROOT_ENV, &root)
            .args([
                "--output",
                "json",
                "confine",
                "run",
                "--dir",
                path(&workspace),
                "--",
                "/bin/sh",
                "-c",
                "true",
            ])
            .output()
            .expect("the CLI runs");
        let owner_state = stat_fields(zombie.owner.id()).map(|(state, _)| state);
        assert_eq!(
            owner_state,
            Some('Z'),
            "the owner was not a zombie throughout"
        );
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
        assert!(
            zombie.group.is_dir(),
            "the run removed {}, whose zombie owner's group held a live process",
            zombie.group.display()
        );
        assert!(
            zombie.holder.try_wait().expect("try_wait").is_none(),
            "the run ended the process held in {}",
            zombie.group.display()
        );
        assert_eq!(procs(&zombie.group), [zombie.holder.id().to_string()]);
        assert_eq!(text(&output.stderr), "", "json wrote beside its document");
        let document: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("one JSON document on stdout");
        assert!(
            zombie.named_in(&document["result"]["left_groups"]),
            "the document does not name {}: {document}",
            zombie.group.display()
        );
        println!("security review confine reaper zombie owner: executed");
    }

    /// A refusal that comes after the reaper ran (`--net` is refused at admission, after the
    /// private group is made): one document on stderr, exit 2, the group named in
    /// `error.data.left_groups`.
    #[test]
    fn review_delegated_a_json_refusal_after_the_reaper_names_the_group_in_error_data() {
        let _lock = DELEGATED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(root) = delegated("json refusal") else {
            return;
        };
        let host = host();
        let workspace = host.workspace("ws_refused");
        let zombie = ZombieOwnersGroup::new(&root);
        let output = cli()
            .env(ROOT_ENV, &root)
            .args([
                "--output",
                "json",
                "confine",
                "run",
                "--net",
                "--dir",
                path(&workspace),
                "--",
                "/bin/sh",
                "-c",
                "echo ran > ran",
            ])
            .output()
            .expect("the CLI runs");
        assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
        let document = the_one_error_document(&output);
        assert_eq!(document["ok"], false, "{document}");
        assert!(
            zombie.named_in(&document["error"]["data"]["left_groups"]),
            "the refusal does not name {} in error.data.left_groups: {document}",
            zombie.group.display()
        );
        assert!(document["result"].is_null(), "{document}");
        assert!(!workspace.join("ran").exists(), "the refused run ran");
        println!("security review confine reaper json refusal: executed");
    }

    /// SIGINT during the run: exit 130, one document on stderr, `confine.interrupted` with the
    /// group named in `error.data.left_groups`.
    #[test]
    fn review_delegated_a_json_interrupt_names_the_group_in_error_data() {
        let _lock = DELEGATED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(root) = delegated("json interrupt") else {
            return;
        };
        let host = host();
        let workspace = host.workspace("ws_interrupted");
        let zombie = ZombieOwnersGroup::new(&root);
        let marker = workspace.join("started");
        let child = cli()
            .env(ROOT_ENV, &root)
            .args([
                "--output",
                "json",
                "confine",
                "run",
                "--dir",
                path(&workspace),
                "--",
                "/bin/sh",
                "-c",
                ": > started; sleep 30",
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn");
        let deadline = Instant::now() + Duration::from_secs(20);
        while !marker.exists() {
            assert!(Instant::now() < deadline, "the run never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        let sent = std::process::Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .expect("kill");
        assert!(sent.success());
        let output = child.wait_with_output().expect("the verb ends");
        assert_eq!(output.status.code(), Some(130), "{}", text(&output.stderr));
        let document = the_one_error_document(&output);
        assert_eq!(
            document["error"]["data"]["code"], "confine.interrupted",
            "{document}"
        );
        assert!(
            zombie.named_in(&document["error"]["data"]["left_groups"]),
            "the interrupt does not name {} in error.data.left_groups: {document}",
            zombie.group.display()
        );
        println!("security review confine reaper json interrupt: executed");
    }
}
