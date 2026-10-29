//! The client against the fake API server (design 34 § 12 U1): the paging bound, a 410 answered by
//! a relist, a preconditioned delete refused 409, an answer lost after the API stored the object,
//! and no credential in any error. Each case runs once, in seconds.

use base64::Engine as _;
use futures_util::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{HeaderValue, header};

use crate::fake::{ExecAnswer, ExecRequest, FakeCluster, Fault};
use crate::{Api, ApiError, ListForm, Method, PodEnd, PodTrace, WatchError, kubeconfig, pod_end};

const NAMESPACE: &str = "substrate-client";
const CREDENTIAL: &str = "fake-bearer-credential";

fn start() -> FakeCluster {
    FakeCluster::start(NAMESPACE, CREDENTIAL).expect("the fake starts")
}

fn client(config: &Value) -> Api {
    let (api, ()) =
        kubeconfig::parse(config.to_string().as_bytes(), |_, _| Ok(())).expect("the fake parses");
    api
}

fn pod(name: &str) -> Value {
    json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {"name": name, "labels": {"b10x-substrate/test": "client"}},
        "spec": {"restartPolicy": "Never", "containers": [{"name": "run", "image": "busybox"}]},
    })
}

#[tokio::test]
async fn a_list_reads_every_page_and_refuses_one_past_its_page_bound() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    for number in 0..5 {
        fake.seed("pods", pod(&format!("paged-{number}")))
            .expect("seeded");
    }

    let listed = api
        .list(&api.pods(), "PodList", ListForm::Metadata, 2, 3)
        .await
        .expect("three pages of two hold five");
    let names: Vec<&str> = listed
        .items
        .iter()
        .filter_map(|item| item["metadata"]["name"].as_str())
        .collect();
    assert_eq!(
        names,
        ["paged-0", "paged-1", "paged-2", "paged-3", "paged-4"]
    );
    assert_eq!(listed.resource_version.as_deref(), Some("5"));
    let pages = fake
        .requests()
        .iter()
        .filter(|request| request.starts_with(&format!("GET {}?limit=2", api.pods())))
        .count();
    assert_eq!(pages, 3, "{:?}", fake.requests());

    let error = api
        .list(&api.pods(), "PodList", ListForm::Metadata, 2, 2)
        .await
        .expect_err("two pages of two do not hold five");
    assert_eq!(
        error,
        ApiError::Invalid("the Pod list did not end within 2 pages of 2".to_owned())
    );
}

#[tokio::test]
async fn a_watch_from_a_compacted_version_is_expired_and_a_relist_resumes_it() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    fake.seed("pods", pod("before-0")).expect("seeded");
    fake.seed("pods", pod("before-1")).expect("seeded");
    fake.compact();
    let watch = |version: &str| {
        format!(
            "{}?watch=1&resourceVersion={version}&timeoutSeconds=5",
            api.pods()
        )
    };

    let mut expired = api.watch(&watch("1")).await.expect("the watch opens");
    assert_eq!(expired.next_event().await, Err(WatchError::Expired));

    let listed = api
        .list(&api.pods(), "PodList", ListForm::Whole, 100, 1)
        .await
        .expect("the relist");
    assert_eq!(listed.items.len(), 2);
    let version = listed.resource_version.expect("the list's version");
    let mut resumed = api.watch(&watch(&version)).await.expect("the watch opens");
    fake.seed("pods", pod("after")).expect("seeded");
    let event = resumed
        .next_event()
        .await
        .expect("an event")
        .expect("before the stream ends");
    assert_eq!(event.kind, "ADDED");
    assert_eq!(event.object["metadata"]["name"], "after");
}

#[tokio::test]
async fn a_delete_whose_uid_precondition_names_another_object_is_refused_409() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    let created = api
        .create(&api.pods(), &pod("guarded"))
        .await
        .expect("created");
    let uid = created["metadata"]["uid"].as_str().expect("a uid");
    let path = format!("{}/guarded", api.pods());

    let error = api
        .delete_preconditioned(&path, 1, "00000000-0000-4000-8000-ffffffffffff")
        .await
        .expect_err("another uid");
    assert_eq!(error.status(), Some(409), "{error}");
    assert!(
        error
            .to_string()
            .starts_with("delete answered 409: Precondition failed"),
        "{error}"
    );
    assert!(fake.object("pods", "guarded").is_some(), "the Pod stays");

    assert_eq!(api.delete_preconditioned(&path, 1, uid).await, Ok(false));
    assert!(fake.object("pods", "guarded").is_none(), "the Pod is gone");
}

