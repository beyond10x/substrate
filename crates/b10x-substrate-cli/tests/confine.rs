//! `b10x-substrate confine run` (`story:sandbox-repository-parity`): the `sandbox` repository's
//! flags over Substrate's confinement adapters, opened through `confinement::open`. Every case
//! spawns the shipped binary and asserts only on what it prints, the status it exits with, and
//! whether the command left its mark on the host.

#![cfg(target_os = "linux")]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Output;

const PINNED: &str =
    "registry.invalid/run@sha256:0000000000000000000000000000000000000000000000000000000000000000";
const REASON: &str = "the confine test runs a host process on purpose";

/// A canonical directory under `CARGO_TARGET_TMPDIR` holding the workspace `ws_confine`.
struct Host {
    _directory: tempfile::TempDir,
    root: PathBuf,
    workspace: PathBuf,
}

fn host() -> Host {
    let directory = tempfile::Builder::new()
        .prefix("confine-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("host directory");
    let root = std::fs::canonicalize(directory.path()).expect("canonical");
    let workspace = root.join("ws_confine");
    std::fs::create_dir(&workspace).expect("workspace");
    Host {
        _directory: directory,
        root,
        workspace,
    }
}

/// The verb's own cgroup root variable; the daemon's `SUBSTRATE_CGROUP_ROOT` is never read.
const ROOT_ENV: &str = "SUBSTRATE_CONFINE_CGROUP_ROOT";

/// Delegated cases share one cgroup root.
static DELEGATED: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The CLI with no daemon socket, no cgroup root and no local policy from this process.
fn cli() -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_b10x-substrate"));
    command
        .env_remove("SUBSTRATE_SOCKET")
        .env_remove("SUBSTRATE_CGROUP_ROOT")
        .env_remove(ROOT_ENV)
        .env_remove("SUBSTRATE_CONFINEMENT_LOCAL")
        .stdin(std::process::Stdio::null());
    command
}

/// Runs the CLI with `args`, no daemon socket, and `path` as `PATH` when given.
fn confine(args: &[&str], path: Option<&str>) -> Output {
    let mut command = cli();
    command.args(args);
    if let Some(path) = path {
        command.env("PATH", path);
    }
    command.output().expect("the CLI runs")
}

/// The delegated cgroup v2 root `scripts/delegated-lane.sh` names, or `None`, having said the
/// case is absent.
fn delegated(case: &str) -> Option<PathBuf> {
    let root = std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT").map(PathBuf::from);
    if root.is_none() {
        println!("confine delegated {case}: ABSENT (SUBSTRATE_VECTORS_CGROUP_ROOT is unset)");
    }
    root
}

/// The groups directly beneath `root` whose names start with `prefix`.
fn groups(root: &Path, prefix: &str) -> Vec<String> {
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.starts_with(prefix))
                .collect()
        })
        .unwrap_or_default()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn dir(host: &Host) -> String {
    host.workspace.to_str().expect("UTF-8").to_owned()
}

/// A command that leaves `ran` in the workspace if it runs at all.
fn marking() -> [&'static str; 4] {
    ["--", "/bin/sh", "-c", "echo ran > ran"]
}

/// A stand-in `docker` that answers `info` and records every other call it gets.
fn stand_in_docker(host: &Host) -> String {
    let bin = host.root.join("bin");
    std::fs::create_dir(&bin).expect("bin");
    let info = "{\"ServerVersion\":\"29.0.0\",\"SecurityOptions\":[\"name=seccomp,profile=builtin\"],\
                \"MemoryLimit\":true,\"SwapLimit\":true,\"PidsLimit\":true,\"CpuCfsQuota\":true,\
                \"NCPU\":4}";
    let calls = host.root.join("calls");
    std::fs::write(
        bin.join("docker"),
        format!(
            "#!/bin/sh\nif [ \"$1\" = info ]; then printf '%s' '{info}'; exit 0; fi\n\
             echo \"$1\" >> '{}'\nexit 1\n",
            calls.display()
        ),
    )
    .expect("stand-in docker");
    std::fs::set_permissions(bin.join("docker"), std::fs::Permissions::from_mode(0o755))
        .expect("mode");
    format!("{}:/usr/bin:/bin", bin.display())
}

fn calls(host: &Host) -> String {
    std::fs::read_to_string(host.root.join("calls")).unwrap_or_default()
}

