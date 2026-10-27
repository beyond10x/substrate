//! The `pod` adapter (`story:pod-adapter`, `docs/design/28-pod-confinement-adapter.md`).
//!
//! Portable: the rendered Pod for a fixed profile equals the committed golden manifest byte for
//! byte, and against a fake API server every field design 28 refuses is refused with no request
//! made after open's one probe.
//!
//! The `pod` lane: with `SUBSTRATE_POD_LANE=1`, every authored `pod-*-on-pod` scenario of
//! `spec/confinement-suite.json` runs against a real cluster named by `SUBSTRATE_POD_KUBECONFIG`
//! (the JSON form of a kubeconfig, design 28), and so do the runs: a memory bound ends
//! `OOMKilled`, and the run's Pod and `NetworkPolicy` are gone after `wait`, after a kill and after a
//! deadline. A cluster that does not answer fails the lane. Without the variable the lane is
//! absent and says so; it never reports a case it did not run.

#![allow(clippy::too_many_lines)] // `refused_profiles` lists every refusal of design 28 in one table.

mod support;

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use parking_lot::Mutex;
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use serde_json::{Value, json};
use substrate_host::confinement::{
    self, AdapterName, AdapterRequest, AuditRecord, AuditSink, Command, ConfinementAdapter,
    ConfinementError, ConfinementProfile, ContractFamily, Fact, InnerAdapter, IoBounds, Ipc,
    Network, ProcessBounds, RunState,
    pod::{self, PodAdapter, PodRequest, PodSettings},
};

/// busybox 1.37.0, by its image index digest.
const IMAGE: &str = "docker.io/library/busybox@sha256:bdf57e528e45e4433820e045b29b4597825a1c9e38353532d90a01445013f82e";
const GOLDEN: &str = include_str!("golden/pod-manifest.json");
const GOLDEN_RUN: &str = "substrate-run-00000000000000000000000000";

fn command(argv: &[&str]) -> Command {
    Command {
        argv: argv.iter().map(|arg| (*arg).to_owned()).collect(),
        env: BTreeMap::from([("LANG".to_owned(), "C.UTF-8".to_owned())]),
    }
}

fn bounds(timeout_ms: u64, memory_bytes: u64) -> ProcessBounds {
    ProcessBounds {
        timeout_ms,
        processes: 1024,
        memory_bytes,
        cpu_millis: timeout_ms / 2,
    }
}

fn recording() -> (AuditSink, Arc<Mutex<Vec<AuditRecord>>>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink_records = Arc::clone(&records);
    (
        AuditSink::new(move |record: &AuditRecord| sink_records.lock().push(record.clone())),
        records,
    )
}

#[test]
fn the_rendered_pod_equals_the_golden_manifest() {
    let settings = PodSettings {
        namespace: "substrate-pod-golden".to_owned(),
        run_as_user: 65_532,
        pod_pids_limit: Some(1024),
    };
    let profile = ConfinementProfile {
        image: Some(IMAGE.to_owned()),
        workdir: Some("src".to_owned()),
        families: vec![ContractFamily::Workspace],
        process_bounds: Some(ProcessBounds {
            timeout_ms: 29_500,
            processes: 1024,
            memory_bytes: 64 << 20,
            cpu_millis: 15_000,
        }),
        io_bounds: Some(IoBounds {
            output_bytes: 65_536,
        }),
        ..ConfinementProfile::default()
    };
    let rendered = pod::render_manifests(
        &settings,
        &profile,
        &command(&["/bin/sh", "-c", "echo hi > out"]),
        GOLDEN_RUN,
    )
    .expect("the golden profile renders");
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&rendered).expect("manifest serializes")
    );
    assert_eq!(
        text, GOLDEN,
        "the rendered Pod differs from tests/golden/pod-manifest.json"
    );
}

/// A TLS API server on loopback that answers open's probe and records every request.
struct FakeCluster {
    kubeconfig: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
    _directory: tempfile::TempDir,
}

