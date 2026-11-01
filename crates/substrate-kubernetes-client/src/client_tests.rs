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

const ACCESS_REVIEWS: &str = "/apis/authorization.k8s.io/v1/selfsubjectaccessreviews";

/// `story:driver-names-a-bare-exec-refusals-verb`: a `SelfSubjectAccessReview` answers whether
/// the credential may do one verb on one named object of a resource or subresource in its
/// namespace, as the rules the API holds for it decide; it is one POST and stores nothing.
#[tokio::test]
async fn an_access_review_answers_whether_the_credential_may_do_one_verb_on_one_object() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    fake.set_rules(vec![
        json!({"verbs": ["get"], "apiGroups": [""], "resources": ["pods/exec"]}),
        json!({
            "verbs": ["create"], "apiGroups": [""], "resources": ["pods/exec"],
            "resourceNames": ["other-keeper"],
        }),
    ]);

    for (resource, verb, name, admitted) in [
        ("pods/exec", "get", "keeper", true),
        ("pods/exec", "create", "keeper", false),
        ("pods/exec", "create", "other-keeper", true),
        ("pods", "get", "keeper", false),
        ("pods/log", "get", "keeper", false),
    ] {
        let allowed = api
            .access_review("", resource, verb, name)
            .await
            .unwrap_or_else(|error| panic!("{resource} {verb} {name} is reviewed: {error}"));
        assert_eq!(allowed, admitted, "{resource} {verb} {name}");
    }
    let requests = fake.requests();
    assert_eq!(requests.len(), 5, "{requests:#?}");
    assert!(
        requests
            .iter()
            .all(|request| *request == format!("POST {ACCESS_REVIEWS}")),
        "{requests:#?}"
    );
    assert!(fake.object("pods", "keeper").is_none());
}

/// `story:driver-names-a-bare-exec-refusals-verb`: the review's own refusal is the API's answer,
/// and an answer that states no `status.allowed` boolean is unreadable, never read as a denial.
#[tokio::test]
async fn an_access_review_refused_or_answered_without_allowed_is_an_error() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    fake.fault(Fault::Answer {
        method: Method::POST,
        path: ACCESS_REVIEWS.to_owned(),
        status: 403,
        body: json!({
            "kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure",
            "message": "selfsubjectaccessreviews.authorization.k8s.io is forbidden: the review \
                        is not granted",
            "reason": "Forbidden", "code": 403,
        }),
    });
    let refused = api
        .access_review("", "pods/exec", "create", "keeper")
        .await
        .expect_err("the review is refused");
    assert_eq!(refused.status(), Some(403), "{refused}");
    assert!(
        refused.to_string().contains("the review is not granted"),
        "{refused}"
    );

    for body in [
        json!({"kind": "SelfSubjectAccessReview"}),
        json!({"status": {"allowed": "false"}}),
    ] {
        fake.fault(Fault::Answer {
            method: Method::POST,
            path: ACCESS_REVIEWS.to_owned(),
            status: 201,
            body: body.clone(),
        });
        let unread = api
            .access_review("", "pods/exec", "create", "keeper")
            .await
            .expect_err("no allowed boolean");
        assert!(matches!(unread, ApiError::Invalid(_)), "{body}: {unread}");
    }
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

/// A driver that replays a gone Pod's watch itself quotes an `ERROR` event's message as this
/// client's own replay does: the bearer token cleared first, then bounded, so a token that
/// straddles the bound is not left as a prefix either.
#[test]
fn an_error_event_a_driver_replays_is_quoted_without_the_bearer_token() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    for message in [
        format!("refused Bearer {CREDENTIAL}"),
        format!("{}{CREDENTIAL}", "e".repeat(290)),
    ] {
        let line = serde_json::to_vec(
            &json!({"type": "ERROR", "object": {"kind": "Status", "message": message}}),
        )
        .expect("serializes");
        let end = api.quoted_watch_event(&line, "gone", &mut PodTrace::since(Some("1")));
        let Some(PodEnd::Unproven(why)) = end else {
            panic!("an ERROR event is unproven: {end:?}");
        };
        assert!(!why.contains(CREDENTIAL), "{why}");
        assert!(!why.contains(&CREDENTIAL[..8]), "{why}");
        assert_eq!(api.redact(&format!("echo {CREDENTIAL}")), "echo [redacted]");
    }
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

/// `story:pod-orphaned-policy-is-swept`: a replay of a Pod never read asks for bookmarks from the
/// trace's version; a `BOOKMARK` alone shows the Pod never created, while an `ERROR` event keeps it
/// unproven and quotes its message without the bearer token, as [`pod_end`] does.
#[tokio::test]
async fn a_replay_ending_in_a_bookmark_alone_shows_the_pod_never_created() {
    use crate::PodHistory;
    let fake = start();
    let api = client(&fake.kubeconfig());
    let watch = format!("{}?watch=1", api.pods());
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: watch.clone(),
        status: 200,
        body: json!({"type": "BOOKMARK",
                     "object": {"kind": "Pod", "metadata": {"resourceVersion": "9"}}}),
    });
    let history = api.pod_history("gone", &PodTrace::since(Some("1"))).await;
    assert_eq!(history, PodHistory::NeverCreated);
    let requests = fake.requests();
    assert!(
        requests.iter().any(|request| request.contains(
            "fieldSelector=metadata.name%3Dgone&resourceVersion=1&allowWatchBookmarks=true\
             &timeoutSeconds=5"
        )),
        "{requests:#?}"
    );

    let fake = start();
    let api = client(&fake.kubeconfig());
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: watch,
        status: 200,
        body: json!({"type": "ERROR",
                     "object": {"kind": "Status", "message": format!("refused {CREDENTIAL}")}}),
    });
    let history = api.pod_history("gone", &PodTrace::since(Some("1"))).await;
    let PodHistory::End(PodEnd::Unproven(why)) = history else {
        panic!("an ERROR event keeps the policy: {history:?}");
    };
    assert!(!why.contains(CREDENTIAL), "{why}");
    for name in ["", "Gone", "x,metadata.namespace=y"] {
        let history = api.pod_history(name, &PodTrace::since(Some("1"))).await;
        assert!(
            matches!(history, PodHistory::End(PodEnd::Unproven(_))),
            "{name:?}: {history:?}"
        );
    }
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

/// A loopback TLS listener under its own CA, and the kubeconfig that names it with [`CREDENTIAL`].
async fn tls_endpoint() -> (tokio_rustls::TlsAcceptor, tokio::net::TcpListener, Value) {
    let ca_key = rcgen::KeyPair::generate().expect("a CA key");
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).expect("CA parameters");
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "raw cluster CA");
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let address = listener.local_addr().expect("the port");
    let config = json!({
        "current-context": "raw",
        "clusters": [{"name": "raw", "cluster": {
            "server": format!("https://{address}"),
            "certificate-authority-data":
                base64::engine::general_purpose::STANDARD.encode(ca.pem()),
        }}],
        "contexts": [{"name": "raw", "context": {
            "cluster": "raw", "user": "raw", "namespace": NAMESPACE,
        }}],
        "users": [{"name": "raw", "user": {"token": CREDENTIAL}}],
    });
    (
        tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(tls)),
        listener,
        config,
    )
}

