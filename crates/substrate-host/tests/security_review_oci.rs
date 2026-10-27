//! Security-review conformance cases for `story:oci-adapter`.
//!
//! Each case checks the `oci` adapter against a guarantee its own documents or its sibling
//! `linux` adapter state, at admission and `render`, over a stand-in runtime that answers `info`.
//! Nothing here starts a container.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile,
    oci::{self, OciRequest},
};
use substrate_wire::ReadOnlyRoot;

const PINNED: &str =
    "registry.invalid/run@sha256:0000000000000000000000000000000000000000000000000000000000000000";

struct Fakes {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn fakes() -> &'static Fakes {
    static FAKES: OnceLock<Fakes> = OnceLock::new();
    FAKES.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = std::fs::canonicalize(dir.path()).expect("canonical");
        for (name, limits) in [("docker-ok", true), ("docker-no-limits", false)] {
            let info = format!(
                "{{\"ServerVersion\":\"29.0.0\",\"SecurityOptions\":[\"name=seccomp,profile=builtin\"],\
                 \"MemoryLimit\":{limits},\"SwapLimit\":{limits},\"PidsLimit\":{limits},\
                 \"CpuCfsQuota\":{limits},\"NCPU\":4}}"
            );
            let body = format!(
                "#!/bin/sh\nif [ \"$1\" = info ]; then printf '%s' '{info}'; exit 0; fi\nexit 1\n"
            );
            let path = root.join(name);
            std::fs::write(&path, body).expect("fake runtime");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("executable");
            executable_now(&path);
        }
        Fakes { _dir: dir, root }
    })
}

fn executable_now(path: &Path) {
    for _ in 0..100 {
        match std::process::Command::new(path)
            .arg("version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(_) => return,
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("{}: {error}", path.display()),
        }
    }
    panic!("{} stayed busy", path.display());
}

fn open_fake(name: &str) -> Arc<dyn ConfinementAdapter> {
    confinement::open(AdapterRequest::Oci(OciRequest {
        runtime: fakes().root.join(name),
        // story:oci-adapter, F1: the operator's workspace root; every `workspace()` sits directly
        // beneath it.
        workspace_root: std::fs::canonicalize(std::env::temp_dir()).expect("canonical"),
        writable_roots: Vec::new(),
    }))
    .expect("oci opens over the stand-in runtime")
}

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = std::fs::canonicalize(dir.path()).expect("canonical");
    (dir, path)
}

fn true_command(env: BTreeMap<String, String>) -> Command {
    Command {
        argv: vec!["/bin/true".into()],
        env,
    }
}

fn render(
    adapter: &Arc<dyn ConfinementAdapter>,
    profile: &ConfinementProfile,
    command: &Command,
) -> Result<Vec<String>, ConfinementError> {
    let admitted = adapter.admit(profile)?;
    adapter
        .render(&admitted, command)
        .map(|rendered| rendered.args)
}

fn pair(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == flag)
        .map(|window| window[1].clone())
}

/// The `linux` adapter refuses a credential-named or non-`[A-Z][A-Z0-9_]*` variable
/// `exec.environment-refused` (`process.rs` `validate_environment`, `is_secretish_name`). The
/// `oci` adapter's environment must be shaped the same way.
#[test]
fn review_oci_refuses_the_environment_linux_refuses() {
    let (_dir, workspace) = workspace();
    let adapter = open_fake("docker-ok");
    let profile = ConfinementProfile {
        image: Some(PINNED.into()),
        workspace: Some(workspace),
        ..ConfinementProfile::default()
    };
    let mut admitted_names = Vec::new();
    for name in [
        "GITHUB_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
        "DB_PASSWORD",
        "HTTPS_PROXY",
        "AUTHORIZATION",
        "lowercase_name",
        "WITH-DASH",
    ] {
        let command = true_command(BTreeMap::from([(name.to_owned(), "x".to_owned())]));
        match render(&adapter, &profile, &command) {
            Err(error) if error.code() == "exec.environment-refused" => {}
            Err(error) => panic!("{name}: refused by another name: {error}"),
            Ok(_) => admitted_names.push(name),
        }
    }
    assert!(
        admitted_names.is_empty(),
        "oci passed variables linux refuses into the container: {admitted_names:?}"
    );
}