impl FakeCluster {
    fn start(extension: Option<Value>) -> Self {
        let directory = tempfile::tempdir().expect("fake cluster directory");
        let identity = rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()])
            .expect("fake cluster identity");
        let config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(
                    vec![identity.cert.der().clone()],
                    rustls::pki_types::PrivatePkcs8KeyDer::from(
                        identity.signing_key.serialize_der(),
                    )
                    .into(),
                )
                .expect("fake cluster TLS config"),
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("fake cluster listener");
        listener.set_nonblocking(true).expect("nonblocking");
        let address = listener.local_addr().expect("address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (observed, stopped) = (Arc::clone(&requests), Arc::clone(&stop));
        std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                socket.set_nonblocking(false).expect("blocking socket");
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .expect("read timeout");
                let tls = ServerConnection::new(Arc::clone(&config)).expect("server session");
                let mut stream = BufReader::new(StreamOwned::new(tls, socket));
                let _ = serve(&mut stream, &observed);
            }
        });
        let mut context = json!({"cluster": "fake", "user": "fake", "namespace": "substrate-fake"});
        if let Some(extension) = extension {
            context["extensions"] = json!([{"name": "b10x-substrate/pod", "extension": extension}]);
        }
        let config = json!({
            "apiVersion": "v1",
            "kind": "Config",
            "current-context": "fake",
            "clusters": [{"name": "fake", "cluster": {
                "server": format!("https://{address}"),
                "certificate-authority-data":
                    base64::engine::general_purpose::STANDARD.encode(identity.cert.pem()),
            }}],
            "contexts": [{"name": "fake", "context": context}],
            "users": [{"name": "fake", "user": {"token": "fake-token"}}],
        });
        let kubeconfig = directory.path().join("kubeconfig.json");
        std::fs::write(&kubeconfig, serde_json::to_vec(&config).expect("json")).expect("write");
        Self {
            kubeconfig,
            requests,
            stop,
            _directory: directory,
        }
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().clone()
    }
}

impl Drop for FakeCluster {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn serve(
    stream: &mut BufReader<StreamOwned<ServerConnection, std::net::TcpStream>>,
    observed: &Mutex<Vec<String>>,
) -> std::io::Result<()> {
    let mut line = String::new();
    stream.read_line(&mut line)?;
    let request = line.trim_end().to_owned();
    let mut length = 0_usize;
    let mut authorized = false;
    loop {
        let mut header = String::new();
        stream.read_line(&mut header)?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        let lower = header.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("content-length:") {
            length = value.trim().parse().unwrap_or(0);
        }
        if header == "authorization: Bearer fake-token"
            || header == "Authorization: Bearer fake-token"
        {
            authorized = true;
        }
    }
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body)?;
    observed.lock().push(format!(
        "{request}{}",
        if authorized { "" } else { " (no bearer token)" }
    ));
    let probe = request.starts_with("GET /api/v1/namespaces/substrate-fake/pods?limit=1 ");
    let (status, payload) = if probe && authorized {
        (
            "200 OK",
            r#"{"kind":"PodList","apiVersion":"v1","items":[]}"#,
        )
    } else {
        (
            "500 Internal Server Error",
            r#"{"kind":"Status","message":"unexpected"}"#,
        )
    };
    let writer = stream.get_mut();
    write!(
        writer,
        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len()
    )?;
    writer.flush()?;
    writer.conn.send_close_notify();
    let _ = writer.flush();
    Ok(())
}

fn open_fake(cluster: &FakeCluster) -> Arc<PodAdapter> {
    pod::open_pod(PodRequest {
        cluster: Some(cluster.kubeconfig.clone()),
    })
    .expect("pod opens against the fake cluster")
}

