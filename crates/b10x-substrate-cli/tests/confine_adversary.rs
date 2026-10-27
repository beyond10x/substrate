//! Adversary cases for `b10x-substrate confine run` (`story:sandbox-repository-parity`).
//!
//! The portable cases need nothing. The delegated cases need a delegated cgroup v2 root named by
//! `SUBSTRATE_VECTORS_CGROUP_ROOT` that this process is inside (`scripts/delegated-lane.sh` sets
//! one up); without it each prints `ABSENT` and returns. They hold one lock, because every
//! `confine run` opens a host driver on the same root.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static DELEGATED: Mutex<()> = Mutex::new(());

struct Host {
    _directory: tempfile::TempDir,
    root: PathBuf,
}

fn host() -> Host {
    let directory = tempfile::Builder::new()
        .prefix("confine-adv-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("host directory");
    let root = std::fs::canonicalize(directory.path()).expect("canonical");
    Host {
        _directory: directory,
        root,
    }
}

fn mkdir(path: &Path) -> String {
    std::fs::create_dir_all(path).expect("directory");
    path.to_str().expect("UTF-8").to_owned()
}

/// `story:sandbox-repository-parity`, adversary 1: the verb reads its own
/// `SUBSTRATE_CONFINE_CGROUP_ROOT` and never the daemon's `SUBSTRATE_CGROUP_ROOT`, so the root is
/// handed over under the verb's name; both are cleared first.
fn cli(cgroup_root: Option<&Path>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_b10x-substrate"));
    command
        .env_remove("SUBSTRATE_SOCKET")
        .env_remove("SUBSTRATE_CGROUP_ROOT")
        .env_remove("SUBSTRATE_CONFINE_CGROUP_ROOT")
        .env_remove("SUBSTRATE_CONFINEMENT_LOCAL")
        .stdin(Stdio::null());
    if let Some(root) = cgroup_root {
        command.env("SUBSTRATE_CONFINE_CGROUP_ROOT", root);
    }
    command
}

/// Every `substrate-ex_*` group beneath `root`, at any depth: each invocation's run groups sit in
/// its own private group beneath the root (`story:sandbox-repository-parity`, adversary 1).
fn run_groups(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("substrate-ex_")
        {
            found.push(entry.path());
        }
        found.extend(run_groups(&entry.path()));
    }
    found
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn delegated() -> Option<PathBuf> {
    let Some(root) = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT") else {
        println!("confine adversary delegated: ABSENT (SUBSTRATE_VECTORS_CGROUP_ROOT is unset)");
        return None;
    };
    let root = PathBuf::from(root);
    if std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids").is_err() {
        println!("confine adversary delegated: ABSENT (the root does not accept controllers)");
        return None;
    }
    Some(root)
}

fn lines(output: &Output) -> Vec<String> {
    text(&output.stdout).lines().map(str::to_owned).collect()
}

fn has_group(argv: &[String], group: &[&str]) -> bool {
    argv.windows(group.len())
        .any(|window| window.iter().zip(group).all(|(word, want)| word == want))
}