/// What the API's authorization answers an exec upgrade under a Role without `pods/exec`
/// `create`, with names of the length the namespace lane gives them (D342).
const EXEC_FORBIDDEN: &str = "pods \"snl-01jabcdefghjkmnpqrstvwxyz0-keeper\" is forbidden: User \
     \"system:serviceaccount:substrate-namespace-lane-6n7mxygg265x:substrate-namespace\" cannot \
     create resource \"pods/exec\" in API group \"\" in the namespace \
     \"substrate-namespace-lane-6n7mxygg265x\"";

/// The part of [`EXEC_FORBIDDEN`] the namespace lane reads a refusal by.
const EXEC_REFUSED: &str = "cannot create resource \"pods/exec\"";

/// D342: an exec upgrade the API refuses 403 is an answer, and its error carries the API's own
/// `Status.message`, so the reader learns which verb on which resource was refused.
#[tokio::test]
async fn a_refused_exec_upgrade_carries_the_apis_message() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    fake.seed("pods", pod("snl-01jabcdefghjkmnpqrstvwxyz0-keeper"))
        .expect("seeded");
    fake.refuse_exec(403, EXEC_FORBIDDEN);

    let error = api
        .exec(
            "snl-01jabcdefghjkmnpqrstvwxyz0-keeper",
            "keeper",
            &["/substrate-executor", "version"],
            b"",
            16,
        )
        .await
        .expect_err("the upgrade is refused");
    assert_eq!(error.status(), Some(403), "{error}");
    let ApiError::Answered { reason, .. } = &error else {
        panic!("not an answer: {error:?}");
    };
    assert_eq!(reason, "Forbidden");
    assert!(
        error.to_string().contains(EXEC_REFUSED),
        "the API's answer is lost: {error}"
    );
}

/// The error of an exec whose upgrade a server refuses 403 with an [`EXEC_FORBIDDEN`] `Status`,
/// writing the response head first and its body 100 ms later: by `Content-Length`, or chunked.
async fn exec_refused_apart(chunked: bool) -> ApiError {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let (acceptor, listener, config) = tls_endpoint().await;
    tokio::spawn(async move {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let Ok(mut tls) = acceptor.accept(tcp).await else {
            return;
        };
        let mut head = Vec::new();
        let mut buffer = [0_u8; 4096];
        while !head.windows(4).any(|window| window == b"\r\n\r\n") {
            match tls.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(read) => head.extend_from_slice(&buffer[..read]),
            }
        }
        let status = serde_json::to_vec(&json!({
            "kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure",
            "message": EXEC_FORBIDDEN, "reason": "Forbidden",
            "details": {"name": "snl-01jabcdefghjkmnpqrstvwxyz0-keeper", "kind": "pods"},
            "code": 403,
        }))
        .unwrap_or_default();
        let (framing, body) = if chunked {
            let (first, second) = status.split_at(status.len() / 2);
            let mut body = Vec::new();
            for chunk in [first, second] {
                body.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
                body.extend_from_slice(chunk);
                body.extend_from_slice(b"\r\n");
            }
            body.extend_from_slice(b"0\r\n\r\n");
            ("Transfer-Encoding: chunked".to_owned(), body)
        } else {
            (format!("Content-Length: {}", status.len()), status)
        };
        let head = format!(
            "HTTP/1.1 403 Forbidden\r\nCache-Control: no-cache, private\r\n\
             Content-Type: application/json\r\nX-Content-Type-Options: nosniff\r\n\
             {framing}\r\n\r\n"
        );
        let _ = tls.write_all(head.as_bytes()).await;
        let _ = tls.flush().await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _ = tls.write_all(&body).await;
        let _ = tls.flush().await;
        let mut rest = [0_u8; 64];
        let _ = tls.read(&mut rest).await;
    });
    let mut api = client(&config);
    api.timeout = std::time::Duration::from_secs(5);
    api.exec(
        "snl-01jabcdefghjkmnpqrstvwxyz0-keeper",
        "keeper",
        &["/substrate-executor", "version"],
        b"",
        16,
    )
    .await
    .expect_err("the upgrade is refused")
}

/// D342: an API server may write the refusal's head and its body apart. The client reads the
/// body the head announces, not only what arrived with the head.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_exec_upgrade_whose_body_follows_its_head_carries_the_apis_message() {
    for chunked in [false, true] {
        let error = exec_refused_apart(chunked).await;
        assert_eq!(error.status(), Some(403), "{error}");
        assert!(
            error.to_string().contains(EXEC_REFUSED),
            "the API's answer is lost (chunked: {chunked}): {error}"
        );
    }
}

/// An HTTPS server that reads each request head, writes `answer` verbatim (or nothing), and then
/// reads until the client closes the connection, which it reports on the returned channel.
async fn raw_http_server(
    answer: Option<&'static [u8]>,
) -> (Value, tokio::sync::mpsc::UnboundedReceiver<()>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let (acceptor, listener, config) = tls_endpoint().await;
    let (closed, notices) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let closed = closed.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let mut head = Vec::new();
                let mut buffer = [0_u8; 4096];
                while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                    match tls.read(&mut buffer).await {
                        Ok(0) | Err(_) => {
                            let _ = closed.send(());
                            return;
                        }
                        Ok(read) => head.extend_from_slice(&buffer[..read]),
                    }
                }
                if let Some(answer) = answer
                    && (tls.write_all(answer).await.is_err() || tls.flush().await.is_err())
                {
                    return;
                }
                while matches!(tls.read(&mut buffer).await, Ok(read) if read > 0) {}
                let _ = closed.send(());
            });
        }
    });
    (config, notices)
}

/// A call that outlasts its bound takes its connection with it, whether the bound passed before
/// the answer's head or in the middle of its body: the task that drives it is aborted, as a
/// watch's is, never left holding the connection open.
#[tokio::test]
async fn a_call_that_outlasts_its_bound_closes_its_connection() {
    const PARTIAL: &[u8] =
        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1000\r\n\r\n{";
    for answer in [None, Some(PARTIAL)] {
        let (config, mut closed) = raw_http_server(answer).await;
        let mut api = client(&config);
        api.timeout = std::time::Duration::from_millis(300);

        let error = api
            .get(&format!("{}/silent", api.pods()))
            .await
            .expect_err("the server never answers whole");
        assert!(matches!(error, ApiError::Unanswered(_)), "{error:?}");
        let notice = tokio::time::timeout(std::time::Duration::from_secs(2), closed.recv()).await;
        assert!(
            matches!(notice, Ok(Some(()))),
            "the connection outlived its call: {notice:?}"
        );
    }
}

/// A 2xx answer to a create, an update or a read whose body is not the object is
/// [`ApiError::Invalid`], never a `Null` the caller then reads as an object.
#[tokio::test]
async fn a_success_whose_body_is_not_the_object_is_invalid() {
    let (config, _closed) = raw_http_server(Some(
        b"HTTP/1.1 201 Created\r\ncontent-type: application/json\r\ncontent-length: 3\r\n\r\nabc",
    ))
    .await;
    let api = client(&config);
    let created = api.create(&api.pods(), &pod("garbled")).await;
    assert!(matches!(created, Err(ApiError::Invalid(_))), "{created:?}");
    let lease = json!({
        "apiVersion": "coordination.k8s.io/v1",
        "kind": "Lease",
        "metadata": {"name": "garbled", "resourceVersion": "7"},
        "spec": {},
    });
    let updated = api
        .update(&format!("{}/garbled", api.leases()), &lease)
        .await;
    assert!(matches!(updated, Err(ApiError::Invalid(_))), "{updated:?}");

    let fake = start();
    let api = client(&fake.kubeconfig());
    fake.fault(Fault::Answer {
        method: Method::POST,
        path: api.pods(),
        status: 201,
        body: json!(["not", "a", "pod"]),
    });
    let created = api.create(&api.pods(), &pod("listed")).await;
    assert!(matches!(created, Err(ApiError::Invalid(_))), "{created:?}");
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: format!("{}/scalar", api.pods()),
        status: 200,
        body: json!("scalar"),
    });
    let read = api.get(&format!("{}/scalar", api.pods())).await;
    assert!(matches!(read, Err(ApiError::Invalid(_))), "{read:?}");
}

