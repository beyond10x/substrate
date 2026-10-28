//! Adversary pass 1 on `story:pod-unknown-cleanup-outlives-adapter`: the open sweep of orphaned
//! `NetworkPolicy` objects (decision D124, design 28 § *Open*).
//!
//! The first two cases drive the binary's fake API server; the rest drive a scripted fake below
//! that answers each request from a closure, so a case controls the `Date` header and the exact
//! JSON of the policy list.

use super::*;

/// An owned policy, labelled for `run`, created `age` ago by the fake's clock.
fn owned_policy(name: &str, run: &str, age: Duration) -> Value {
    policy(
        name,
        &[
            (pod::MANAGED_BY_LABEL, pod::MANAGED_BY),
            (pod::RUN_LABEL, run),
        ],
        age,
    )
}

/// The approval record's condition: the sweep "never removes the default-deny policy of a run
/// another adapter is still starting". `start` creates the policy, waits for that create's answer
/// (bounded by the 30 s API timeout) and only then creates the Pod (another 30 s), and both the
/// `creationTimestamp` and the `Date` the age is read from are whole seconds. A policy 30 or 45 s
/// old by the server's clock whose Pod is not there yet may therefore still be the only fence of a
/// run whose Pod create is in flight; the age floor equals one API timeout and has no margin.
#[test]
fn adv1_open_keeps_an_owned_policy_whose_run_may_still_be_starting() {
    let fake = Fake::start(Behaviour::default());
    {
        let mut objects = fake.objects.lock();
        for (name, age) in [
            ("substrate-run-thirty", Duration::from_secs(30)),
            ("substrate-run-fortyfive", Duration::from_secs(45)),
        ] {
            objects.insert(
                format!("networkpolicies/{name}"),
                owned_policy(name, name, age),
            );
        }
    }
    let _adapter = fake.open();
    assert_eq!(
        fake.objects(),
        [
            "networkpolicies/substrate-run-fortyfive",
            "networkpolicies/substrate-run-thirty",
        ],
        "story:pod-unknown-cleanup-outlives-adapter: open swept a policy younger than start's two \
         bounded creates (policy, then Pod, 30 s each) plus one second of timestamp truncation, \
         which may fence a run another adapter is still starting; requests: {:#?}",
        fake.requests()
    );
}

/// The sweep reads the Pod its run label names and deletes the policy its own name names. The
/// adapter always names both the same, but nothing checks that: a policy named for a live run and
/// labelled for a gone one is removed while the live run's Pod still runs behind it.
#[test]
fn adv1_open_never_removes_the_policy_of_a_pod_that_exists() {
    let fake = Fake::start(Behaviour::default());
    {
        let mut objects = fake.objects.lock();
        objects.insert(
            "networkpolicies/substrate-run-live".to_owned(),
            owned_policy(
                "substrate-run-live",
                "substrate-run-gone",
                Duration::from_hours(1),
            ),
        );
        objects.insert(
            "pods/substrate-run-live".to_owned(),
            json!({"metadata": {"name": "substrate-run-live"}}),
        );
    }
    let _adapter = fake.open();
    assert!(
        fake.objects()
            .contains(&"networkpolicies/substrate-run-live".to_owned()),
        "open removed the NetworkPolicy named for a Pod that exists, because its run label named \
         another; requests: {:#?}",
        fake.requests()
    );
}

// --- a scripted fake ---------------------------------------------------------------------------

/// One answer: status, `Date` header (absent when `None`) and body.
type Answer = (u16, Option<String>, String);
type Script = Arc<dyn Fn(&str, &str) -> Answer + Send + Sync>;

struct Scripted {
    kubeconfig: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
    _directory: tempfile::TempDir,
}

