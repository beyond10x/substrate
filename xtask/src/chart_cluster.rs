//! `cargo xtask chart-lane cluster`: ADR 0046's install, upgrade and uninstall scenarios on kind.
//!
//! The verb creates its own kind cluster (one control plane, two workers it labels and taints as
//! the dedicated pool), loads the daemon image built from the checkout, and always deletes the
//! cluster again. Identity is a stub on the runner, reachable from the pods at the kind network's
//! IPv4 gateway, with a certificate for that address; the daemon's own certificate names exactly
//! the per-pod hosts the chart's `certificate-names` `ConfigMap` computes (K2 A). The scenarios:
//!
//! 1. every daemon pod is ready, each on a different worker of the pool;
//! 2. each pod's certificate verifies under its own host name and fails under the Service's;
//! 3. a workspace, a file in it and an operation replay survive a rolling restart by
//!    `helm upgrade`, and the replayed operation answers the original workspace;
//! 4. the daemon dials a declared Git source and its connection to an undeclared port on the same
//!    host never arrives (the `NetworkPolicy`); if it did not dial the declared one either, the
//!    scenario is reported absent, never passed;
//! 5. the execution-enabled pod is admitted by server dry run in a `privileged` namespace and
//!    refused in a `baseline` one, and the execution-off install itself runs under `restricted`;
//! 6. `helm uninstall` leaves exactly the ledger claims.

use std::collections::BTreeSet;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use clap::Args as ClapArgs;
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
};
use serde_json::{Value, json};

use crate::chart::{self, CHART};
use crate::chart_yaml;

const RELEASE: &str = "lane";
const NAMESPACE: &str = "substrate-lane";
const PRIVILEGED_NAMESPACE: &str = "substrate-lane-privileged";
const BASELINE_NAMESPACE: &str = "substrate-lane-baseline";
const POOL_KEY: &str = "b10x.dev/substrate-pool";
const POOL_VALUE: &str = "lane";
const REPLICAS: usize = 2;
const AUDIENCE: &str = "urn:b10x:substrate";
const SELECTOR: &str = "app.kubernetes.io/instance=lane,app.kubernetes.io/component=daemon";

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// The daemon image, `repository:tag`, as built from this checkout; loaded into the cluster.
    #[arg(long)]
    image: String,
    /// The pinned kind node image the cluster runs.
    #[arg(long)]
    node_image: String,
    /// The `kind` executable.
    #[arg(long, default_value = "kind")]
    kind: PathBuf,
    /// The `kubectl` executable.
    #[arg(long, default_value = "kubectl")]
    kubectl: PathBuf,
    /// The `helm` executable.
    #[arg(long, default_value = "helm")]
    helm: PathBuf,
    /// The `docker` executable.
    #[arg(long, default_value = "docker")]
    docker: PathBuf,
    /// The kind cluster's name; the verb refuses one that already exists.
    #[arg(long, default_value = "substrate-chart-lane")]
    cluster: String,
}

/// What one scenario came to.
enum Outcome {
    Passed(String),
    Absent(String),
}

/// The execution-enabled scenarios of ADR 0046 the lane declares and never runs: kind's nodes load
/// no Substrate seccomp or `AppArmor` profile and grant no delegated execution, so each is reported
/// absent, by name, on every run (K9 C records them locally).
pub const EXECUTION_ABSENT: [(&str, &str); 4] = [
    (
        "execution bootstrap",
        "the execution bootstrap starting as root and dropping to uid 65532 is not run on kind",
    ),
    (
        "Localhost seccomp profile",
        "the daemon under the b10x-substrate/host-exec-v1-amd64.json Localhost seccomp profile is not run on kind",
    ),
    (
        "Localhost AppArmor profile",
        "the daemon under the substrate-host-exec-v1 Localhost AppArmor profile is not run on kind",
    ),
    (
        "TLS key after the uid drop",
        "reading the 0400 TLS key after the bootstrap drops to uid 65532 is not run on kind",
    ),
];

/// Every scenario the lane names: six it runs, and the execution-enabled ones it reports absent.
const SCENARIOS: usize = 6 + EXECUTION_ABSENT.len();

/// The absent scenarios of one run, by name: those a run could not prove, then every
/// execution-enabled scenario.
fn absent_scenarios(outcomes: &[(String, Outcome)]) -> Vec<(String, String)> {
    let mut absent: Vec<(String, String)> = outcomes
        .iter()
        .filter_map(|(name, outcome)| match outcome {
            Outcome::Absent(reason) => Some((name.clone(), reason.clone())),
            Outcome::Passed(_) => None,
        })
        .collect();
    absent.extend(
        EXECUTION_ABSENT
            .iter()
            .map(|(name, reason)| ((*name).to_owned(), (*reason).to_owned())),
    );
    absent
}

/// One workflow notice per absent scenario, titled so the release preflight can read them back
/// from the run's annotations. Line breaks and the `%` workflow-command escape are encoded.
pub fn absent_annotations(absent: &[(String, String)]) -> Vec<String> {
    absent
        .iter()
        .map(|(name, reason)| {
            let message = format!("{name}: {reason}")
                .replace('%', "%25")
                .replace('\r', "%0D")
                .replace('\n', "%0A");
            format!("::notice title=chart-lane absent::{message}")
        })
        .collect()
}