/// An update is conditional or it is not sent: an object without a `metadata.resourceVersion`
/// would replace whatever the API holds, so it is refused before any request.
#[tokio::test]
async fn an_update_without_a_resource_version_is_refused_before_any_request() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    let path = format!("{}/b10x-substrate-driver", api.leases());
    for metadata in [
        json!({"name": "b10x-substrate-driver"}),
        json!({"name": "b10x-substrate-driver", "resourceVersion": ""}),
        json!({"name": "b10x-substrate-driver", "resourceVersion": 7}),
    ] {
        let lease = json!({
            "apiVersion": "coordination.k8s.io/v1",
            "kind": "Lease",
            "metadata": metadata,
            "spec": {"holderIdentity": "deployment-a/one"},
        });
        let error = api.update(&path, &lease).await.expect_err("refused");
        assert!(matches!(error, ApiError::Invalid(_)), "{error:?}");
    }
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
}

/// A Pod or container name the client puts into a path, a query or a field selector is a
/// DNS-1123 name or it is refused before any request: no name reaches another path or another
/// query member.
#[tokio::test]
async fn a_name_that_is_not_dns_1123_is_refused_before_any_request() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    let long = "a".repeat(254);
    for name in [
        "",
        "Upper",
        "a/b",
        "../x",
        "x?watch=1",
        "x&container=y",
        "x%2Fy",
        "x,metadata.namespace=y",
        "-x",
        "x-",
        "x..y",
        long.as_str(),
    ] {
        let error = api.log(name, "run", 16).await.expect_err("refused");
        assert!(matches!(error, ApiError::Invalid(_)), "{name:?}: {error:?}");
        let error = api
            .exec(name, "run", &["true"], b"", 16)
            .await
            .expect_err("refused");
        assert!(matches!(error, ApiError::Invalid(_)), "{name:?}: {error:?}");
        let end = pod_end(&api, name, &PodTrace::since(Some("1"))).await;
        assert!(matches!(end, PodEnd::Unproven(_)), "{name:?}: {end:?}");
    }
    let long = "a".repeat(64);
    for container in ["", "Run", "run&limitBytes=1", "run/x", "a.b", long.as_str()] {
        let error = api.log("pod", container, 16).await.expect_err("refused");
        assert!(
            matches!(error, ApiError::Invalid(_)),
            "{container:?}: {error:?}"
        );
        let error = api
            .exec("pod", container, &["true"], b"", 16)
            .await
            .expect_err("refused");
        assert!(
            matches!(error, ApiError::Invalid(_)),
            "{container:?}: {error:?}"
        );
    }
    assert!(fake.requests().is_empty(), "{:?}", fake.requests());
}

/// A list whose pages hold more items than `page_items` × `max_pages` is refused, whatever each
/// page's own size: a server that ignores `limit` cannot hand the caller more than it asked for.
#[tokio::test]
async fn a_list_holding_more_items_than_its_pages_allow_is_refused() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    let items: Vec<Value> = (0..5)
        .map(|number| json!({"metadata": {"name": format!("unpaged-{number}")}}))
        .collect();
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: format!("{}?limit=2", api.pods()),
        status: 200,
        body: json!({"kind": "PodList", "metadata": {"resourceVersion": "9"}, "items": items}),
    });

    let error = api
        .list(&api.pods(), "PodList", ListForm::Whole, 2, 2)
        .await
        .expect_err("five items do not fit two pages of two");
    assert!(matches!(error, ApiError::Invalid(_)), "{error:?}");
}

/// A command that exits without reading its input still has its `Status`: the server sends it and
/// closes, the client's input has nowhere to go, and the exec answers the exit code it was sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_exec_whose_command_exits_before_reading_its_input_keeps_its_status() {
    let (acceptor, listener, config) = tls_endpoint().await;
    tokio::spawn(async move {
        let Ok((tcp, _)) = listener.accept().await else {
            return;
        };
        let Ok(tls) = acceptor.accept(tcp).await else {
            return;
        };
        #[allow(
            clippy::result_large_err,
            reason = "the handshake callback's error type is fixed by tungstenite"
        )]
        let choose = |_: &Request, mut response: Response| -> Result<Response, ErrorResponse> {
            response.headers_mut().insert(
                header::SEC_WEBSOCKET_PROTOCOL,
                HeaderValue::from_static("v5.channel.k8s.io"),
            );
            Ok(response)
        };
        let Ok(mut socket) = tokio_tungstenite::accept_hdr_async(tls, choose).await else {
            return;
        };
        let status = json!({
            "metadata": {},
            "status": "Failure",
            "reason": "NonZeroExitCode",
            "details": {"causes": [{"reason": "ExitCode", "message": "7"}]},
        });
        let mut frame = vec![3_u8];
        frame.extend_from_slice(&serde_json::to_vec(&status).unwrap_or_default());
        let _ = socket.send(Message::binary(frame)).await;
        let _ = socket.close(None).await;
    });
    let mut api = client(&config);
    api.timeout = std::time::Duration::from_secs(5);

    let input = vec![b'x'; 16 * 1024 * 1024];
    let output = api
        .exec("keeper", "run", &["true"], &input, 16)
        .await
        .expect("the status the server sent is kept");
    assert_eq!(output.exit_code(), Some(7));
}