#[tokio::test]
async fn a_create_whose_answer_is_lost_is_unanswered_and_its_object_is_stored() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    fake.fault(Fault::DropAnswer {
        method: Method::POST,
        path: api.pods(),
    });

    let error = api
        .create(&api.pods(), &pod("lost"))
        .await
        .expect_err("no answer came");
    assert!(matches!(error, ApiError::Unanswered(_)), "{error:?}");
    let stored = api
        .get(&format!("{}/lost", api.pods()))
        .await
        .expect("the read is answered")
        .expect("the API stored the Pod before the answer was lost");
    assert!(stored["metadata"]["uid"].is_string());
}

#[tokio::test]
async fn a_dry_run_create_is_admitted_and_stores_nothing() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    let admitted = api
        .create_dry_run(&api.pods(), &pod("dry"))
        .await
        .expect("admitted");
    assert_eq!(admitted["metadata"]["name"], "dry");
    assert!(fake.object("pods", "dry").is_none());
    assert!(
        fake.requests()
            .contains(&format!("POST {}?dryRun=All", api.pods())),
        "{:?}",
        fake.requests()
    );
}

#[tokio::test]
async fn a_lease_update_from_a_stale_version_is_refused_409() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    let lease = json!({
        "apiVersion": "coordination.k8s.io/v1",
        "kind": "Lease",
        "metadata": {"name": "b10x-substrate-driver"},
        "spec": {"holderIdentity": "deployment-a/one", "leaseDurationSeconds": 30},
    });
    let created = api.create(&api.leases(), &lease).await.expect("created");
    let path = format!("{}/b10x-substrate-driver", api.leases());
    let mut renewed = created.clone();
    renewed["spec"]["holderIdentity"] = json!("deployment-a/two");

    let updated = api.update(&path, &renewed).await.expect("renewed");
    assert_ne!(
        updated["metadata"]["resourceVersion"],
        created["metadata"]["resourceVersion"]
    );
    let error = api
        .update(&path, &created)
        .await
        .expect_err("a stale version");
    assert_eq!(error.status(), Some(409), "{error}");
    let read = api.get(&path).await.expect("read").expect("present");
    assert_eq!(read["spec"]["holderIdentity"], "deployment-a/two");
}

#[tokio::test]
async fn a_rules_review_answers_the_rules_of_the_credential_in_its_namespace() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    let rule = json!({"verbs": ["get"], "apiGroups": [""], "resources": ["pods"]});
    fake.set_rules(vec![rule.clone()]);

    let status = api.rules_review().await.expect("reviewed");
    assert_eq!(status["resourceRules"], json!([rule]));
    assert_eq!(status["incomplete"], false);
}

#[tokio::test]
async fn an_exec_over_websocket_v5_carries_stdin_both_streams_and_the_exit_code() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    fake.seed("pods", pod("keeper")).expect("seeded");
    fake.set_exec(|call: ExecRequest| ExecAnswer {
        stdout: call.stdin.clone(),
        stderr: format!("{} {}", call.container, call.command.join(" ")).into_bytes(),
        exit_code: i32::from(call.stdin.is_empty()) * 3,
    });

    let output = api
        .exec(
            "keeper",
            "keeper",
            &["/substrate-executor", "fs", "$HOME"],
            b"the whole input",
            1024,
        )
        .await
        .expect("the exec ran");
    assert_eq!(output.stdout, b"the whole input");
    assert_eq!(output.stderr, b"keeper /substrate-executor fs $HOME");
    assert_eq!(output.exit_code(), Some(0));

    let output = api
        .exec("keeper", "keeper", &["true"], b"", 4)
        .await
        .expect("the exec ran");
    assert_eq!(output.exit_code(), Some(3));
    assert_eq!(output.stderr, b"keep");
    assert!(output.stderr_truncated);
    assert!(!output.stdout_truncated);
}