/// The names of the absent scenarios, `; `-separated, for the step output and the summary line.
pub fn absent_list(absent: &[(String, String)]) -> String {
    absent
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

struct Lane<'a> {
    args: &'a Args,
    kubeconfig: PathBuf,
    work: PathBuf,
}

impl Lane<'_> {
    fn tool(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command.env("KUBECONFIG", &self.kubeconfig);
        command
    }

    fn kubectl(&self, args: &[&str]) -> Result<String> {
        output(self.tool(&self.args.kubectl).args(args))
    }

    fn helm(&self, args: &[&str]) -> Result<String> {
        output(self.tool(&self.args.helm).args(args))
    }
}

fn output(command: &mut Command) -> Result<String> {
    let rendered = format!("{command:?}");
    let result = command
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("starting {rendered}"))?;
    ensure!(
        result.status.success(),
        "{rendered} exited {}: {}{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(String::from_utf8(result.stdout)?)
}

/// Deletes the cluster on every exit path.
struct Cluster<'a> {
    args: &'a Args,
    kubeconfig: PathBuf,
}

impl Drop for Cluster<'_> {
    fn drop(&mut self) {
        let _ = Command::new(&self.args.kind)
            .args([
                "delete",
                "cluster",
                "--name",
                &self.args.cluster,
                "--kubeconfig",
            ])
            .arg(&self.kubeconfig)
            .stdin(Stdio::null())
            .status();
    }
}

#[allow(clippy::too_many_lines)] // The lane in its fixed order: cluster, fixtures, install, six scenarios, teardown.
pub fn run(args: &Args) -> Result<ExitCode> {
    let root = crate::repo::root()?;
    let chart_dir = root.join(CHART);
    let (repository, tag) = args
        .image
        .rsplit_once(':')
        .filter(|(repository, _)| !repository.is_empty())
        .ok_or_else(|| anyhow!("--image {:?} is not repository:tag", args.image))?;
    let clusters = output(Command::new(&args.kind).args(["get", "clusters"]))?;
    ensure!(
        !clusters.lines().any(|line| line == args.cluster),
        "a kind cluster named {} already exists; the lane creates and deletes its own",
        args.cluster
    );

    let work = tempfile::tempdir()?;
    let kubeconfig = work.path().join("kubeconfig");
    let config = work.path().join("kind.yaml");
    fs::write(
        &config,
        "kind: Cluster\napiVersion: kind.x-k8s.io/v1alpha4\nnodes:\n  - role: control-plane\n  - role: worker\n  - role: worker\n",
    )?;
    let cluster = Cluster {
        args,
        kubeconfig: kubeconfig.clone(),
    };
    output(
        Command::new(&args.kind)
            .args([
                "create",
                "cluster",
                "--name",
                &args.cluster,
                "--image",
                &args.node_image,
            ])
            .arg("--config")
            .arg(&config)
            .arg("--kubeconfig")
            .arg(&kubeconfig)
            .args(["--wait", "120s"]),
    )?;
    output(Command::new(&args.kind).args([
        "load",
        "docker-image",
        &args.image,
        "--name",
        &args.cluster,
    ]))?;
    let lane = Lane {
        args,
        kubeconfig,
        work: work.path().to_path_buf(),
    };

    let workers = lane.kubectl(&[
        "get",
        "nodes",
        "-l",
        "!node-role.kubernetes.io/control-plane",
        "-o",
        "name",
    ])?;
    let workers: Vec<&str> = workers.lines().collect();
    ensure!(
        workers.len() == REPLICAS,
        "kind created {} workers, not {REPLICAS}",
        workers.len()
    );
    for worker in &workers {
        lane.kubectl(&["label", worker, &format!("{POOL_KEY}={POOL_VALUE}")])?;
        // `kubectl taint` takes the type and the name as two arguments, not `node/<name>`.
        let name = worker
            .strip_prefix("node/")
            .with_context(|| format!("kubectl named a worker {worker:?}, not node/<name>"))?;
        lane.kubectl(&[
            "taint",
            "nodes",
            name,
            &format!("{POOL_KEY}={POOL_VALUE}:NoSchedule"),
        ])?;
    }
    let gateway = kind_gateway(args)?;

    let material = Material::new(gateway, &chart_dir, &args.helm)?;
    let identity = Listener::identity(gateway, &material)?;
    let allowed = Listener::counting(gateway)?;
    let denied = Listener::counting(gateway)?;

    for (namespace, level) in [
        (NAMESPACE, "restricted"),
        (PRIVILEGED_NAMESPACE, "privileged"),
        (BASELINE_NAMESPACE, "baseline"),
    ] {
        lane.kubectl(&["create", "namespace", namespace])?;
        lane.kubectl(&[
            "label",
            "namespace",
            namespace,
            &format!("pod-security.kubernetes.io/enforce={level}"),
        ])?;
    }
    let certificate = lane.work.join("tls.crt");
    let key = lane.work.join("tls.key");
    let identity_ca = lane.work.join("identity-ca.crt");
    fs::write(&certificate, &material.daemon_pem)?;
    fs::write(&key, &material.daemon_key_pem)?;
    fs::write(&identity_ca, &material.identity_pem)?;
    lane.kubectl(&[
        "create",
        "secret",
        "tls",
        "substrate-tls",
        "-n",
        NAMESPACE,
        &format!("--cert={}", certificate.display()),
        &format!("--key={}", key.display()),
    ])?;
    lane.kubectl(&[
        "create",
        "configmap",
        "identity-ca",
        "-n",
        NAMESPACE,
        &format!("--from-file=ca.crt={}", identity_ca.display()),
    ])?;

    let values = cluster_values(repository, tag, gateway, &identity, &allowed, &denied);
    let values_file = lane.work.join("values.json");
    fs::write(&values_file, serde_json::to_vec_pretty(&values)?)?;
    let values_path = values_file.display().to_string();
    let chart_path = chart_dir.display().to_string();
    lane.helm(&[
        "install",
        RELEASE,
        &chart_path,
        "-n",
        NAMESPACE,
        "--values",
        &values_path,
        "--wait",
        "--timeout",
        "6m",
    ])?;

    let client = material.client()?;
    let mut outcomes = Vec::new();
    let mut failures = Vec::new();
    let mut record = |name: &str, result: Result<Outcome>| match result {
        Ok(outcome) => outcomes.push((name.to_owned(), outcome)),
        Err(error) => failures.push(format!("{name}: {error:#}")),
    };
    record("placement", placement(&lane));
    record(
        "certificate names",
        certificate_names(&lane, &client, &material),
    );
    record(
        "restart survival",
        restart_survival(&lane, &client, &material, &values),
    );
    record(
        "egress",
        egress(&lane, &client, &material, &allowed, &denied),
    );
    record("pod security", pod_security(&lane, &chart_dir, &values));
    record("uninstall", uninstall(&lane));

    identity.stop();
    allowed.stop();
    denied.stop();
    drop(cluster);

    for (name, outcome) in &outcomes {
        match outcome {
            Outcome::Passed(detail) => println!("chart lane cluster: {name}: passed: {detail}"),
            Outcome::Absent(reason) => {
                println!("chart lane cluster: {name}: absent, not passed: {reason}");
            }
        }
    }
    let passed = outcomes
        .iter()
        .filter(|(_, outcome)| matches!(outcome, Outcome::Passed(_)))
        .count();
    let absent = absent_scenarios(&outcomes);
    for line in absent_annotations(&absent) {
        println!("{line}");
    }
    if let Some(path) = std::env::var_os("GITHUB_OUTPUT") {
        let mut file = fs::OpenOptions::new().append(true).open(path)?;
        writeln!(file, "absent={}", absent_list(&absent))?;
    }
    if failures.is_empty() {
        println!(
            "chart lane cluster: {passed} of {SCENARIOS} scenarios passed, {} absent: {}",
            absent.len(),
            absent_list(&absent)
        );
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("chart lane cluster: {} scenario(s) failed", failures.len());
        for failure in &failures {
            eprintln!("  {failure}");
        }
        Ok(ExitCode::FAILURE)
    }
}