fn assert_refused(output: &Output, line: &str, host: &Host) {
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains(line),
        "expected {line:?} on stderr: {stderr}"
    );
    assert!(
        !Path::new(&host.workspace.join("ran")).exists(),
        "the command ran although it was refused"
    );
}

#[test]
fn linux_net_exits_2_naming_host_network_before_anything_runs() {
    let host = host();
    let ws = dir(&host);
    let mut args = vec![
        "confine",
        "run",
        "--backend",
        "linux",
        "--net",
        "--dir",
        &ws,
    ];
    args.extend(marking());
    let output = confine(&args, None);
    assert_refused(&output, "confinement.fact-unserved: host_network", &host);
}

#[test]
fn oci_net_exits_2_naming_host_network_before_anything_runs() {
    let host = host();
    let path = stand_in_docker(&host);
    let ws = dir(&host);
    let mut args = vec![
        "confine",
        "run",
        "--backend",
        "oci",
        "--image",
        PINNED,
        "--net",
        "--dir",
        &ws,
    ];
    args.extend(marking());
    let output = confine(&args, Some(&path));
    assert_refused(&output, "confinement.fact-unserved: host_network", &host);
    assert!(
        !calls(&host)
            .lines()
            .any(|call| call == "create" || call == "start"),
        "the runtime was asked to run something: {}",
        calls(&host)
    );
}

#[test]
fn a_refusal_under_output_json_is_the_refused_envelope_with_exit_2() {
    let host = host();
    let ws = dir(&host);
    let mut args = vec![
        "--output",
        "json",
        "confine",
        "run",
        "--backend",
        "linux",
        "--net",
        "--dir",
        &ws,
    ];
    args.extend(marking());
    let output = confine(&args, None);
    assert_eq!(output.status.code(), Some(2));
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stderr).expect("one JSON envelope on stderr");
    assert_eq!(envelope["ok"], false);
    assert_eq!(envelope["error"]["code"], "refused");
    assert_eq!(
        envelope["error"]["data"]["code"],
        "confinement.fact-unserved"
    );
    assert_eq!(envelope["error"]["data"]["message"], "host_network");
}

#[test]
fn local_with_a_reason_runs_the_command_and_says_so_on_stderr() {
    let host = host();
    let ws = dir(&host);
    let output = confine(
        &[
            "confine",
            "run",
            "--backend",
            "linux",
            "--net",
            "--local",
            REASON,
            "--dir",
            &ws,
            "--",
            "/bin/sh",
            "-c",
            "echo ran > ran; echo out; echo err >&2; exit 3",
        ],
        None,
    );
    let stderr = text(&output.stderr);
    assert_eq!(output.status.code(), Some(3), "stderr: {stderr}");
    assert_eq!(text(&output.stdout), "out\n");
    assert!(
        stderr.contains(&format!("adapter=local declared_insecure={REASON}")),
        "stderr: {stderr}"
    );
    assert!(stderr.contains("err\n"), "stderr: {stderr}");
    assert!(
        host.workspace.join("ran").exists(),
        "the command did not run"
    );
}

#[test]
fn local_with_a_blank_reason_exits_2() {
    let host = host();
    let ws = dir(&host);
    let mut args = vec!["confine", "run", "--net", "--local", " ", "--dir", &ws];
    args.extend(marking());
    let output = confine(&args, None);
    assert_refused(&output, "confinement.local-reason-missing", &host);
}

#[test]
fn an_image_named_by_tag_exits_2_unpinned() {
    let host = host();
    let path = stand_in_docker(&host);
    let ws = dir(&host);
    let mut args = vec![
        "confine",
        "run",
        "--backend",
        "oci",
        "--image",
        "docker.io/library/alpine:3.20",
        "--dir",
        &ws,
    ];
    args.extend(marking());
    let output = confine(&args, Some(&path));
    assert_refused(&output, "confinement.image-unpinned", &host);
    assert!(
        calls(&host).is_empty(),
        "the runtime was called: {}",
        calls(&host)
    );
}