/// A kubeconfig path that names a FIFO is refused at once: the loader inspects the path before it
/// opens it, so an open that waits for a writer never happens.
#[test]
fn a_kubeconfig_path_naming_a_fifo_is_refused_without_blocking() {
    let directory = std::env::temp_dir().join(format!("client-fifo-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    let fifo = directory.join("kubeconfig");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo runs");
    assert!(made.success(), "mkfifo made the FIFO");
    let (sender, receiver) = std::sync::mpsc::channel();
    let path = fifo.clone();
    std::thread::spawn(move || {
        let _ = sender.send(kubeconfig::read(&path));
    });
    let answer = receiver.recv_timeout(std::time::Duration::from_secs(2));
    if answer.is_err() {
        // Pairs with the blocked open so that its thread ends.
        let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
    }
    let _ = std::fs::remove_dir_all(&directory);
    let read = answer.expect("the read returns without a writer on the FIFO");
    assert!(read.is_err(), "a FIFO is no regular file: {read:?}");
}

/// The loader keeps at most [`kubeconfig::MAX_KUBECONFIG_BYTES`] whatever the file's inspected
/// length said: a file that grew between the inspection and the read is refused, not returned
/// one byte past the bound.
#[test]
fn a_kubeconfig_that_grows_past_its_bound_while_read_is_refused() {
    use std::io::Read as _;
    let bound = kubeconfig::MAX_KUBECONFIG_BYTES;
    let grown = kubeconfig::read_at_most(std::io::repeat(b' ').take(bound + 1));
    assert!(grown.is_err(), "{:?}", grown.map(|bytes| bytes.len()));
    let whole = kubeconfig::read_at_most(std::io::repeat(b' ').take(bound));
    assert_eq!(
        whole.map(|bytes| bytes.len() as u64),
        Ok(bound),
        "a file of exactly the bound is read"
    );
}

/// Every member design 28 refuses is refused by name, each on its own: the two cluster file
/// members, `insecure-skip-tls-verify`, and the eleven user members. The list is design 28's,
/// written out here, so a member dropped from the loader's own constants fails this case.
#[tokio::test]
async fn every_member_design_28_refuses_is_refused_by_name() {
    let fake = start();
    let config = fake.kubeconfig();
    let refusal = |config: &Value| {
        kubeconfig::parse(config.to_string().as_bytes(), |_, _| Ok(())).expect_err("refused")
    };

    let mut insecure = config.clone();
    insecure["clusters"][0]["cluster"]["insecure-skip-tls-verify"] = json!(true);
    assert_eq!(refusal(&insecure), "insecure-skip-tls-verify is refused");
    let cluster = [
        ("proxy-url", json!("http://127.0.0.1:3128")),
        ("certificate-authority", json!("/etc/kubernetes/ca.crt")),
    ];
    for (member, value) in &cluster {
        let mut refused = config.clone();
        refused["clusters"][0]["cluster"][member] = value.clone();
        assert_eq!(
            refusal(&refused),
            format!("the cluster member {member} is refused")
        );
    }
    let user = [
        ("exec", json!({"command": "credential-helper"})),
        ("auth-provider", json!({"name": "oidc"})),
        ("username", json!("operator")),
        ("password", json!("not-a-password")),
        ("tokenFile", json!("/var/run/token")),
        ("client-certificate", json!("/etc/kubernetes/client.crt")),
        ("client-key", json!("/etc/kubernetes/client.key")),
        ("as", json!("system:admin")),
        ("as-uid", json!("0")),
        ("as-groups", json!(["system:masters"])),
        ("as-user-extra", json!({"scopes": ["all"]})),
    ];
    for (member, value) in &user {
        let mut refused = config.clone();
        refused["users"][0]["user"][member] = value.clone();
        assert_eq!(
            refusal(&refused),
            format!("the user member {member} is refused")
        );
    }
    assert_eq!(
        kubeconfig::REFUSED_CLUSTER_MEMBERS.len() + kubeconfig::REFUSED_USER_MEMBERS.len(),
        cluster.len() + user.len(),
        "the loader refuses exactly design 28's members"
    );
}

/// The API refuses a body whose `metadata.namespace` names another namespace than the request's:
/// 400. The fake stored such an object in its own namespace instead, so a driver that wrote a
/// stray namespace would pass against the fake and be refused by the API.
#[tokio::test]
async fn a_create_whose_body_names_another_namespace_is_refused_400() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    let mut stray = pod("stray-namespace");
    stray["metadata"]["namespace"] = json!("elsewhere");

    let error = api
        .create(&api.pods(), &stray)
        .await
        .expect_err("another namespace");
    assert_eq!(error.status(), Some(400), "{error}");
    assert!(fake.object("pods", "stray-namespace").is_none());
}

/// The API refuses a replacement whose `metadata.name` is not the name in the URL: 400. The fake
/// stored the replacement under the URL's name and kept the stored name, so the body's name was
/// dropped without a word.
#[tokio::test]
async fn an_update_whose_body_names_another_object_is_refused_400() {
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
    let mut renamed = created.clone();
    renamed["metadata"]["name"] = json!("another-lease");

    let error = api.update(&path, &renamed).await.expect_err("another name");
    assert_eq!(error.status(), Some(400), "{error}");
    assert!(fake.object("leases", "another-lease").is_none());
}

/// A paged list is one snapshot: every page is read at the version the first page answered, and
/// the client watches from that version (`Api::list`, whose first page's version is what a watch
/// after the list starts from). So no item of the list is newer than the list's version: a newer
/// item would be delivered again by that watch. The fake answered each page at its own current
/// version, so an object created while the list paged is an item at a version after the list's.
#[tokio::test]
async fn an_item_created_while_a_list_pages_is_not_newer_than_the_list() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    for number in 0..5 {
        fake.seed("pods", pod(&format!("paged-{number}")))
            .expect("seeded");
    }
    fake.fault(Fault::Delay {
        method: Method::GET,
        path: format!("{}?limit=2&continue=", api.pods()),
        by: std::time::Duration::from_millis(400),
    });
    let writer = async {
        while !fake
            .requests()
            .iter()
            .any(|request| request.contains("continue="))
        {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        fake.seed("pods", pod("paged-z-late")).expect("seeded");
    };

    let pods = api.pods();
    let (listed, ()) = tokio::join!(api.list(&pods, "PodList", ListForm::Whole, 2, 5), writer);
    let listed = listed.expect("the list");
    let version: u64 = listed
        .resource_version
        .expect("the list's version")
        .parse()
        .expect("a number");
    for item in &listed.items {
        let item_version: u64 = item["metadata"]["resourceVersion"]
            .as_str()
            .expect("a version")
            .parse()
            .expect("a number");
        assert!(
            item_version <= version,
            "{} is at {item_version} in a list at {version}",
            item["metadata"]["name"]
        );
    }
}

/// A preconditioned delete whose body the API cannot read is refused; it is never carried out
/// without its precondition. The fake read a body past its bound as no body at all and deleted the
/// Pod under a `uid` that was not the Pod's.
#[tokio::test]
async fn a_delete_whose_body_is_past_its_bound_does_not_delete_under_another_uid() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    api.create(&api.pods(), &pod("oversized"))
        .await
        .expect("created");
    let path = format!("{}/oversized", api.pods());
    let options = json!({
        "apiVersion": "v1",
        "kind": "DeleteOptions",
        "preconditions": {"uid": "00000000-0000-4000-8000-ffffffffffff"},
        "padding": "x".repeat(4 * 1_048_576 + 1),
    });

    let (status, _) = api
        .call(
            Method::DELETE,
            &path,
            Some(&options),
            crate::MAX_API_BODY_BYTES,
        )
        .await
        .expect("answered");
    assert_ne!(status, 200, "the delete was carried out");
    assert!(
        fake.object("pods", "oversized").is_some(),
        "the Pod was deleted under another uid"
    );
}

/// A `Lease` created through the client, and the path that names it.
async fn lease(api: &Api) -> (Value, String) {
    let lease = json!({
        "apiVersion": "coordination.k8s.io/v1",
        "kind": "Lease",
        "metadata": {"name": "b10x-substrate-driver"},
        "spec": {"holderIdentity": "deployment-a/one", "leaseDurationSeconds": 30},
    });
    let created = api.create(&api.leases(), &lease).await.expect("created");
    (created, format!("{}/b10x-substrate-driver", api.leases()))
}