fn kind_gateway(args: &Args) -> Result<Ipv4Addr> {
    let config = output(Command::new(&args.docker).args([
        "network",
        "inspect",
        "kind",
        "--format",
        "{{json .IPAM.Config}}",
    ]))?;
    let config: Value = serde_json::from_str(config.trim())?;
    config
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|entry| entry["Gateway"].as_str()?.parse::<Ipv4Addr>().ok())
        .ok_or_else(|| anyhow!("the kind network has no IPv4 gateway: {config}"))
}

/// The lane's certificates and the one bearer credential its Identity stub resolves.
struct Material {
    daemon_pem: String,
    daemon_key_pem: String,
    daemon_der: CertificateDer<'static>,
    identity_pem: String,
    identity_der: CertificateDer<'static>,
    identity_key: Vec<u8>,
    pod_names: Vec<String>,
    service_name: String,
    credential: String,
}

impl Material {
    fn new(gateway: Ipv4Addr, chart_dir: &Path, helm: &Path) -> Result<Self> {
        let rendered = chart::render(helm, chart_dir, RELEASE, NAMESPACE, &chart::base_values())?
            .map_err(|stderr| anyhow!("helm template: {stderr}"))?;
        let names = chart_yaml::documents(&rendered)?
            .into_iter()
            .find(|document| {
                document["kind"] == "ConfigMap"
                    && document["metadata"]["name"] == format!("{RELEASE}-certificate-names")
            })
            .and_then(|document| document["data"]["dns-names"].as_str().map(str::to_owned))
            .ok_or_else(|| anyhow!("the chart rendered no certificate-names ConfigMap"))?;
        let pod_names: Vec<String> = names.lines().map(str::to_owned).collect();
        ensure!(pod_names.len() == REPLICAS, "the chart names {pod_names:?}");
        let daemon_key = KeyPair::generate()?;
        let daemon = CertificateParams::new(pod_names.clone())?.self_signed(&daemon_key)?;
        let identity_key = KeyPair::generate()?;
        let identity =
            CertificateParams::new(vec![gateway.to_string()])?.self_signed(&identity_key)?;
        let secret = ulid::Ulid::generate()
            .to_string()
            .repeat(2)
            .to_ascii_lowercase();
        let credential = format!("identity_access_v1_{}", &secret[..43]);
        Ok(Self {
            daemon_pem: daemon.pem(),
            daemon_key_pem: daemon_key.serialize_pem(),
            daemon_der: daemon.der().clone(),
            identity_pem: identity.pem(),
            identity_der: identity.der().clone(),
            identity_key: identity_key.serialize_der(),
            pod_names,
            service_name: format!("{RELEASE}.{NAMESPACE}.svc.cluster.local"),
            credential,
        })
    }

