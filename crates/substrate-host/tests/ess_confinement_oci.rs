//! The confinement contract's `oci` lane (`story:oci-adapter`).
//!
//! With `SUBSTRATE_OCI_LANE=1` it executes every authored `oci-*-on-oci` scenario of
//! `spec/confinement-suite.json` against the real `oci` adapter over a container runtime
//! (`SUBSTRATE_OCI_RUNTIME`, default `docker` on `PATH`), and runs containers through it: the pids
//! bound refuses a fork, a write outside the declared mounts fails, an outbound connection fails,
//! and no container with the run's label remains after `wait`, a kill or a timeout. With the
//! variable set, an absent runtime **fails** these cases. Without it they are **absent, never
//! reported as passed** (invariant 3): each prints `ABSENT` and its reason.
//!
//! The stdin cases and the scenarios' `stdin_pipe` follow the runtime's own probe: on a runtime
//! outside the stdin pin each asserts that a piped profile is refused `confinement.fact-unserved`
//! naming `stdin_pipe`, and prints which branch it took.
//!
//! The portable cases need no runtime: a stand-in program answers `info`, and they assert what
//! `open` reads from it, the named refusals of admission, and the `render` argv.

mod support;

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use substrate_host::DeclaredWritableRoot;
use substrate_host::confinement::{
    self, AdapterName, AdapterRequest, AuditRecord, AuditSink, Command, ConfinementAdapter,
    ConfinementError, ConfinementProfile, ContractFamily, Fact, Network, ProcessBounds, RunState,
    oci::{self, OciRequest, RuntimeFlavor},
};
use substrate_wire::{ReadOnlyRoot, SecretSlotRequest, WritableHostRoot};

const LANE_ENV: &str = "SUBSTRATE_OCI_LANE";
const RUNTIME_ENV: &str = "SUBSTRATE_OCI_RUNTIME";
const IMAGE_ENV: &str = "SUBSTRATE_OCI_IMAGE";
/// Alpine 3.20's image index by digest: `sh`, `touch` and `wget` from busybox.
const IMAGE: &str = "docker.io/library/alpine@sha256:d9e853e87e55526f6b2917df91a2115c36dd7c696a35be12163d44e6e2a4b6bc";
const PINNED: &str =
    "registry.invalid/run@sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn recording() -> (AuditSink, Arc<Mutex<Vec<AuditRecord>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink_records = Arc::clone(&records);
    (
        AuditSink::new(move |record: &AuditRecord| sink_records.lock().push(record.clone())),
        records,
    )
}

fn events(records: &Mutex<Vec<AuditRecord>>) -> Vec<serde_json::Value> {
    records
        .lock()
        .iter()
        .map(|record| serde_json::to_value(record).expect("record"))
        .collect()
}

fn canonical_tempdir() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = std::fs::canonicalize(dir.path()).expect("canonical");
    (dir, path)
}

/// The operator's workspace root these cases declare: every [`canonical_tempdir`] sits directly
/// beneath it.
fn workspace_root() -> PathBuf {
    std::fs::canonicalize(std::env::temp_dir()).expect("canonical temporary directory")
}

// ---- portable: a stand-in runtime answers `info` ----

/// Stand-in runtime programs, written once before any of them runs.
struct Fakes {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn fakes() -> &'static Fakes {
    static FAKES: OnceLock<Fakes> = OnceLock::new();
    FAKES.get_or_init(|| {
        let (dir, root) = canonical_tempdir();
        let docker = |security: &str, limits: bool| {
            format!(
                "{{\"ServerVersion\":\"29.0.0\",\"SecurityOptions\":[{security}],\
                 \"MemoryLimit\":{limits},\"SwapLimit\":{limits},\"PidsLimit\":{limits},\
                 \"CpuCfsQuota\":{limits},\"NCPU\":4}}"
            )
        };
        let seccomp = "\"name=seccomp,profile=builtin\"";
        // A podman whose API socket is beneath `held/`, a directory beside the stand-ins.
        std::fs::create_dir(root.join("held")).expect("socket directory");
        let podman = format!(
            "{{\"host\":{{\"security\":{{\"seccompEnabled\":true,\"rootless\":true}},\
             \"cgroupControllers\":[\"cpu\",\"memory\",\"pids\"],\"cpus\":4,\
             \"remoteSocket\":{{\"exists\":true,\"path\":\"unix://{}/held/podman.sock\"}}}},\
             \"version\":{{\"Version\":\"5.0.0\"}}}}",
            root.display()
        );
        for (name, info) in [
            ("podman-socket", Some(podman)),
            ("docker-ok", Some(docker(seccomp, true))),
            ("docker-no-limits", Some(docker(seccomp, false))),
            (
                "docker-unconfined",
                Some(docker("\"name=seccomp,profile=unconfined\"", true)),
            ),
            ("docker-no-seccomp", Some(docker("\"name=apparmor\"", true))),
            ("docker-down", None),
        ] {
            let body = match info {
                Some(info) => format!(
                    "#!/bin/sh\nif [ \"$1\" = info ]; then printf '%s' '{info}'; exit 0; fi\nexit 1\n"
                ),
                None => "#!/bin/sh\necho 'Cannot connect' >&2\nexit 1\n".to_owned(),
            };
            let path = root.join(name);
            std::fs::write(&path, body).expect("fake runtime");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("executable");
            executable_now(&path);
        }
        Fakes { _dir: dir, root }
    })
}

/// Waits until a just-written script can be executed. A thread of this process that forks while
/// the script's write descriptor is open hands that descriptor to its child until the child execs,
/// and the kernel refuses to execute a file open for writing (`ETXTBSY`). Once every script is
/// written and has run once, nothing writes them again.
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

fn fake(name: &str) -> PathBuf {
    fakes().root.join(name)
}

fn open_fake(
    name: &str,
    writable_roots: Vec<DeclaredWritableRoot>,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    confinement::open(AdapterRequest::Oci(OciRequest {
        runtime: fake(name),
        workspace_root: workspace_root(),
        writable_roots,
    }))
}

fn pair(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == flag)
        .map(|window| window[1].clone())
}