/// `story:sandbox-repository-parity`, adversary 6: a dry run admits as a run does, and `linux`
/// without a delegated cgroup root runs nothing, so the render is read on the delegated lane and
/// the portable lane asserts the refusal the run would meet.
#[test]
fn dry_run_prints_the_program_and_argv_one_per_line_and_unshares_the_network() {
    let _lock = DELEGATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let host = host();
    let ws = dir(&host);
    let args = ["confine", "run", "--dry-run", "--dir", &ws, "--", "/bin/sh"];
    let Some(root) = delegated("dry run render") else {
        let output = confine(&args, None);
        assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
        assert!(
            text(&output.stderr).contains("confinement.family-unserved: process_run"),
            "{}",
            text(&output.stderr)
        );
        assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
        return;
    };
    let output = cli()
        .env(ROOT_ENV, &root)
        .args(args)
        .output()
        .expect("the CLI runs");
    println!("confine delegated dry run render: executed");
    let stdout = text(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        text(&output.stderr)
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.first(), Some(&"/usr/bin/bwrap"), "{stdout}");
    // The O6 loop's own check on the recorded argv: `grep -q -- '--unshare-net'`.
    assert!(lines.contains(&"--unshare-net"), "{stdout}");
    assert_eq!(lines.last(), Some(&"/bin/sh"), "{stdout}");
    assert!(!host.workspace.join("ran").exists());
}

#[test]
fn oci_dry_run_prints_the_runtime_and_its_create_call() {
    let host = host();
    let path = stand_in_docker(&host);
    let ws = dir(&host);
    let output = confine(
        &[
            "confine",
            "run",
            "--backend",
            "oci",
            "--image",
            PINNED,
            "--dry-run",
            "--dir",
            &ws,
            "--",
            "/bin/true",
        ],
        Some(&path),
    );
    let stdout = text(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        text(&output.stderr)
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines.first().copied(),
        Some(
            host.root
                .join("bin")
                .join("docker")
                .to_str()
                .expect("UTF-8")
        ),
        "{stdout}"
    );
    assert_eq!(lines.get(1), Some(&"create"), "{stdout}");
    assert!(
        lines.windows(2).any(|pair| pair == ["--network", "none"]),
        "{stdout}"
    );
    assert_eq!(lines.last(), Some(&"/bin/true"), "{stdout}");
    assert!(calls(&host).lines().all(|call| call != "create"));
}

// --- The cgroup root: the verb's own variable and a private group per invocation -------------

/// A host driver opened on a root kills every run group it finds there, so the verb never reads
/// the daemon's variable: a root named only by it is not touched, and linux runs nothing.
#[test]
fn the_daemons_cgroup_root_variable_is_not_read() {
    let host = host();
    let ws = dir(&host);
    let daemons = host.root.join("daemons-root");
    std::fs::create_dir_all(daemons.join("substrate-ex_daemon")).expect("a daemon's run group");
    let mut args = vec!["confine", "run", "--dir", &ws];
    args.extend(marking());
    let output = cli()
        .env("SUBSTRATE_CGROUP_ROOT", &daemons)
        .args(&args)
        .output()
        .expect("the CLI runs");
    assert_refused(&output, "confinement.family-unserved: process_run", &host);
    assert!(
        !text(&output.stderr).contains("cgroup"),
        "{}",
        text(&output.stderr)
    );
    assert!(daemons.join("substrate-ex_daemon").is_dir());
}