    fn client(&self) -> Result<Arc<ClientConfig>> {
        let mut roots = RootCertStore::empty();
        roots.add(self.daemon_der.clone())?;
        Ok(Arc::new(
            ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::aws_lc_rs::default_provider(),
            ))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth(),
        ))
    }
}

/// A listener on the kind gateway, served from a thread until stopped.
struct Listener {
    address: SocketAddr,
    accepted: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Listener {
    fn serve(
        gateway: Ipv4Addr,
        handle: impl Fn(TcpStream) + Send + Sync + 'static,
    ) -> Result<Self> {
        let listener = TcpListener::bind((gateway, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let accepted = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let handle = Arc::new(handle);
        let thread = {
            let accepted = Arc::clone(&accepted);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            accepted.fetch_add(1, Ordering::SeqCst);
                            let handle = Arc::clone(&handle);
                            thread::spawn(move || {
                                let _ = stream.set_nonblocking(false);
                                handle(stream);
                            });
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => return,
                    }
                }
            })
        };
        Ok(Self {
            address,
            accepted,
            stop,
            thread: Some(thread),
        })
    }

    fn counting(gateway: Ipv4Addr) -> Result<Self> {
        Self::serve(gateway, drop)
    }

    fn identity(gateway: Ipv4Addr, material: &Material) -> Result<Self> {
        let builder = ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth();
        // The refusal is not propagated: nothing that came out of the call holding the key may
        // reach a printed line. The pair is generated a few lines earlier, so a refusal is a bug.
        let Ok(config) = builder.with_single_cert(
            vec![material.identity_der.clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(material.identity_key.clone())),
        ) else {
            bail!("rustls refused the Identity stub's own generated certificate and key");
        };
        let config = Arc::new(config);
        let credential = material.credential.clone();
        let origin = Arc::new(std::sync::OnceLock::<String>::new());
        let served_origin = Arc::clone(&origin);
        let listener = Self::serve(gateway, move |stream| {
            let origin = served_origin.get().cloned().unwrap_or_default();
            let _ = resolve(stream, &config, &credential, &origin);
        })?;
        origin
            .set(format!("https://{}", listener.address))
            .map_err(|_| anyhow!("the Identity origin was set twice"))?;
        Ok(listener)
    }

    fn origin(&self) -> String {
        format!("https://{}", self.address)
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One `GET /v1/access-authority` answer, then close.
fn resolve(
    stream: TcpStream,
    config: &Arc<ServerConfig>,
    credential: &str,
    origin: &str,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut tls = StreamOwned::new(ServerConnection::new(Arc::clone(config))?, stream);
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = tls.read(&mut buffer)?;
        ensure!(count > 0 && request.len() < 16 * 1024, "incomplete request");
        request.extend_from_slice(&buffer[..count]);
    }
    let head = String::from_utf8_lossy(&request).to_ascii_lowercase();
    let lines: Vec<&str> = head.lines().collect();
    let header = |name: &str| {
        lines
            .iter()
            .find_map(|line| line.strip_prefix(&format!("{name}:")).map(str::trim))
    };
    let raw = String::from_utf8_lossy(&request).into_owned();
    let bearer = raw
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.trim().to_owned())
        })
        .and_then(|value| value.strip_prefix("Bearer ").map(str::to_owned));
    let (status, body) = if !lines
        .first()
        .is_some_and(|line| line.starts_with("get /v1/access-authority "))
        || header("x-b10x-audience") != Some(AUDIENCE)
    {
        ("400 Bad Request", String::new())
    } else if bearer.as_deref() != Some(credential) {
        ("401 Unauthorized", String::new())
    } else {
        let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
        let authority = json!({
            "iss": origin,
            "sub": "chart-lane-subject",
            "aud": AUDIENCE,
            "iat": now,
            "nbf": now,
            "exp": now + 300,
            "jti": ulid::Ulid::generate().to_string(),
            "act": {"sub": "chart-lane-actor"},
            "scope": "observe workspaces",
            "principal_kind": "human",
            "tenant_id": "chart-lane-tenant",
            "email": null,
            "groups": []
        });
        ("200 OK", authority.to_string())
    };
    write!(
        tls,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )?;
    tls.conn.send_close_notify();
    tls.flush()?;
    Ok(())
}

