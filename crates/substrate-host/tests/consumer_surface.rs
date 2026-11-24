//! The confinement module as its two consumers call it (atlas ADR 0109 decision 6,
//! `story:confinement-consumer-surface`).
//!
//! Written as a consumer would: everything it chooses with comes from
//! `substrate_host::confinement`. It opens what the host offers, then picks, for each need, an
//! opened adapter by reading its `served_families` and `facts`, never its name and never the OS.
//! A declared-insecure adapter is chosen only when no other serves the need, and gets a reason.
//! Where an adapter proves `paths_are_host_paths` the run is addressed by host paths; otherwise
//! by the paths the profile mounts.
//!
//! The needs:
//!
//! | need | from | profile |
//! |---|---|---|
//! | `toolchain-read-only` | loom | a toolchain root read-only, its program run, a write refused |
//! | `workspace-target-writable` | loom | the workspace, with `target/` written |
//! | `workdir-beneath-workspace` | loom | a working directory beneath the workspace |
//! | `promotion-build` | takt | an install root outside the clone, writable through a declared writable root |
//! | `version-run` | takt | a `--version` run of the installed result |
//!
//! Two lanes. The portable lane offers only `local` (no delegated cgroup root is configured): the
//! needs it serves run, and each need whose profile names a filesystem view or a writable root is
//! refused `confinement.family-unserved` naming `filesystem_view`, a refusal read from the facts
//! before the call. The delegated lane also offers `linux` with the delegated root
//! `SUBSTRATE_VECTORS_CGROUP_ROOT` names, and every need runs on an adapter that does not prove
//! `declared_insecure`. Without that root the delegated lane is **absent, never reported as
//! passed** (invariant 3); `bash scripts/delegated-lane.sh` runs it and requires its count.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use substrate_host::confinement::{
    self, AdapterFacts, AdapterRequest, AuditRecord, AuditSink, Command, ConfinementAdapter,
    ConfinementError, ConfinementProfile, ContractFamily, DeclaredWritableRoot, FAMILY_ORDER,
    FAMILY_UNSERVED, Fact, HostConfig, Network, ProcessBounds, ReadOnlyRoot, RunLimit, RunState,
    WritableHostRoot, linux::LinuxRequest, local::LocalRequest,
};

/// Every need this file states. Pinned, so a need lost from the list fails here.
const NEEDS: usize = 5;
/// The needs that name a filesystem view or a writable root, which `local` refuses.
const VIEW_NEEDS: [&str; 2] = ["toolchain-read-only", "promotion-build"];

const REASON: &str = "the consumer surface test runs a host process on purpose";
const TOOLCHAIN_MOUNT: &str = "/opt/toolchain";
const INSTALL_ROOT_NAME: &str = "install";
const INSTALL_MOUNT: &str = "/install";
const INSTALLED_MOUNT: &str = "/installed";
const BASELINE: &str = "#!/bin/sh\necho tool 0.1.0\n";
const CANDIDATE: &str = "#!/bin/sh\necho tool 0.2.0\n";

/// The host directories every need works in, canonical and under `CARGO_TARGET_TMPDIR`.
struct Fixture {
    _directory: tempfile::TempDir,
    workspaces: PathBuf,
    /// loom's workspace: a crate with `crates/app` beneath it.
    loom: PathBuf,
    /// takt's clone: the candidate's source.
    clone: PathBuf,
    /// The read-only toolchain root.
    toolchain: PathBuf,
    /// The install root outside the clone, holding the baseline `bin/tool`.
    install: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("consumer-surface-")
            .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
            .expect("fixture directory under CARGO_TARGET_TMPDIR");
        let host = std::fs::canonicalize(directory.path()).expect("canonical fixture");
        let workspaces = host.join("workspaces");
        let loom = workspaces.join("ws_loom");
        let clone = workspaces.join("ws_clone");
        let toolchain = host.join("toolchain");
        let install = host.join("install");
        std::fs::create_dir_all(loom.join("crates").join("app")).expect("loom workspace");
        std::fs::create_dir_all(&clone).expect("takt clone");
        write_program(&clone.join("tool.sh"), CANDIDATE);
        write_program(
            &toolchain.join("bin").join("tool"),
            "#!/bin/sh\necho toolchain 1.0\n",
        );
        write_program(&install.join("bin").join("tool"), BASELINE);
        Self {
            _directory: directory,
            workspaces,
            loom,
            clone,
            toolchain,
            install,
        }
    }
}

