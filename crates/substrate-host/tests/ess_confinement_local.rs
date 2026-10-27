//! The confinement contract's portable lane (`story:confinement-contract-on-linux-and-local`).
//!
//! Executes every authored `-on-portable` scenario of `spec/confinement-suite.json` against the
//! real `local` adapter, `systemd` with no user manager, `oci` without a runtime program and
//! the S0 stubs of `macos` and `pod`, through `confinement::open`. The count is pinned per
//! host: 18 declared, all executed off macOS; on macOS `absent-macos-off-macos-on-portable` is
//! named as not executable there, because the adapter it asserts absent is present, and the
//! other 17 run. Every other `absent-*` scenario opens its adapter with the scenario's own
//! unreachable setup, so it executes on every host. Beside the
//! suite: `linux` without a delegated cgroup refuses `limits` by name, each stub refuses to open,
//! `render` under `linux` produces the bubblewrap argv, and a `local` run reports itself declared
//! insecure in every observation.

mod support;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use substrate_host::confinement::{
    self, AdapterName, AdapterRequest, AuditRecord, AuditSink, Command, ConfinementAdapter,
    ConfinementError, ConfinementProfile, ContractFamily, Fact, InnerAdapter, Network,
    ProcessBounds, RunState,
    local::{LocalPolicy, LocalRequest},
    macos::MacosRequest,
    oci::OciRequest,
    pod::PodRequest,
    systemd::{InnerRequest, SystemdRequest},
};

const REASON: &str = "the test runs a host process on purpose";

/// The real adapter each portable scenario's open step names, set up as its summary describes.
fn open_portable(
    step: &support::OpenStep,
) -> Result<Arc<dyn ConfinementAdapter>, ConfinementError> {
    let local = || LocalRequest {
        // The scenario that runs on a host whose policy forbids `local` declares the adapter
        // without `local_permitted`; every other opens `local` on a host that permits it.
        policy: if step.facts.iter().any(|fact| fact == "local_permitted") {
            LocalPolicy::FromHost
        } else {
            LocalPolicy::Refuse
        },
    };
    let request = match step.adapter.as_str() {
        "local" => AdapterRequest::Local(local()),
        "macos" => AdapterRequest::Macos(MacosRequest::default()),
        // The setup the scenario names: `DBUS_SESSION_BUS_ADDRESS` unset, so no user manager.
        "systemd" => AdapterRequest::Systemd(SystemdRequest {
            inner: match step.inner.as_deref() {
                Some("local") => InnerRequest::Local(local()),
                other => panic!("no portable systemd scenario wraps {other:?}"),
            },
            session_bus: None,
        }),
        // The setup the scenario names: a runtime program path that does not exist.
        "oci" => AdapterRequest::Oci(OciRequest {
            runtime: PathBuf::from("/nonexistent/substrate-oci-runtime"),
            writable_roots: Vec::new(),
        }),
        // The setup the scenario names: no cluster configuration.
        "pod" => AdapterRequest::Pod(PodRequest { cluster: None }),
        other => panic!("no portable scenario opens {other}"),
    };
    confinement::open(request)
}

/// The authored `-on-portable` scenarios `spec/confinement-suite.json` holds. Pinned, so a
/// scenario lost or renamed out of the lane fails here instead of shrinking what runs.
const PORTABLE_DECLARED: usize = 18;

/// The portable scenarios this host cannot execute, by name: an `absent-<adapter>-*` scenario on
/// the platform where that adapter is present.
#[cfg(target_os = "macos")]
const NOT_EXECUTABLE_HERE: &[&str] =
    &["substrate.confinement/authored/absent-macos-off-macos-on-portable"];
#[cfg(not(target_os = "macos"))]
const NOT_EXECUTABLE_HERE: &[&str] = &[];

/// What this host executes of the portable lane.
#[cfg(target_os = "macos")]
const PORTABLE_EXECUTED: usize = 17;
#[cfg(not(target_os = "macos"))]
const PORTABLE_EXECUTED: usize = 18;