fn cluster_values(
    repository: &str,
    tag: &str,
    gateway: Ipv4Addr,
    identity: &Listener,
    allowed: &Listener,
    denied: &Listener,
) -> Value {
    let host = format!("{gateway}/32");
    chart::merged(
        &chart::base_values(),
        &json!({
            "image": {"repository": repository, "tag": tag, "digest": "", "pullPolicy": "Never"},
            "nodePool": {
                "label": {"key": POOL_KEY, "value": POOL_VALUE},
                "taint": {"key": POOL_KEY, "value": POOL_VALUE, "effect": "NoSchedule"}
            },
            "persistence": {"storageClassName": "standard", "size": "1Gi"},
            "identity": {
                "origin": identity.origin(),
                "caConfigMap": {"name": "identity-ca"},
                "egress": {"cidr": host, "port": identity.address.port()}
            },
            "networkPolicy": {"clients": [], "scrapers": []},
            "resources": {
                "requests": {"cpu": "100m", "memory": "128Mi"},
                "limits": {"cpu": "1", "memory": "1Gi"}
            },
            "daemon": {
                "gitSources": [
                    {"name": "declared", "urlPrefix": format!("https://{}/", allowed.address), "cidr": host, "port": allowed.address.port()},
                    {"name": "undeclared", "urlPrefix": format!("https://{}/", denied.address), "cidr": "192.0.2.20/32", "port": denied.address.port()}
                ],
                "egressApertures": []
            }
        }),
    )
}

/// `kubectl port-forward` to one pod's control port, stopped on drop.
struct Forward(Child, u16);

impl Forward {
    fn open(lane: &Lane<'_>, ordinal: usize) -> Result<Self> {
        let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
        let child = lane
            .tool(&lane.args.kubectl)
            .args([
                "port-forward",
                "-n",
                NAMESPACE,
                &format!("pod/{RELEASE}-{ordinal}"),
                &format!("{port}:8443"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let forward = Self(child, port);
        let deadline = Instant::now() + Duration::from_secs(30);
        while TcpStream::connect(("127.0.0.1", port)).is_err() {
            ensure!(
                Instant::now() < deadline,
                "port-forward to {RELEASE}-{ordinal} never listened"
            );
            thread::sleep(Duration::from_millis(200));
        }
        Ok(forward)
    }
}

impl Drop for Forward {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Where one request goes: a forwarded pod port, verified for `name`.
struct Target<'a> {
    forward: &'a Forward,
    client: &'a Arc<ClientConfig>,
    name: &'a str,
    credential: &'a str,
    timeout: Duration,
    /// The transient Git source authorization a Git workspace create carries; without it the
    /// daemon refuses `workspace.git-authority-absent` before it dials anything.
    source_authorization: Option<&'a str>,
}

impl<'a> Target<'a> {
    fn pod(
        forward: &'a Forward,
        client: &'a Arc<ClientConfig>,
        material: &'a Material,
        ordinal: usize,
    ) -> Self {
        Self {
            forward,
            client,
            name: &material.pod_names[ordinal],
            credential: &material.credential,
            timeout: Duration::from_secs(30),
            source_authorization: None,
        }
    }
}

/// One request over TLS verified for the target's name; the TLS error itself when that fails.
fn https(
    target: &Target<'_>,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<std::result::Result<(u16, Value), rustls::Error>> {
    let Target {
        forward,
        client,
        name,
        credential,
        timeout,
        source_authorization,
    } = *target;
    let stream = TcpStream::connect(("127.0.0.1", forward.1))?;
    stream.set_read_timeout(Some(timeout))?;
    let server = ServerName::try_from(name.to_owned())?;
    let mut tls = StreamOwned::new(ClientConnection::new(Arc::clone(client), server)?, stream);
    let body = body.map(Value::to_string).unwrap_or_default();
    let authorization = source_authorization
        .map(|value| format!("x-b10x-workspace-source-authorization: {value}\r\n"))
        .unwrap_or_default();
    let content = if body.is_empty() {
        String::new()
    } else {
        format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        )
    };
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {name}\r\nAuthorization: Bearer {credential}\r\n{authorization}{content}Connection: close\r\n\r\n{body}"
    );
    let mut response = Vec::new();
    let result = tls
        .write_all(request.as_bytes())
        .and_then(|()| tls.read_to_end(&mut response));
    if let Err(error) = result {
        if let Some(tls_error) = error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<rustls::Error>())
        {
            return Ok(Err(tls_error.clone()));
        }
        if error.kind() != ErrorKind::UnexpectedEof || response.is_empty() {
            return Err(error.into());
        }
    }
    let text = String::from_utf8_lossy(&response);
    let (head, payload) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("incomplete response: {text}"))?;
    let status: u16 = head
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| anyhow!("no status in {head}"))?;
    let payload = if payload.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(payload.trim()).unwrap_or_else(|_| Value::String(payload.to_owned()))
    };
    Ok(Ok((status, payload)))
}

fn call(
    target: &Target<'_>,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<(u16, Value)> {
    https(target, method, path, body)?.map_err(|error| anyhow!("TLS to {}: {error}", target.name))
}