/// A root the verb cannot make its private group beneath is a named refusal, exit 2, naming the
/// variable; the driver is never opened on it.
#[test]
fn a_cgroup_root_that_is_not_a_cgroup_exits_2_naming_the_variable() {
    let host = host();
    let ws = dir(&host);
    let not_a_cgroup = host.root.join("not-a-cgroup");
    std::fs::create_dir_all(not_a_cgroup.join("substrate-ex_other")).expect("a run group");
    let mut args = vec!["confine", "run", "--dry-run", "--dir", &ws];
    args.extend(marking());
    let output = cli()
        .env(ROOT_ENV, &not_a_cgroup)
        .args(&args)
        .output()
        .expect("the CLI runs");
    assert_refused(&output, "exec.sandbox-unavailable", &host);
    assert!(
        text(&output.stderr).contains(ROOT_ENV),
        "{}",
        text(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
    assert!(not_a_cgroup.join("substrate-ex_other").is_dir());
}

/// Each invocation opens its driver on a private group of its own: a run group already beneath
/// the root survives a run and a dry run, and no private group is left behind.
#[test]
fn a_delegated_run_spares_other_run_groups_and_removes_its_private_group() {
    let _lock = DELEGATED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(root) = delegated("private group") else {
        return;
    };
    let host = host();
    let ws = dir(&host);
    let foreign = root.join("substrate-ex_foreign");
    std::fs::create_dir(&foreign).expect("a run group the verb does not own");
    for dry_run in [false, true] {
        let mut args = vec!["confine", "run", "--dir", &ws];
        if dry_run {
            args.push("--dry-run");
        }
        args.extend(["--", "/bin/sh", "-c", "echo ran > ran"]);
        let output = cli()
            .env(ROOT_ENV, &root)
            .args(&args)
            .output()
            .expect("the CLI runs");
        let survived = foreign.is_dir();
        let left = groups(&root, "confine-");
        assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
        assert!(
            survived,
            "dry run {dry_run}: the verb removed a run group it does not own"
        );
        assert!(
            left.is_empty(),
            "dry run {dry_run}: left {left:?} beneath the root"
        );
    }
    assert!(host.workspace.join("ran").exists(), "the run did not run");
    let _ = std::fs::remove_dir(&foreign);
    println!("confine delegated private group: executed");
}

// --- --image: only oci runs an image -------------------------------------------------------

/// Pinned or tagged, an image under linux or local is the fact those adapters do not prove,
/// refused before any adapter opens.
#[test]
fn an_image_outside_oci_exits_2_naming_image_digest_pinned() {
    let host = host();
    let ws = dir(&host);
    for image in [PINNED, "docker.io/library/alpine:3.20"] {
        for local in [false, true] {
            let mut args = vec!["confine", "run", "--image", image, "--dir", &ws];
            if local {
                args.extend(["--net", "--local", REASON]);
            }
            args.extend(marking());
            let output = confine(&args, None);
            assert_refused(
                &output,
                "confinement.fact-unserved: image_digest_pinned",
                &host,
            );
        }
    }
}

// --- --dir: the host's workspace identity rule is linux's -----------------------------------

/// `oci` binds the workspace by path, so a name holding a dot is a workspace there.
#[test]
fn oci_takes_a_workspace_whose_name_holds_a_dot() {
    let host = host();
    let path = stand_in_docker(&host);
    let dotted = host.root.join("my.project");
    std::fs::create_dir(&dotted).expect("workspace");
    let output = confine(
        &[
            "confine",
            "run",
            "--backend",
            "oci",
            "--image",
            PINNED,
            "--dry-run",
            "--dir",
            dotted.to_str().expect("UTF-8"),
            "--",
            "/bin/true",
        ],
        Some(&path),
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
}

// --- --dry-run: its descriptor numbers are the render's own ---------------------------------

#[test]
fn a_dry_run_says_its_descriptor_numbers_are_local_to_the_render() {
    let host = host();
    let ws = dir(&host);
    let output = confine(
        &[
            "confine",
            "run",
            "--net",
            "--local",
            REASON,
            "--dry-run",
            "--dir",
            &ws,
            "--",
            "/bin/true",
        ],
        None,
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("descriptor numbers in this argv are local to the render"),
        "{}",
        text(&output.stderr)
    );
    let output = confine(
        &[
            "--output",
            "json",
            "confine",
            "run",
            "--net",
            "--local",
            REASON,
            "--dry-run",
            "--dir",
            &ws,
            "--",
            "/bin/true",
        ],
        None,
    );
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).expect("one result");
    assert!(
        envelope["result"]["note"]
            .as_str()
            .is_some_and(|note| note.contains("local to the render")),
        "{envelope}"
    );
}

// --- Signals: an interrupted run is killed and leaves nothing --------------------------------

#[test]
fn sigint_kills_the_run_removes_the_state_directory_and_exits_130() {
    let host = host();
    let ws = dir(&host);
    let temp = host.root.join("tmpdir");
    std::fs::create_dir(&temp).expect("tmpdir");
    let state = |temp: &Path| groups(temp, "b10x-substrate-confine-");
    let child = cli()
        .env("TMPDIR", &temp)
        .args([
            "confine",
            "run",
            "--net",
            "--local",
            REASON,
            "--dir",
            &ws,
            "--",
            "/bin/sh",
            "-c",
            "sleep 2; echo ran > ran",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while state(&temp).is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "no state directory appeared"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // The run has started once the verb waits on it; the state directory appears before that.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let status = std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("kill");
    assert!(status.success());
    let output = child.wait_with_output().expect("the verb ends");
    std::thread::sleep(std::time::Duration::from_millis(2_500));
    assert_eq!(output.status.code(), Some(130), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("confine.interrupted"),
        "{}",
        text(&output.stderr)
    );
    assert!(state(&temp).is_empty(), "left {:?}", state(&temp));
    assert!(
        !host.workspace.join("ran").exists(),
        "the run outlived the interrupted verb"
    );
}