#[tokio::test]
async fn no_error_carries_the_credential() {
    let fake = start();
    let config = fake.kubeconfig();
    let api = client(&config);
    let mut errors = Vec::new();

    for member in [
        "exec",
        "auth-provider",
        "username",
        "password",
        "tokenFile",
        "as",
    ] {
        let mut refused = config.clone();
        refused["users"][0]["user"][member] = json!(CREDENTIAL);
        errors.push(
            kubeconfig::parse(refused.to_string().as_bytes(), |_, _| Ok(())).expect_err("refused"),
        );
    }
    let mut unusable = config.clone();
    unusable["users"][0]["user"]["token"] = json!([CREDENTIAL]);
    errors.push(
        kubeconfig::parse(unusable.to_string().as_bytes(), |_, _| Ok(())).expect_err("refused"),
    );
    errors.push(format!("{api:?}"));

    let mut stranger = config.clone();
    stranger["users"][0]["user"]["token"] = json!("another-credential");
    let error = client(&stranger)
        .get(&format!("{}/absent", api.pods()))
        .await
        .expect_err("another credential is refused");
    assert_eq!(error.status(), Some(401));
    errors.push(error.to_string());

    let echo = json!({"kind": "Status", "message": format!("refused Bearer {CREDENTIAL}")});
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: format!("{}/echoed", api.pods()),
        status: 403,
        body: echo.clone(),
    });
    let error = api
        .get(&format!("{}/echoed", api.pods()))
        .await
        .expect_err("refused");
    assert!(error.to_string().contains("[redacted]"), "{error}");
    errors.push(error.to_string());

    fake.fault(Fault::Answer {
        method: Method::GET,
        path: format!("{}?watch=1", api.pods()),
        status: 200,
        body: json!({"type": "ERROR", "object": echo}),
    });
    let mut watch = api
        .watch(&format!("{}?watch=1&resourceVersion=1", api.pods()))
        .await
        .expect("the watch opens");
    let error = watch.next_event().await.expect_err("an error event");
    errors.push(error.to_string());

    fake.seed("pods", pod("echoed-exec")).expect("seeded");
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: format!("{}/echoed-exec/exec", api.pods()),
        status: 403,
        body: echo,
    });
    let error = api
        .exec("echoed-exec", "run", &["true"], b"", 16)
        .await
        .expect_err("refused");
    assert_eq!(error.status(), Some(403), "{error}");
    errors.push(error.to_string());

    for error in &errors {
        assert!(
            !error.contains(CREDENTIAL),
            "the credential leaked: {error}"
        );
    }
}

/// Security review pass 1 (design 34 § 9, design 28): an echoed `Status.message` is cut to 300
/// characters, and the bearer token is cleared from what is left. A token the cut splits is no
/// longer whole, so the clearing misses it and its first characters reach the error. Clearing
/// before cutting keeps every part of it out.
#[tokio::test]
async fn an_echoed_credential_split_by_the_message_bound_leaves_no_part_of_it() {
    const SPLIT: &str = "split-fake-bearer-credential-0123456789abcdef";
    let fake = FakeCluster::start(NAMESPACE, SPLIT).expect("the fake starts");
    let api = client(&fake.kubeconfig());
    let message = format!("{}{SPLIT}", "x".repeat(290));
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: format!("{}/split", api.pods()),
        status: 403,
        body: json!({"kind": "Status", "message": message}),
    });

    let error = api
        .get(&format!("{}/split", api.pods()))
        .await
        .expect_err("refused")
        .to_string();
    assert!(
        !error.contains(&SPLIT[..10]),
        "the credential's first characters leaked: {error}"
    );
}

/// A watch whose response head is later than the client's timeout is unanswered at that timeout,
/// like every other request, instead of waiting for whatever the server does.
#[tokio::test]
async fn a_watch_whose_answer_is_later_than_the_timeout_is_unanswered_at_it() {
    const LATE: std::time::Duration = std::time::Duration::from_secs(5);
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    api.timeout = std::time::Duration::from_millis(300);
    let path = format!("{}?watch=1&resourceVersion=1", api.pods());
    fake.fault(Fault::Delay {
        method: Method::GET,
        path: path.clone(),
        by: LATE,
    });

    let started = std::time::Instant::now();
    let error = api.watch(&path).await.err();
    let waited = started.elapsed();
    assert!(
        matches!(error, Some(WatchError::Unanswered(_))),
        "a late watch is unanswered, not {error:?}"
    );
    assert!(
        waited < LATE,
        "the watch waited {waited:?}, past its timeout"
    );
}