fn daemon_pods(lane: &Lane<'_>) -> Result<Vec<Value>> {
    let pods: Value = serde_json::from_str(
        &lane.kubectl(&["get", "pods", "-n", NAMESPACE, "-l", SELECTOR, "-o", "json"])?,
    )?;
    Ok(pods["items"].as_array().cloned().unwrap_or_default())
}

fn placement(lane: &Lane<'_>) -> Result<Outcome> {
    let pods = daemon_pods(lane)?;
    ensure!(
        pods.len() == REPLICAS,
        "{} daemon pods, not {REPLICAS}",
        pods.len()
    );
    let mut nodes = BTreeSet::new();
    for pod in &pods {
        let ready = pod["status"]["conditions"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|condition| condition["type"] == "Ready" && condition["status"] == "True");
        ensure!(ready, "{} is not ready", pod["metadata"]["name"]);
        let node = pod["spec"]["nodeName"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let labels: Value = serde_json::from_str(&lane.kubectl(&[
            "get",
            "node",
            &node,
            "-o",
            "jsonpath={.metadata.labels}",
        ])?)?;
        ensure!(labels[POOL_KEY] == POOL_VALUE, "{node} is outside the pool");
        nodes.insert(node);
    }
    ensure!(
        nodes.len() == REPLICAS,
        "two daemons share a node: {nodes:?}"
    );
    Ok(Outcome::Passed(format!(
        "{REPLICAS} ready pods on {nodes:?}"
    )))
}

fn certificate_names(
    lane: &Lane<'_>,
    client: &Arc<ClientConfig>,
    material: &Material,
) -> Result<Outcome> {
    for ordinal in 0..REPLICAS {
        let forward = Forward::open(lane, ordinal)?;
        let pod = Target::pod(&forward, client, material, ordinal);
        let (status, payload) = call(&pod, "GET", "/v1/machine", None)?;
        ensure!(
            status == 200,
            "{} answered {status}: {payload}",
            material.pod_names[ordinal]
        );
        let service = Target {
            name: &material.service_name,
            timeout: Duration::from_secs(10),
            ..pod
        };
        match https(&service, "GET", "/v1/machine", None)? {
            Err(rustls::Error::InvalidCertificate(_)) => {}
            other => bail!(
                "{} verified under the Service name {}: {other:?}",
                material.pod_names[ordinal],
                material.service_name
            ),
        }
    }
    Ok(Outcome::Passed(
        "each pod verifies under its own host and fails under the Service name".into(),
    ))
}

fn pod_uid(lane: &Lane<'_>, ordinal: usize) -> Result<String> {
    lane.kubectl(&[
        "get",
        "pod",
        "-n",
        NAMESPACE,
        &format!("{RELEASE}-{ordinal}"),
        "-o",
        "jsonpath={.metadata.uid}",
    ])
}

fn restart_survival(
    lane: &Lane<'_>,
    client: &Arc<ClientConfig>,
    material: &Material,
    values: &Value,
) -> Result<Outcome> {
    let create = json!({"op": ulid::Ulid::generate().to_string(), "input": {"source": "empty", "labels": {"lane": "chart"}}});
    let content = "c3Vydml2ZXMgYSByb2xsaW5nIHJlc3RhcnQ=";
    let before = pod_uid(lane, 0)?;
    let workspace = {
        let forward = Forward::open(lane, 0)?;
        let pod = Target::pod(&forward, client, material, 0);
        let (status, created) = call(&pod, "POST", "/v1/workspaces", Some(&create))?;
        ensure!(
            status == 201,
            "workspace create answered {status}: {created}"
        );
        let workspace = created["result"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let write = json!({"op": ulid::Ulid::generate().to_string(), "input": {"content": {"encoding": "base64", "data": content}}});
        let (status, written) = call(
            &pod,
            "PUT",
            &format!("/v1/workspaces/{workspace}/files/chart-lane.txt"),
            Some(&write),
        )?;
        ensure!(status == 200, "file write answered {status}: {written}");
        workspace
    };

    let rolled = lane.work.join("rolled.json");
    fs::write(
        &rolled,
        serde_json::to_vec_pretty(&chart::merged(
            values,
            &json!({"podAnnotations": {"b10x.dev/lane-roll": "1"}}),
        ))?,
    )?;
    let chart_path = crate::repo::root()?.join(CHART).display().to_string();
    lane.helm(&[
        "upgrade",
        RELEASE,
        &chart_path,
        "-n",
        NAMESPACE,
        "--values",
        &rolled.display().to_string(),
        "--wait",
        "--timeout",
        "6m",
    ])?;
    lane.kubectl(&[
        "rollout",
        "status",
        "-n",
        NAMESPACE,
        &format!("statefulset/{RELEASE}"),
        "--timeout",
        "6m",
    ])?;
    ensure!(
        pod_uid(lane, 0)? != before,
        "the upgrade did not restart {RELEASE}-0"
    );

    let forward = Forward::open(lane, 0)?;
    let pod = Target::pod(&forward, client, material, 0);
    let (status, read) = call(
        &pod,
        "GET",
        &format!(
            "/v1/workspaces/{workspace}/files/chart-lane.txt?mode=file&offset=0&limit_bytes=64"
        ),
        None,
    )?;
    ensure!(
        status == 200 && read["result"]["content"]["data"] == content,
        "the file did not survive the restart: {status} {read}"
    );
    let (status, replayed) = call(&pod, "POST", "/v1/workspaces", Some(&create))?;
    ensure!(
        (200..300).contains(&status) && replayed["result"]["id"] == workspace.as_str(),
        "the replayed create did not answer {workspace}: {status} {replayed}"
    );
    Ok(Outcome::Passed(format!(
        "{workspace} and its file survived a rolling restart; the replayed operation answered it"
    )))
}

fn egress(
    lane: &Lane<'_>,
    client: &Arc<ClientConfig>,
    material: &Material,
    allowed: &Listener,
    denied: &Listener,
) -> Result<Outcome> {
    let forward = Forward::open(lane, 1)?;
    // The listeners only count connections and never read a request, so a fixed value suffices.
    let pod = Target {
        timeout: Duration::from_secs(45),
        source_authorization: Some("chart-lane-source-authorization"),
        ..Target::pod(&forward, client, material, 1)
    };
    let attempt = |source: &str, listener: &Listener| {
        let body = json!({"op": ulid::Ulid::generate().to_string(), "input": {
            "source": {"git": {
                "source": source,
                "locator": format!("https://{}/repository.git", listener.address),
                "ref": "main",
                "commit": "1".repeat(40),
                "depth": 1
            }},
            "labels": {}
        }});
        https(&pod, "POST", "/v1/workspaces", Some(&body))
    };
    let declared = attempt("declared", allowed);
    if allowed.accepted.load(Ordering::SeqCst) == 0 {
        return Ok(Outcome::Absent(format!(
            "the daemon never dialled the declared Git source at {}, so a silent undeclared one proves nothing: {declared:?}",
            allowed.address
        )));
    }
    let undeclared = attempt("undeclared", denied);
    ensure!(
        denied.accepted.load(Ordering::SeqCst) == 0,
        "a connection to the undeclared {} arrived; the NetworkPolicy admits that host on other ports only",
        denied.address
    );
    let dialled = dial_stage(&undeclared)
        .map_err(|answer| anyhow!("the undeclared Git source was refused before the daemon dialled {}, so its silence proves nothing about the NetworkPolicy: {answer}", denied.address))?;
    Ok(Outcome::Passed(format!(
        "the declared {} was reached; the undeclared {} never was, and the daemon dialled it ({dialled})",
        allowed.address, denied.address
    )))
}

/// The refusals the daemon answers only after it dialled a Git source.
const DIAL_STAGE: [&str; 2] = ["workspace.git-fetch-failed", "workspace.git-fetch-timeout"];

/// The first `error.code` anywhere in an answer: the refusal itself or an operation's outcome.
fn refusal_code(payload: &Value) -> Option<&str> {
    match payload {
        Value::Object(map) => map
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .or_else(|| map.values().find_map(refusal_code)),
        Value::Array(items) => items.iter().find_map(refusal_code),
        _ => None,
    }
}

/// Whether the undeclared request reached the dial stage: the daemon answered a dial-stage
/// refusal, or was still dialling when the lane's own read timed out. `Err` carries the answer
/// that shows it refused earlier.
fn dial_stage(
    answer: &Result<std::result::Result<(u16, Value), rustls::Error>>,
) -> std::result::Result<String, String> {
    match answer {
        Ok(Ok((status, payload))) => match refusal_code(payload) {
            Some(code) if DIAL_STAGE.contains(&code) => Ok(format!("{status} {code}")),
            _ => Err(format!("{status} {payload}")),
        },
        Ok(Err(tls)) => Err(format!("TLS to the daemon failed: {tls}")),
        Err(error) => match error.downcast_ref::<std::io::Error>() {
            Some(io) if matches!(io.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                Ok("still dialling when the lane's 45 s read timed out".into())
            }
            _ => Err(format!("{error:#}")),
        },
    }
}

fn pod_security(lane: &Lane<'_>, chart_dir: &Path, values: &Value) -> Result<Outcome> {
    let execution = chart::merged(values, &json!({"execution": {"enabled": true}}));
    let rendered = chart::render(&lane.args.helm, chart_dir, RELEASE, NAMESPACE, &execution)?
        .map_err(|stderr| anyhow!("helm template with execution: {stderr}"))?;
    let statefulset = chart_yaml::documents(&rendered)?
        .into_iter()
        .find(|document| document["kind"] == "StatefulSet")
        .ok_or_else(|| anyhow!("no StatefulSet rendered"))?;
    let mut spec = statefulset["spec"]["template"]["spec"].clone();
    spec["serviceAccountName"] = json!("default");
    spec["volumes"]
        .as_array_mut()
        .ok_or_else(|| anyhow!("the pod has no volumes"))?
        .push(json!({"name": "ledger", "emptyDir": {}}));
    let pod = json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {"name": "pod-security-probe", "labels": statefulset["spec"]["template"]["metadata"]["labels"]},
        "spec": spec
    });
    let file = lane.work.join("pod-security-probe.json");
    fs::write(&file, serde_json::to_vec_pretty(&pod)?)?;
    let file = file.display().to_string();
    lane.kubectl(&[
        "create",
        "--dry-run=server",
        "-n",
        PRIVILEGED_NAMESPACE,
        "-f",
        &file,
        "-o",
        "name",
    ])
    .context("the privileged namespace refused the execution-enabled pod")?;
    match lane.kubectl(&[
        "create",
        "--dry-run=server",
        "-n",
        BASELINE_NAMESPACE,
        "-f",
        &file,
        "-o",
        "name",
    ]) {
        Ok(_) => bail!("the baseline namespace admitted the execution-enabled pod"),
        Err(error) if format!("{error:#}").contains("PodSecurity") => {}
        Err(error) => bail!("the baseline namespace refused for another reason: {error:#}"),
    }
    Ok(Outcome::Passed(
        "execution admitted under privileged, refused under baseline; execution off runs under restricted"
            .into(),
    ))
}