#[test]
fn pod_opens_on_a_fake_cluster_with_its_declared_set_after_one_probe() {
    let cluster = FakeCluster::start(Some(json!({"pod_pids_limit": 1024})));
    let adapter = confinement::open(AdapterRequest::Pod(PodRequest {
        cluster: Some(cluster.kubeconfig.clone()),
    }))
    .expect("pod opens");
    let facts = adapter.facts();
    assert_eq!(facts.adapter, AdapterName::Pod);
    assert_eq!(
        facts.served_families,
        [
            ContractFamily::ProcessRun,
            ContractFamily::ProcessWatch,
            ContractFamily::OutputRead,
            ContractFamily::Wait,
            ContractFamily::Workspace,
            ContractFamily::Limits,
            ContractFamily::Audit,
        ]
    );
    assert_eq!(facts.facts, [Fact::ImageDigestPinned]);
    assert_eq!(
        cluster.requests(),
        ["GET /api/v1/namespaces/substrate-fake/pods?limit=1 HTTP/1.1"]
    );
}

#[test]
fn pod_does_not_open_without_a_usable_cluster_configuration() {
    let cluster = FakeCluster::start(None);
    let original: Value =
        serde_json::from_slice(&std::fs::read(&cluster.kubeconfig).expect("read")).expect("json");
    let directory = tempfile::tempdir().expect("directory");
    let mut cases: Vec<(&str, Option<PathBuf>)> = vec![
        ("no cluster configuration", None),
        ("an absent file", Some(directory.path().join("absent.json"))),
    ];
    let mut variant = |name: &'static str, edit: &dyn Fn(&mut Value)| {
        let mut config = original.clone();
        edit(&mut config);
        let path = directory.path().join(format!("{}.json", cases.len()));
        std::fs::write(&path, serde_json::to_vec(&config).expect("json")).expect("write");
        cases.push((name, Some(path)));
    };
    variant("insecure-skip-tls-verify", &|config| {
        config["clusters"][0]["cluster"]["insecure-skip-tls-verify"] = json!(true);
    });
    variant("a plain http server", &|config| {
        let server = config["clusters"][0]["cluster"]["server"]
            .as_str()
            .expect("server")
            .replace("https://", "http://");
        config["clusters"][0]["cluster"]["server"] = json!(server);
    });
    variant("no trust roots", &|config| {
        config["clusters"][0]["cluster"]
            .as_object_mut()
            .expect("cluster")
            .remove("certificate-authority-data");
    });
    variant("a proxy", &|config| {
        config["clusters"][0]["cluster"]["proxy-url"] = json!("http://127.0.0.1:3128");
    });
    variant("an exec credential plugin", &|config| {
        config["users"][0]["user"]["exec"] = json!({"command": "/bin/true"});
    });
    variant("impersonation", &|config| {
        config["users"][0]["user"]["as"] = json!("system:admin");
    });
    variant("a token file", &|config| {
        config["users"][0]["user"]["tokenFile"] = json!("/var/run/token");
    });
    variant("no credential", &|config| {
        config["users"][0]["user"] = json!({});
    });
    variant("no namespace", &|config| {
        config["contexts"][0]["context"]
            .as_object_mut()
            .expect("context")
            .remove("namespace");
    });
    variant("a namespace that is not a DNS label", &|config| {
        config["contexts"][0]["context"]["namespace"] = json!("../kube-system");
    });
    variant("a root run_as_user", &|config| {
        config["contexts"][0]["context"]["extensions"] =
            json!([{"name": "b10x-substrate/pod", "extension": {"run_as_user": 0}}]);
    });
    variant("no current context", &|config| {
        config["current-context"] = json!("other");
    });
    let yaml = directory.path().join("yaml");
    std::fs::write(&yaml, "apiVersion: v1\nkind: Config\n").expect("write");
    cases.push(("the YAML form", Some(yaml)));
    for (name, path) in cases {
        let error = confinement::open(AdapterRequest::Pod(PodRequest { cluster: path }))
            .err()
            .unwrap_or_else(|| panic!("{name}: pod opened"));
        assert!(
            matches!(
                &error,
                ConfinementError::AdapterUnserved {
                    adapter: AdapterName::Pod,
                    ..
                }
            ),
            "{name}: {error}"
        );
        assert!(
            !error.to_string().contains("fake-token"),
            "{name}: the error carries the credential: {error}"
        );
    }
    assert!(
        cluster.requests().is_empty(),
        "a refused configuration reached the cluster: {:?}",
        cluster.requests()
    );
}