fn write_program(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(path.parent().expect("parent")).expect("program directory");
    std::fs::write(path, text).expect("program");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("mode");
}

fn text(path: &Path) -> String {
    path.to_str().expect("UTF-8 fixture path").to_owned()
}

/// The delegated cgroup v2 subtree this process is inside, or the reason the lane is absent.
fn delegated() -> Result<PathBuf, &'static str> {
    let root = PathBuf::from(
        std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")
            .ok_or("SUBSTRATE_VECTORS_CGROUP_ROOT is unset")?,
    );
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids")
        .map_err(|_| "the delegated cgroup root does not accept controllers")?;
    Ok(root)
}

/// What the host offers, opened. A request that does not open is not offered; nothing here
/// depends on which one did.
fn offer(fixture: &Fixture, cgroup_root: Option<&Path>) -> Vec<Arc<dyn ConfinementAdapter>> {
    let mut requests = Vec::new();
    if let Some(cgroup_root) = cgroup_root {
        let mut config = HostConfig::minimum(&fixture.workspaces);
        config.writable_roots = vec![DeclaredWritableRoot {
            name: INSTALL_ROOT_NAME.to_owned(),
            path: fixture.install.clone(),
        }];
        requests.push(AdapterRequest::Linux(Box::new(
            LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root.to_path_buf())),
        )));
    }
    requests.push(AdapterRequest::Local(LocalRequest::default()));
    requests
        .into_iter()
        .filter_map(|request| confinement::open(request).ok())
        .collect()
}

/// What one need asks of an adapter with these facts, and how its output is judged.
struct Need {
    name: &'static str,
    ask: fn(&AdapterFacts, &Fixture) -> (ConfinementProfile, Vec<String>),
    /// Checks the run's stdout and the host afterwards; `built` says whether the promotion build
    /// ran earlier in this lane.
    judge: fn(&AdapterFacts, &Fixture, &str, bool),
}

fn shell(script: &str) -> Vec<String> {
    vec!["/bin/sh".into(), "-c".into(), script.into()]
}

/// The path a run sees for `host`, which `mount` names when paths are not host paths.
fn seen(facts: &AdapterFacts, host: &Path, mount: &str) -> String {
    if facts.proves(&Fact::PathsAreHostPaths) {
        text(host)
    } else {
        mount.to_owned()
    }
}