#[test]
fn ess_confinement_portable_lane_executes_every_authored_portable_scenario() {
    let scenarios = support::scenarios("portable", |_| true);
    assert_eq!(
        scenarios.len(),
        PORTABLE_DECLARED,
        "spec/confinement-suite.json holds another number of authored -on-portable scenarios \
         than this lane pins"
    );
    let (executable, not_here) = support::executable_here(scenarios);
    for id in &not_here {
        println!("ESS scenario not executable on this host: {id}");
    }
    assert_eq!(
        not_here, NOT_EXECUTABLE_HERE,
        "spec/confinement-suite.json: the portable scenarios this host cannot execute"
    );
    assert_eq!(
        executable.len(),
        PORTABLE_EXECUTED,
        "spec/confinement-suite.json: the portable scenarios this host executes"
    );
    let (executed, audited, open_checks) = support::run(&executable, open_portable);
    assert_eq!(
        executed, PORTABLE_EXECUTED,
        "a portable scenario of spec/confinement-suite.json did not run"
    );
    assert!(audited > 0, "no refusal audit record was asserted");
    println!(
        "ESS confinement portable lane: {executed} scenarios executed of {PORTABLE_DECLARED} \
         declared, {} not executable on this host, {audited} refusal audit records asserted, \
         {open_checks} open checks refused, 0 failed",
        not_here.len()
    );
}

fn recording() -> (AuditSink, Arc<Mutex<Vec<AuditRecord>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink_records = Arc::clone(&records);
    (
        AuditSink::new(move |record: &AuditRecord| sink_records.lock().push(record.clone())),
        records,
    )
}

fn local() -> Arc<dyn ConfinementAdapter> {
    confinement::open(AdapterRequest::Local(LocalRequest::default())).expect("local opens")
}

fn code(error: &ConfinementError) -> &'static str {
    error.code()
}

