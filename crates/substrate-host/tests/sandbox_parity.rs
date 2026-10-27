//! The `sandbox` repository's settings, covered by Substrate (atlas ADR 0109 decision 5,
//! `story:sandbox-repository-parity`).
//!
//! `sandbox` held a mirror test (`tests/substrate_mirror.rs` at `1b0377d`) proving its bubblewrap
//! list followed Substrate's. This is that test moved here and turned around: each argument
//! `sandbox` passed is a row, transcribed from its source at `1b0377d`, and the row is asserted
//! present in what Substrate's own adapter renders for the equivalent profile, so Substrate's set
//! is a superset of what `sandbox` ran.
//!
//! - [`BUBBLEWRAP`]: `src/confinement.rs:179-258`, against the `linux` adapter's `render`.
//! - [`DOCKER`]: `src/docker.rs:42-130`, against the `oci` adapter's `render`.
//!
//! The equivalent profile is `sandbox`'s default layout: the working directory bound writable at
//! `/workspace` and started in, one declared read-only root at its own path, no network, and the
//! five baseline variables, which come from `b10x-substrate confine run`'s own command. A row
//! whose second column is `None` has no counterpart in any profile Substrate serves; those rows are
//! pinned by name ([`BUBBLEWRAP_UNMIRRORED`], [`DOCKER_UNMIRRORED`]), so an argument Substrate
//! stops carrying fails here rather than joining that list unnoticed.
//!
//! In a group, `{fd}` is any descriptor number, and `{root}`, `{workspace}`, `{user}` and
//! `{image}` are the profile's values. `linux` binds a read-only root only on a host that proves
//! `filesystem_view`, which needs the delegated cgroup v2 root `SUBSTRATE_VECTORS_CGROUP_ROOT`
//! names; without it that case is **absent, never reported as passed** (invariant 3), and the
//! portable case asserts every other row.

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementProfile, Network, ReadOnlyRoot,
    RenderedCommand, linux::LinuxRequest, oci::OciRequest,
};

/// One row: what `sandbox` passed, and the group Substrate's render carries for it, or `None`.
type Row = (&'static [&'static str], Option<&'static [&'static str]>);

/// `sandbox`'s bubblewrap list, `src/confinement.rs:179-258` at `1b0377d`, in its order.
const BUBBLEWRAP: &[Row] = &[
    // 1. Namespaces and lifetime.
    (
        &["--unshare-user", "--disable-userns"],
        Some(&["--unshare-user", "--disable-userns"]),
    ),
    (&["--unshare-ipc"], Some(&["--unshare-ipc"])),
    (&["--unshare-pid"], Some(&["--unshare-pid"])),
    (&["--unshare-uts"], Some(&["--unshare-uts"])),
    // Omitted by `sandbox --net`; Substrate's `linux` has no such subtraction.
    (&["--unshare-net"], Some(&["--unshare-net"])),
    (&["--die-with-parent"], Some(&["--die-with-parent"])),
    // 2. Base system read-only, private proc, dev and tmp.
    (
        &["--ro-bind", "/usr", "/usr"],
        Some(&["--ro-bind", "/usr", "/usr"]),
    ),
    (
        &["--ro-bind-try", "/bin", "/bin"],
        Some(&["--ro-bind-try", "/bin", "/bin"]),
    ),
    (
        &["--ro-bind-try", "/lib", "/lib"],
        Some(&["--ro-bind-try", "/lib", "/lib"]),
    ),
    (
        &["--ro-bind-try", "/lib64", "/lib64"],
        Some(&["--ro-bind-try", "/lib64", "/lib64"]),
    ),
    (&["--proc", "/proc"], Some(&["--proc", "/proc"])),
    (&["--dev", "/dev"], Some(&["--dev", "/dev"])),
    (&["--tmpfs", "/tmp"], Some(&["--tmpfs", "/tmp"])),
    // 3. A declared read-only root at its own path; Substrate binds it by descriptor.
    (
        &["--ro-bind", "{root}", "{root}"],
        Some(&["--ro-bind-fd", "{fd}", "{root}"]),
    ),
    // 4. The workspace. `sandbox` lays a tmpfs at `/workspace` for the parents of a mirrored
    // layout; Substrate binds the one workspace directory at `/workspace` itself and synthesizes
    // no parents, so neither the tmpfs nor the remount has a counterpart.
    (&["--tmpfs", "/workspace"], None),
    (
        &["--bind", "{workspace}", "/workspace"],
        Some(&["--bind-fd", "{fd}", "/workspace"]),
    ),
    (&["--remount-ro", "/workspace"], None),
    // 5. Working directory and the shaped environment.
    (&["--chdir", "/workspace"], Some(&["--chdir", "/workspace"])),
    (&["--clearenv"], Some(&["--clearenv"])),
    (
        &["--setenv", "PATH", "/usr/bin:/bin"],
        Some(&["--setenv", "PATH", "/usr/bin:/bin"]),
    ),
    (
        &["--setenv", "HOME", "/tmp"],
        Some(&["--setenv", "HOME", "/tmp"]),
    ),
    (
        &["--setenv", "LANG", "C.UTF-8"],
        Some(&["--setenv", "LANG", "C.UTF-8"]),
    ),
    (
        &["--setenv", "LC_ALL", "C.UTF-8"],
        Some(&["--setenv", "LC_ALL", "C.UTF-8"]),
    ),
    (&["--setenv", "TZ", "UTC"], Some(&["--setenv", "TZ", "UTC"])),
    // 6. The command.
    (&["--"], Some(&["--"])),
];