#[test]
#[allow(clippy::too_many_lines)] // One rendered argv, then each flag it holds.
fn render_holds_the_bounds_and_the_image_digest_sandbox_lacks() {
    let (_ws, workspace) = canonical_tempdir();
    std::fs::create_dir(workspace.join("sub")).expect("workdir");
    let (_ro, read_only) = canonical_tempdir();
    let adapter =
        open_fake("docker-ok", Vec::new()).expect("oci opens over a runtime with seccomp");
    let admitted = adapter
        .admit(&ConfinementProfile {
            image: Some(PINNED.into()),
            workspace: Some(workspace.clone()),
            workdir: Some("sub".into()),
            read_only_roots: vec![ReadOnlyRoot {
                host_path: read_only.display().to_string(),
                mount: "/opt/ro".into(),
            }],
            process_bounds: Some(ProcessBounds {
                timeout_ms: 10_000,
                processes: 8,
                memory_bytes: 64 << 20,
                cpu_millis: 5_000,
            }),
            network: Some(Network::Isolated),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let rendered = adapter
        .render(
            &admitted,
            &Command {
                argv: vec!["/bin/sh".into(), "-c".into(), "true".into()],
                env: BTreeMap::from([("TERM".to_owned(), "xterm".to_owned())]),
            },
        )
        .expect("rendered");
    assert_eq!(rendered.program, fake("docker-ok"));
    let args = &rendered.args;
    // What `sandbox` lacks.
    assert_eq!(pair(args, "--pids-limit").as_deref(), Some("8"), "{args:?}");
    assert_eq!(pair(args, "--memory").as_deref(), Some("67108864"));
    assert_eq!(pair(args, "--memory-swap").as_deref(), Some("67108864"));
    assert_eq!(pair(args, "--cpus").as_deref(), Some("0.500"));
    assert_eq!(pair(args, "--").as_deref(), Some(PINNED));
    assert!(!args.iter().any(|arg| arg.contains("seccomp")), "{args:?}");
    // The posture, and the command after the image. The container is created, then started
    // attached, inspected and removed: never `--rm`, which would discard its state.
    assert_eq!(args[0], "create", "{args:?}");
    assert!(!args.iter().any(|arg| arg == "--rm"), "{args:?}");
    assert!(args.iter().any(|arg| arg == "--init"), "{args:?}");
    for (flag, value) in [
        ("--pull", "never"),
        ("--ipc", "none"),
        ("--network", "none"),
        ("--cap-drop", "ALL"),
        ("--security-opt", "no-new-privileges"),
        ("--tmpfs", "/tmp"),
        ("--log-driver", "none"),
        ("-w", "/workspace/sub"),
        ("--entrypoint", ""),
    ] {
        assert_eq!(pair(args, flag).as_deref(), Some(value), "{flag}: {args:?}");
    }
    assert!(args.iter().any(|arg| arg == "--read-only"));
    assert!(!args.iter().any(|arg| arg == "-v"), "{args:?}");
    let binds: Vec<&String> = args
        .windows(2)
        .filter(|window| window[0] == "--mount")
        .map(|window| &window[1])
        .collect();
    assert_eq!(
        binds,
        [
            &format!("type=bind,source={},target=/workspace", workspace.display()),
            &format!(
                "type=bind,source={},target=/opt/ro,readonly",
                read_only.display()
            ),
        ]
    );
    let embedder = args
        .iter()
        .find_map(|arg| arg.strip_prefix(&format!("{}=", oci::EMBEDDER_LABEL)));
    // The embedder identity is read from `/proc`; without it no label is written
    // (`story:oci-adapter`).
    if cfg!(target_os = "linux") {
        let embedder = embedder.expect("the embedder label");
        assert_eq!(
            embedder.split('/').nth(2),
            Some(std::process::id().to_string().as_str()),
            "{embedder}"
        );
    } else {
        assert_eq!(embedder, None, "{args:?}");
    }
    let env: Vec<&String> = args
        .windows(2)
        .filter(|window| window[0] == "-e")
        .map(|window| &window[1])
        .collect();
    assert_eq!(
        env,
        [
            "PATH=/usr/bin:/bin",
            "HOME=/tmp",
            "LANG=C.UTF-8",
            "LC_ALL=C.UTF-8",
            "TZ=UTC",
            "TERM=xterm"
        ]
    );
    let user = pair(args, "--user").expect("--user");
    assert!(
        user.split(':').count() == 2 && user.split(':').all(|id| id.parse::<u32>().is_ok()),
        "{user}"
    );
    assert_eq!(&args[args.len() - 3..], ["/bin/sh", "-c", "true"]);
    let name = pair(args, "--name").expect("--name");
    assert!(
        args.iter()
            .any(|arg| *arg == format!("{}={name}", oci::RUN_LABEL)),
        "{args:?}"
    );
}

#[test]
fn open_fails_closed_without_a_runtime_seccomp_or_info() {
    for runtime in [
        fake("docker-unconfined"),
        fake("docker-no-seccomp"),
        fake("docker-down"),
        PathBuf::from("/nonexistent/substrate-oci-runtime"),
        PathBuf::from("relative/docker"),
    ] {
        let error = confinement::open(AdapterRequest::Oci(OciRequest {
            runtime: runtime.clone(),
            workspace_root: workspace_root(),
            writable_roots: Vec::new(),
        }))
        .err()
        .unwrap_or_else(|| panic!("{} must not open", runtime.display()));
        assert!(
            matches!(
                &error,
                ConfinementError::AdapterUnserved {
                    adapter: AdapterName::Oci,
                    ..
                }
            ),
            "{}: {error}",
            runtime.display()
        );
        assert_eq!(error.code(), "confinement.adapter-unserved");
    }
}

#[test]
fn open_reads_rootless_and_proves_image_digest_pinned() {
    let probe = oci::probe(&OciRequest {
        runtime: fake("docker-ok"),
        workspace_root: workspace_root(),
        writable_roots: Vec::new(),
    })
    .expect("probed");
    assert_eq!(probe.flavor, RuntimeFlavor::Docker);
    assert!(!probe.rootless);
    let adapter = open_fake("docker-ok", Vec::new()).expect("opens");
    let facts = serde_json::to_value(adapter.facts()).expect("facts");
    assert_eq!(
        facts["served_families"],
        serde_json::json!([
            "process_run",
            "process_watch",
            "output_read",
            "wait",
            "workspace",
            "filesystem_view",
            "limits",
            "audit"
        ])
    );
    assert_eq!(
        facts["facts"],
        serde_json::json!(["image_digest_pinned", "output_live"])
    );
}

#[test]
fn a_runtime_without_limit_control_does_not_serve_limits() {
    let adapter = open_fake("docker-no-limits", Vec::new()).expect("opens");
    assert!(!adapter.facts().serves(&ContractFamily::Limits));
    let (sink, records) = recording();
    let error = adapter
        .admit(&ConfinementProfile {
            image: Some(PINNED.into()),
            process_bounds: Some(DEFAULT_TEST_BOUNDS),
            audit: Some(sink),
            ..ConfinementProfile::default()
        })
        .expect_err("limits is refused");
    assert!(
        matches!(
            error,
            ConfinementError::FamilyUnserved {
                adapter: AdapterName::Oci,
                family: ContractFamily::Limits
            }
        ),
        "{error}"
    );
    let records = events(&records);
    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(records[0]["event"], "profile_refused");
    assert_eq!(records[0]["family"], "limits");
}

const DEFAULT_TEST_BOUNDS: ProcessBounds = ProcessBounds {
    timeout_ms: 30_000,
    processes: 32,
    memory_bytes: 128 << 20,
    cpu_millis: 30_000,
};

/// Every refusal an `oci` admission names, each with exactly one `profile_refused` record.
#[test]
#[allow(clippy::too_many_lines)] // One table of cases.
fn admission_refuses_by_name_and_audits_each_refusal() {
    let digest = "0".repeat(64);
    let base = || ConfinementProfile {
        image: Some(PINNED.into()),
        ..ConfinementProfile::default()
    };
    let cases: Vec<(&str, ConfinementProfile, &str)> = vec![
        (
            "a tag only",
            ConfinementProfile {
                image: Some("registry.invalid/run:latest".into()),
                ..base()
            },
            "confinement.image-unpinned",
        ),
        (
            "no image",
            ConfinementProfile {
                image: None,
                ..base()
            },
            "confinement.image-unpinned",
        ),
        (
            "an upper-case digest",
            ConfinementProfile {
                image: Some(format!("registry.invalid/run@sha256:{}", "A".repeat(64))),
                ..base()
            },
            "confinement.image-unpinned",
        ),
        (
            "a digest with a tag after it",
            ConfinementProfile {
                image: Some(format!("registry.invalid/run@sha256:{digest}:tag")),
                ..base()
            },
            "confinement.image-unpinned",
        ),
        (
            "a name the runtime would read as a flag",
            ConfinementProfile {
                image: Some(format!("-v/:/host@sha256:{digest}")),
                ..base()
            },
            "confinement.image-unpinned",
        ),
        (
            "network egress",
            ConfinementProfile {
                network: Some(Network::Egress {
                    apertures: vec!["registry".into()],
                }),
                ..base()
            },
            "confinement.family-unserved",
        ),
        (
            "a fact oci does not prove",
            ConfinementProfile {
                facts: vec![Fact::WholeTreeKill],
                ..base()
            },
            "confinement.fact-unserved",
        ),
        (
            "a secret slot",
            ConfinementProfile {
                secret_slots: vec![SecretSlotRequest {
                    slot: "token".into(),
                    fd: 3,
                }],
                ..base()
            },
            "exec.secret-slots-unserved",
        ),
        (
            "a read-only root mount with a colon",
            ConfinementProfile {
                read_only_roots: vec![ReadOnlyRoot {
                    host_path: "/srv/data".into(),
                    mount: "/opt/a:b".into(),
                }],
                ..base()
            },
            oci::PATH_UNREPRESENTABLE,
        ),
        (
            "a read-only root at a reserved mount",
            ConfinementProfile {
                read_only_roots: vec![ReadOnlyRoot {
                    host_path: "/srv/data".into(),
                    mount: "/workspace".into(),
                }],
                ..base()
            },
            substrate_wire::READ_ONLY_ROOT_INVALID,
        ),
        (
            "an undeclared writable root",
            ConfinementProfile {
                writable_roots: vec![WritableHostRoot {
                    host: "cache".into(),
                    mount: "/cache".into(),
                }],
                ..base()
            },
            substrate_wire::EXEC_WRITABLE_ROOTS_HOST_UNSERVED,
        ),
        (
            "a memory bound below the runtime's minimum",
            ConfinementProfile {
                process_bounds: Some(ProcessBounds {
                    memory_bytes: 1 << 20,
                    ..DEFAULT_TEST_BOUNDS
                }),
                ..base()
            },
            "exec.limit-unserved",
        ),
        (
            "a CPU budget below the smallest share",
            ConfinementProfile {
                process_bounds: Some(ProcessBounds {
                    timeout_ms: 600_000,
                    cpu_millis: 1_000,
                    ..DEFAULT_TEST_BOUNDS
                }),
                ..base()
            },
            "exec.limit-unserved",
        ),
    ];
    let adapter = open_fake("docker-ok", Vec::new()).expect("opens");
    for (case, mut profile, code) in cases {
        let (sink, records) = recording();
        profile.audit = Some(sink);
        let error = adapter.admit(&profile).expect_err(case);
        assert_eq!(error.code(), code, "{case}: {error}");
        let records = events(&records);
        assert_eq!(records.len(), 1, "{case}: {records:?}");
        assert_eq!(records[0]["event"], "profile_refused", "{case}");
        assert_eq!(records[0]["code"], code, "{case}");
        assert_eq!(records[0]["adapter"], "oci", "{case}");
    }
}

#[test]
fn a_declared_writable_root_is_bound_writable_and_checked_on_the_host() {
    let (_ws, workspace) = canonical_tempdir();
    let (_cache, cache) = canonical_tempdir();
    let declared = vec![
        DeclaredWritableRoot {
            name: "cache".into(),
            path: cache.clone(),
        },
        DeclaredWritableRoot {
            name: "gone".into(),
            path: cache.join("absent"),
        },
    ];
    let adapter = open_fake("docker-ok", declared).expect("opens");
    let command = Command {
        argv: vec!["/bin/true".into()],
        env: BTreeMap::new(),
    };
    let profile = |host: &str| ConfinementProfile {
        image: Some(PINNED.into()),
        workspace: Some(workspace.clone()),
        writable_roots: vec![WritableHostRoot {
            host: host.into(),
            mount: "/cache".into(),
        }],
        ..ConfinementProfile::default()
    };
    let admitted = adapter.admit(&profile("cache")).expect("admitted");
    let rendered = adapter.render(&admitted, &command).expect("rendered");
    assert!(
        rendered.args.contains(&format!(
            "type=bind,source={},target=/cache",
            cache.display()
        )),
        "{:?}",
        rendered.args
    );
    let admitted = adapter.admit(&profile("gone")).expect("admitted");
    let error = adapter
        .render(&admitted, &command)
        .expect_err("an absent root is refused");
    assert_eq!(error.code(), substrate_wire::EXEC_WRITABLE_ROOT_ABSENT);
}

#[test]
fn render_refuses_an_environment_the_runtime_cannot_carry_exactly() {
    let (_ws, workspace) = canonical_tempdir();
    let adapter = open_fake("docker-ok", Vec::new()).expect("opens");
    let admitted = adapter
        .admit(&ConfinementProfile {
            image: Some(PINNED.into()),
            workspace: Some(workspace),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    for (name, value) in [("A=B", "x"), ("", "x"), ("A", "nul\0")] {
        let error = adapter
            .render(
                &admitted,
                &Command {
                    argv: vec!["/bin/true".into()],
                    env: BTreeMap::from([(name.to_owned(), value.to_owned())]),
                },
            )
            .expect_err("refused");
        assert_eq!(error.code(), "exec.environment-refused", "{name:?}");
    }
}

/// Security F1 (`story:oci-adapter`): the workspace is a directory directly beneath the operator's
/// workspace root, as under `linux`; anything else is `workspace.path-escape`, and the refused
/// render leaves an audit record.
#[test]
fn a_workspace_not_directly_beneath_the_workspace_root_is_refused_and_audited() {
    let (_ws, workspace) = canonical_tempdir();
    std::fs::create_dir(workspace.join("nested")).expect("nested");
    let adapter = open_fake("docker-ok", Vec::new()).expect("opens");
    let command = Command {
        argv: vec!["/bin/true".into()],
        env: BTreeMap::new(),
    };
    for refused in [PathBuf::from("/"), workspace.join("nested")] {
        let (sink, records) = recording();
        let admitted = adapter
            .admit(&ConfinementProfile {
                image: Some(PINNED.into()),
                workspace: Some(refused.clone()),
                audit: Some(sink),
                ..ConfinementProfile::default()
            })
            .expect("admitted");
        let error = adapter
            .render(&admitted, &command)
            .expect_err("a workspace elsewhere is refused");
        assert_eq!(
            error.code(),
            "workspace.path-escape",
            "{}",
            refused.display()
        );
        let records = events(&records);
        assert_eq!(records.len(), 2, "{records:?}");
        assert_eq!(records[1]["event"], "profile_refused");
        assert_eq!(records[1]["code"], "workspace.path-escape");
    }
    let admitted = adapter
        .admit(&ConfinementProfile {
            image: Some(PINNED.into()),
            workspace: Some(workspace),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    adapter
        .render(&admitted, &command)
        .expect("a workspace directly beneath the root renders");
}

/// Security F4 (`story:oci-adapter`): a workspace, read-only root or writable root that is, or
/// holds, the socket the runtime reports is refused by name.
#[test]
fn a_bind_holding_the_runtime_socket_is_refused_by_name() {
    let holder = fakes().root.join("held");
    let adapter = open_fake(
        "podman-socket",
        vec![DeclaredWritableRoot {
            name: "held".into(),
            path: holder.clone(),
        }],
    )
    .expect("opens");
    let (_ws, workspace) = canonical_tempdir();
    let command = Command {
        argv: vec!["/bin/true".into()],
        env: BTreeMap::new(),
    };
    let base = || ConfinementProfile {
        image: Some(PINNED.into()),
        workspace: Some(workspace.clone()),
        ..ConfinementProfile::default()
    };
    for (case, profile) in [
        (
            "the workspace",
            ConfinementProfile {
                workspace: Some(fakes().root.clone()),
                ..base()
            },
        ),
        (
            "a read-only root",
            ConfinementProfile {
                read_only_roots: vec![ReadOnlyRoot {
                    host_path: holder.display().to_string(),
                    mount: "/opt/held".into(),
                }],
                ..base()
            },
        ),
        (
            "a writable root",
            ConfinementProfile {
                writable_roots: vec![WritableHostRoot {
                    host: "held".into(),
                    mount: "/held".into(),
                }],
                ..base()
            },
        ),
    ] {
        let admitted = adapter.admit(&profile).expect(case);
        let error = adapter.render(&admitted, &command).expect_err(case);
        assert_eq!(error.code(), oci::RUNTIME_SOCKET_EXPOSED, "{case}: {error}");
    }
    let admitted = adapter.admit(&base()).expect("admitted");
    adapter
        .render(&admitted, &command)
        .expect("a workspace beside the socket renders");
}

// ---- the oci lane: a real container runtime ----

/// Held while an adapter opens, so the orphan case's sweep is the only one in this process.
static OPENING: Mutex<()> = Mutex::new(());

/// The runtime's program, or the reason the lane is absent.
fn lane() -> Result<PathBuf, String> {
    if std::env::var_os(LANE_ENV).as_deref() != Some("1".as_ref()) {
        return Err(format!("{LANE_ENV} is not 1"));
    }
    Ok(std::env::var_os(RUNTIME_ENV).map_or_else(|| PathBuf::from("docker"), PathBuf::from))
}

fn image() -> String {
    std::env::var(IMAGE_ENV).unwrap_or_else(|_| IMAGE.to_owned())
}

/// Opens `oci` over the lane's runtime; with the lane on, a runtime that does not open fails.
fn open_lane(
    runtime: &Path,
    writable_roots: Vec<DeclaredWritableRoot>,
) -> Arc<dyn ConfinementAdapter> {
    let _opening = OPENING.lock();
    let adapter = confinement::open(AdapterRequest::Oci(OciRequest {
        runtime: runtime.to_path_buf(),
        workspace_root: workspace_root(),
        writable_roots,
    }))
    .unwrap_or_else(|error| panic!("{LANE_ENV}=1 and oci does not open: {error}"));
    pull(runtime);
    adapter
}

/// Pulls the test image once, so no run's bound is spent on the registry.
fn pull(runtime: &Path) {
    static PULLED: OnceLock<()> = OnceLock::new();
    PULLED.get_or_init(|| {
        let status = std::process::Command::new(runtime)
            .args(["pull", &image()])
            .stdin(std::process::Stdio::null())
            .status()
            .expect("the runtime runs");
        assert!(status.success(), "pulling {} failed", image());
    });
}

/// The containers carrying `run`'s label, removed or not.
fn containers(runtime: &Path, run: &str) -> String {
    let output = std::process::Command::new(runtime)
        .args([
            "ps",
            "--all",
            "--quiet",
            "--filter",
            &format!("label={}={run}", oci::RUN_LABEL),
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the runtime lists containers");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn ess_confinement_oci_lane_executes_every_authored_oci_scenario() {
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("ESS confinement oci lane: ABSENT ({reason}); 0 executed");
            return;
        }
    };
    let declared = support::lane_size("oci");
    let scenarios: Vec<_> = support::scenarios("oci", |adapter| adapter == "oci")
        .into_iter()
        .filter(|(id, _)| id.starts_with("substrate.confinement/authored/oci-"))
        .collect();
    assert!(declared > 0, "the suite has no oci scenario");
    assert_eq!(
        scenarios.len(),
        declared,
        "an oci-lane scenario is not an oci-* scenario"
    );
    // The scenarios state the stdin pin's runtime. On any other `stdin_pipe` is unproved: each
    // open proves the declared set without it, and an admission requiring it is its refusal.
    let pinned = stdin_pinned("ESS confinement oci lane", &runtime);
    let unproved: &[Fact] = if pinned { &[] } else { &[Fact::StdinPipe] };
    let requiring_stdin = scenarios
        .iter()
        .filter(|(_, scenario)| {
            scenario["steps"]
                .as_array()
                .expect("steps")
                .iter()
                .any(|step| {
                    step["command"] == "substrate.confinement.AdmitProfile"
                        && step["input"]["required_facts"]["value"]
                            .as_array()
                            .expect("required facts")
                            .contains(&serde_json::json!("stdin_pipe"))
                })
        })
        .count();
    assert!(requiring_stdin > 0, "no oci scenario admits stdin_pipe");
    let (executed, audited, open_checks, refused) =
        support::run_without(&scenarios, unproved, |step| {
            assert_eq!(step.adapter, "oci");
            Ok(open_lane(&runtime, Vec::new()))
        });
    assert_eq!(executed, declared, "an oci scenario did not run");
    assert_eq!(
        refused,
        if pinned { 0 } else { requiring_stdin },
        "each admission requiring stdin_pipe follows the probe"
    );
    println!(
        "ESS confinement oci lane: {executed} scenarios executed of {declared} declared, \
         {audited} refusal audit records asserted, {open_checks} open checks refused, \
         {refused} admissions refused fact-unserved stdin_pipe on an unpinned runtime, 0 failed"
    );
}

/// Whether the lane's runtime, probed as `open` probes it, is an entry of the stdin pin (design 32
/// D13), so `oci` proves `stdin_pipe` on it. Prints the probed versions and the branch taken.
fn stdin_pinned(name: &str, runtime: &Path) -> bool {
    let probe = oci::probe(&OciRequest {
        runtime: runtime.to_path_buf(),
        workspace_root: workspace_root(),
        writable_roots: Vec::new(),
    })
    .unwrap_or_else(|error| panic!("{LANE_ENV}=1 and the runtime does not probe: {error}"));
    let pinned = oci::proof(&probe).proves(&Fact::StdinPipe);
    println!(
        "{name}: runtime {:?} server {} client {:?}: stdin {}",
        probe.flavor,
        probe.version,
        probe.client_version,
        if pinned { "pinned" } else { "not pinned" }
    );
    pinned
}

/// The branch of a stdin case on the lane's runtime: `true` on the stdin pin, where the opened
/// adapter proves `stdin_pipe` and the caller's piped assertions apply. Otherwise `piped` is
/// refused `confinement.fact-unserved` naming `stdin_pipe` (501) and audited, and the same profile
/// with stdin `Null` still runs `cat` to exit 0 on its end of file with no output; `false`.
async fn stdin_served(
    name: &str,
    runtime: &Path,
    adapter: &Arc<dyn ConfinementAdapter>,
    piped: &ConfinementProfile,
) -> bool {
    let pinned = stdin_pinned(name, runtime);
    assert_eq!(
        adapter.facts().proves(&Fact::StdinPipe),
        pinned,
        "{name}: the opened adapter's stdin_pipe follows its probe"
    );
    if pinned {
        return true;
    }
    let (sink, records) = recording();
    let error = adapter
        .admit(&ConfinementProfile {
            audit: Some(sink),
            ..piped.clone()
        })
        .expect_err("a piped profile on an unpinned runtime");
    assert!(
        matches!(
            &error,
            ConfinementError::FactUnserved {
                adapter: AdapterName::Oci,
                fact: Fact::StdinPipe
            }
        ),
        "{name}: {error:?}"
    );
    support::assert_refused(&error, "confinement.fact-unserved", 501);
    let audited = events(&records);
    assert_eq!(audited.len(), 1, "{name}: {audited:?}");
    assert_eq!(audited[0]["event"], "profile_refused", "{name}");
    assert_eq!(audited[0]["fact"], "stdin_pipe", "{name}");
    let admitted = adapter
        .admit(&ConfinementProfile {
            stdin: confinement::StdinMode::Null,
            ..piped.clone()
        })
        .expect("the same profile without a piped stdin is admitted");
    let cat = Command {
        argv: vec!["/bin/cat".into()],
        env: BTreeMap::new(),
    };
    let run = adapter.start(&admitted, &cat).await.expect("cat starts");
    let ended = tokio::time::timeout(Duration::from_secs(30), adapter.wait(&run))
        .await
        .expect("cat read end of file and ended")
        .expect("waited");
    assert_eq!(ended.exit.as_ref().and_then(|exit| exit.code), Some(0));
    let output = adapter.output(&run).await.expect("output");
    assert!(output.stdout.is_empty(), "{name}");
    println!("{name}: piped refused fact-unserved stdin_pipe, unpiped cat ran");
    false
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // One run, then each thing it observed.
async fn an_oci_run_is_confined_bounded_and_removed_after_wait() {
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("ESS confinement oci run: ABSENT ({reason})");
            return;
        }
    };
    let (_ws, workspace) = canonical_tempdir();
    std::fs::create_dir(workspace.join("sub")).expect("workdir");
    let (_ro, read_only) = canonical_tempdir();
    std::fs::write(read_only.join("kept"), "kept").expect("read-only file");
    let (_cache, cache) = canonical_tempdir();
    let adapter = open_lane(
        &runtime,
        vec![DeclaredWritableRoot {
            name: "cache".into(),
            path: cache.clone(),
        }],
    );
    let (sink, records) = recording();
    let admitted = adapter
        .admit(&ConfinementProfile {
            image: Some(image()),
            workspace: Some(workspace.clone()),
            workdir: Some("sub".into()),
            read_only_roots: vec![ReadOnlyRoot {
                host_path: read_only.display().to_string(),
                mount: "/opt/ro".into(),
            }],
            writable_roots: vec![WritableHostRoot {
                host: "cache".into(),
                mount: "/cache".into(),
            }],
            network: Some(Network::Isolated),
            process_bounds: Some(ProcessBounds {
                timeout_ms: 60_000,
                processes: 8,
                memory_bytes: 64 << 20,
                cpu_millis: 30_000,
            }),
            audit: Some(sink),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    assert!(!admitted.declared_insecure());
    // The fork loop runs last: a shell that cannot fork stops there.
    let script = "pwd; \
        touch /workspace/inside && echo wrote-workspace; \
        touch /cache/written && echo wrote-cache; \
        touch /etc/outside 2>/dev/null && echo wrote-etc || echo etc-refused; \
        touch /opt/ro/x 2>/dev/null && echo wrote-ro || echo ro-refused; \
        touch /outside 2>/dev/null && echo wrote-root || echo root-refused; \
        wget -q -T 5 -O /dev/null http://1.1.1.1/ 2>/dev/null && echo net-open || echo net-refused; \
        i=0; while [ $i -lt 16 ]; do sleep 5 & i=$((i+1)); done; echo forked-all";
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect("started");
    let ended = tokio::time::timeout(Duration::from_mins(2), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    let output = adapter.output(&run).await.expect("output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(ended.state, RunState::Ended, "{ended:?} {stdout} {stderr}");
    assert_eq!(ended.adapter, AdapterName::Oci);
    assert_eq!(ended.declared_insecure, None);
    assert_eq!(
        stdout.lines().collect::<Vec<_>>(),
        [
            "/workspace/sub",
            "wrote-workspace",
            "wrote-cache",
            "etc-refused",
            "ro-refused",
            "root-refused",
            "net-refused",
        ],
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("fork"),
        "the pids bound refused no fork: {stderr}"
    );
    assert_ne!(ended.exit.and_then(|exit| exit.code), Some(0), "{ended:?}");
    assert!(workspace.join("inside").is_file());
    assert!(cache.join("written").is_file());
    assert_eq!(
        std::fs::read_dir(&read_only)
            .expect("read-only root")
            .count(),
        1
    );
    assert_eq!(
        containers(&runtime, run.as_str()),
        "",
        "the container remains"
    );
    let events: Vec<_> = events(&records)
        .into_iter()
        .map(|record| record["event"].as_str().expect("event").to_owned())
        .collect();
    assert_eq!(events, ["profile_admitted", "run_started"]);
    println!("ESS confinement oci run: executed");
}

/// Starts `sleep 60` and returns once its container exists.
async fn sleeping(
    adapter: &Arc<dyn ConfinementAdapter>,
    runtime: &Path,
    bounds: ProcessBounds,
) -> (confinement::RunId, tempfile::TempDir) {
    let (dir, workspace) = canonical_tempdir();
    let admitted = adapter
        .admit(&ConfinementProfile {
            image: Some(image()),
            workspace: Some(workspace),
            process_bounds: Some(bounds),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/sleep".into(), "60".into()],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect("started");
    let deadline = Instant::now() + Duration::from_mins(1);
    while containers(runtime, run.as_str()).is_empty() {
        assert!(Instant::now() < deadline, "the container never appeared");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    (run, dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_oci_run_is_removed_after_a_kill() {
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("ESS confinement oci kill: ABSENT ({reason})");
            return;
        }
    };
    let adapter = open_lane(&runtime, Vec::new());
    let (run, _dir) = sleeping(&adapter, &runtime, DEFAULT_TEST_BOUNDS).await;
    assert_eq!(
        adapter.observe(&run).await.expect("observed").state,
        RunState::Running
    );
    adapter
        .signal(&run, substrate_wire::Signal::Kill)
        .await
        .expect("signalled");
    let ended = tokio::time::timeout(Duration::from_mins(1), adapter.wait(&run))
        .await
        .expect("the run ends after a kill")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    assert_eq!(
        ended.exit.and_then(|exit| exit.signal),
        Some(9),
        "{ended:?}"
    );
    assert_eq!(
        containers(&runtime, run.as_str()),
        "",
        "the container remains"
    );
    println!("ESS confinement oci kill: executed");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_oci_run_is_removed_after_its_timeout() {
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("ESS confinement oci timeout: ABSENT ({reason})");
            return;
        }
    };
    let adapter = open_lane(&runtime, Vec::new());
    let started = Instant::now();
    let (run, _dir) = sleeping(
        &adapter,
        &runtime,
        ProcessBounds {
            timeout_ms: 3_000,
            cpu_millis: 3_000,
            ..DEFAULT_TEST_BOUNDS
        },
    )
    .await;
    let ended = tokio::time::timeout(Duration::from_mins(1), adapter.wait(&run))
        .await
        .expect("the run ends at its timeout")
        .expect("waited");
    assert!(
        started.elapsed() < Duration::from_secs(50),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(ended.state, RunState::Ended, "{ended:?}");
    assert_eq!(
        ended.exit.and_then(|exit| exit.signal),
        Some(9),
        "{ended:?}"
    );
    assert_eq!(
        containers(&runtime, run.as_str()),
        "",
        "the container remains"
    );
    println!("ESS confinement oci timeout: executed");
}

#[test]
fn an_egress_profile_is_refused_before_any_container_starts() {
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("ESS confinement oci egress: ABSENT ({reason})");
            return;
        }
    };
    let adapter = open_lane(&runtime, Vec::new());
    let (sink, records) = recording();
    let error = adapter
        .admit(&ConfinementProfile {
            image: Some(image()),
            network: Some(Network::Egress {
                apertures: vec!["registry".into()],
            }),
            audit: Some(sink),
            ..ConfinementProfile::default()
        })
        .expect_err("egress is refused");
    assert!(
        matches!(
            error,
            ConfinementError::FamilyUnserved {
                adapter: AdapterName::Oci,
                family: ContractFamily::NetworkEgress
            }
        ),
        "{error}"
    );
    let tag = adapter
        .admit(&ConfinementProfile {
            image: Some("docker.io/library/alpine:3.20".into()),
            ..ConfinementProfile::default()
        })
        .expect_err("a tag is refused");
    assert_eq!(tag.code(), "confinement.image-unpinned");
    let events: Vec<_> = events(&records)
        .into_iter()
        .map(|record| record["event"].as_str().expect("event").to_owned())
        .collect();
    assert_eq!(events, ["profile_refused"], "no run started");
    println!("ESS confinement oci egress: executed");
}

/// Creates (never starts) a container carrying the `oci` labels and `embedder`, returning its id.
fn labelled_container(runtime: &Path, embedder: &str) -> String {
    let output = std::process::Command::new(runtime)
        .args([
            "create",
            "--pull",
            "never",
            "--label",
            &format!("{}=oci", oci::LABEL),
            "--label",
            &format!("{}={embedder}", oci::EMBEDDER_LABEL),
            "--entrypoint",
            "",
            "--",
            &image(),
            "/bin/true",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the runtime runs");
    assert!(output.status.success(), "create: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn exists(runtime: &Path, id: &str) -> bool {
    std::process::Command::new(runtime)
        .args(["container", "inspect", "--format", "{{.Id}}", id])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("the runtime runs")
        .success()
}

/// Security F6 (`story:oci-adapter`): `open` removes a labelled container whose embedder is gone
/// and reports how many it removed; one whose embedder it cannot judge (another pid namespace)
/// stays.
#[test]
fn open_removes_a_container_whose_embedder_is_gone() {
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("ESS confinement oci orphans: ABSENT ({reason})");
            return;
        }
    };
    drop(open_lane(&runtime, Vec::new()));
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").expect("boot id");
    let namespace = std::fs::read_link("/proc/self/ns/pid").expect("pid namespace");
    let namespace = namespace
        .to_str()
        .and_then(|link| link.strip_prefix("pid:["))
        .and_then(|link| link.strip_suffix(']'))
        .expect("pid:[inode]")
        .to_owned();
    let pid = std::process::id();
    let _opening = OPENING.lock();
    // This process, but started at tick 0: a process that is gone.
    let orphan = labelled_container(&runtime, &format!("{}/{namespace}/{pid}/0", boot.trim()));
    let foreign = labelled_container(&runtime, &format!("{}/1/{pid}/0", boot.trim()));
    let opened = oci::open_adapter(OciRequest {
        runtime: runtime.clone(),
        workspace_root: workspace_root(),
        writable_roots: Vec::new(),
    });
    let orphan_left = exists(&runtime, &orphan);
    let foreign_left = exists(&runtime, &foreign);
    for id in [&orphan, &foreign] {
        let _ = std::process::Command::new(&runtime)
            .args(["rm", "--force", id])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status();
    }
    let adapter = opened.expect("opens");
    assert!(!orphan_left, "the orphan remains");
    assert!(
        foreign_left,
        "a container of another pid namespace was removed"
    );
    assert!(adapter.orphans_removed() >= 1, "no orphan was removed");
    println!("ESS confinement oci orphans: executed");
}

// T15 of design 32: on the lane's pinned runtime `cat` in the container reads three writes, the
// bytes `0x10 0x11` (Docker's detach sequence) among them, and the end of file after
// `close_stdin`: every byte crossed the runtime client.
#[tokio::test(flavor = "multi_thread")]
async fn an_oci_run_reads_every_byte_of_its_piped_stdin_and_the_end_of_file() {
    const NAME: &str = "ESS confinement oci stdin";
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let (_ws, workspace) = canonical_tempdir();
    let adapter = open_lane(&runtime, Vec::new());
    let piped = ConfinementProfile {
        image: Some(image()),
        workspace: Some(workspace),
        process_bounds: Some(ProcessBounds {
            timeout_ms: 60_000,
            processes: 8,
            memory_bytes: 64 << 20,
            cpu_millis: 30_000,
        }),
        stdin: confinement::StdinMode::Piped { limit_bytes: 1_024 },
        ..ConfinementProfile::default()
    };
    if !stdin_served(NAME, &runtime, &adapter, &piped).await {
        return;
    }
    support::stdin_echoes_through(
        &adapter,
        &piped,
        &Command {
            argv: vec!["/bin/cat".into()],
            env: BTreeMap::new(),
        },
        &[b"one\n", b"\x10\x11 two\n", b"three\x10\x11\n"],
    )
    .await;
    println!("{NAME}: executed");
}

// B10: an `oci` run keeps the last bytes under `Tail`.
#[tokio::test(flavor = "multi_thread")]
async fn an_oci_run_under_tail_keeps_the_last_bytes() {
    const NAME: &str = "ESS confinement oci retention tail";
    const BOUND: usize = 4096;
    const LAST: u32 = 3000;
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let (_ws, workspace) = canonical_tempdir();
    let adapter = open_lane(&runtime, Vec::new());
    let admitted = adapter
        .admit(&ConfinementProfile {
            image: Some(image()),
            workspace: Some(workspace),
            process_bounds: Some(ProcessBounds {
                timeout_ms: 60_000,
                processes: 8,
                memory_bytes: 64 << 20,
                cpu_millis: 30_000,
            }),
            io_bounds: Some(confinement::IoBounds {
                output_bytes: BOUND as u64,
            }),
            output_retention: confinement::OutputRetention {
                stdout: confinement::Retention::Tail,
                stderr: confinement::Retention::Head,
            },
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/sh".into(), "-c".into(), format!("seq 1 {LAST}")],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect("started");
    let ended = tokio::time::timeout(Duration::from_mins(2), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.exit.and_then(|exit| exit.code), Some(0), "{ended:?}");
    let output = adapter.output(&run).await.expect("output");
    support::assert_retained(
        NAME,
        &support::seq_bytes(LAST),
        &output.stdout,
        &output.stdout_kept,
        confinement::Retention::Tail,
        BOUND,
        None,
    );
    assert!(output.stdout_truncated);
    println!("{NAME}: executed");
}

// T16 of design 32: on the lane's pinned runtime a live run's chunks on both streams cross the
// runtime client, are contiguous, and are exactly the record at its natural end.
#[tokio::test(flavor = "multi_thread")]
async fn a_live_oci_run_delivers_its_record_on_a_natural_end() {
    const NAME: &str = "ESS confinement oci live natural end";
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let (_ws, workspace) = canonical_tempdir();
    let adapter = open_lane(&runtime, Vec::new());
    support::live_natural_end(
        NAME,
        &adapter,
        &ConfinementProfile {
            image: Some(image()),
            workspace: Some(workspace),
            process_bounds: Some(ProcessBounds {
                timeout_ms: 60_000,
                processes: 8,
                memory_bytes: 64 << 20,
                cpu_millis: 30_000,
            }),
            ..ConfinementProfile::default()
        },
        &Command {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                "printf 'out-one\\n'; printf 'err-one\\n' >&2; printf 'out-two\\n'; \
                 printf 'err-two\\n' >&2"
                    .into(),
            ],
            env: BTreeMap::new(),
        },
        b"out-one\nout-two\n",
        b"err-one\nerr-two\n",
    )
    .await;
    println!("{NAME}: executed");
}

// ---- adversary pass on `story:adapter-stdin-pipe` ----

/// A piped `oci` profile in `workspace` with a run timeout of `timeout_ms`.
fn oci_piped(workspace: PathBuf, timeout_ms: u64) -> ConfinementProfile {
    ConfinementProfile {
        image: Some(image()),
        workspace: Some(workspace),
        process_bounds: Some(ProcessBounds {
            timeout_ms,
            processes: 8,
            memory_bytes: 64 << 20,
            cpu_millis: 30_000,
        }),
        stdin: confinement::StdinMode::Piped {
            limit_bytes: confinement::MAX_STDIN_BYTES,
        },
        ..ConfinementProfile::default()
    }
}

// The end of file with no byte before it: `close_stdin` before any write, possibly before the
// runtime client attached, still ends `cat` in the container; and `run_with_stdin` with an empty
// input does the same.
#[tokio::test(flavor = "multi_thread")]
async fn adv_an_oci_run_whose_stdin_is_closed_before_any_write_reads_end_of_file() {
    const NAME: &str = "ESS confinement oci adv immediate eof";
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let (_ws, workspace) = canonical_tempdir();
    let adapter = open_lane(&runtime, Vec::new());
    if !stdin_served(
        NAME,
        &runtime,
        &adapter,
        &oci_piped(workspace.clone(), 60_000),
    )
    .await
    {
        return;
    }
    let admitted = adapter
        .admit(&oci_piped(workspace, 60_000))
        .expect("admitted");
    let cat = Command {
        argv: vec!["/bin/cat".into()],
        env: BTreeMap::new(),
    };
    let run = adapter.start(&admitted, &cat).await.expect("cat starts");
    adapter.close_stdin(&run).await.expect("closed");
    let ended = tokio::time::timeout(Duration::from_secs(30), adapter.wait(&run))
        .await
        .expect("cat read end of file and ended")
        .expect("waited");
    assert_eq!(ended.exit.as_ref().and_then(|exit| exit.code), Some(0));
    let (ended, output, accepted) = tokio::time::timeout(
        Duration::from_secs(30),
        confinement::run_with_stdin(&adapter, &admitted, &cat, b""),
    )
    .await
    .expect("run_with_stdin returned")
    .expect("an empty input is no error");
    assert_eq!(ended.exit.as_ref().and_then(|exit| exit.code), Some(0));
    assert_eq!(accepted, 0);
    assert!(output.stdout.is_empty());
    println!("{NAME}: executed");
}

// Backpressure through the runtime client: a container that never reads its standard input and
// never ends cannot hold `run_with_stdin` past the run's timeout (3 s) by more than seconds, and
// 200 000 bytes through `cat` come back whole.
#[tokio::test(flavor = "multi_thread")]
async fn adv_an_oci_container_that_never_reads_stdin_is_bounded_by_the_run_timeout() {
    const NAME: &str = "ESS confinement oci adv never reads";
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let (_ws, workspace) = canonical_tempdir();
    let adapter = open_lane(&runtime, Vec::new());
    if !stdin_served(
        NAME,
        &runtime,
        &adapter,
        &oci_piped(workspace.clone(), 3_000),
    )
    .await
    {
        return;
    }
    let admitted = adapter
        .admit(&oci_piped(workspace.clone(), 3_000))
        .expect("admitted");
    let large = vec![b'z'; 4 << 20];
    let started = Instant::now();
    let (ended, _, accepted) = tokio::time::timeout(
        Duration::from_secs(30),
        confinement::run_with_stdin(
            &adapter,
            &admitted,
            &Command {
                argv: vec!["/bin/sleep".into(), "60".into()],
                env: BTreeMap::new(),
            },
            &large,
        ),
    )
    .await
    .expect("run_with_stdin returned within 30 s of a 3 s timeout")
    .expect("the timeout is an end, not an error");
    assert_ne!(ended.state, RunState::Running);
    assert!(accepted < large.len() as u64, "{NAME}: {accepted}");
    assert!(started.elapsed() >= Duration::from_secs(2), "{NAME}");

    let admitted = adapter
        .admit(&oci_piped(workspace, 60_000))
        .expect("admitted");
    let input: Vec<u8> = (0..200_000_u32)
        .map(|i| u8::try_from(i % 251).expect("below 251"))
        .collect();
    let (ended, output, accepted) = tokio::time::timeout(
        Duration::from_secs(45),
        confinement::run_with_stdin(
            &adapter,
            &admitted,
            &Command {
                argv: vec!["/bin/cat".into()],
                env: BTreeMap::new(),
            },
            &input,
        ),
    )
    .await
    .expect("returned")
    .expect("cat ran");
    assert_eq!(ended.exit.as_ref().and_then(|exit| exit.code), Some(0));
    assert_eq!(accepted, input.len() as u64);
    assert_eq!(
        output.stdout, input,
        "{NAME}: every byte crossed the client"
    );
    println!("{NAME}: executed");
}

// ---- adversary pass on `story:adapter-live-output` ----

/// An `oci` profile in `workspace` with a run timeout of `timeout_ms`, output recorded only.
fn oci_recorded(workspace: PathBuf, timeout_ms: u64) -> ConfinementProfile {
    ConfinementProfile {
        image: Some(image()),
        workspace: Some(workspace),
        process_bounds: Some(ProcessBounds {
            timeout_ms,
            processes: 8,
            memory_bytes: 64 << 20,
            cpu_millis: 30_000,
        }),
        ..ConfinementProfile::default()
    }
}

// § 3: `Kill` stops delivery at once; only the chunk queued before it is delivered.
#[tokio::test(flavor = "multi_thread")]
async fn adv_a_killed_live_oci_run_delivers_only_the_queued_chunk() {
    const NAME: &str = "ESS confinement oci adv live kill";
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let (_ws, workspace) = canonical_tempdir();
    let adapter = open_lane(&runtime, Vec::new());
    support::adv_live_kill_delivers_only_the_queued_chunk(
        NAME,
        &adapter,
        &oci_recorded(workspace.clone(), 60_000),
        &workspace.join("written"),
        Path::new("/workspace/written"),
    )
    .await;
    println!("{NAME}: executed");
}

// § 3: a timeout is an imposed end that closes the queue first (`monitor`), not a stall.
#[tokio::test(flavor = "multi_thread")]
async fn adv_a_live_oci_run_ended_by_its_timeout_did_not_stall() {
    const NAME: &str = "ESS confinement oci adv live timeout";
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let (_ws, workspace) = canonical_tempdir();
    let adapter = open_lane(&runtime, Vec::new());
    support::adv_live_timeout_is_not_a_stall(NAME, &adapter, &oci_recorded(workspace, 300)).await;
    println!("{NAME}: executed");
}

// § 3 *Backpressure*, which the design runs on `oci` nowhere (T15 is T4 only): a stall ends the
// container as `signal(Kill)` does, through `kill_retried`. The run's timeout is 60 s, so a stall
// that killed nothing would leave `yes` running far past this case's 15 s.
#[tokio::test(flavor = "multi_thread")]
async fn adv_an_unread_live_oci_run_stalls_and_its_container_ends() {
    const NAME: &str = "ESS confinement oci adv live stall";
    let runtime = match lane() {
        Ok(runtime) => runtime,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let (_ws, workspace) = canonical_tempdir();
    let adapter = open_lane(&runtime, Vec::new());
    let admitted = adapter
        .admit(&support::live(&oci_recorded(workspace, 60_000), 1))
        .expect("admitted");
    let run = adapter
        .start(&admitted, &support::sh("exec yes"))
        .await
        .expect("started");
    let began = Instant::now();
    tokio::time::timeout(Duration::from_secs(15), adapter.wait(&run))
        .await
        .expect("the stall ends the container")
        .expect("waited");
    assert!(began.elapsed() >= Duration::from_secs(1), "{NAME}");
    assert!(adapter.output_stalled(&run), "{NAME}");
    let events = support::read_events(&adapter, &run).await;
    let (streams, losses) = support::delivered(&events);
    assert_eq!(losses, [confinement::OutputEvent::Stalled], "{NAME}");
    assert!(!streams[0].is_empty(), "{NAME}");
    println!("{NAME}: executed");
}