fn scripted(script: Script) -> Scripted {
    let directory = tempfile::tempdir().expect("directory");
    let identity =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).expect("identity");
    let config = Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![identity.cert.der().clone()],
                rustls::pki_types::PrivatePkcs8KeyDer::from(identity.signing_key.serialize_der())
                    .into(),
            )
            .expect("TLS config"),
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    std::thread::spawn(move || {
        for socket in listener.incoming().flatten() {
            let (config, script, seen) =
                (Arc::clone(&config), Arc::clone(&script), Arc::clone(&seen));
            std::thread::spawn(move || -> std::io::Result<()> {
                socket.set_read_timeout(Some(Duration::from_secs(5)))?;
                let tls = ServerConnection::new(config).map_err(std::io::Error::other)?;
                let mut stream = BufReader::new(StreamOwned::new(tls, socket));
                let mut line = String::new();
                stream.read_line(&mut line)?;
                let mut length = 0_usize;
                loop {
                    let mut header = String::new();
                    stream.read_line(&mut header)?;
                    let header = header.trim_end();
                    if header.is_empty() {
                        break;
                    }
                    if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:")
                    {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0_u8; length];
                stream.read_exact(&mut body)?;
                let mut parts = line.split(' ');
                let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                seen.lock().push(format!("{method} {target}"));
                let (status, date, payload) = script(method, target);
                let date = date
                    .map(|date| format!("date: {date}\r\n"))
                    .unwrap_or_default();
                let writer = stream.get_mut();
                write!(
                    writer,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{date}connection: close\r\n\r\n{payload}",
                    payload.len()
                )?;
                writer.flush()?;
                writer.conn.send_close_notify();
                writer.flush()
            });
        }
    });
    let kubeconfig = json!({
        "apiVersion": "v1",
        "kind": "Config",
        "current-context": "fake",
        "clusters": [{"name": "fake", "cluster": {
            "server": format!("https://{address}"),
            "certificate-authority-data":
                base64::engine::general_purpose::STANDARD.encode(identity.cert.pem()),
        }}],
        "contexts": [{"name": "fake", "context": {
            "cluster": "fake", "user": "fake", "namespace": NAMESPACE,
            "extensions": [{"name": "b10x-substrate/pod", "extension": {"pod_pids_limit": 1024}}],
        }}],
        "users": [{"name": "fake", "user": {"token": "fake-token"}}],
    });
    let path = directory.path().join("kubeconfig.json");
    std::fs::write(&path, serde_json::to_vec(&kubeconfig).expect("json")).expect("write");
    Scripted {
        kubeconfig: path,
        requests,
        _directory: directory,
    }
}

const SERVER_DATE: &str = "Mon, 01 Jan 2024 00:01:10 GMT";

/// Answers open's two probes; `rest` answers everything else.
fn probes_then(rest: impl Fn(&str, &str) -> Answer + Send + Sync + 'static) -> Script {
    Arc::new(move |method, target| {
        let pods = format!("/api/v1/namespaces/{NAMESPACE}/pods");
        if method == "GET" && target == format!("{pods}?limit=1") {
            return (
                200,
                Some(SERVER_DATE.to_owned()),
                r#"{"kind":"PodList","items":[]}"#.to_owned(),
            );
        }
        if method == "GET" && target.starts_with(&format!("{pods}/substrate-log-access-probe/log"))
        {
            return (
                404,
                Some(SERVER_DATE.to_owned()),
                r#"{"kind":"Status","code":404}"#.to_owned(),
            );
        }
        rest(method, target)
    })
}

fn policies_path() -> String {
    format!("/apis/networking.k8s.io/v1/namespaces/{NAMESPACE}/networkpolicies")
}