/// `linux` binds as the workspace only a directory directly beneath the driver's configured
/// workspace root; ADR 0038 requires every other writable host root to be operator-declared. The
/// `oci` adapter has no operator workspace root, so a profile can name the host's `/` as its
/// writable workspace.
#[test]
fn review_oci_does_not_bind_the_host_root_as_a_writable_workspace() {
    let adapter = open_fake("docker-ok");
    let profile = ConfinementProfile {
        image: Some(PINNED.into()),
        workspace: Some(PathBuf::from("/")),
        ..ConfinementProfile::default()
    };
    match render(&adapter, &profile, &true_command(BTreeMap::new())) {
        Err(_) => {}
        Ok(args) => {
            let binds: Vec<&String> = args
                .windows(2)
                .filter(|window| window[0] == "-v")
                .map(|window| &window[1])
                .collect();
            panic!("the host root is bound writable as the workspace: {binds:?}");
        }
    }
}

/// `DEFAULT_BOUNDS` is documented as "the bounds a run gets when its profile states none". Over a
/// runtime that reports no limit control (`SwapLimit: false` is common), a profile stating no
/// bounds is admitted and rendered with no `--pids-limit`, `--memory` or `--cpus` at all.
#[test]
fn review_oci_without_limit_control_does_not_run_unbounded() {
    let (_dir, workspace) = workspace();
    let adapter = open_fake("docker-no-limits");
    let profile = ConfinementProfile {
        image: Some(PINNED.into()),
        workspace: Some(workspace),
        ..ConfinementProfile::default()
    };
    match render(&adapter, &profile, &true_command(BTreeMap::new())) {
        Err(_) => {}
        Ok(args) => {
            let expected = oci::DEFAULT_BOUNDS.processes.to_string();
            assert_eq!(
                pair(&args, "--pids-limit").as_deref(),
                Some(expected.as_str()),
                "a run with no stated bounds renders without the default bounds: {args:?}"
            );
            assert!(pair(&args, "--memory").is_some(), "{args:?}");
            assert!(pair(&args, "--memory-swap").is_some(), "{args:?}");
            assert!(pair(&args, "--cpus").is_some(), "{args:?}");
        }
    }
}

/// The adapter's authority paragraph treats the runtime's socket as root-equivalent. A read-only
/// root whose host directory holds that socket places it inside the container; the adapter must
/// refuse such a root by name rather than bind it.
#[test]
fn review_oci_does_not_bind_a_directory_holding_the_runtime_socket() {
    let socket = Path::new("/run/docker.sock");
    let holder = Path::new("/run");
    let is_socket = std::fs::symlink_metadata(socket)
        .is_ok_and(|metadata| std::os::unix::fs::FileTypeExt::is_socket(&metadata.file_type()));
    if !is_socket {
        println!(
            "review: no runtime socket at {}; case not applicable",
            socket.display()
        );
        return;
    }
    let (_dir, workspace) = workspace();
    let adapter = open_fake("docker-ok");
    let profile = ConfinementProfile {
        image: Some(PINNED.into()),
        workspace: Some(workspace),
        read_only_roots: vec![ReadOnlyRoot {
            host_path: holder.display().to_string(),
            mount: "/opt/hostrun".into(),
        }],
        ..ConfinementProfile::default()
    };
    match render(&adapter, &profile, &true_command(BTreeMap::new())) {
        Err(_) => {}
        Ok(args) => panic!(
            "a read-only root holding {} is bound: {:?}",
            socket.display(),
            args.windows(2)
                .filter(|window| window[0] == "-v")
                .map(|window| window[1].clone())
                .collect::<Vec<_>>()
        ),
    }
}