fn uninstall(lane: &Lane<'_>) -> Result<Outcome> {
    lane.helm(&[
        "uninstall",
        RELEASE,
        "-n",
        NAMESPACE,
        "--wait",
        "--timeout",
        "3m",
    ])?;
    let deadline = Instant::now() + Duration::from_mins(2);
    loop {
        let left = lane.kubectl(&[
            "get",
            "statefulsets,services,networkpolicies,serviceaccounts,configmaps,pods",
            "-n",
            NAMESPACE,
            "-l",
            "app.kubernetes.io/instance=lane",
            "-o",
            "name",
        ])?;
        if left.trim().is_empty() {
            break;
        }
        ensure!(Instant::now() < deadline, "uninstall left {left}");
        thread::sleep(Duration::from_secs(2));
    }
    let claims: BTreeSet<String> = lane
        .kubectl(&["get", "pvc", "-n", NAMESPACE, "-o", "name"])?
        .lines()
        .map(str::to_owned)
        .collect();
    let wanted: BTreeSet<String> = (0..REPLICAS)
        .map(|ordinal| format!("persistentvolumeclaim/ledger-{RELEASE}-{ordinal}"))
        .collect();
    ensure!(
        claims == wanted,
        "after uninstall the claims are {claims:?}, not {wanted:?}"
    );
    Ok(Outcome::Passed(format!("only {wanted:?} remain")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_execution_enabled_scenario_is_reported_absent_by_name_on_every_run() {
        let outcomes = vec![
            ("placement".to_owned(), Outcome::Passed("ok".into())),
            (
                "egress".to_owned(),
                Outcome::Absent("never dialled\nthe declared source".into()),
            ),
        ];
        let absent = absent_scenarios(&outcomes);
        let names: Vec<&str> = absent.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "egress",
                "execution bootstrap",
                "Localhost seccomp profile",
                "Localhost AppArmor profile",
                "TLS key after the uid drop"
            ]
        );
        let lines = absent_annotations(&absent);
        assert_eq!(lines.len(), 5);
        assert_eq!(
            lines[0],
            "::notice title=chart-lane absent::egress: never dialled%0Athe declared source"
        );
        assert!(lines.iter().all(|line| !line.contains('\n')));
        assert!(
            lines[1].starts_with("::notice title=chart-lane absent::execution bootstrap: "),
            "{}",
            lines[1]
        );
        assert_eq!(
            absent_list(&absent_scenarios(&[])),
            "execution bootstrap; Localhost seccomp profile; Localhost AppArmor profile; TLS key after the uid drop"
        );
        assert_eq!(SCENARIOS, 10);
    }

    #[test]
    fn the_undeclared_attempt_counts_only_when_it_reached_the_dial_stage() {
        let answer = |status: u16,
                      payload: Value|
         -> Result<std::result::Result<(u16, Value), rustls::Error>> {
            Ok(Ok((status, payload)))
        };
        for code in DIAL_STAGE {
            let direct = answer(502, json!({"error": {"code": code}}));
            assert!(dial_stage(&direct).is_ok(), "{code}");
            let operation = answer(
                200,
                json!({"op": "x", "outcome": {"status": "failed", "error": {"code": code}}}),
            );
            assert!(dial_stage(&operation).is_ok(), "{code} in an operation");
        }
        for early in [
            answer(
                422,
                json!({"error": {"code": "workspace.git-source-invalid"}}),
            ),
            answer(
                501,
                json!({"error": {"code": "workspace.git-source-unserved"}}),
            ),
            answer(
                403,
                json!({"error": {"code": "workspace.git-locator-refused"}}),
            ),
            answer(201, json!({"result": {"id": "ws_1"}})),
            answer(500, Value::Null),
        ] {
            assert!(dial_stage(&early).is_err(), "{early:?}");
        }
        let timed_out: Result<std::result::Result<(u16, Value), rustls::Error>> =
            Err(std::io::Error::from(ErrorKind::WouldBlock).into());
        assert!(dial_stage(&timed_out).is_ok());
        let refused: Result<std::result::Result<(u16, Value), rustls::Error>> =
            Err(std::io::Error::from(ErrorKind::ConnectionRefused).into());
        assert!(dial_stage(&refused).is_err());
    }
}