/// A replacement with `dryRun=All` is admitted and answered, and the API stores nothing.
#[tokio::test]
async fn a_dry_run_update_is_answered_and_stores_nothing() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    let (created, path) = lease(&api).await;
    let mut renewed = created.clone();
    renewed["spec"]["holderIdentity"] = json!("deployment-a/two");

    let answered = api
        .update(&format!("{path}?dryRun=All"), &renewed)
        .await
        .expect("admitted");
    assert_eq!(answered["spec"]["holderIdentity"], "deployment-a/two");
    let stored = fake
        .object("leases", "b10x-substrate-driver")
        .expect("stored");
    assert_eq!(stored, created, "a dry run changed the stored Lease");
}

/// `metadata.uid` is immutable: a replacement naming another uid is refused 422 `Invalid` and the
/// stored object keeps its own, as the API server's update validation does.
#[tokio::test]
async fn an_update_naming_another_uid_is_refused_422() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    let (created, path) = lease(&api).await;
    let mut foreign = created.clone();
    foreign["metadata"]["uid"] = json!("00000000-0000-4000-8000-ffffffffffff");
    foreign["spec"]["holderIdentity"] = json!("deployment-b/one");

    let error = api.update(&path, &foreign).await.expect_err("another uid");
    assert_eq!(error.status(), Some(422), "{error}");
    let stored = fake
        .object("leases", "b10x-substrate-driver")
        .expect("stored");
    assert_eq!(stored, created);
}

/// A continue token from before a compaction can no longer be answered at its version: the page
/// is 410 `Expired`, and the client's list answers that status, so its caller lists again.
#[tokio::test]
async fn a_list_continued_past_a_compaction_is_expired_410() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    for number in 0..5 {
        fake.seed("pods", pod(&format!("paged-{number}")))
            .expect("seeded");
    }
    fake.fault(Fault::Delay {
        method: Method::GET,
        path: format!("{}?limit=2&continue=", api.pods()),
        by: std::time::Duration::from_millis(300),
    });
    let compactor = async {
        while !fake
            .requests()
            .iter()
            .any(|request| request.contains("continue="))
        {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        fake.seed("pods", pod("paged-z-late")).expect("seeded");
        fake.compact();
    };

    let pods = api.pods();
    let (listed, ()) = tokio::join!(
        api.list(&pods, "PodList", ListForm::Metadata, 2, 5),
        compactor
    );
    let error = listed.expect_err("the continued page is expired");
    assert_eq!(error.status(), Some(410), "{error}");
}

/// A request without the bearer is refused 401 before anything else, and a fault planned for the
/// next request is still there for the next authorized one.
#[tokio::test]
async fn an_unauthorized_request_does_not_meet_a_planned_fault() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    let mut stranger = fake.kubeconfig();
    stranger["users"][0]["user"]["token"] = json!("another-credential");
    let stranger = client(&stranger);
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: api.pods(),
        status: 503,
        body: json!({"kind": "Status", "code": 503}),
    });

    let refused = stranger
        .list(&stranger.pods(), "PodList", ListForm::Metadata, 10, 1)
        .await
        .expect_err("no credential");
    assert_eq!(refused.status(), Some(401), "{refused}");
    let faulted = api
        .list(&api.pods(), "PodList", ListForm::Metadata, 10, 1)
        .await
        .expect_err("the planned fault");
    assert_eq!(faulted.status(), Some(503), "{faulted}");
}

/// The fake reads equality selectors alone (`key=value`, `key==value`). Any other requirement is
/// refused 400, never read as an equality on another key: `a!=b` matched nothing as `a!` = `b`.
#[tokio::test]
async fn a_label_selector_the_fake_cannot_read_is_refused_400() {
    let fake = start();
    let api = client(&fake.kubeconfig());
    fake.seed("pods", pod("selected")).expect("seeded");

    for selector in [
        "b10x-substrate/test!=client",
        "!b10x-substrate/test",
        "b10x-substrate/test",
        "b10x-substrate/test in (client)",
        "b10x-substrate/test=a=b",
    ] {
        let query: String = url::form_urlencoded::byte_serialize(selector.as_bytes()).collect();
        let error = api
            .list(
                &format!("{}?labelSelector={query}", api.pods()),
                "PodList",
                ListForm::Metadata,
                10,
                1,
            )
            .await
            .expect_err(selector);
        assert_eq!(error.status(), Some(400), "{selector}: {error}");
    }
    for selector in ["b10x-substrate/test=client", "b10x-substrate/test==client"] {
        let query: String = url::form_urlencoded::byte_serialize(selector.as_bytes()).collect();
        let listed = api
            .list(
                &format!("{}?labelSelector={query}", api.pods()),
                "PodList",
                ListForm::Metadata,
                10,
                1,
            )
            .await
            .expect(selector);
        assert_eq!(listed.items.len(), 1, "{selector}");
    }
}

/// A removal whose trace has read a Pod's `uid` deletes only that Pod. Design 28 § *A gone Pod*
/// says an object with another `uid` than the first one read is another Pod and is ignored. The
/// removal deletes by name alone (design 28 § *A run* says "deletes the Pod by name" for a start
/// that fails), so a
/// same-named replacement created after the read is deleted and the removal reports the Pod gone.
/// Whether the delete must carry the trace's uid as `preconditions.uid` is the design decision this
/// case asks for; until it is taken the case stays red.
#[tokio::test]
async fn a_removal_that_has_read_a_pods_uid_does_not_delete_a_same_named_replacement() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    api.pacing = crate::Pacing {
        poll: std::time::Duration::from_millis(1),
        remove_attempts: 3,
        notice: std::time::Duration::from_millis(1),
    };
    let path = format!("{}/swapped", api.pods());
    let first = api
        .create(&api.pods(), &pod("swapped"))
        .await
        .expect("created");
    let first_uid = first["metadata"]["uid"].as_str().expect("a uid").to_owned();
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    assert_eq!(
        api.delete_preconditioned(&path, 1, &first_uid).await,
        Ok(false)
    );
    let replacement = api
        .create(&api.pods(), &pod("swapped"))
        .await
        .expect("replacement created");
    let replacement_uid = replacement["metadata"]["uid"]
        .as_str()
        .expect("a uid")
        .to_owned();
    assert_ne!(first_uid, replacement_uid);

    let removed = api.remove_seen(&path, 1, &mut seen).await;

    let kept = fake.object("pods", "swapped");
    assert_eq!(
        kept.as_ref()
            .map(|object| object["metadata"]["uid"].clone()),
        Some(json!(replacement_uid)),
        "the removal deleted a same-named Pod whose uid it had not read (removed: {removed})"
    );
    assert!(
        removed,
        "a name holding another uid is the Pod read gone (story:pod-removal-never-deletes-a-replacement)"
    );
    assert_eq!(
        seen.uid(),
        Some(first_uid.as_str()),
        "the removal took the replacement as its own Pod"
    );
}