#[test]
fn the_registry_names_all_six_adapters_in_the_contract_order() {
    assert_eq!(
        confinement::ADAPTERS.to_vec(),
        vec![
            AdapterName::Local,
            AdapterName::Linux,
            AdapterName::Macos,
            AdapterName::Systemd,
            AdapterName::Oci,
            AdapterName::Pod,
        ]
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn macos_does_not_open_off_macos() {
    let error = confinement::open(AdapterRequest::Macos(MacosRequest::default()))
        .err()
        .expect("macos is absent");
    assert!(
        matches!(
            &error,
            ConfinementError::AdapterUnserved {
                adapter: AdapterName::Macos,
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(code(&error), "confinement.adapter-unserved");
    assert_eq!(error.status(), 501);
}

#[test]
fn systemd_without_a_user_manager_is_unserved() {
    for inner in [
        InnerRequest::Local(LocalRequest::default()),
        InnerRequest::Linux(Box::new(confinement::linux::LinuxRequest::new(
            substrate_host::HostConfig::minimum(
                tempfile::tempdir().expect("root").path().join("workspaces"),
            ),
        ))),
    ] {
        let error = confinement::open(AdapterRequest::Systemd(SystemdRequest {
            inner,
            session_bus: None,
        }))
        .err()
        .expect("systemd is absent without a session bus");
        assert!(
            matches!(
                &error,
                ConfinementError::AdapterUnserved {
                    adapter: AdapterName::Systemd,
                    ..
                }
            ),
            "{error}"
        );
        assert_eq!(code(&error), "confinement.adapter-unserved");
    }
}

#[test]
fn oci_without_a_runtime_program_is_unserved() {
    let error = confinement::open(AdapterRequest::Oci(OciRequest {
        runtime: PathBuf::from("/nonexistent/substrate-oci-runtime"),
        writable_roots: Vec::new(),
    }))
    .err()
    .expect("oci is absent without a runtime");
    assert!(
        matches!(
            &error,
            ConfinementError::AdapterUnserved {
                adapter: AdapterName::Oci,
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(code(&error), "confinement.adapter-unserved");
}

#[test]
fn pod_with_a_nonexistent_kubeconfig_is_unserved() {
    let error = confinement::open(AdapterRequest::Pod(PodRequest {
        cluster: Some(PathBuf::from("/nonexistent/kubeconfig")),
    }))
    .err()
    .expect("pod is absent");
    assert!(
        matches!(
            &error,
            ConfinementError::AdapterUnserved {
                adapter: AdapterName::Pod,
                ..
            }
        ),
        "{error}"
    );
    assert_eq!(code(&error), "confinement.adapter-unserved");
}

/// A `linux` adapter over a host driver with no delegated cgroup root: the probe proves no
/// `exec.cgroup-limits`, so `limits` is not served.
#[cfg(target_os = "linux")]
fn undelegated_linux(root: &std::path::Path) -> Arc<dyn ConfinementAdapter> {
    let mut config = substrate_host::HostConfig::minimum(root.join("workspaces"));
    config.cgroup_root = None;
    confinement::open(AdapterRequest::Linux(Box::new(
        confinement::linux::LinuxRequest::new(config),
    )))
    .expect("linux opens without a delegated cgroup")
}

#[cfg(target_os = "linux")]
#[test]
fn linux_without_cgroup_limits_refuses_limits_by_name() {
    let root = tempfile::tempdir().expect("root");
    let adapter = undelegated_linux(root.path());
    assert!(
        !adapter.facts().serves(&ContractFamily::Limits),
        "a host without exec.cgroup-limits must not serve limits: {:?}",
        adapter.facts()
    );
    let (sink, records) = recording();
    let profile = ConfinementProfile {
        process_bounds: Some(ProcessBounds {
            timeout_ms: 10_000,
            processes: 8,
            memory_bytes: 64 << 20,
            cpu_millis: 5_000,
        }),
        audit: Some(sink),
        ..ConfinementProfile::default()
    };
    let error = adapter.admit(&profile).expect_err("limits is refused");
    assert!(
        matches!(
            &error,
            ConfinementError::FamilyUnserved {
                adapter: AdapterName::Linux,
                family: ContractFamily::Limits
            }
        ),
        "{error}"
    );
    assert_eq!(code(&error), "confinement.family-unserved");
    let records = records.lock();
    assert_eq!(records.len(), 1, "{records:?}");
    let record = serde_json::to_value(&records[0]).expect("record");
    assert_eq!(record["event"], "profile_refused");
    assert_eq!(record["code"], "confinement.family-unserved");
    assert_eq!(record["family"], "limits");
    assert_eq!(record["adapter"], "linux");
}

#[cfg(target_os = "linux")]
#[test]
fn render_under_linux_is_the_bubblewrap_argv() {
    let root = tempfile::tempdir().expect("root");
    let adapter = undelegated_linux(root.path());
    let workspace = root.path().join("workspaces").join("ws_render");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let profile = ConfinementProfile {
        workspace: Some(workspace),
        ..ConfinementProfile::default()
    };
    let admitted = adapter
        .admit(&profile)
        .expect("a workspace-only profile is admitted");
    let rendered = adapter
        .render(
            &admitted,
            &Command {
                argv: vec!["/bin/true".into()],
                env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
            },
        )
        .expect("linux renders");
    assert!(
        rendered.program.ends_with("bwrap"),
        "{}",
        rendered.program.display()
    );
    for option in ["--unshare-net", "--new-session", "--die-with-parent"] {
        assert!(
            rendered.args.iter().any(|arg| arg == option),
            "the rendered argv lacks {option}: {:?}",
            rendered.args
        );
    }
    assert_eq!(rendered.args.last().map(String::as_str), Some("/bin/true"));
}

#[test]
fn local_refuses_a_filesystem_view_a_network_policy_and_limits_by_family() {
    let adapter = local();
    let cases: Vec<(ConfinementProfile, ContractFamily)> = vec![
        (
            ConfinementProfile {
                read_only_roots: vec![substrate_wire::ReadOnlyRoot {
                    host_path: "/usr".into(),
                    mount: "/opt/usr".into(),
                }],
                ..ConfinementProfile::default()
            },
            ContractFamily::FilesystemView,
        ),
        (
            ConfinementProfile {
                network: Some(Network::Isolated),
                ..ConfinementProfile::default()
            },
            ContractFamily::NetworkEgress,
        ),
        (
            ConfinementProfile {
                network: Some(Network::Egress {
                    apertures: vec!["registry".into()],
                }),
                ..ConfinementProfile::default()
            },
            ContractFamily::NetworkEgress,
        ),
        (
            ConfinementProfile {
                process_bounds: Some(ProcessBounds {
                    timeout_ms: 1_000,
                    processes: 4,
                    memory_bytes: 64 << 20,
                    cpu_millis: 1_000,
                }),
                ..ConfinementProfile::default()
            },
            ContractFamily::Limits,
        ),
    ];
    for (mut profile, family) in cases {
        profile.local_reason = Some(REASON.into());
        let error = adapter.admit(&profile).expect_err("refused");
        match &error {
            ConfinementError::FamilyUnserved {
                adapter: AdapterName::Local,
                family: refused,
            } => assert_eq!(refused, &family),
            other => panic!("expected family-unserved {family:?}, got {other}"),
        }
    }
}

#[test]
fn local_needs_a_reason_that_is_not_blank() {
    let adapter = local();
    for reason in [None, Some(String::new()), Some("   ".to_owned())] {
        let error = adapter
            .admit(&ConfinementProfile {
                local_reason: reason.clone(),
                ..ConfinementProfile::default()
            })
            .expect_err("refused");
        assert!(
            matches!(
                error,
                ConfinementError::LocalReasonMissing {
                    adapter: AdapterName::Local
                }
            ),
            "{reason:?}: {error}"
        );
        assert_eq!(error.status(), 422);
    }
}

#[test]
fn an_inner_other_than_the_opened_one_is_refused_as_adapter_unserved() {
    let adapter = local();
    let error = adapter
        .admit(&ConfinementProfile {
            local_reason: Some(REASON.into()),
            inner: Some(InnerAdapter::Linux),
            ..ConfinementProfile::default()
        })
        .expect_err("refused");
    assert!(
        matches!(
            error,
            ConfinementError::AdapterUnserved {
                adapter: AdapterName::Local,
                ..
            }
        ),
        "{error}"
    );
}

#[test]
fn an_image_is_pinned_only_by_a_full_lowercase_sha256_digest() {
    let digest = "0".repeat(64);
    assert!(confinement::image_is_pinned(&format!(
        "registry.invalid/run@sha256:{digest}"
    )));
    for unpinned in [
        "registry.invalid/run:latest".to_owned(),
        "registry.invalid/run@sha256:".to_owned(),
        format!("registry.invalid/run@sha256:{}", "A".repeat(64)),
        format!("registry.invalid/run@sha256:{}", "0".repeat(63)),
        format!("registry.invalid/run@sha256:{digest}:tag"),
        format!("registry.invalid/run@sha256:{digest}0"),
    ] {
        assert!(!confinement::image_is_pinned(&unpinned), "{unpinned}");
    }
}

#[test]
fn local_refuses_facts_it_does_not_prove_by_name() {
    let adapter = local();
    for fact in [
        Fact::WholeTreeKill,
        Fact::IpcSocketAllowlist,
        Fact::SealedPaths,
        Fact::ImageDigestPinned,
    ] {
        let error = adapter
            .admit(&ConfinementProfile {
                local_reason: Some(REASON.into()),
                facts: vec![fact.clone()],
                ..ConfinementProfile::default()
            })
            .expect_err("refused");
        match &error {
            ConfinementError::FactUnserved { fact: refused, .. } => assert_eq!(refused, &fact),
            other => panic!("expected fact-unserved {fact:?}, got {other}"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_run_is_declared_insecure_in_every_observation() {
    let adapter = local();
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir(workspace.path().join("sub")).expect("workdir");
    let (sink, records) = recording();
    let admitted = adapter
        .admit(&ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            workdir: Some("sub".into()),
            families: vec![ContractFamily::ProcessRun, ContractFamily::Wait],
            audit: Some(sink),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    assert!(admitted.declared_insecure());
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    "pwd; printf done > ../marker; echo oops >&2; exit 3".into(),
                ],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect("started");
    let ended = adapter.wait(&run).await.expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    assert_eq!(ended.exit.as_ref().and_then(|exit| exit.code), Some(3));
    assert_eq!(ended.adapter, AdapterName::Local);
    assert_eq!(ended.declared_insecure.as_deref(), Some(REASON));
    let observed = adapter.observe(&run).await.expect("observed");
    assert_eq!(observed.declared_insecure.as_deref(), Some(REASON));
    let output = adapter.output(&run).await.expect("output");
    let expected = format!(
        "{}\n",
        std::fs::canonicalize(workspace.path().join("sub"))
            .expect("canonical")
            .display()
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    assert_eq!(output.stderr, b"oops\n");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("marker")).expect("marker"),
        "done"
    );
    let events: Vec<String> = records
        .lock()
        .iter()
        .map(|record| {
            serde_json::to_value(record).expect("record")["event"]
                .as_str()
                .expect("event")
                .to_owned()
        })
        .collect();
    assert_eq!(events, ["profile_admitted", "run_started"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_run_ends_on_a_signal() {
    let adapter = local();
    let workspace = tempfile::tempdir().expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let run = adapter
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/sleep".into(), "30".into()],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect("started");
    assert_eq!(
        adapter.observe(&run).await.expect("observed").state,
        RunState::Running
    );
    adapter
        .signal(&run, substrate_wire::Signal::Kill)
        .await
        .expect("signalled");
    let ended = tokio::time::timeout(std::time::Duration::from_secs(10), adapter.wait(&run))
        .await
        .expect("the run ends after a kill")
        .expect("waited");
    assert_eq!(ended.exit.as_ref().and_then(|exit| exit.signal), Some(9));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_admitted_on_one_adapter_does_not_start_on_another() {
    let first = local();
    let second = local();
    let workspace = tempfile::tempdir().expect("workspace");
    let admitted = first
        .admit(&ConfinementProfile {
            local_reason: Some(REASON.into()),
            workspace: Some(workspace.path().to_path_buf()),
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let error = second
        .start(
            &admitted,
            &Command {
                argv: vec!["/bin/true".into()],
                env: BTreeMap::new(),
            },
        )
        .await
        .expect_err("refused");
    assert_eq!(code(&error), "confinement.adapter-unserved");
}