/// The `sandbox` bubblewrap rows no Substrate profile reproduces, by their flag and operand.
const BUBBLEWRAP_UNMIRRORED: [&str; 2] = ["--tmpfs /workspace", "--remount-ro /workspace"];

/// What Substrate's `linux` carries beyond `sandbox`'s list.
const BUBBLEWRAP_ADDED: &[&[&str]] = &[&["--new-session"], &["--seccomp", "{fd}"]];

/// `sandbox`'s `docker run` list, `src/docker.rs:42-130` at `1b0377d`, in its order.
const DOCKER: &[Row] = &[
    // 1. One-shot, interactive, a terminal when the caller has one. `oci` renders the `create`
    // call, follows it with `start --attach`, and removes the container itself once the run has
    // ended, so `--rm` has no argument; it gives the run no standard input and no terminal.
    (&["run"], Some(&["create"])),
    (&["--rm"], None),
    (&["-i"], None),
    (&["-t"], None),
    // 2. The caller's identity.
    (&["--user", "{user}"], Some(&["--user", "{user}"])),
    // 3. No network; omitted by `sandbox --net`, never by `oci`.
    (&["--network", "none"], Some(&["--network", "none"])),
    // 4. Capabilities, privilege escalation, the image root and a private tmp.
    (&["--cap-drop", "ALL"], Some(&["--cap-drop", "ALL"])),
    (
        &["--security-opt", "no-new-privileges"],
        Some(&["--security-opt", "no-new-privileges"]),
    ),
    (&["--read-only"], Some(&["--read-only"])),
    (&["--tmpfs", "/tmp"], Some(&["--tmpfs", "/tmp"])),
    // 5. The tmpfs a mirrored layout's parents hang under; no Substrate profile has one.
    (
        &[
            "--mount",
            "type=tmpfs,destination=/workspace,tmpfs-mode=0555",
        ],
        None,
    ),
    // 6. The read-only root at its own path, then the writable workspace.
    (
        &["-v", "{root}:{root}:ro"],
        Some(&["--mount", "type=bind,source={root},target={root},readonly"]),
    ),
    (
        &["-v", "{workspace}:/workspace"],
        Some(&["--mount", "type=bind,source={workspace},target=/workspace"]),
    ),
    // 7. Working directory.
    (&["-w", "/workspace"], Some(&["-w", "/workspace"])),
    // 8. The baseline both backends set.
    (
        &["-e", "PATH=/usr/bin:/bin"],
        Some(&["-e", "PATH=/usr/bin:/bin"]),
    ),
    (&["-e", "HOME=/tmp"], Some(&["-e", "HOME=/tmp"])),
    (&["-e", "LANG=C.UTF-8"], Some(&["-e", "LANG=C.UTF-8"])),
    (&["-e", "LC_ALL=C.UTF-8"], Some(&["-e", "LC_ALL=C.UTF-8"])),
    (&["-e", "TZ=UTC"], Some(&["-e", "TZ=UTC"])),
    // 9. The image, then the command.
    (&["{image}"], Some(&["--", "{image}"])),
];

/// The `sandbox` Docker rows no `oci` render reproduces, by their flag and operand.
const DOCKER_UNMIRRORED: [&str; 4] = [
    "--rm",
    "-i",
    "-t",
    "--mount type=tmpfs,destination=/workspace,tmpfs-mode=0555",
];

/// What `oci` carries beyond `sandbox`'s list.
const DOCKER_ADDED: &[&[&str]] = &[
    &["--pull", "never"],
    &["--init"],
    &["--ipc", "none"],
    &["--log-driver", "none"],
];

const IMAGE: &str =
    "registry.invalid/run@sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// The values a row's placeholders stand for.