#[allow(clippy::too_many_lines)] // Every need, side by side, as the table at the top lists them.
fn needs() -> [Need; NEEDS] {
    [
        Need {
            name: "toolchain-read-only",
            ask: |_, fixture| {
                (
                    ConfinementProfile {
                        workspace: Some(fixture.loom.clone()),
                        read_only_roots: vec![ReadOnlyRoot {
                            host_path: text(&fixture.toolchain),
                            mount: TOOLCHAIN_MOUNT.to_owned(),
                        }],
                        ..ConfinementProfile::default()
                    },
                    shell(&format!(
                        "{TOOLCHAIN_MOUNT}/bin/tool --version \
                         && if (: > {TOOLCHAIN_MOUNT}/bin/written) 2>/dev/null; \
                            then echo wrote; exit 9; fi"
                    )),
                )
            },
            judge: |_, fixture, stdout, _| {
                assert_eq!(stdout, "toolchain 1.0\n");
                assert!(
                    !fixture.toolchain.join("bin").join("written").exists(),
                    "a write reached the read-only toolchain root"
                );
            },
        },
        Need {
            name: "workspace-target-writable",
            ask: |_, fixture| {
                (
                    ConfinementProfile {
                        workspace: Some(fixture.loom.clone()),
                        ..ConfinementProfile::default()
                    },
                    shell("mkdir -p target/debug && printf built > target/debug/artifact"),
                )
            },
            judge: |_, fixture, _, _| {
                let artifact = fixture.loom.join("target").join("debug").join("artifact");
                assert_eq!(
                    std::fs::read_to_string(artifact).expect("the artifact on the host"),
                    "built"
                );
            },
        },
        Need {
            name: "workdir-beneath-workspace",
            ask: |_, fixture| {
                (
                    ConfinementProfile {
                        workspace: Some(fixture.loom.clone()),
                        workdir: Some("crates/app".into()),
                        ..ConfinementProfile::default()
                    },
                    shell("pwd"),
                )
            },
            judge: |facts, fixture, stdout, _| {
                let expected = seen(
                    facts,
                    &fixture.loom.join("crates").join("app"),
                    "/workspace/crates/app",
                );
                assert_eq!(stdout, format!("{expected}\n"));
            },
        },
        Need {
            name: "promotion-build",
            ask: |_, fixture| {
                (
                    ConfinementProfile {
                        workspace: Some(fixture.clone.clone()),
                        writable_roots: vec![WritableHostRoot {
                            host: INSTALL_ROOT_NAME.to_owned(),
                            mount: INSTALL_MOUNT.to_owned(),
                        }],
                        ..ConfinementProfile::default()
                    },
                    shell(&format!(
                        "mkdir -p {INSTALL_MOUNT}/bin && cp tool.sh {INSTALL_MOUNT}/bin/tool \
                         && chmod 755 {INSTALL_MOUNT}/bin/tool"
                    )),
                )
            },
            judge: |_, fixture, _, _| {
                assert_eq!(
                    std::fs::read_to_string(fixture.install.join("bin").join("tool"))
                        .expect("the installed result on the host"),
                    CANDIDATE
                );
            },
        },
        Need {
            name: "version-run",
            ask: |facts, fixture| {
                let installed = fixture.install.join("bin").join("tool");
                // Where paths are host paths the installed program is run where it is; otherwise
                // the install root is mounted read-only and the program is run there.
                let read_only_roots = if facts.proves(&Fact::PathsAreHostPaths) {
                    Vec::new()
                } else {
                    vec![ReadOnlyRoot {
                        host_path: text(&fixture.install),
                        mount: INSTALLED_MOUNT.to_owned(),
                    }]
                };
                (
                    ConfinementProfile {
                        workspace: Some(fixture.clone.clone()),
                        read_only_roots,
                        ..ConfinementProfile::default()
                    },
                    vec![
                        seen(facts, &installed, &format!("{INSTALLED_MOUNT}/bin/tool")),
                        "--version".into(),
                    ],
                )
            },
            judge: |_, _, stdout, built| {
                let expected = if built {
                    "tool 0.2.0\n"
                } else {
                    "tool 0.1.0\n"
                };
                assert_eq!(stdout, expected);
            },
        },
    ]
}

/// The profile a need asks of `facts`, with the reason a declared-insecure adapter requires.
fn profile_for(
    need: &Need,
    facts: &AdapterFacts,
    fixture: &Fixture,
) -> (ConfinementProfile, Command) {
    let (mut profile, argv) = (need.ask)(facts, fixture);
    if facts.declared_insecure() {
        profile.local_reason = Some(REASON.into());
    }
    let command = Command {
        argv,
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
    };
    (profile, command)
}

/// The first family `profile` needs that `facts` does not serve, in the contract's order: the
/// families of `required_families()`, plus `audit` for a sink and `network_egress` for an isolated
/// network on an adapter that proves `host_network`, as the page's `needed` states.
fn first_unserved(profile: &ConfinementProfile, facts: &AdapterFacts) -> Option<ContractFamily> {
    let implied = profile.required_families();
    FAMILY_ORDER.into_iter().find(|family| {
        let needed = implied.contains(family)
            || (*family == ContractFamily::Audit && profile.audit.is_some())
            || (*family == ContractFamily::NetworkEgress
                && profile.network == Some(Network::Isolated)
                && facts.proves(&Fact::HostNetwork));
        needed && !facts.serves(family)
    })
}