/// sandbox `src/confinement.rs:233-242` at `1b0377d`: `--clearenv`, then these five, on every run.
/// The parity test asserts them only because its own `Command` supplies them; the verb is what
/// replaces `b10x-sandbox`, so its render is what has to carry them.
///
/// `story:sandbox-repository-parity`, adversary 6: a dry run admits as a run does, and `linux`
/// runs nothing without a delegated cgroup root, so the render is read on the delegated lane; the
/// portable lane asserts the refusal the run would meet, and `sandbox_parity` (host crate) asserts
/// these rows portably from the verb's own command.
#[test]
fn confine_dry_run_sets_sandboxs_five_baseline_variables() {
    let _lock = DELEGATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let host = host();
    let ws = mkdir(&host.root.join("ws_env"));
    let Some(root) = delegated() else {
        let output = cli(None)
            .args([
                "confine",
                "run",
                "--dry-run",
                "--dir",
                &ws,
                "--",
                "/bin/true",
            ])
            .output()
            .expect("the CLI runs");
        assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
        assert!(
            text(&output.stderr).contains("confinement.family-unserved: process_run"),
            "{}",
            text(&output.stderr)
        );
        assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
        return;
    };
    let output = cli(Some(&root))
        .args([
            "confine",
            "run",
            "--dry-run",
            "--dir",
            &ws,
            "--",
            "/bin/true",
        ])
        .output()
        .expect("the CLI runs");
    println!("confine adversary delegated: executed, baseline variables");
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    let argv = lines(&output);
    let missing: Vec<&str> = [
        ["--setenv", "PATH", "/usr/bin:/bin"],
        ["--setenv", "HOME", "/tmp"],
        ["--setenv", "LANG", "C.UTF-8"],
        ["--setenv", "LC_ALL", "C.UTF-8"],
        ["--setenv", "TZ", "UTC"],
    ]
    .iter()
    .filter(|group| !has_group(&argv, &group[..]))
    .map(|group| group[1])
    .collect();
    assert!(
        missing.is_empty(),
        "confine run's render does not set sandbox's {missing:?}: {argv:?}"
    );
}

/// sandbox's `--dir` takes any directory (`src/main.rs`, `Layout::plan`); a project directory
/// whose name holds a dot is the common case, and it was refused here as a path escape.
///
/// `story:sandbox-repository-parity`, adversary 4: `linux` keeps the host's workspace identity
/// rule, `[A-Za-z0-9_-]+`, so this asserts the verb's own refusal naming that rule, before any
/// adapter opens, and that the misleading `workspace.path-escape` does not reach the user.
#[test]
fn confine_dir_with_a_dot_in_its_name_is_a_workspace_as_in_sandbox() {
    let host = host();
    let ws = mkdir(&host.root.join("my.project"));
    let output = cli(None)
        .args([
            "confine",
            "run",
            "--dry-run",
            "--dir",
            &ws,
            "--",
            "/bin/true",
        ])
        .output()
        .expect("the CLI runs");
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("[A-Za-z0-9_-]+"), "{stderr}");
    assert!(stderr.contains("workspace identity rule"), "{stderr}");
    assert!(!stderr.contains("workspace.path-escape"), "{stderr}");
    assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
}

/// Two `confine run` invocations on one delegated root: the second opens a host driver, whose
/// `ProcessRuntime` reconciles every `substrate-ex_*` cgroup under the root at construction, and
/// so kills the first one's command. A dry run is enough to do it.
#[test]
fn confine_a_second_run_on_the_same_root_does_not_kill_the_first() {
    let _lock = DELEGATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(root) = delegated() else { return };
    let host = host();
    let first_ws = mkdir(&host.root.join("ws_first"));
    let second_ws = mkdir(&host.root.join("ws_second"));
    let first = cli(Some(&root))
        .args([
            "confine",
            "run",
            "--dir",
            &first_ws,
            "--",
            "/bin/sh",
            "-c",
            "sleep 3; echo survived > survived",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the first run starts");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        // `story:sandbox-repository-parity`, adversary 1: the first run's group sits in its own
        // private group beneath the root, so it is looked for at any depth.
        let running = !run_groups(&root).is_empty();
        if running {
            break;
        }
        assert!(Instant::now() < deadline, "the first run never started");
        std::thread::sleep(Duration::from_millis(50));
    }
    let second = cli(Some(&root))
        .args([
            "confine",
            "run",
            "--dry-run",
            "--dir",
            &second_ws,
            "--",
            "/bin/true",
        ])
        .output()
        .expect("the second CLI runs");
    let first = first.wait_with_output().expect("the first run ends");
    println!("confine adversary delegated: executed, two runs on one root");
    assert!(
        first.status.success() && Path::new(&first_ws).join("survived").exists(),
        "the first run was ended by the second ({:?}); first stderr: {}; second ({:?}) stderr: {}",
        first.status,
        text(&first.stderr),
        second.status,
        text(&second.stderr)
    );
    assert_eq!(second.status.code(), Some(0), "{}", text(&second.stderr));
}

/// A run's stderr is the command's stderr: nothing the host driver's probe prints at open.
#[test]
fn confine_a_linux_run_prints_only_the_commands_stderr() {
    let _lock = DELEGATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(root) = delegated() else { return };
    let host = host();
    let ws = mkdir(&host.root.join("ws_stderr"));
    let output = cli(Some(&root))
        .args([
            "confine",
            "run",
            "--dir",
            &ws,
            "--",
            "/bin/sh",
            "-c",
            "echo err >&2",
        ])
        .output()
        .expect("the CLI runs");
    println!("confine adversary delegated: executed, stderr");
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stderr), "err\n");
}

