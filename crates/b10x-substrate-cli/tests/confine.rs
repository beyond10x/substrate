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

/// Runs the CLI with `args`, no daemon socket, and `path` as `PATH` when given.
fn confine(args: &[&str], path: Option<&str>) -> Output {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_b10x-substrate"));
    command
        .args(args)
        .env_remove("SUBSTRATE_SOCKET")
        .env_remove("SUBSTRATE_CGROUP_ROOT")
        .env_remove("SUBSTRATE_CONFINEMENT_LOCAL")
        .stdin(std::process::Stdio::null());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    command.output().expect("the CLI runs")
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

#[test]
fn dry_run_prints_the_program_and_argv_one_per_line_and_unshares_the_network() {
    let host = host();
    let ws = dir(&host);
    let output = confine(
        &["confine", "run", "--dry-run", "--dir", &ws, "--", "/bin/sh"],
        None,
    );
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