/// The unbounded removal holds the same rule: once its trace has a Pod's `uid`, a same-named
/// replacement is the Pod gone, and it ends without deleting the replacement.
#[tokio::test]
async fn an_unbounded_removal_that_has_read_a_pods_uid_ends_at_a_same_named_replacement() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    api.pacing = crate::Pacing {
        poll: std::time::Duration::from_millis(1),
        remove_attempts: 3,
        notice: std::time::Duration::from_millis(1),
    };
    let path = format!("{}/replaced", api.pods());
    let first = api
        .create(&api.pods(), &pod("replaced"))
        .await
        .expect("created");
    let first_uid = first["metadata"]["uid"].as_str().expect("a uid").to_owned();
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    assert_eq!(
        api.delete_preconditioned(&path, 1, &first_uid).await,
        Ok(false)
    );
    let replacement = api
        .create(&api.pods(), &pod("replaced"))
        .await
        .expect("replacement created");

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        api.remove_until_gone_seen(&path, 1, &mut seen),
    )
    .await
    .expect("the removal ends at a replacement");

    assert_eq!(
        fake.object("pods", "replaced")
            .map(|object| object["metadata"]["uid"].clone()),
        Some(replacement["metadata"]["uid"].clone()),
        "the unbounded removal deleted a same-named replacement"
    );
    assert_eq!(seen.uid(), Some(first_uid.as_str()));
}

/// A removal paced for a unit case: three tries a round, a millisecond apart.
fn quickly(api: &mut Api) {
    api.pacing = crate::Pacing {
        poll: std::time::Duration::from_millis(1),
        remove_attempts: 3,
        notice: std::time::Duration::from_millis(1),
    };
}

/// A 409 `Status` on a delete of the Pod `name` that is not a uid mismatch: an admission webhook
/// that denies a DELETE chooses its own code, and this one chose 409 with the API's `Conflict`
/// reason and `details` naming the Pod, as a uid mismatch's `Status` does.
fn webhook_conflict(name: &str) -> Value {
    json!({
        "kind": "Status",
        "apiVersion": "v1",
        "metadata": {},
        "status": "Failure",
        "message": "admission webhook \"hold.example.invalid\" denied the request: the Pod is held",
        "reason": "Conflict",
        "details": {"name": name, "kind": "pods"},
        "code": 409,
    })
}

/// The removal's requests to the Pod at `path`, as `METHOD`, in order.
fn calls_to(fake: &FakeCluster, path: &str) -> Vec<String> {
    fake.requests()
        .iter()
        .filter_map(|request| {
            let (method, target) = request.split_once(' ')?;
            (target == path).then(|| method.to_owned())
        })
        .collect()
}