struct Values {
    root: Option<String>,
    workspace: String,
    user: String,
    image: String,
}

impl Values {
    /// Whether `word` matches `pattern`, every placeholder in it replaced.
    fn matches(&self, pattern: &str, word: &str) -> bool {
        if pattern == "{fd}" {
            return word.parse::<u32>().is_ok();
        }
        let mut expanded = pattern
            .replace("{workspace}", &self.workspace)
            .replace("{user}", &self.user)
            .replace("{image}", &self.image);
        if let Some(root) = &self.root {
            expanded = expanded.replace("{root}", root);
        }
        expanded == word
    }

    fn mentions_root(group: &[&str]) -> bool {
        group.iter().any(|word| word.contains("{root}"))
    }
}

/// Whether `group` appears as consecutive arguments of `argv`.
fn contains(values: &Values, argv: &[String], group: &[&str]) -> bool {
    argv.windows(group.len()).any(|window| {
        window
            .iter()
            .zip(group)
            .all(|(word, pattern)| values.matches(pattern, word))
    })
}

/// Asserts every row with a counterpart is in `argv`, that the rows without one are exactly
/// `unmirrored`, and that `added` is there too. Rows naming the read-only root are skipped when
/// the profile has none. Returns how many rows were asserted present.
fn assert_superset(
    table: &[Row],
    unmirrored: &[&str],
    added: &[&[&str]],
    values: &Values,
    argv: &[String],
) -> usize {
    let mut asserted = 0;
    let mut without: Vec<String> = Vec::new();
    for (sandbox, substrate) in table {
        let Some(substrate) = substrate else {
            without.push(sandbox.join(" "));
            continue;
        };
        if values.root.is_none() && Values::mentions_root(substrate) {
            continue;
        }
        assert!(
            contains(values, argv, substrate),
            "sandbox's {sandbox:?} has no {substrate:?} in Substrate's render: {argv:?}"
        );
        asserted += 1;
    }
    assert_eq!(
        without, unmirrored,
        "the sandbox rows without a Substrate counterpart changed"
    );
    for group in added {
        assert!(
            contains(values, argv, group),
            "Substrate no longer carries {group:?}: {argv:?}"
        );
    }
    asserted
}

/// The command `b10x-substrate confine run` itself hands its adapter: its environment is the
/// verb's, not one this test supplies, so the `--setenv` and `-e` rows hold only while the verb
/// sets `sandbox`'s five baseline variables (`story:sandbox-repository-parity`).
fn command() -> Command {
    b10x_substrate_cli::confine_command(vec!["/bin/sh".into(), "-c".into(), "true".into()])
}

/// A canonical directory under `CARGO_TARGET_TMPDIR`, holding `workspaces/ws_parity` (the
/// workspace) and `toolchain` (the read-only root).
struct Host {
    _directory: tempfile::TempDir,
    workspaces: PathBuf,
    workspace: PathBuf,
    toolchain: PathBuf,
}

fn host() -> Host {
    let directory = tempfile::Builder::new()
        .prefix("sandbox-parity-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .expect("host directory");
    let root = std::fs::canonicalize(directory.path()).expect("canonical");
    let workspaces = root.join("workspaces");
    let workspace = workspaces.join("ws_parity");
    let toolchain = root.join("toolchain");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(&toolchain).expect("toolchain");
    Host {
        _directory: directory,
        workspaces,
        workspace,
        toolchain,
    }
}

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8 path").to_owned()
}

fn profile(host: &Host, root: bool) -> ConfinementProfile {
    ConfinementProfile {
        workspace: Some(host.workspace.clone()),
        read_only_roots: if root {
            vec![ReadOnlyRoot {
                host_path: text(&host.toolchain),
                mount: text(&host.toolchain),
            }]
        } else {
            Vec::new()
        },
        network: Some(Network::Isolated),
        ..ConfinementProfile::default()
    }
}

fn user() -> String {
    let metadata = std::fs::metadata("/proc/self").expect("/proc/self");
    format!("{}:{}", metadata.uid(), metadata.gid())
}

fn render(adapter: &Arc<dyn ConfinementAdapter>, profile: &ConfinementProfile) -> RenderedCommand {
    let admitted = adapter
        .admit(profile)
        .expect("the equivalent profile is admitted");
    adapter
        .render(&admitted, &command())
        .expect("the equivalent profile renders")
}

fn open_linux(host: &Host, cgroup_root: Option<PathBuf>) -> Arc<dyn ConfinementAdapter> {
    confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(HostConfig::minimum(&host.workspaces)).with_cgroup_root(cgroup_root),
    )))
    .expect("linux opens")
}