fn serves(profile: &ConfinementProfile, facts: &AdapterFacts) -> bool {
    first_unserved(profile, facts).is_none()
        && profile
            .required_facts()
            .iter()
            .all(|fact| facts.proves(fact))
}

/// Picks the adapter for a need: one that serves it and does not prove `declared_insecure` if
/// any does, else one that serves it.
fn choose<'a>(
    offered: &'a [Arc<dyn ConfinementAdapter>],
    need: &Need,
    fixture: &Fixture,
) -> Option<&'a Arc<dyn ConfinementAdapter>> {
    let serving = || {
        offered.iter().filter(|adapter| {
            let (profile, _) = profile_for(need, adapter.facts(), fixture);
            serves(&profile, adapter.facts())
        })
    };
    serving()
        .find(|adapter| !adapter.facts().declared_insecure())
        .or_else(|| serving().next())
}

#[derive(Default)]
struct Outcome {
    /// Each need run, with whether its adapter proved `declared_insecure`.
    executed: Vec<(&'static str, bool)>,
    refused: Vec<&'static str>,
}

async fn run(adapter: &Arc<dyn ConfinementAdapter>, need: &Need, fixture: &Fixture, built: bool) {
    let facts = adapter.facts();
    let (profile, command) = profile_for(need, facts, fixture);
    let admitted = adapter
        .admit(&profile)
        .unwrap_or_else(|error| panic!("{}: admission refused: {error}", need.name));
    let started = adapter
        .start(&admitted, &command)
        .await
        .unwrap_or_else(|error| panic!("{}: start refused: {error}", need.name));
    let ended = adapter.wait(&started).await.expect("waited");
    let output = adapter.output(&started).await.expect("output");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert_eq!(ended.state, RunState::Ended, "{}: {ended:?}", need.name);
    assert_eq!(
        ended.exit.as_ref().and_then(|exit| exit.code),
        Some(0),
        "{}: stdout {stdout:?} stderr {stderr:?}",
        need.name
    );
    assert_eq!(
        ended.declared_insecure.is_some(),
        facts.declared_insecure(),
        "{}: the observation and the facts disagree on declared_insecure",
        need.name
    );
    (need.judge)(facts, fixture, &stdout, built);
}

/// Runs every need on the adapter chosen for it; a need no offered adapter serves is asserted
/// refused by the family the facts name, before and by the call.
async fn exercise(offered: &[Arc<dyn ConfinementAdapter>], fixture: &Fixture) -> Outcome {
    let mut outcome = Outcome::default();
    let mut built = false;
    for need in &needs() {
        if let Some(adapter) = choose(offered, need, fixture) {
            run(adapter, need, fixture, built).await;
            built |= need.name == "promotion-build";
            outcome
                .executed
                .push((need.name, adapter.facts().declared_insecure()));
            continue;
        }
        for adapter in offered {
            let facts = adapter.facts();
            let (profile, _) = profile_for(need, facts, fixture);
            let Some(family) = first_unserved(&profile, facts) else {
                panic!(
                    "{}: no adapter was chosen, yet {facts:?} serves every family it needs",
                    need.name
                );
            };
            let error = adapter
                .admit(&profile)
                .expect_err("a need the facts do not serve is refused");
            match &error {
                ConfinementError::FamilyUnserved {
                    family: refused, ..
                } => assert_eq!(refused, &family, "{}", need.name),
                other => panic!(
                    "{}: expected family-unserved {family:?}, got {other}",
                    need.name
                ),
            }
            assert_eq!(error.code(), FAMILY_UNSERVED);
            assert_eq!(error.status(), 501);
        }
        outcome.refused.push(need.name);
    }
    outcome
}

#[tokio::test(flavor = "multi_thread")]
async fn portable_lane_runs_what_local_serves_and_refuses_each_view_by_its_facts() {
    let fixture = Fixture::new();
    let offered = offer(&fixture, None);
    assert!(!offered.is_empty(), "the host offers no adapter at all");
    // Read before any call: what this host's offer does not serve, by the contract's family.
    for adapter in &offered {
        assert!(
            !adapter.facts().serves(&ContractFamily::FilesystemView),
            "the portable lane offers an adapter serving filesystem_view: {:?}",
            adapter.facts()
        );
    }
    for need in &needs() {
        let refused_by_facts = offered.iter().all(|adapter| {
            let (profile, _) = profile_for(need, adapter.facts(), &fixture);
            first_unserved(&profile, adapter.facts()) == Some(ContractFamily::FilesystemView)
        });
        assert_eq!(
            refused_by_facts,
            VIEW_NEEDS.contains(&need.name),
            "{}: the facts and the view needs disagree",
            need.name
        );
    }
    let outcome = exercise(&offered, &fixture).await;
    assert_eq!(outcome.refused, VIEW_NEEDS);
    assert_eq!(outcome.executed.len() + outcome.refused.len(), NEEDS);
    println!(
        "consumer surface portable lane: {} needs executed, {} refused confinement.family-unserved \
         naming filesystem_view, of {NEEDS}",
        outcome.executed.len(),
        outcome.refused.len()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn delegated_lane_runs_every_need_on_an_adapter_not_declared_insecure() {
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("consumer surface delegated lane: ABSENT ({reason}); 0 executed");
            return;
        }
    };
    let fixture = Fixture::new();
    let offered = offer(&fixture, Some(&cgroup_root));
    let outcome = exercise(&offered, &fixture).await;
    assert_eq!(outcome.refused, Vec::<&str>::new(), "a need was refused");
    assert_eq!(outcome.executed.len(), NEEDS);
    for (name, insecure) in &outcome.executed {
        assert!(!insecure, "{name} ran on a declared-insecure adapter");
    }
    println!(
        "consumer surface delegated lane: {} needs executed of {NEEDS}, each on an adapter not \
         declared insecure",
        outcome.executed.len()
    );
}

/// The governed run's script: the denied program fails, `xargs` then tries to keep 16 `sleep`
/// processes running, one per line of `sixteen`, and the shell replaces itself with a `sleep`
/// that outlives the run's timeout.
///
/// The case assumes three processes of the run's cgroup before `xargs` under the profile's bound
/// of four: bubblewrap's monitor, its init and the shell (`env` and the exec gate replace
/// themselves). A change to that tree changes what this run does under the bound; the
/// assertions on its output and its count name the assumption when they fail.
const GOVERNED: &str = "/workspace/bin/denied 2>/dev/null; echo \"denied $?\"; \
                        xargs -P 16 -n 1 sleep < sixteen 2>/dev/null; echo tried; exec sleep 30";

/// The host's copy of a base tool, where this distribution keeps it; the run sees it at the
/// same path.
fn host_tool(name: &str) -> PathBuf {
    ["/usr/bin", "/bin"]
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("no {name} in /usr/bin or /bin"))
}

