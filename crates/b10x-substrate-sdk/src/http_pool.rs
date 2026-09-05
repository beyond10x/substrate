//! Bounded HTTP/1.1 reuse. Connections own transport state, never authentication state.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderValue, Request, header};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::client::conn::http1::{Builder, SendRequest};
use hyper_util::rt::TokioIo;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use ulid::Ulid;

use crate::SdkError;
use crate::transport::{BoxedIo, HttpResponse, verify_contract_headers};

const CONNECTION_LIMIT: usize = 16;
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_HEAD: usize = 16 * 1024;
const MAX_RESPONSE_BODY: usize = 4 * 1024 * 1024;

type Body = Full<Bytes>;

struct Connection {
    sender: SendRequest<Body>,
    driver: JoinHandle<()>,
    idle_timer: Option<JoinHandle<()>>,
}

impl Connection {
    fn mark_idle(&mut self) {
        let driver = self.driver.abort_handle();
        self.idle_timer = Some(tokio::spawn(async move {
            tokio::time::sleep(IDLE_TIMEOUT).await;
            driver.abort();
        }));
    }

    fn take(&mut self) {
        if let Some(timer) = self.idle_timer.take() {
            timer.abort();
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.take();
        self.driver.abort();
    }
}

pub(crate) struct HttpPool {
    idle: Mutex<Vec<Connection>>,
    permits: Semaphore,
}

impl HttpPool {
    pub(crate) fn new() -> Self {
        Self {
            idle: Mutex::new(Vec::new()),
            permits: Semaphore::new(CONNECTION_LIMIT),
        }
    }

    pub(crate) async fn request<F, C>(
        &self,
        connect: F,
        request: Request<Body>,
    ) -> Result<HttpResponse, SdkError>
    where
        F: FnOnce() -> C,
        C: Future<Output = Result<BoxedIo, SdkError>>,
    {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| transport_failed())?;
        let mut connection = loop {
            let candidate = self
                .idle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop();
            let Some(mut candidate) = candidate else {
                break None;
            };
            candidate.take();
            if candidate.sender.ready().await.is_ok() {
                break Some(candidate);
            }
        };
        if connection.is_none() {
            let stream = connect().await?;
            let (sender, driver) = Builder::new()
                .max_buf_size(MAX_RESPONSE_HEAD)
                .handshake(TokioIo::new(stream))
                .await
                .map_err(|_| transport_failed())?;
            connection = Some(Connection {
                sender,
                driver: tokio::spawn(async move {
                    let _ = driver.await;
                }),
                idle_timer: None,
            });
        }
        let mut connection = connection.expect("an admitted connection exists");
        // Never replay a sent request here. Mutation recovery belongs to the operation ledger.
        let response = connection
            .sender
            .send_request(request)
            .await
            .map_err(|_| transport_failed())?;
        let status = response.status().as_u16();
        let mut headers = BTreeMap::new();
        for (name, value) in response.headers() {
            let value = value
                .to_str()
                .map_err(|_| SdkError::Protocol("response header is not ASCII".to_owned()))?;
            if headers.insert(name.to_string(), value.to_owned()).is_some()
                && name.as_str().starts_with("x-b10x-contract")
            {
                return Err(SdkError::Protocol("duplicate contract header".to_owned()));
            }
        }
        verify_contract_headers(&headers)?;
        let body = Limited::new(response.into_body(), MAX_RESPONSE_BODY)
            .collect()
            .await
            .map_err(|_| {
                SdkError::Transport("response body failed or exceeded its bound".to_owned())
            })?
            .to_bytes()
            .to_vec();
        if !connection.sender.is_closed() {
            connection.mark_idle();
            self.idle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(connection);
        }
        Ok(HttpResponse { status, body })
    }
}

fn transport_failed() -> SdkError {
    SdkError::Transport("remote HTTP exchange failed".to_owned())
}