/// The delegated cgroup v2 subtree this process is inside, or the reason the case is absent.
fn delegated() -> Result<PathBuf, &'static str> {
    let root = PathBuf::from(
        std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")
            .ok_or("SUBSTRATE_VECTORS_CGROUP_ROOT is unset")?,
    );
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids")
        .map_err(|_| "the delegated cgroup root does not accept controllers")?;
    Ok(root)
}

#[test]
fn the_tables_hold_sandboxs_lists() {
    assert_eq!(
        BUBBLEWRAP.len(),
        25,
        "the bubblewrap table lost or gained a row"
    );
    assert_eq!(DOCKER.len(), 20, "the Docker table lost or gained a row");
}

#[test]
fn linux_render_carries_every_sandbox_bubblewrap_row_but_the_read_only_root() {
    let host = host();
    let rendered = render(&open_linux(&host, None), &profile(&host, false));
    let values = Values {
        root: None,
        workspace: text(&host.workspace),
        user: user(),
        image: IMAGE.to_owned(),
    };
    let asserted = assert_superset(
        BUBBLEWRAP,
        &BUBBLEWRAP_UNMIRRORED,
        BUBBLEWRAP_ADDED,
        &values,
        &rendered.args,
    );
    assert_eq!(asserted, 22);
    println!("sandbox parity bubblewrap portable: executed, {asserted} rows present");
}

#[test]
fn linux_render_carries_every_sandbox_bubblewrap_row_with_a_read_only_root() {
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("sandbox parity bubblewrap delegated: ABSENT ({reason})");
            return;
        }
    };
    let host = host();
    let rendered = render(&open_linux(&host, Some(cgroup_root)), &profile(&host, true));
    let values = Values {
        root: Some(text(&host.toolchain)),
        workspace: text(&host.workspace),
        user: user(),
        image: IMAGE.to_owned(),
    };
    let asserted = assert_superset(
        BUBBLEWRAP,
        &BUBBLEWRAP_UNMIRRORED,
        BUBBLEWRAP_ADDED,
        &values,
        &rendered.args,
    );
    assert_eq!(asserted, 23);
    println!("sandbox parity bubblewrap delegated: executed, {asserted} rows present");
}

/// A stand-in runtime that answers `info` as a Docker with seccomp and every limit, and nothing
/// else: `render` runs no container.
fn stand_in_runtime(directory: &Path) -> PathBuf {
    let path = directory.join("docker");
    let info = "{\"ServerVersion\":\"29.0.0\",\"SecurityOptions\":[\"name=seccomp,profile=builtin\"],\
                \"MemoryLimit\":true,\"SwapLimit\":true,\"PidsLimit\":true,\"CpuCfsQuota\":true,\
                \"NCPU\":4}";
    std::fs::write(
        &path,
        format!("#!/bin/sh\nif [ \"$1\" = info ]; then printf '%s' '{info}'; exit 0; fi\nexit 1\n"),
    )
    .expect("stand-in runtime");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("mode");
    path
}

#[test]
fn oci_render_carries_every_sandbox_docker_row() {
    let host = host();
    let bin = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("runtime directory");
    let runtime = stand_in_runtime(bin.path());
    // A just-written script can be busy for a moment if another test thread forked while it was
    // open for writing (`ETXTBSY`); open retries nothing, so this does.
    let mut opened = None;
    for _ in 0..100 {
        match confinement::open(AdapterRequest::Oci(OciRequest {
            runtime: runtime.clone(),
            workspace_root: host.workspaces.clone(),
            writable_roots: Vec::new(),
        })) {
            Ok(adapter) => {
                opened = Some(adapter);
                break;
            }
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
    let adapter = opened.expect("oci opens over the stand-in runtime");
    let mut equivalent = profile(&host, true);
    equivalent.image = Some(IMAGE.to_owned());
    let rendered = render(&adapter, &equivalent);
    assert_eq!(rendered.program, runtime);
    let values = Values {
        root: Some(text(&host.toolchain)),
        workspace: text(&host.workspace),
        user: user(),
        image: IMAGE.to_owned(),
    };
    let asserted = assert_superset(
        DOCKER,
        &DOCKER_UNMIRRORED,
        DOCKER_ADDED,
        &values,
        &rendered.args,
    );
    assert_eq!(asserted, 16);
    let image_at = rendered
        .args
        .iter()
        .position(|word| word == IMAGE)
        .expect("the image");
    assert_eq!(rendered.args[image_at + 1..], command().argv);
    println!("sandbox parity docker: executed, {asserted} rows present");
}