/// What holds: no profile value is ever a runtime flag, and the fixed posture carries no flag
/// that widens it.
#[test]
fn review_oci_posture_has_no_widening_flag_and_profile_values_stay_values() {
    let (_dir, workspace) = workspace();
    std::fs::create_dir(workspace.join("--privileged")).expect("workdir");
    let ro = tempfile::tempdir().expect("tempdir");
    let ro_path = std::fs::canonicalize(ro.path()).expect("canonical");
    let adapter = open_fake("docker-ok");
    let profile = ConfinementProfile {
        image: Some(PINNED.into()),
        workspace: Some(workspace),
        workdir: Some("--privileged".into()),
        read_only_roots: vec![ReadOnlyRoot {
            host_path: ro_path.display().to_string(),
            mount: "/opt/--device".into(),
        }],
        ..ConfinementProfile::default()
    };
    let args = render(
        &adapter,
        &profile,
        &Command {
            argv: vec!["--privileged".into(), "--network=host".into()],
            env: BTreeMap::from([("A".to_owned(), "--cap-add=ALL".to_owned())]),
        },
    )
    .expect("rendered");
    let image_at = args.iter().position(|arg| arg == PINNED).expect("image");
    assert_eq!(args[image_at - 1], "--", "{args:?}");
    let options = &args[..image_at - 1];
    // story:oci-adapter, F7 and adversary 6 (coordinator's decisions): binds are `--mount
    // type=bind` and the run has `--ipc none`, so `--mount` and `--ipc` now appear. Each is
    // held to its exact value here instead of being refused outright: the only binds are the
    // workspace and the declared read-only root, and the only IPC mode is `none`.
    let mounts: Vec<&String> = options
        .windows(2)
        .filter(|window| window[0] == "--mount")
        .map(|window| &window[1])
        .collect();
    let workspace_text = profile
        .workspace
        .as_ref()
        .expect("workspace")
        .display()
        .to_string();
    assert_eq!(
        mounts,
        [
            &format!("type=bind,source={workspace_text},target=/workspace"),
            &format!(
                "type=bind,source={},target=/opt/--device,readonly",
                ro_path.display()
            ),
        ],
        "{args:?}"
    );
    let ipc: Vec<&String> = options
        .windows(2)
        .filter(|window| window[0] == "--ipc")
        .map(|window| &window[1])
        .collect();
    assert_eq!(ipc, ["none"], "{args:?}");
    for (index, arg) in options.iter().enumerate() {
        let after_value_flag = index > 0
            && matches!(
                options[index - 1].as_str(),
                "-v" | "-e"
                    | "-w"
                    | "--name"
                    | "--label"
                    | "--user"
                    | "--tmpfs"
                    | "--mount"
                    | "--ipc"
            );
        if after_value_flag || arg == "--mount" || arg == "--ipc" {
            continue;
        }
        for widening in [
            "--privileged",
            "--cap-add",
            "--device",
            "--pid",
            "--ipc",
            "--uts",
            "--userns=host",
            "--group-add",
            "--mount",
            "--volumes-from",
        ] {
            let flagged = arg == widening || arg.starts_with(&format!("{widening}="));
            assert!(!flagged, "{arg}: {args:?}");
        }
        assert!(!arg.contains("seccomp"), "{arg}: {args:?}");
        assert!(!arg.contains("unconfined"), "{arg}: {args:?}");
    }
    assert_eq!(pair(&args, "--network").as_deref(), Some("none"));
    assert_eq!(pair(&args, "--cap-drop").as_deref(), Some("ALL"));
    assert_eq!(
        pair(&args, "--security-opt").as_deref(),
        Some("no-new-privileges")
    );
    assert!(options.iter().any(|arg| arg == "--read-only"));
    assert_eq!(&args[image_at + 1..], ["--privileged", "--network=host"]);
}