/// The directories that hold `name` on the host, where its path is and where it leads, as list
/// entries: an entry admits a program by where the file itself lies.
fn tool_directories(name: &str) -> Vec<String> {
    let path = host_tool(name);
    let target = std::fs::canonicalize(&path).expect("the tool resolves");
    [path, target]
        .iter()
        .map(|file| text(file.parent().expect("a directory")))
        .collect()
}

/// Design 36 § 8, story 2: the acceptance test of `epic:runs-are-governed-and-limit-hits-are-
/// events`, through public items only. One profile names an allow-list, a deny-list and process
/// bounds, and runs on an adapter whose facts serve it. The run sees its denied program fail
/// with status 126, has process creations refused by its bound, and is then ended by its
/// timeout: its observation and its `run_ended` record each name the wall-time limit and a
/// count of refused creations above zero. Absent, never reported as passed, without the
/// delegated root or without the executable gate `SUBSTRATE_EXEC_GATE` names.
#[tokio::test(flavor = "multi_thread")]
async fn delegated_lane_a_governed_run_names_its_timeout_and_counts_its_refused_creations() {
    const NAME: &str = "consumer surface governed run";
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("{NAME}: ABSENT ({reason})");
            return;
        }
    };
    let Some(gate) = std::env::var_os("SUBSTRATE_EXEC_GATE").map(PathBuf::from) else {
        println!("{NAME}: ABSENT (SUBSTRATE_EXEC_GATE is unset)");
        return;
    };
    let fixture = Fixture::new();
    let mut config = HostConfig::minimum(&fixture.workspaces);
    config.exec_gate = Some(gate);
    let adapter = confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root)),
    )))
    .expect("linux opens on the delegated root with the executable gate");
    let workspace = &fixture.loom;
    std::fs::create_dir_all(workspace.join("bin")).expect("bin");
    std::fs::copy(host_tool("true"), workspace.join("bin").join("denied"))
        .expect("the denied program");
    std::fs::write(workspace.join("sixteen"), "0.05\n".repeat(16)).expect("sixteen");
    let mut entries: Vec<String> = ["sh", "xargs", "sleep"]
        .into_iter()
        .flat_map(tool_directories)
        .collect();
    entries.sort();
    entries.dedup();
    entries.push("/workspace/bin".to_owned());
    let (sender, mut ended_records) = tokio::sync::mpsc::unbounded_channel();
    let sink = AuditSink::new(move |record: &AuditRecord| {
        let value = serde_json::to_value(record).expect("an audit record serializes");
        if value["event"] == "run_ended" {
            let _ = sender.send(value);
        }
    });
    let profile = ConfinementProfile {
        workspace: Some(workspace.clone()),
        exec_subpaths: entries,
        exec_denied_paths: vec!["/workspace/bin/denied".to_owned()],
        process_bounds: Some(ProcessBounds {
            timeout_ms: 2_000,
            processes: 4,
            memory_bytes: 64 << 20,
            cpu_millis: 2_000,
        }),
        audit: Some(sink),
        ..ConfinementProfile::default()
    };
    assert!(
        serves(&profile, adapter.facts()),
        "{NAME}: the adapter's facts do not serve the governed profile: {:?}",
        adapter.facts()
    );
    let admitted = adapter.admit(&profile).expect("admitted");
    let command = Command {
        argv: vec![text(&host_tool("sh")), "-c".into(), GOVERNED.into()],
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
    };
    let began = std::time::Instant::now();
    let run = adapter.start(&admitted, &command).await.expect("started");
    let ended = tokio::time::timeout(std::time::Duration::from_secs(20), adapter.wait(&run))
        .await
        .expect("the run ends at its timeout")
        .expect("waited");
    println!("{NAME}: the run ended after {:?}", began.elapsed());
    let output = adapter.output(&run).await.expect("output");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "denied 126\ntried\n",
        "{NAME}: the case assumes three processes before `xargs` under a bound of four; stderr \
         {:?}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(ended.state, RunState::Ended, "{NAME}: {ended:?}");
    assert_eq!(ended.limit, Some(RunLimit::WallTime), "{NAME}: {ended:?}");
    let hits = ended
        .process_limit_hits
        .filter(|hits| *hits > 0)
        .unwrap_or_else(|| {
            panic!(
                "{NAME}: no refused creation counted; the case assumes three processes before \
                 `xargs` under a bound of four: {ended:?}"
            )
        });
    let record = tokio::time::timeout(std::time::Duration::from_secs(5), ended_records.recv())
        .await
        .expect("run_ended reaches the sink")
        .expect("the sink lives");
    assert_eq!(record["limit"], "wall_time", "{NAME}: {record}");
    assert_eq!(record["process_limit_hits"], hits, "{NAME}: {record}");
    println!("{NAME}: executed, {hits} process creations refused");
}