/// A 409 on a uid-preconditioned delete is not by itself the Pod gone: the API answers 409 for a
/// failed precondition and for any conflict an admission webhook chooses to name one. The removal
/// reads the name after it, and a name still holding the read uid is the Pod present: it deletes
/// again, and only then is the Pod gone
/// (`story:preconditioned-delete-409-is-not-a-uid-mismatch`).
#[tokio::test]
async fn a_409_whose_follow_up_read_still_shows_the_read_uid_is_not_the_pod_gone() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    quickly(&mut api);
    let path = format!("{}/held", api.pods());
    let first = api
        .create(&api.pods(), &pod("held"))
        .await
        .expect("created");
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    fake.fault(Fault::Answer {
        method: Method::DELETE,
        path: path.clone(),
        status: 409,
        body: webhook_conflict("held"),
    });

    let removed = api.remove_seen(&path, 1, &mut seen).await;

    assert_eq!(
        fake.object("pods", "held"),
        None,
        "a 409 whose follow-up read showed the read uid ended the removal with the Pod present \
         (removed: {removed}; story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
    assert!(removed, "the second delete removed the Pod");
    assert_eq!(
        calls_to(&fake, &path),
        ["DELETE", "GET", "DELETE", "GET"],
        "the 409 is followed by a read of the name, and the read uid by another delete \
         (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
}

/// A conflict that persists while the name holds the read uid is never the Pod gone: the bounded
/// removal tries its round, reads the name after each 409, and reports the Pod not removed, with
/// the Pod untouched (`story:preconditioned-delete-409-is-not-a-uid-mismatch`).
#[tokio::test]
async fn a_409_that_persists_while_the_name_holds_the_read_uid_is_never_the_pod_gone() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    quickly(&mut api);
    let path = format!("{}/stuck", api.pods());
    let first = api
        .create(&api.pods(), &pod("stuck"))
        .await
        .expect("created");
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    for _ in 0..api.pacing.remove_attempts {
        fake.fault(Fault::Answer {
            method: Method::DELETE,
            path: path.clone(),
            status: 409,
            body: webhook_conflict("stuck"),
        });
    }

    let removed = api.remove_seen(&path, 1, &mut seen).await;

    assert!(
        !removed,
        "a 409 with the read uid still at the name was reported gone \
         (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
    assert_eq!(
        fake.object("pods", "stuck")
            .map(|object| object["metadata"]["uid"].clone()),
        Some(first["metadata"]["uid"].clone()),
        "the Pod is untouched"
    );
    assert_eq!(
        calls_to(&fake, &path),
        ["DELETE", "GET", "DELETE", "GET", "DELETE", "GET"],
        "one delete and one read per try, and no more tries than the round's bound \
         (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
}

/// The unbounded removal holds the same rule: a 409 while the name holds the read uid does not end
/// it, and it ends once the Pod is gone (`story:preconditioned-delete-409-is-not-a-uid-mismatch`).
#[tokio::test]
async fn an_unbounded_removal_does_not_end_at_a_409_while_the_name_holds_the_read_uid() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    quickly(&mut api);
    let path = format!("{}/held-long", api.pods());
    let first = api
        .create(&api.pods(), &pod("held-long"))
        .await
        .expect("created");
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    fake.fault(Fault::Answer {
        method: Method::DELETE,
        path: path.clone(),
        status: 409,
        body: webhook_conflict("held-long"),
    });

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        api.remove_until_gone_seen(&path, 1, &mut seen),
    )
    .await
    .expect("the removal ends once the Pod is gone");

    assert_eq!(
        fake.object("pods", "held-long"),
        None,
        "the unbounded removal ended at a 409 with the Pod present \
         (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
}

/// A 409 whose follow-up read finds the name empty is the Pod gone, and nothing is deleted after
/// it (`story:preconditioned-delete-409-is-not-a-uid-mismatch`).
#[tokio::test]
async fn a_409_whose_follow_up_read_finds_the_name_empty_is_the_pod_gone() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    quickly(&mut api);
    let path = format!("{}/vanished", api.pods());
    let first = api
        .create(&api.pods(), &pod("vanished"))
        .await
        .expect("created");
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    assert_eq!(api.delete(&path, 1).await, Ok(false), "the fake removes it");
    let before = calls_to(&fake, &path).len();
    fake.fault(Fault::Answer {
        method: Method::DELETE,
        path: path.clone(),
        status: 409,
        body: webhook_conflict("vanished"),
    });

    let removed = api.remove_seen(&path, 1, &mut seen).await;

    assert!(removed, "an empty name after a 409 is the Pod gone");
    assert_eq!(
        calls_to(&fake, &path)[before..],
        ["DELETE", "GET"],
        "the 409 is followed by one read and no other delete \
         (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
}

/// Adversary, u3: a 409 whose follow-up read the API answers 503 decides nothing, so a round of
/// them is the Pod not confirmed gone, one delete and one read per try, the Pod untouched
/// (`story:preconditioned-delete-409-is-not-a-uid-mismatch`).
#[tokio::test]
async fn adversary_a_409_then_a_read_answered_503_is_never_the_pod_gone() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    quickly(&mut api);
    let path = format!("{}/unread", api.pods());
    let first = api
        .create(&api.pods(), &pod("unread"))
        .await
        .expect("created");
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    for _ in 0..api.pacing.remove_attempts {
        fake.fault(Fault::Answer {
            method: Method::DELETE,
            path: path.clone(),
            status: 409,
            body: webhook_conflict("unread"),
        });
        fake.fault(Fault::Answer {
            method: Method::GET,
            path: path.clone(),
            status: 503,
            body: json!({"kind": "Status", "status": "Failure", "reason": "ServiceUnavailable", "code": 503}),
        });
    }

    let removed = api.remove_seen(&path, 1, &mut seen).await;

    assert!(
        !removed,
        "a 409 whose read answered 503 was reported gone \
         (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
    assert_eq!(
        fake.object("pods", "unread")
            .map(|object| object["metadata"]["uid"].clone()),
        Some(first["metadata"]["uid"].clone()),
        "the Pod is untouched"
    );
    assert_eq!(
        calls_to(&fake, &path),
        ["DELETE", "GET", "DELETE", "GET", "DELETE", "GET"],
        "one delete and one read per try, and no more tries than the round's bound \
         (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
}

/// Adversary, u3: a 409 whose follow-up read gets no answer in time decides nothing either
/// (`story:preconditioned-delete-409-is-not-a-uid-mismatch`).
#[tokio::test]
async fn adversary_a_409_then_a_read_that_times_out_is_never_the_pod_gone() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    quickly(&mut api);
    api.timeout = std::time::Duration::from_millis(200);
    let path = format!("{}/slow", api.pods());
    let first = api
        .create(&api.pods(), &pod("slow"))
        .await
        .expect("created");
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    for _ in 0..api.pacing.remove_attempts {
        fake.fault(Fault::Answer {
            method: Method::DELETE,
            path: path.clone(),
            status: 409,
            body: webhook_conflict("slow"),
        });
        fake.fault(Fault::Delay {
            method: Method::GET,
            path: path.clone(),
            by: std::time::Duration::from_secs(2),
        });
    }

    let removed = api.remove_seen(&path, 1, &mut seen).await;

    assert!(
        !removed,
        "a 409 whose read timed out was reported gone \
         (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
    assert!(
        fake.object("pods", "slow").is_some(),
        "the Pod is untouched"
    );
}

/// Adversary, u3: a 409 whose follow-up read shows the read uid with a `deletionTimestamp` (a Pod
/// terminating in its grace period) is the Pod present, not gone: the API still holds it and its
/// container may still run (`story:preconditioned-delete-409-is-not-a-uid-mismatch`).
#[tokio::test]
async fn adversary_a_409_then_a_read_of_the_read_uid_terminating_is_not_the_pod_gone() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    quickly(&mut api);
    let path = format!("{}/terminating", api.pods());
    let first = api
        .create(&api.pods(), &pod("terminating"))
        .await
        .expect("created");
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    let mut terminating = fake.object("pods", "terminating").expect("stored");
    terminating["metadata"]["deletionTimestamp"] = json!("2026-10-30T00:00:00Z");
    terminating["metadata"]["deletionGracePeriodSeconds"] = json!(30);
    for _ in 0..api.pacing.remove_attempts {
        fake.fault(Fault::Answer {
            method: Method::DELETE,
            path: path.clone(),
            status: 409,
            body: webhook_conflict("terminating"),
        });
        fake.fault(Fault::Answer {
            method: Method::GET,
            path: path.clone(),
            status: 200,
            body: terminating.clone(),
        });
    }

    let removed = api.remove_seen(&path, 1, &mut seen).await;

    assert!(
        !removed,
        "a 409 whose read showed the read uid terminating was reported gone \
         (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
    assert_eq!(
        seen.uid(),
        first["metadata"]["uid"].as_str(),
        "the trace still holds the first Pod"
    );
    assert_eq!(
        calls_to(&fake, &path),
        ["DELETE", "GET", "DELETE", "GET", "DELETE", "GET"],
        "one delete and one read per try (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
}

/// Adversary, u3: a uid-mismatch 409 (the fake's own precondition check) whose follow-up read shows
/// another uid ends the removal at once: one delete, one read, and no delete after it, so the
/// replacement is never asked to go (`story:preconditioned-delete-409-is-not-a-uid-mismatch`).
#[tokio::test]
async fn adversary_a_409_then_a_read_of_another_uid_is_gone_after_one_delete_and_one_read() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    quickly(&mut api);
    let path = format!("{}/swapped-once", api.pods());
    let first = api
        .create(&api.pods(), &pod("swapped-once"))
        .await
        .expect("created");
    let first_uid = first["metadata"]["uid"].as_str().expect("a uid").to_owned();
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    assert_eq!(
        api.delete_preconditioned(&path, 1, &first_uid).await,
        Ok(false)
    );
    let replacement = api
        .create(&api.pods(), &pod("swapped-once"))
        .await
        .expect("replacement created");
    let before = calls_to(&fake, &path).len();

    let removed = api.remove_seen(&path, 1, &mut seen).await;

    assert!(removed, "a name holding another uid is the Pod read gone");
    assert_eq!(
        calls_to(&fake, &path)[before..],
        ["DELETE", "GET"],
        "the uid-mismatch 409 is followed by one read and nothing else \
         (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
    assert_eq!(
        fake.object("pods", "swapped-once")
            .map(|object| object["metadata"]["uid"].clone()),
        Some(replacement["metadata"]["uid"].clone()),
        "the replacement is untouched"
    );
}

/// Adversary, u3: the unbounded removal against a 409 that persists while the name holds the read
/// uid does not end, and deletes nothing: it is never the Pod gone, however long it runs
/// (`story:preconditioned-delete-409-is-not-a-uid-mismatch`).
#[tokio::test]
async fn adversary_an_unbounded_removal_against_a_persistent_409_never_ends() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    api.pacing = crate::Pacing {
        poll: std::time::Duration::from_millis(10),
        remove_attempts: 3,
        notice: std::time::Duration::from_millis(5),
    };
    let path = format!("{}/held-forever", api.pods());
    let first = api
        .create(&api.pods(), &pod("held-forever"))
        .await
        .expect("created");
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    for _ in 0..200 {
        fake.fault(Fault::Answer {
            method: Method::DELETE,
            path: path.clone(),
            status: 409,
            body: webhook_conflict("held-forever"),
        });
    }

    let ended = tokio::time::timeout(
        std::time::Duration::from_millis(400),
        api.remove_until_gone_seen(&path, 1, &mut seen),
    )
    .await;

    assert!(
        ended.is_err(),
        "the unbounded removal ended while every delete was refused 409 and the name held the \
         read uid (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
    assert!(
        fake.object("pods", "held-forever").is_some(),
        "the Pod is untouched"
    );
    let deletes = calls_to(&fake, &path)
        .iter()
        .filter(|method| *method == "DELETE")
        .count();
    assert!(
        (2..200).contains(&deletes),
        "the removal kept deleting within its faults ({deletes} deletes)"
    );
}

/// Security review of `story:preconditioned-delete-409-is-not-a-uid-mismatch`: a 409 whose
/// follow-up read is not answered (an error status, or an answer lost) decides nothing, so it is
/// never the Pod gone, within the round's bound, and the Pod stays untouched (invariant 3: a
/// missing answer is not an optimistic one).
#[tokio::test]
async fn a_409_whose_follow_up_read_is_not_answered_is_never_the_pod_gone() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    quickly(&mut api);
    let path = format!("{}/unread", api.pods());
    let first = api
        .create(&api.pods(), &pod("unread"))
        .await
        .expect("created");
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    for _ in 0..api.pacing.remove_attempts {
        fake.fault(Fault::Answer {
            method: Method::DELETE,
            path: path.clone(),
            status: 409,
            body: webhook_conflict("unread"),
        });
    }
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: path.clone(),
        status: 500,
        body: json!({"kind": "Status", "status": "Failure", "code": 500}),
    });
    fake.fault(Fault::DropAnswer {
        method: Method::GET,
        path: path.clone(),
    });
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: path.clone(),
        status: 503,
        body: json!({"kind": "Status", "status": "Failure", "code": 503}),
    });

    let removed = api.remove_seen(&path, 1, &mut seen).await;

    assert!(
        !removed,
        "a 409 whose follow-up read was not answered was reported gone \
         (story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
    assert_eq!(
        fake.object("pods", "unread")
            .map(|object| object["metadata"]["uid"].clone()),
        Some(first["metadata"]["uid"].clone()),
        "the Pod is untouched"
    );
    assert_eq!(
        calls_to(&fake, &path),
        ["DELETE", "GET", "DELETE", "GET", "DELETE", "GET"],
        "one delete and one read per try, within the round's bound"
    );
}