#[test]
fn a_cluster_that_refuses_the_probe_does_not_open_pod() {
    let cluster = FakeCluster::start(None);
    let mut config: Value =
        serde_json::from_slice(&std::fs::read(&cluster.kubeconfig).expect("read")).expect("json");
    config["contexts"][0]["context"]["namespace"] = json!("substrate-other");
    std::fs::write(
        &cluster.kubeconfig,
        serde_json::to_vec(&config).expect("json"),
    )
    .expect("write");
    let error = confinement::open(AdapterRequest::Pod(PodRequest {
        cluster: Some(cluster.kubeconfig.clone()),
    }))
    .err()
    .expect("a 500 does not open");
    assert_eq!(error.code(), "confinement.adapter-unserved", "{error}");
}

/// Every profile field design 28 refuses, each with the refusal it names.
fn refused_profiles() -> Vec<(&'static str, ConfinementProfile, &'static str, Value)> {
    let image = || Some(IMAGE.to_owned());
    let base = || ConfinementProfile {
        image: image(),
        ..ConfinementProfile::default()
    };
    let digest = "0".repeat(64);
    let mut cases = vec![
        (
            "ingress_routes",
            ConfinementProfile {
                ingress_routes: Some(json!({})),
                ..base()
            },
            "confinement.reserved-field-unserved",
            json!({"field": "ingress-routes"}),
        ),
        (
            "rate_bounds",
            ConfinementProfile {
                rate_bounds: Some(json!({})),
                ..base()
            },
            "confinement.reserved-field-unserved",
            json!({"field": "rate-bounds"}),
        ),
        (
            "inner",
            ConfinementProfile {
                inner: Some(InnerAdapter::Linux),
                ..base()
            },
            "confinement.adapter-unserved",
            json!({}),
        ),
        (
            "no image",
            ConfinementProfile::default(),
            "confinement.image-unpinned",
            json!({}),
        ),
        (
            "writable_roots",
            ConfinementProfile {
                writable_roots: vec![substrate_wire::WritableHostRoot {
                    host: "srv".into(),
                    mount: "/srv".into(),
                }],
                ..base()
            },
            "confinement.family-unserved",
            json!({"family": "filesystem_view"}),
        ),
        (
            "read_only_roots",
            ConfinementProfile {
                read_only_roots: vec![substrate_wire::ReadOnlyRoot {
                    host_path: "/usr".into(),
                    mount: "/opt/usr".into(),
                }],
                ..base()
            },
            "confinement.family-unserved",
            json!({"family": "filesystem_view"}),
        ),
        (
            "read_only_files",
            ConfinementProfile {
                read_only_files: vec![substrate_wire::ReadOnlyFile {
                    host_path: "/etc/hosts".into(),
                    mount: "/etc/hosts".into(),
                }],
                ..base()
            },
            "confinement.family-unserved",
            json!({"family": "filesystem_view"}),
        ),
        (
            "a host workspace",
            ConfinementProfile {
                workspace: Some(PathBuf::from("/srv/workspace")),
                ..base()
            },
            "confinement.family-unserved",
            json!({"family": "filesystem_view"}),
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
            json!({"family": "network_egress"}),
        ),
        (
            "an isolated network",
            ConfinementProfile {
                network: Some(Network::Isolated),
                ..base()
            },
            "confinement.family-unserved",
            json!({"family": "network_egress"}),
        ),
        (
            "sealed_paths",
            ConfinementProfile {
                sealed_paths: vec![".git".into()],
                ..base()
            },
            "confinement.fact-unserved",
            json!({"fact": "sealed_paths"}),
        ),
        (
            "exec_subpaths",
            ConfinementProfile {
                exec_subpaths: vec!["bin".into()],
                ..base()
            },
            "confinement.fact-unserved",
            json!({"fact": "exec_subpaths"}),
        ),
        (
            "private_tmpdir",
            ConfinementProfile {
                private_tmpdir: true,
                ..base()
            },
            "confinement.fact-unserved",
            json!({"fact": "private_tmpdir"}),
        ),
        (
            "ipc.socket_allowlist",
            ConfinementProfile {
                ipc: Ipc {
                    socket_allowlist: true,
                    signals_contained: false,
                },
                ..base()
            },
            "confinement.fact-unserved",
            json!({"fact": "ipc_socket_allowlist"}),
        ),
        (
            "ipc.signals_contained",
            ConfinementProfile {
                ipc: Ipc {
                    socket_allowlist: false,
                    signals_contained: true,
                },
                ..base()
            },
            "confinement.fact-unserved",
            json!({"fact": "ipc_signals_contained"}),
        ),
        (
            "whole_tree_kill",
            ConfinementProfile {
                facts: vec![Fact::WholeTreeKill],
                ..base()
            },
            "confinement.fact-unserved",
            json!({"fact": "whole_tree_kill"}),
        ),
        (
            "secret_slots",
            ConfinementProfile {
                secret_slots: vec![substrate_wire::SecretSlotRequest {
                    slot: "token".into(),
                    fd: 3,
                }],
                ..base()
            },
            "exec.secret-slots-unserved",
            json!({}),
        ),
        (
            "a workdir escaping the workspace",
            ConfinementProfile {
                workdir: Some("../etc".into()),
                ..base()
            },
            "exec.workdir-invalid",
            json!({}),
        ),
        (
            "a processes bound below the cluster's",
            ConfinementProfile {
                process_bounds: Some(ProcessBounds {
                    processes: 8,
                    ..bounds(10_000, 64 << 20)
                }),
                ..base()
            },
            "confinement.pod-processes-unserved",
            json!({}),
        ),
        (
            "a CPU rate under one millicore",
            ConfinementProfile {
                process_bounds: Some(ProcessBounds {
                    cpu_millis: 5,
                    ..bounds(10_000, 64 << 20)
                }),
                ..base()
            },
            "exec.limit-unserved",
            json!({}),
        ),
        (
            "a memory bound under 1 MiB",
            ConfinementProfile {
                process_bounds: Some(bounds(10_000, 4096)),
                ..base()
            },
            "exec.limit-unserved",
            json!({}),
        ),
        (
            "an output bound above 1 MiB",
            ConfinementProfile {
                io_bounds: Some(IoBounds {
                    output_bytes: 2 << 20,
                }),
                ..base()
            },
            "exec.limit-unserved",
            json!({}),
        ),
    ];
    for unpinned in [
        "registry.invalid/run:latest".to_owned(),
        "registry.invalid/run@sha256:latest".to_owned(),
        format!("registry.invalid/run@sha256:{}", &digest[..63]),
        format!("registry.invalid/run@sha256:{}", "A".repeat(64)),
        format!("registry.invalid/run@sha256:{digest}0"),
        format!("registry.invalid/run@sha256:{digest}:tag"),
    ] {
        cases.push((
            "an unpinned image",
            ConfinementProfile {
                image: Some(unpinned),
                ..ConfinementProfile::default()
            },
            "confinement.image-unpinned",
            json!({}),
        ));
    }
    cases
}