pub(crate) fn request(
    authority: &str,
    token: &str,
    source_authority: Option<&str>,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
) -> Result<Request<Body>, SdkError> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, authority)
        .header("x-request-id", format!("sdk_{}", Ulid::generate()));
    let mut authorization = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| SdkError::TokenUnavailable)?;
    authorization.set_sensitive(true);
    request = request.header(header::AUTHORIZATION, authorization);
    if let Some(source) = source_authority {
        if source.is_empty()
            || source.len() > 512
            || !source.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        {
            return Err(SdkError::Protocol(
                "workspace source authority is invalid".to_owned(),
            ));
        }
        let mut source = HeaderValue::from_str(source)
            .map_err(|_| SdkError::Protocol("workspace source authority is invalid".to_owned()))?;
        source.set_sensitive(true);
        request = request.header("x-b10x-workspace-source-authorization", source);
    }
    if body.is_some() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    request
        .body(Full::new(Bytes::copy_from_slice(body.unwrap_or_default())))
        .map_err(|_| SdkError::Protocol("invalid HTTP request".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CONTRACT, CONTRACT_SHA256};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn reuses_one_connection_with_authority_only_on_each_request() {
        let pool = HttpPool::new();
        let connections = Arc::new(AtomicUsize::new(0));
        let observed = Arc::new(Mutex::new(Vec::new()));
        for (token, source) in [("actor-one", Some("source-one")), ("actor-two", None)] {
            let connections = connections.clone();
            let observed = observed.clone();
            let response = pool
                .request(
                    move || async move {
                        connections.fetch_add(1, Ordering::SeqCst);
                        let (client, server) = tokio::io::duplex(65_536);
                        tokio::spawn(async move {
                            let service = hyper::service::service_fn(
                                move |incoming: Request<hyper::body::Incoming>| {
                                    let observed = observed.clone();
                                    async move {
                                        observed.lock().unwrap().push((
                                            incoming.headers()[header::AUTHORIZATION]
                                                .to_str()
                                                .unwrap()
                                                .to_owned(),
                                            incoming
                                                .headers()
                                                .get("x-b10x-workspace-source-authorization")
                                                .cloned(),
                                        ));
                                        incoming.into_body().collect().await.unwrap();
                                        Ok::<_, std::convert::Infallible>(
                                            http::Response::builder()
                                                .header("x-b10x-contract", CONTRACT)
                                                .header(
                                                    "x-b10x-contract-bundle-sha256",
                                                    CONTRACT_SHA256,
                                                )
                                                .body(Full::new(Bytes::from_static(b"{}")))
                                                .unwrap(),
                                        )
                                    }
                                },
                            );
                            let _ = hyper::server::conn::http1::Builder::new()
                                .serve_connection(TokioIo::new(server), service)
                                .await;
                        });
                        Ok(Box::new(client) as BoxedIo)
                    },
                    request(
                        "substrate.test",
                        token,
                        source,
                        "POST",
                        "/v1/test",
                        Some(b"{}"),
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status, 200);
        }
        assert_eq!(connections.load(Ordering::SeqCst), 1);
        let observed = observed.lock().unwrap();
        assert_eq!(observed[0].0, "Bearer actor-one");
        assert_eq!(observed[0].1.as_ref().unwrap(), "source-one");
        assert_eq!(observed[1], ("Bearer actor-two".to_owned(), None));
    }

    #[tokio::test]
    async fn rejects_contract_before_waiting_for_body_and_discards_connection() {
        let pool = HttpPool::new();
        let error = pool.request(
            || async {
                let (client, mut server) = tokio::io::duplex(65_536);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
                    let mut input = [0_u8; 4096];
                    assert!(server.read(&mut input).await.unwrap() > 0);
                    server
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 1024\r\n\r\n")
                        .await
                        .unwrap();
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
                Ok(Box::new(client) as BoxedIo)
            },
            request("substrate.test", "actor", None, "GET", "/v1/test", None).unwrap(),
        );
        let result = tokio::time::timeout(Duration::from_secs(1), error)
            .await
            .unwrap();
        assert!(matches!(result, Err(SdkError::ContractMismatch { .. })));
        assert!(pool.idle.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn broken_exchange_does_not_replay_a_mutation() {
        let pool = HttpPool::new();
        let connections = AtomicUsize::new(0);
        let result = pool
            .request(
                || async {
                    connections.fetch_add(1, Ordering::SeqCst);
                    let (client, mut server) = tokio::io::duplex(65_536);
                    tokio::spawn(async move {
                        use tokio::io::AsyncReadExt as _;
                        let mut input = [0_u8; 4096];
                        assert!(server.read(&mut input).await.unwrap() > 0);
                        // The server accepted the request then lost its answer.
                    });
                    Ok(Box::new(client) as BoxedIo)
                },
                request(
                    "substrate.test",
                    "actor",
                    None,
                    "POST",
                    "/v1/test",
                    Some(b"{}"),
                )
                .unwrap(),
            )
            .await;
        assert!(matches!(result, Err(SdkError::Transport(_))));
        assert_eq!(connections.load(Ordering::SeqCst), 1);
        assert!(pool.idle.lock().unwrap().is_empty());
    }

    async fn read_request_head(stream: &mut tokio::io::DuplexStream) -> bool {
        use tokio::io::AsyncReadExt as _;
        let mut head = Vec::new();
        loop {
            let mut byte = [0_u8; 1];
            if stream.read(&mut byte).await.unwrap() == 0 {
                return false;
            }
            head.push(byte[0]);
            assert!(head.len() <= MAX_RESPONSE_HEAD);
            if head.ends_with(b"\r\n\r\n") {
                return true;
            }
        }
    }

    fn persistent_reply_connection(closed: tokio::sync::oneshot::Sender<()>) -> BoxedIo {
        use tokio::io::AsyncWriteExt as _;
        let (client, mut server) = tokio::io::duplex(65_536);
        tokio::spawn(async move {
            while read_request_head(&mut server).await {
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: 2\r\nx-b10x-contract: {CONTRACT}\r\nx-b10x-contract-bundle-sha256: {CONTRACT_SHA256}\r\n\r\n{{}}"
                );
                server.write_all(response.as_bytes()).await.unwrap();
            }
            let _ = closed.send(());
        });
        Box::new(client)
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_discards_partial_exchange_and_releases_capacity() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let pool = Arc::new(HttpPool::new());
        let (partial_written, partial_received) = tokio::sync::oneshot::channel();
        let (closed, closed_received) = tokio::sync::oneshot::channel();
        let pending_pool = pool.clone();
        let pending = tokio::spawn(async move {
            pending_pool
                .request(
                    || async move {
                        let (client, mut server) = tokio::io::duplex(65_536);
                        tokio::spawn(async move {
                            assert!(read_request_head(&mut server).await);
                            let response = format!(
                                "HTTP/1.1 200 OK\r\ncontent-length: 1024\r\nx-b10x-contract: {CONTRACT}\r\nx-b10x-contract-bundle-sha256: {CONTRACT_SHA256}\r\n\r\n{{"
                            );
                            server.write_all(response.as_bytes()).await.unwrap();
                            partial_written.send(()).unwrap();
                            let mut byte = [0_u8; 1];
                            assert_eq!(server.read(&mut byte).await.unwrap(), 0);
                            closed.send(()).unwrap();
                        });
                        Ok(Box::new(client) as BoxedIo)
                    },
                    request("substrate.test", "actor", None, "GET", "/v1/test", None)
                        .unwrap(),
                )
                .await
        });
        partial_received.await.unwrap();
        assert_eq!(pool.permits.available_permits(), CONNECTION_LIMIT - 1);
        pending.abort();
        assert!(matches!(pending.await, Err(error) if error.is_cancelled()));
        tokio::time::timeout(Duration::from_secs(1), closed_received)
            .await
            .expect("cancellation must close the incomplete connection")
            .unwrap();
        assert_eq!(pool.permits.available_permits(), CONNECTION_LIMIT);
        assert!(pool.idle.lock().unwrap().is_empty());

        let (closed, closed_received) = tokio::sync::oneshot::channel();
        let response = pool
            .request(
                || async move { Ok(persistent_reply_connection(closed)) },
                request(
                    "substrate.test",
                    "next-actor",
                    None,
                    "GET",
                    "/v1/test",
                    None,
                )
                .unwrap(),
            )
            .await
            .expect("the released slot accepts a clean exchange");
        assert_eq!(response.body, b"{}");
        drop(pool);
        closed_received.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn seventeen_pending_requests_admit_only_sixteen_connections() {
        use tokio::io::AsyncReadExt as _;
        let pool = Arc::new(HttpPool::new());
        let (started, mut started_received) = tokio::sync::mpsc::unbounded_channel();
        let (connected, mut connected_received) = tokio::sync::mpsc::unbounded_channel();
        let (closed, mut closed_received) = tokio::sync::mpsc::unbounded_channel();
        let mut requests = BTreeMap::new();
        for index in 0..=CONNECTION_LIMIT {
            let pool = pool.clone();
            let started = started.clone();
            let connected = connected.clone();
            let closed = closed.clone();
            requests.insert(
                index,
                tokio::spawn(async move {
                    started.send(index).unwrap();
                    pool.request(
                        || async move {
                            let (client, mut server) = tokio::io::duplex(65_536);
                            connected.send(index).unwrap();
                            tokio::spawn(async move {
                                if read_request_head(&mut server).await {
                                    let mut byte = [0_u8; 1];
                                    assert_eq!(server.read(&mut byte).await.unwrap(), 0);
                                }
                                closed.send(index).unwrap();
                            });
                            Ok(Box::new(client) as BoxedIo)
                        },
                        request("substrate.test", "actor", None, "GET", "/v1/test", None).unwrap(),
                    )
                    .await
                }),
            );
        }
        for _ in 0..=CONNECTION_LIMIT {
            started_received.recv().await.unwrap();
        }
        let mut admitted = Vec::new();
        for _ in 0..CONNECTION_LIMIT {
            admitted.push(connected_received.recv().await.unwrap());
        }
        assert!(connected_received.try_recv().is_err());
        assert_eq!(pool.permits.available_permits(), 0);

        let first = requests.remove(&admitted[0]).unwrap();
        first.abort();
        assert!(matches!(first.await, Err(error) if error.is_cancelled()));
        assert_eq!(closed_received.recv().await, Some(admitted[0]));
        let replacement = connected_received.recv().await.unwrap();
        assert!(!admitted.contains(&replacement));
        assert!(connected_received.try_recv().is_err());
        assert_eq!(pool.permits.available_permits(), 0);
        for (_, pending) in requests {
            pending.abort();
            assert!(matches!(pending.await, Err(error) if error.is_cancelled()));
        }
        for _ in 0..CONNECTION_LIMIT {
            closed_received.recv().await.unwrap();
        }
        assert_eq!(pool.permits.available_permits(), CONNECTION_LIMIT);
        assert!(pool.idle.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn idle_connections_expire_after_thirty_seconds() {
        let pool = HttpPool::new();
        let connections = AtomicUsize::new(0);
        let (closed, closed_received) = tokio::sync::oneshot::channel();
        pool.request(
            || async {
                connections.fetch_add(1, Ordering::SeqCst);
                Ok(persistent_reply_connection(closed))
            },
            request("substrate.test", "actor", None, "GET", "/v1/test", None).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(connections.load(Ordering::SeqCst), 1);
        // Let the spawned idle timer register before advancing the paused clock.
        tokio::time::sleep(Duration::from_millis(1)).await;
        tokio::time::advance(IDLE_TIMEOUT.checked_sub(Duration::from_millis(2)).unwrap()).await;
        assert!(!pool.idle.lock().unwrap()[0].driver.is_finished());
        tokio::time::advance(Duration::from_millis(2)).await;
        tokio::time::timeout(Duration::from_secs(1), closed_received)
            .await
            .expect("idle expiry must close the TCP/TLS driver")
            .unwrap();

        let (closed, closed_received) = tokio::sync::oneshot::channel();
        pool.request(
            || async {
                connections.fetch_add(1, Ordering::SeqCst);
                Ok(persistent_reply_connection(closed))
            },
            request("substrate.test", "actor", None, "GET", "/v1/test", None).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(connections.load(Ordering::SeqCst), 2);
        drop(pool);
        closed_received.await.unwrap();
    }
}