/// The list as the API server writes it: three owned policies whose ages the server's `Date`
/// (00:01:10) decides: 90 s (the age floor itself), 10 s, and 9.5 s written with fractional
/// seconds and an offset. By this host's clock all three are years old.
fn list_body() -> String {
    let item = |name: &str, created: &str| {
        json!({"metadata": {"name": name, "namespace": NAMESPACE, "creationTimestamp": created,
                            "labels": {pod::MANAGED_BY_LABEL: pod::MANAGED_BY, pod::RUN_LABEL: name}},
               "spec": {"podSelector": {}}})
    };
    json!({"kind": "NetworkPolicyList", "apiVersion": "networking.k8s.io/v1", "metadata": {},
    "items": [
        item("substrate-run-old", "2023-12-31T23:59:40Z"),
        item("substrate-run-young", "2024-01-01T00:01:00Z"),
        item("substrate-run-young-offset", "2024-01-01T01:01:00.5+01:00"),
        {"metadata": {"labels": {pod::MANAGED_BY_LABEL: pod::MANAGED_BY}}},
        {"metadata": {"name": "substrate-run-nolabels"}},
    ]})
    .to_string()
}

/// Design 28: the age is read by "the API server's clock (its `Date`)", never this host's. Only
/// the 90 s policy is removed; a nameless item and one without labels are skipped.
#[test]
fn adv1_open_reads_policy_age_by_the_server_date_with_any_rfc3339_form() {
    let fake = scripted(probes_then(|method, target| {
        let policies = policies_path();
        let pods = format!("/api/v1/namespaces/{NAMESPACE}/pods/");
        if method == "GET" && target.starts_with(&format!("{policies}?")) {
            (200, Some(SERVER_DATE.to_owned()), list_body())
        } else if method == "GET" && target.starts_with(&pods) {
            (
                404,
                Some(SERVER_DATE.to_owned()),
                r#"{"kind":"Status","code":404}"#.to_owned(),
            )
        } else if method == "DELETE" && target.starts_with(&format!("{policies}/")) {
            (200, Some(SERVER_DATE.to_owned()), "{}".to_owned())
        } else {
            (
                500,
                None,
                r#"{"kind":"Status","message":"unexpected"}"#.to_owned(),
            )
        }
    }));
    pod::open_pod(PodRequest {
        cluster: Some(fake.kubeconfig.clone()),
    })
    .unwrap_or_else(|error| panic!("open: {error}"));
    let deletions: Vec<String> = fake
        .requests
        .lock()
        .iter()
        .filter(|request| request.starts_with("DELETE "))
        .cloned()
        .collect();
    assert_eq!(
        deletions,
        [format!("DELETE {}/substrate-run-old", policies_path())],
        "requests: {:#?}",
        fake.requests.lock()
    );
}

/// Another adapter swept first: the policy's delete answers 404, which is the policy gone, and
/// open still opens.
#[test]
fn adv1_open_opens_when_another_adapter_swept_the_policy_first() {
    let fake = scripted(probes_then(|method, target| {
        let policies = policies_path();
        if method == "GET" && target.starts_with(&format!("{policies}?")) {
            (200, Some(SERVER_DATE.to_owned()), list_body())
        } else {
            (
                404,
                Some(SERVER_DATE.to_owned()),
                r#"{"kind":"Status","code":404}"#.to_owned(),
            )
        }
    }));
    pod::open_pod(PodRequest {
        cluster: Some(fake.kubeconfig.clone()),
    })
    .unwrap_or_else(|error| panic!("open after a concurrent sweep: {error}"));
}

/// A Pod read that fails after the list succeeded fails the sweep, and open is refused by name.
#[test]
fn adv1_open_is_refused_when_the_pod_read_fails_after_the_list() {
    let fake = scripted(probes_then(|method, target| {
        let policies = policies_path();
        if method == "GET" && target.starts_with(&format!("{policies}?")) {
            (200, Some(SERVER_DATE.to_owned()), list_body())
        } else {
            (
                500,
                Some(SERVER_DATE.to_owned()),
                r#"{"kind":"Status","message":"etcd"}"#.to_owned(),
            )
        }
    }));
    let error = pod::open_pod(PodRequest {
        cluster: Some(fake.kubeconfig.clone()),
    })
    .err()
    .expect("open is refused");
    assert_eq!(error.code(), "confinement.adapter-unserved", "{error}");
    assert!(error.to_string().contains("sweep"), "{error}");
}