#[tokio::test(flavor = "multi_thread")]
async fn every_refused_field_is_refused_before_any_api_call() {
    let cluster = FakeCluster::start(Some(json!({"pod_pids_limit": 1024})));
    let adapter = open_fake(&cluster);
    let probe = cluster.requests();
    assert_eq!(probe.len(), 1, "{probe:?}");
    let mut refused = 0;
    for (name, mut profile, code, fields) in refused_profiles() {
        let (sink, records) = recording();
        profile.audit = Some(sink);
        let error = adapter
            .admit(&profile)
            .err()
            .unwrap_or_else(|| panic!("{name} was admitted"));
        assert_eq!(error.code(), code, "{name}: {error}");
        let records = records.lock();
        assert_eq!(records.len(), 1, "{name}: {records:?}");
        let record = serde_json::to_value(&records[0]).expect("record");
        assert_eq!(record["event"], "profile_refused", "{name}: {record}");
        assert_eq!(record["code"], code, "{name}: {record}");
        assert_eq!(record["adapter"], "pod", "{name}: {record}");
        for (field, value) in fields.as_object().expect("fields") {
            assert_eq!(&record[field], value, "{name}: {record}");
        }
        refused += 1;
    }
    // A processes bound at the cluster's limit is admitted, and every refusal of a command is
    // still made before the API is asked anything.
    let admitted = adapter
        .admit(&ConfinementProfile {
            image: Some(IMAGE.to_owned()),
            process_bounds: Some(bounds(10_000, 64 << 20)),
            ..ConfinementProfile::default()
        })
        .expect("a profile within the cluster's bounds is admitted");
    let mut long = command(&["/bin/true"]);
    long.argv.push("x".repeat(300 * 1024));
    let commands: Vec<(&str, Command, &str)> = vec![
        ("an empty argv", command(&[]), "exec.argv-invalid"),
        ("an argv above 256 KiB", long, "exec.argv-invalid"),
        (
            "the local policy variable",
            Command {
                env: BTreeMap::from([(
                    "SUBSTRATE_CONFINEMENT_LOCAL".to_owned(),
                    "permit".to_owned(),
                )]),
                ..command(&["/bin/true"])
            },
            "exec.environment-refused",
        ),
        (
            "a secret-like name",
            Command {
                env: BTreeMap::from([("API_TOKEN".to_owned(), "x".to_owned())]),
                ..command(&["/bin/true"])
            },
            "exec.environment-refused",
        ),
        (
            "a lower-case name",
            Command {
                env: BTreeMap::from([("path".to_owned(), "/bin".to_owned())]),
                ..command(&["/bin/true"])
            },
            "exec.environment-refused",
        ),
    ];
    for (name, command, code) in commands {
        let error = adapter
            .start(&admitted, &command)
            .await
            .err()
            .unwrap_or_else(|| panic!("{name} started"));
        assert_eq!(error.code(), code, "{name}: {error}");
        refused += 1;
    }
    assert_eq!(
        cluster.requests(),
        probe,
        "a refusal reached the cluster API"
    );
    println!("pod portable refusals: {refused} refused before any API call, 0 requests made");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_with_a_processes_bound_is_refused_where_the_operator_declared_no_pids_limit() {
    let cluster = FakeCluster::start(None);
    let adapter = open_fake(&cluster);
    let error = adapter
        .admit(&ConfinementProfile {
            image: Some(IMAGE.to_owned()),
            process_bounds: Some(bounds(10_000, 64 << 20)),
            ..ConfinementProfile::default()
        })
        .expect_err("no processes bound is provable");
    assert_eq!(error.code(), "confinement.pod-processes-unserved");
    assert_eq!(error.status(), 501);
    assert_eq!(cluster.requests().len(), 1);
}

// --- the `pod` lane --------------------------------------------------------------------------

/// The cluster configuration the lane runs against, or why the lane is absent. With
/// `SUBSTRATE_POD_LANE=1` a missing configuration fails the lane.
fn lane() -> Option<PathBuf> {
    if std::env::var_os("SUBSTRATE_POD_LANE").as_deref() != Some("1".as_ref()) {
        return None;
    }
    let path = std::env::var_os("SUBSTRATE_POD_KUBECONFIG")
        .map(PathBuf::from)
        .expect("SUBSTRATE_POD_LANE=1 needs SUBSTRATE_POD_KUBECONFIG");
    assert!(path.is_file(), "{} is not a file", path.display());
    Some(path)
}

fn open_lane(kubeconfig: &Path) -> Arc<PodAdapter> {
    pod::open_pod(PodRequest {
        cluster: Some(kubeconfig.to_path_buf()),
    })
    .unwrap_or_else(|error| panic!("SUBSTRATE_POD_LANE=1 and the cluster does not open: {error}"))
}

#[test]
fn ess_confinement_pod_lane_executes_every_authored_pod_scenario() {
    let Some(kubeconfig) = lane() else {
        println!("ESS confinement pod lane: ABSENT (SUBSTRATE_POD_LANE is not 1); 0 executed");
        return;
    };
    // A cluster that does not open fails here, by name, before any scenario expects it to.
    open_lane(&kubeconfig);
    let scenarios = support::scenarios("pod", |adapter| adapter == "pod");
    let declared = support::lane_size("pod");
    assert!(declared > 0, "the suite has no pod scenario");
    assert_eq!(
        scenarios.len(),
        declared,
        "a pod-lane scenario opens another adapter"
    );
    let (executed, audited, open_checks) = support::run(&scenarios, |step| {
        assert_eq!(step.adapter, "pod");
        confinement::open(AdapterRequest::Pod(PodRequest {
            cluster: Some(kubeconfig.clone()),
        }))
    });
    assert_eq!(executed, declared, "a pod scenario did not run");
    println!(
        "ESS confinement pod lane: {executed} scenarios executed of {declared} declared, \
         {audited} refusal audit records asserted, {open_checks} open checks refused, 0 failed"
    );
}

/// The run's objects still in the cluster, read with `kubectl` rather than the adapter.
fn leftovers(kubeconfig: &Path, namespace: &str, run: &str) -> String {
    let output = std::process::Command::new("kubectl")
        .arg("--kubeconfig")
        .arg(kubeconfig)
        .args(["--namespace", namespace, "get", "pods,networkpolicies"])
        .arg(format!("--selector=b10x-substrate/run={run}"))
        .args(["--output", "name"])
        .output()
        .expect("kubectl runs");
    assert!(
        output.status.success(),
        "kubectl: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8")
}

async fn admit_and_start(
    adapter: &PodAdapter,
    profile: ConfinementProfile,
    argv: &[&str],
) -> confinement::RunId {
    let admitted = adapter.admit(&profile).expect("admitted");
    adapter
        .start(&admitted, &command(argv))
        .await
        .expect("started")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_writes_its_workspace_reports_its_output_and_leaves_nothing_after_wait() {
    let Some(kubeconfig) = lane() else {
        println!("pod lane: ABSENT; 0 executed");
        return;
    };
    let adapter = open_lane(&kubeconfig);
    let (sink, records) = recording();
    let run = admit_and_start(
        &adapter,
        ConfinementProfile {
            image: Some(IMAGE.to_owned()),
            workdir: Some("sub".into()),
            process_bounds: Some(bounds(120_000, 64 << 20)),
            audit: Some(sink),
            ..ConfinementProfile::default()
        },
        &[
            "/bin/sh",
            "-c",
            "pwd; id -u; echo $SUBSTRATE_CONFINEMENT_LOCAL; echo x > /workspace/marker && cat /workspace/marker; touch /etc/x || echo read-only; exit 3",
        ],
    )
    .await;
    let ended = tokio::time::timeout(Duration::from_mins(3), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    assert_eq!(ended.adapter, AdapterName::Pod);
    assert_eq!(ended.declared_insecure, None);
    assert_eq!(ended.exit.and_then(|exit| exit.code), Some(3));
    let output = adapter.output(&run).await.expect("output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("/workspace/sub\n65532\nrefuse\nx\n"),
        "{stdout}"
    );
    assert!(stdout.contains("read-only"), "{stdout}");
    let events: Vec<Value> = records
        .lock()
        .iter()
        .map(|record| serde_json::to_value(record).expect("record")["event"].clone())
        .collect();
    assert_eq!(events, [json!("profile_admitted"), json!("run_started")]);
    assert_eq!(
        leftovers(&kubeconfig, &adapter.settings().namespace, run.as_str()),
        ""
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_over_its_memory_bound_is_oom_killed_and_leaves_nothing() {
    let Some(kubeconfig) = lane() else {
        println!("pod lane: ABSENT; 0 executed");
        return;
    };
    let adapter = open_lane(&kubeconfig);
    let run = admit_and_start(
        &adapter,
        ConfinementProfile {
            image: Some(IMAGE.to_owned()),
            process_bounds: Some(bounds(120_000, 16 << 20)),
            ..ConfinementProfile::default()
        },
        &[
            "/bin/dd",
            "if=/dev/zero",
            "of=/dev/null",
            "bs=64M",
            "count=1",
        ],
    )
    .await;
    let ended = tokio::time::timeout(Duration::from_mins(3), adapter.wait(&run))
        .await
        .expect("the run ends")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    let exit = ended.exit.expect("an exit");
    assert!(
        exit.code.is_some_and(|code| code != 0) || exit.signal.is_some(),
        "{exit:?}"
    );
    let termination = adapter
        .termination(&run)
        .expect("tracked")
        .expect("the run ended");
    assert_eq!(termination.reason.as_deref(), Some("OOMKilled"));
    assert_eq!(termination.memory_oom_kills, 1);
    assert_eq!(
        leftovers(&kubeconfig, &adapter.settings().namespace, run.as_str()),
        ""
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_killed_run_ends_on_sigkill_and_leaves_nothing() {
    let Some(kubeconfig) = lane() else {
        println!("pod lane: ABSENT; 0 executed");
        return;
    };
    let adapter = open_lane(&kubeconfig);
    let run = admit_and_start(
        &adapter,
        ConfinementProfile {
            image: Some(IMAGE.to_owned()),
            process_bounds: Some(bounds(300_000, 64 << 20)),
            ..ConfinementProfile::default()
        },
        &["/bin/sh", "-c", "echo started; exec sleep 600"],
    )
    .await;
    // Wait until the container runs, so the kill reaches a process.
    let deadline = std::time::Instant::now() + Duration::from_mins(2);
    while !String::from_utf8_lossy(&adapter.output(&run).await.expect("output").stdout)
        .contains("started")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the container did not start"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert_eq!(
        adapter.observe(&run).await.expect("observed").state,
        RunState::Running
    );
    assert_eq!(
        adapter
            .signal(&run, substrate_wire::Signal::Int)
            .await
            .expect_err("no signal API")
            .code(),
        "confinement.pod-signal-unserved"
    );
    adapter
        .signal(&run, substrate_wire::Signal::Kill)
        .await
        .expect("killed");
    let ended = tokio::time::timeout(Duration::from_mins(2), adapter.wait(&run))
        .await
        .expect("the run ends after a kill")
        .expect("waited");
    assert_eq!(ended.exit.and_then(|exit| exit.signal), Some(9));
    let output = adapter.output(&run).await.expect("output");
    assert_eq!(output.stdout, b"started\n");
    assert_eq!(
        leftovers(&kubeconfig, &adapter.settings().namespace, run.as_str()),
        ""
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_past_its_deadline_ends_and_leaves_nothing() {
    let Some(kubeconfig) = lane() else {
        println!("pod lane: ABSENT; 0 executed");
        return;
    };
    let adapter = open_lane(&kubeconfig);
    let run = admit_and_start(
        &adapter,
        ConfinementProfile {
            image: Some(IMAGE.to_owned()),
            process_bounds: Some(bounds(5_000, 64 << 20)),
            ..ConfinementProfile::default()
        },
        &["/bin/sleep", "600"],
    )
    .await;
    let ended = tokio::time::timeout(Duration::from_mins(3), adapter.wait(&run))
        .await
        .expect("the run ends at its deadline")
        .expect("waited");
    assert_eq!(ended.state, RunState::Ended);
    let termination = adapter
        .termination(&run)
        .expect("tracked")
        .expect("the run ended");
    assert_eq!(termination.reason.as_deref(), Some("DeadlineExceeded"));
    assert_eq!(
        leftovers(&kubeconfig, &adapter.settings().namespace, run.as_str()),
        ""
    );
}