/// The message a gone Pod's watch quotes from an `ERROR` event is bounded to the 300 characters
/// that every other API message the client quotes is bounded to (`WatchError::Error`).
#[tokio::test]
async fn an_error_event_quoted_for_a_gone_pod_is_bounded_like_every_other_api_message() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: format!("{}?watch=1", api.pods()),
        status: 200,
        body: json!({"type": "ERROR", "object": {"kind": "Status", "message": "e".repeat(5_000)}}),
    });

    let end = pod_end(&api, "gone", &PodTrace::since(Some("1"))).await;
    let PodEnd::Unproven(why) = end else {
        panic!("an ERROR event leaves the policy in place: {end:?}");
    };
    let quoted = why
        .strip_prefix("the watch answered an error: ")
        .expect("the reason names the ERROR event");
    assert!(
        quoted.chars().count() <= 300,
        "the quoted message is {} characters",
        quoted.chars().count()
    );
}

/// A TLS WebSocket server under its own CA that answers `pods/exec` as a kubelet streams a command
/// which writes while it reads: every stdin frame is echoed on stdout as it arrives, and the
/// channel-3 `Success` follows the client's close. Returns the kubeconfig that names it.
async fn echoing_exec_server() -> Value {
    let ca_key = rcgen::KeyPair::generate().expect("a CA key");
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("CA parameters");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "echo cluster CA");
    let ca = ca_params.self_signed(&ca_key).expect("the CA");
    let issuer = rcgen::Issuer::from_params(&ca_params, &ca_key);
    let server_key = rcgen::KeyPair::generate().expect("a server key");
    let server = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()])
        .and_then(|params| params.signed_by(&server_key, &issuer))
        .expect("the server certificate");
    let mut tls = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("TLS versions")
    .with_no_client_auth()
    .with_single_cert(
        vec![server.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(server_key.serialize_der()).into(),
    )
    .expect("the server identity");
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(tls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let address = listener.local_addr().expect("the port");

    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                #[allow(
                    clippy::result_large_err,
                    reason = "the handshake callback's error type is fixed by tungstenite"
                )]
                let echo =
                    |_: &Request, mut response: Response| -> Result<Response, ErrorResponse> {
                        response.headers_mut().insert(
                            header::SEC_WEBSOCKET_PROTOCOL,
                            HeaderValue::from_static("v5.channel.k8s.io"),
                        );
                        Ok(response)
                    };
                let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(tls, echo).await else {
                    return;
                };
                while let Some(Ok(message)) = socket.next().await {
                    let Message::Binary(data) = message else {
                        continue;
                    };
                    match data.split_first() {
                        Some((0, input)) => {
                            let mut frame = vec![1_u8];
                            frame.extend_from_slice(input);
                            if socket.send(Message::binary(frame)).await.is_err() {
                                return;
                            }
                        }
                        Some((255, _)) => break,
                        _ => {}
                    }
                }
                let status = serde_json::to_vec(&json!({"metadata": {}, "status": "Success"}))
                    .unwrap_or_default();
                let mut frame = vec![3_u8];
                frame.extend_from_slice(&status);
                let _ = socket.send(Message::binary(frame)).await;
                let _ = socket.close(None).await;
            });
        }
    });

    json!({
        "current-context": "echo",
        "clusters": [{"name": "echo", "cluster": {
            "server": format!("https://{address}"),
            "certificate-authority-data":
                base64::engine::general_purpose::STANDARD.encode(ca.pem()),
        }}],
        "contexts": [{"name": "echo", "context": {
            "cluster": "echo", "user": "echo", "namespace": NAMESPACE,
        }}],
        "users": [{"name": "echo", "user": {"token": CREDENTIAL}}],
    })
}

/// An exec whose command writes while it reads completes: a 64 MiB input the command echoes back
/// is answered in full, not held until the call bound, because the client sends its whole input
/// before it reads any output and both sides then wait on full socket buffers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_exec_whose_command_writes_while_it_reads_completes_its_echo() {
    let config = echoing_exec_server().await;
    let (mut api, ()) =
        kubeconfig::parse(config.to_string().as_bytes(), |_, _| Ok(())).expect("parses");
    api.timeout = std::time::Duration::from_secs(5);
    let small = api
        .exec("keeper", "run", &["cat"], b"ping", 16)
        .await
        .expect("a small echo completes");
    assert_eq!(small.stdout, b"ping");

    let input = vec![b'x'; 64 * 1024 * 1024];
    let output = api
        .exec("keeper", "run", &["cat"], &input, input.len())
        .await
        .expect("the echo of 64 MiB completes within the call bound");
    assert!(
        output.stdout == input,
        "the echo returned {} of {} bytes",
        output.stdout.len(),
        input.len()
    );
}