/// Security review of `story:preconditioned-delete-409-is-not-a-uid-mismatch`: the delete the
/// change now sends after a 409 still carries the read uid as `preconditions.uid`. A replacement
/// stored under the name between the follow-up read (which still showed the read uid) and that
/// delete is refused by the precondition, never deleted, and the removal ends at it as the Pod
/// gone with the trace's uid unchanged.
#[tokio::test]
async fn the_delete_after_a_409_carries_the_read_uid_and_spares_a_late_replacement() {
    let fake = start();
    let mut api = client(&fake.kubeconfig());
    quickly(&mut api);
    let path = format!("{}/late", api.pods());
    let first = api
        .create(&api.pods(), &pod("late"))
        .await
        .expect("created");
    let first_uid = first["metadata"]["uid"].as_str().expect("a uid").to_owned();
    let mut seen = PodTrace::default();
    assert!(seen.note(&first), "the first object is noted");
    assert_eq!(
        api.delete_preconditioned(&path, 1, &first_uid).await,
        Ok(false),
        "the first Pod goes"
    );
    let replacement = api
        .create(&api.pods(), &pod("late"))
        .await
        .expect("replacement created");
    assert_ne!(replacement["metadata"]["uid"], first["metadata"]["uid"]);
    let before = calls_to(&fake, &path).len();
    fake.fault(Fault::Answer {
        method: Method::DELETE,
        path: path.clone(),
        status: 409,
        body: webhook_conflict("late"),
    });
    fake.fault(Fault::Answer {
        method: Method::GET,
        path: path.clone(),
        status: 200,
        body: first.clone(),
    });

    let removed = api.remove_seen(&path, 1, &mut seen).await;

    assert_eq!(
        fake.object("pods", "late")
            .map(|object| object["metadata"]["uid"].clone()),
        Some(replacement["metadata"]["uid"].clone()),
        "the delete after a 409 removed a same-named replacement: it did not carry the read uid \
         (removed: {removed}; story:preconditioned-delete-409-is-not-a-uid-mismatch)"
    );
    assert!(removed, "a name holding another uid is the Pod read gone");
    assert_eq!(
        calls_to(&fake, &path)[before..],
        ["DELETE", "GET", "DELETE", "GET"],
        "the read uid after the 409 is deleted again, refused, and read once more"
    );
    assert_eq!(
        seen.uid(),
        Some(first_uid.as_str()),
        "the removal took the replacement as its own Pod"
    );
}

/// Adversary pass of wave D339, unit 4 (`story:pod-orphaned-policy-is-swept`).
mod adversary_d339 {
    use super::*;
    use crate::PodHistory;

    /// A `BOOKMARK` says every event up to its own `resourceVersion` was sent. One that carries the
    /// version the replay started from, or none, says nothing of what followed that version, so it
    /// cannot show the replay from it complete: the Pod is not shown never created. (An older
    /// version is not told apart: versions are opaque.)
    #[tokio::test]
    async fn a_bookmark_at_the_start_version_or_none_does_not_show_the_pod_never_created() {
        for (carried, metadata) in [
            ("the start version", json!({"resourceVersion": "9"})),
            ("no version", json!({})),
        ] {
            let fake = start();
            let api = client(&fake.kubeconfig());
            fake.fault(Fault::Answer {
                method: Method::GET,
                path: format!("{}?watch=1", api.pods()),
                status: 200,
                body: json!({"type": "BOOKMARK",
                             "object": {"kind": "Pod", "apiVersion": "v1", "metadata": metadata}}),
            });
            let history = api.pod_history("gone", &PodTrace::since(Some("9"))).await;
            assert_ne!(
                history,
                PodHistory::NeverCreated,
                "a BOOKMARK carrying {carried} showed a replay from version 9 complete"
            );
        }
    }
}

/// Adversary cases for D342 U1: the body of a refused exec upgrade, as `refusal_body` and
/// `dechunk` read it.
mod adversary_d342 {
    use super::{client, raw_http_server};

    /// A chunked 403 whose chunk-size line is the largest `usize` (16 hex digits, which
    /// `from_str_radix` accepts) is still the API's 403. Reading its body must not panic on
    /// `start + size` in `dechunk`: the refusal stands, as a body that does not parse does.
    #[tokio::test]
    async fn a_refusal_whose_chunk_size_overflows_is_still_the_apis_403() {
        let (config, _closed) = raw_http_server(Some(
            b"HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\n\
              Transfer-Encoding: chunked\r\n\r\n\
              ffffffffffffffff\r\n{\"kind\":\"Status\",\"code\":403}\r\n0\r\n\r\n",
        ))
        .await;
        let mut api = client(&config);
        api.timeout = std::time::Duration::from_secs(5);
        let outcome = tokio::spawn(async move {
            api.exec(
                "snl-01jabcdefghjkmnpqrstvwxyz0-keeper",
                "keeper",
                &["/substrate-executor", "version"],
                b"",
                16,
            )
            .await
        })
        .await;
        let error = match outcome {
            Ok(Err(error)) => error,
            Ok(Ok(output)) => panic!("the refused upgrade was admitted: {output:?}"),
            Err(join) => panic!("the client panicked reading the refusal's body: {join}"),
        };
        assert_eq!(error.status(), Some(403), "{error}");
    }
}