/// Under `--output json` a refusal is one envelope on stderr, on a delegated host too.
#[test]
fn confine_a_json_refusal_on_a_delegated_host_is_one_envelope() {
    let _lock = DELEGATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(root) = delegated() else { return };
    let host = host();
    let ws = mkdir(&host.root.join("ws_json"));
    let output = cli(Some(&root))
        .args([
            "--output",
            "json",
            "confine",
            "run",
            "--net",
            "--dir",
            &ws,
            "--",
            "/bin/true",
        ])
        .output()
        .expect("the CLI runs");
    println!("confine adversary delegated: executed, json envelope");
    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    let envelope: Result<serde_json::Value, _> = serde_json::from_slice(&output.stderr);
    assert!(
        envelope.is_ok(),
        "stderr is not one JSON envelope: {:?}",
        text(&output.stderr)
    );
}

/// sandbox resolves a relative `--ro` against the working directory lexically, "so `../lib` is
/// usable from a shell" (`src/main.rs` at `1b0377d`); here `std::path::absolute` keeps the `..`
/// and the driver refuses the root as not canonical.
#[test]
fn confine_a_relative_ro_with_dotdot_binds_as_in_sandbox() {
    let _lock = DELEGATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(root) = delegated() else { return };
    let host = host();
    let ws = mkdir(&host.root.join("ws_rel"));
    let tool = mkdir(&host.root.join("tool"));
    std::fs::write(Path::new(&tool).join("file"), "tool\n").expect("tool file");
    let output = cli(Some(&root))
        .current_dir(&ws)
        .args([
            "confine",
            "run",
            "--dir",
            ".",
            "--ro",
            "../tool",
            "--",
            "/bin/cat",
            &format!("{tool}/file"),
        ])
        .output()
        .expect("the CLI runs");
    println!("confine adversary delegated: executed, relative ro");
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "tool\n");
}

/// Coverage the unit's suite does not have: a real `linux` run through the verb, with a
/// read-only root, writing the workspace and refused writing the root.
#[test]
fn confine_a_linux_run_writes_the_workspace_and_not_a_read_only_root() {
    let _lock = DELEGATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(root) = delegated() else { return };
    let host = host();
    let ws = mkdir(&host.root.join("ws_run"));
    let tool = mkdir(&host.root.join("toolchain"));
    std::fs::write(Path::new(&tool).join("file"), "tool\n").expect("tool file");
    let output = cli(Some(&root))
        .args([
            "confine",
            "run",
            "--dir",
            &ws,
            "--ro",
            &tool,
            "--",
            "/bin/sh",
            "-c",
            &format!(
                "echo ran > ran; cat {tool}/file; if echo x > {tool}/new 2>/dev/null; \
                 then echo wrote; else echo refused; fi"
            ),
        ])
        .output()
        .expect("the CLI runs");
    println!("confine adversary delegated: executed, linux run");
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "tool\nrefused\n");
    assert!(Path::new(&ws).join("ran").exists());
    assert!(!Path::new(&tool).join("new").exists());
}
