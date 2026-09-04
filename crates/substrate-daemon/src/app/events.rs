use std::collections::HashMap;
use std::sync::Arc;

use axum::Extension;
use axum::body::Body;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse as _, Response};
use futures_util::StreamExt as _;
use parking_lot::Mutex as ParkingMutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use substrate_store::{
    CommitEffect, CommitEffectSink, EventCursorError, Scope, SnapshotReadError, StoreError,
};
use substrate_wire::{
    EmptyInput, ErrorClass, EventPage, EventQuery, MAX_EVENT_PAGE_ITEMS, MAX_SNAPSHOT_PAGE_ITEMS,
    Success,
};
use tokio::sync::{Semaphore, watch};

use crate::runtime::TransportPermit;

use super::operations::read_bounded_body;
use super::responses::{
    failure, not_found, query_is_empty, request_id, schema_invalid, store_failure, success,
};
use super::{App, BODY_LIMIT, Identity};

#[derive(Clone, Copy)]
pub(super) struct EventStreamPolicy {
    pub(super) global_streams: usize,
    pub(super) streams_per_subject: usize,
    pub(super) max_input_bytes: usize,
    pub(super) max_output_bytes: usize,
    pub(super) write_buffer_bytes: usize,
    pub(super) max_controls_per_window: u32,
    pub(super) control_window: std::time::Duration,
    pub(super) send_timeout: std::time::Duration,
    pub(super) lifetime: std::time::Duration,
    max_catch_up_pages: usize,
    max_page_items: u32,
}

impl EventStreamPolicy {
    pub(super) const fn production() -> Self {
        Self {
            global_streams: 64,
            streams_per_subject: 4,
            max_input_bytes: 1_024,
            max_output_bytes: BODY_LIMIT,
            write_buffer_bytes: 16 * 1_024,
            max_controls_per_window: 120,
            control_window: std::time::Duration::from_mins(1),
            send_timeout: std::time::Duration::from_secs(5),
            lifetime: std::time::Duration::from_hours(1),
            max_catch_up_pages: 16,
            max_page_items: 64,
        }
    }
}

pub(super) struct EventStreamLimits {
    pub(super) global: Arc<Semaphore>,
    streams_per_subject: usize,
    pub(super) scopes: ParkingMutex<HashMap<Scope, ScopeStreamLimit>>,
}

pub(super) struct ScopeStreamLimit {
    permits: Arc<Semaphore>,
    holders: usize,
}

pub(super) struct EventStreamPermit {
    limits: Arc<EventStreamLimits>,
    scope: Scope,
    _global: tokio::sync::OwnedSemaphorePermit,
    _subject: tokio::sync::OwnedSemaphorePermit,
}

impl EventStreamLimits {
    pub(super) fn new(global_streams: usize, streams_per_subject: usize) -> Arc<Self> {
        assert!(
            global_streams > 0,
            "global event stream limit must be nonzero"
        );
        assert!(
            streams_per_subject > 0,
            "subject event stream limit must be nonzero"
        );
        Arc::new(Self {
            global: Arc::new(Semaphore::new(global_streams)),
            streams_per_subject,
            scopes: ParkingMutex::new(HashMap::new()),
        })
    }

    pub(super) fn acquire(self: &Arc<Self>, scope: &Scope) -> Option<EventStreamPermit> {
        let global = Arc::clone(&self.global).try_acquire_owned().ok()?;
        let mut scopes = self.scopes.lock();
        let entry = scopes
            .entry(scope.clone())
            .or_insert_with(|| ScopeStreamLimit {
                permits: Arc::new(Semaphore::new(self.streams_per_subject)),
                holders: 0,
            });
        let subject = Arc::clone(&entry.permits).try_acquire_owned().ok()?;
        entry.holders += 1;
        Some(EventStreamPermit {
            limits: Arc::clone(self),
            scope: scope.clone(),
            _global: global,
            _subject: subject,
        })
    }
}

impl Drop for EventStreamPermit {
    fn drop(&mut self) {
        let mut scopes = self.limits.scopes.lock();
        let Some(entry) = scopes.get_mut(&self.scope) else {
            return;
        };
        entry.holders = entry.holders.saturating_sub(1);
        if entry.holders == 0 {
            scopes.remove(&self.scope);
        }
    }
}

#[derive(Default)]
pub(super) struct EventWakeups {
    pub(super) scopes: ParkingMutex<HashMap<Scope, ScopeWakeup>>,
}

pub(super) struct ScopeWakeup {
    sender: watch::Sender<Option<WakePosition>>,
    subscribers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct WakePosition {
    pub(super) generation: u64,
    pub(super) through_seq: u64,
    pub(super) source_scope: String,
}

pub(super) struct EventSubscription {
    wakeups: Arc<EventWakeups>,
    scope: Scope,
    pub(super) receiver: watch::Receiver<Option<WakePosition>>,
}

impl EventWakeups {
    pub(super) fn subscribe(self: &Arc<Self>, scope: &Scope) -> EventSubscription {
        let mut scopes = self.scopes.lock();
        let wakeup = scopes.entry(scope.clone()).or_insert_with(|| {
            let (sender, receiver) = watch::channel(None);
            drop(receiver);
            ScopeWakeup {
                sender,
                subscribers: 0,
            }
        });
        wakeup.subscribers += 1;
        let receiver = wakeup.sender.subscribe();
        EventSubscription {
            wakeups: Arc::clone(self),
            scope: scope.clone(),
            receiver,
        }
    }
}

impl CommitEffectSink for EventWakeups {
    fn committed(&self, effects: &[CommitEffect]) {
        let mut scopes = self.scopes.lock();
        let mut latest = HashMap::<Scope, WakePosition>::new();
        for effect in effects {
            let candidate = WakePosition {
                generation: effect.generation,
                through_seq: effect.through_seq,
                source_scope: effect.source_scope.clone(),
            };
            let entry = latest
                .entry(effect.scope.clone())
                .or_insert(candidate.clone());
            if candidate > *entry {
                *entry = candidate;
            }
        }
        for (scope, candidate) in latest {
            if let Some(wakeup) = scopes.get_mut(&scope) {
                let should_replace = wakeup
                    .sender
                    .borrow()
                    .as_ref()
                    .is_none_or(|current| candidate > *current);
                if should_replace {
                    wakeup.sender.send_replace(Some(candidate));
                }
            }
        }
    }
}

impl EventSubscription {
    pub(super) async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        self.receiver.changed().await
    }
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        let mut scopes = self.wakeups.scopes.lock();
        let Some(wakeup) = scopes.get_mut(&self.scope) else {
            return;
        };
        wakeup.subscribers = wakeup.subscribers.saturating_sub(1);
        if wakeup.subscribers == 0 {
            scopes.remove(&self.scope);
        }
    }
}

pub(super) async fn event_list(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    let Some(query) = decode_event_query(raw_query.as_deref()) else {
        return schema_invalid(&request_id, None, "query");
    };
    event_page_response(&app, &identity, &request_id, &query).await
}

pub(super) async fn event_stream(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    transport: Option<Extension<TransportPermit>>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    ws: WebSocketUpgrade,
) -> Response {
    let request_id = request_id(&app, &headers);
    let Some(query) = decode_event_query(raw_query.as_deref()) else {
        return schema_invalid(&request_id, None, "query");
    };
    let scope = app.scope(&identity);
    let Some(stream_permit) = app.event_stream_limits.acquire(&scope) else {
        return failure(
            StatusCode::TOO_MANY_REQUESTS,
            &request_id,
            None,
            ErrorClass::Exhausted,
            "event.stream-capacity",
            "The bounded event stream capacity is exhausted.",
            Some("stream"),
            true,
        );
    };
    // Register before the first durable read. A concurrent commit then either appears in this
    // catch-up or advances the coalesced watch version and forces one more catch-up after upgrade.
    let subscription = app.event_wakeups.subscribe(&scope);
    match app
        .store_io(|| {
            app.store
                .events(&scope, query.cursor.as_deref(), query.limit)
        })
        .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => return event_cursor_failure(&request_id, &error),
        Err(error) => return store_failure(&request_id, None, &error),
    }
    let policy = app.event_stream_policy;
    // The transport admission this connection was accepted under, moved into the upgraded task
    // below. hyper resolves an upgradeable connection future when it hands the socket over, so an
    // admission left with the connection stops counting a socket that is still serving. Absent
    // when no listener published one — the crate's own tests drive this route without a transport.
    let transport_admission = transport.map(|Extension(permit)| permit);
    ws.read_buffer_size(policy.max_input_bytes)
        .write_buffer_size(policy.write_buffer_bytes)
        .max_frame_size(policy.max_input_bytes)
        .max_message_size(policy.max_input_bytes)
        .max_write_buffer_size(
            policy
                .max_output_bytes
                .saturating_add(policy.write_buffer_bytes),
        )
        .on_upgrade(move |socket| async move {
            // Held for as long as this socket serves, so the transport budget counts it.
            let _transport_admission = transport_admission;
            let session = run_event_stream(
                app,
                scope,
                query,
                subscription,
                stream_permit,
                policy,
                socket,
            );
            let _completed = enforce_event_stream_lifetime(policy.lifetime, session).await;
        })
        .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotPageQuery {
    #[serde(default)]
    cursor: Option<String>,
    limit: u32,
}

pub(super) async fn reconciliation_snapshot_create(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    RawQuery(raw_query): RawQuery,
    body: Body,
) -> Response {
    let request_id = request_id(&app, &headers);
    if !query_is_empty(raw_query.as_deref()) {
        return schema_invalid(&request_id, None, "query");
    }
    let bytes = match read_bounded_body(body, &request_id).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if serde_json::from_slice::<EmptyInput>(&bytes).is_err() {
        return schema_invalid(&request_id, None, "input");
    }
    let snapshot_id = app.authority.snapshot_id();
    let scope = app.scope(&identity);
    let observed_at = app.authority.now();
    match app
        .store_io(|| {
            app.store.complete_snapshot(
                &scope,
                &identity.actor,
                identity.principal.as_deref(),
                observed_at,
                &snapshot_id,
                observed_at + chrono::Duration::minutes(5),
            )
        })
        .await
    {
        Ok(metadata) => success(StatusCode::CREATED, Success::observed(request_id, metadata)),
        Err(StoreError::SnapshotLimit) => failure(
            StatusCode::INSUFFICIENT_STORAGE,
            &request_id,
            None,
            ErrorClass::Exhausted,
            "snapshot.materialization-limit",
            "Snapshot materialization exceeds the bounded item limit.",
            Some("snapshot"),
            false,
        ),
        Err(error) => store_failure(&request_id, None, &error),
    }
}

pub(super) async fn reconciliation_snapshot_get(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    headers: HeaderMap,
    Path(snapshot_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> Response {
    let request_id = request_id(&app, &headers);
    let query =
        match serde_urlencoded::from_str::<SnapshotPageQuery>(raw_query.as_deref().unwrap_or("")) {
            Ok(value) if (1..=MAX_SNAPSHOT_PAGE_ITEMS).contains(&value.limit) => value,
            _ => return schema_invalid(&request_id, None, "query"),
        };
    match app
        .store_io(|| {
            app.store.snapshot_page(
                &app.scope(&identity),
                &snapshot_id,
                query.cursor.as_deref(),
                query.limit,
                app.authority.now(),
            )
        })
        .await
    {
        Ok(Ok(page)) => success(StatusCode::OK, Success::observed(request_id, page)),
        Ok(Err(SnapshotReadError::NotFound)) => not_found(&request_id),
        Ok(Err(SnapshotReadError::Expired)) => failure(
            StatusCode::CONFLICT,
            &request_id,
            None,
            ErrorClass::Conflict,
            "snapshot.expired",
            "The reconciliation snapshot has expired.",
            Some("snapshot"),
            false,
        ),
        Ok(Err(SnapshotReadError::Incomplete)) => failure(
            StatusCode::CONFLICT,
            &request_id,
            None,
            ErrorClass::Conflict,
            "snapshot.incomplete",
            "The materialized reconciliation snapshot is incomplete.",
            Some("snapshot"),
            true,
        ),
        Ok(Err(SnapshotReadError::InvalidCursor)) => failure(
            StatusCode::CONFLICT,
            &request_id,
            None,
            ErrorClass::Conflict,
            "snapshot.cursor-invalid",
            "The cursor does not belong to this reconciliation snapshot.",
            Some("cursor"),
            false,
        ),
        Err(error) => store_failure(&request_id, None, &error),
    }
}

fn decode_event_query(raw_query: Option<&str>) -> Option<EventQuery> {
    serde_urlencoded::from_str::<EventQuery>(raw_query.unwrap_or(""))
        .ok()
        .filter(|query| (1..=MAX_EVENT_PAGE_ITEMS).contains(&query.limit))
}

async fn event_page_response(
    app: &App,
    identity: &Identity,
    request_id: &str,
    query: &EventQuery,
) -> Response {
    match app
        .store_io(|| {
            app.store
                .events(&app.scope(identity), query.cursor.as_deref(), query.limit)
        })
        .await
    {
        Ok(Ok(page)) => success(StatusCode::OK, Success::observed(request_id, page)),
        Ok(Err(error)) => event_cursor_failure(request_id, &error),
        Err(error) => store_failure(request_id, None, &error),
    }
}

fn event_cursor_failure(request_id: &str, error: &EventCursorError) -> Response {
    let (code, message, retriable) = match error {
        EventCursorError::Source => (
            "event.source-mismatch",
            "The cursor does not belong to the current event source; reconcile before resuming.",
            true,
        ),
        EventCursorError::Retention { .. } => (
            "event.retention-gap",
            "The cursor is older than retained native history; reconcile before resuming.",
            true,
        ),
        EventCursorError::Invalid => (
            "event.cursor-invalid",
            "The event cursor is malformed or beyond the stream barrier.",
            false,
        ),
    };
    failure(
        StatusCode::CONFLICT,
        request_id,
        None,
        ErrorClass::Conflict,
        code,
        message,
        Some("cursor"),
        retriable,
    )
}

async fn run_event_stream(
    app: Arc<App>,
    scope: Scope,
    query: EventQuery,
    mut subscription: EventSubscription,
    _stream_permit: EventStreamPermit,
    policy: EventStreamPolicy,
    mut socket: WebSocket,
) {
    let mut cursor = query.cursor;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
    let mut control_rate = ControlRate::new();
    loop {
        match stream_catch_up(&app, &scope, &mut cursor, query.limit, policy, &mut socket).await {
            Ok(()) => {}
            Err(()) => return,
        }
        loop {
            tokio::select! {
                wakeup = subscription.changed() => {
                    if wakeup.is_err() {
                        return;
                    }
                    break;
                }
                incoming = socket.next() => {
                    match incoming {
                        Some(Err(_)) | None => return,
                        Some(Ok(message)) => match classify_client_frame(&message) {
                            ClientFrame::Close => return,
                            ClientFrame::Data => {
                                let _ = send_protocol_close(
                                    &mut socket,
                                    1003,
                                    "event streams accept control frames only",
                                    policy.send_timeout,
                                )
                                .await;
                                return;
                            }
                            ClientFrame::Control => {
                                if control_rate.exceeded(
                                    policy.max_controls_per_window,
                                    policy.control_window,
                                ) {
                                    let _ = send_protocol_close(
                                        &mut socket,
                                        1008,
                                        "event stream control-frame rate exceeded",
                                        policy.send_timeout,
                                    )
                                    .await;
                                    return;
                                }
                            }
                        },
                    }
                }
                _ = interval.tick() => break,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ClientFrame {
    Close,
    Data,
    Control,
}

#[derive(Serialize)]
struct EventStreamPageFrame<'a> {
    kind: &'static str,
    page: &'a EventPage,
}

pub(super) struct ControlRate {
    window_started: tokio::time::Instant,
    count: u32,
}

impl ControlRate {
    pub(super) fn new() -> Self {
        Self {
            window_started: tokio::time::Instant::now(),
            count: 0,
        }
    }

    pub(super) fn exceeded(&mut self, maximum: u32, window: std::time::Duration) -> bool {
        if self.window_started.elapsed() >= window {
            self.window_started = tokio::time::Instant::now();
            self.count = 0;
        }
        self.count = self.count.saturating_add(1);
        self.count > maximum
    }
}

struct BoundedJsonWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl std::io::Write for BoundedJsonWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other(
                "serialized event frame limit exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) fn bounded_event_frame(page: &EventPage, limit: usize) -> Result<String, ()> {
    let mut writer = BoundedJsonWriter {
        bytes: Vec::with_capacity(limit.min(16 * 1_024)),
        limit,
    };
    serde_json::to_writer(
        &mut writer,
        &EventStreamPageFrame {
            kind: "events",
            page,
        },
    )
    .map_err(|_| ())?;
    String::from_utf8(writer.bytes).map_err(|_| ())
}

pub(super) fn event_frame_or_backpressure(
    page: &EventPage,
    limit: usize,
    last_cursor: Option<&str>,
) -> Result<String, Value> {
    bounded_event_frame(page, limit).map_err(|()| {
        stream_boundary_payload("backpressure", "event.stream-backpressure", last_cursor)
    })
}

pub(super) fn classify_client_frame(message: &Message) -> ClientFrame {
    match message {
        Message::Close(_) => ClientFrame::Close,
        Message::Text(_) | Message::Binary(_) => ClientFrame::Data,
        Message::Ping(_) | Message::Pong(_) => ClientFrame::Control,
    }
}

pub(super) async fn enforce_event_stream_lifetime<F>(
    lifetime: std::time::Duration,
    session: F,
) -> bool
where
    F: std::future::Future<Output = ()>,
{
    tokio::time::timeout(lifetime, session).await.is_ok()
}

async fn stream_catch_up(
    app: &App,
    scope: &Scope,
    cursor: &mut Option<String>,
    limit: u32,
    policy: EventStreamPolicy,
    socket: &mut WebSocket,
) -> Result<(), ()> {
    let page_limit = limit.min(policy.max_page_items);
    for _ in 0..policy.max_catch_up_pages {
        let page = match app
            .store_io(|| app.store.events(scope, cursor.as_deref(), page_limit))
            .await
        {
            Ok(Ok(page)) => page,
            Ok(Err(_)) => {
                send_stream_boundary(
                    socket,
                    "gap",
                    "event.cursor-gap",
                    cursor.as_deref(),
                    policy.send_timeout,
                )
                .await?;
                return Err(());
            }
            Err(_) => {
                send_stream_boundary(
                    socket,
                    "failure",
                    "event.store-failed",
                    cursor.as_deref(),
                    policy.send_timeout,
                )
                .await?;
                return Err(());
            }
        };
        let next = page.next_cursor.clone();
        let caught_up = page
            .items
            .last()
            .is_none_or(|event| event.seq == page.through_seq)
            || page.items.len() < usize::try_from(page_limit).unwrap_or(usize::MAX);
        if !page.items.is_empty() {
            let encoded = match event_frame_or_backpressure(
                &page,
                policy.max_output_bytes,
                cursor.as_deref(),
            ) {
                Ok(encoded) => encoded,
                Err(boundary) => {
                    send_stream_boundary_payload(socket, boundary, policy.send_timeout).await?;
                    return Err(());
                }
            };
            send_stream_message(socket, Message::Text(encoded.into()), policy.send_timeout).await?;
        }
        *cursor = Some(next);
        if caught_up {
            return Ok(());
        }
    }
    send_stream_boundary(
        socket,
        "backpressure",
        "event.catch-up-limit",
        cursor.as_deref(),
        policy.send_timeout,
    )
    .await?;
    Err(())
}

async fn send_stream_boundary(
    socket: &mut WebSocket,
    kind: &str,
    code: &str,
    cursor: Option<&str>,
    send_timeout: std::time::Duration,
) -> Result<(), ()> {
    send_stream_boundary_payload(
        socket,
        stream_boundary_payload(kind, code, cursor),
        send_timeout,
    )
    .await
}

async fn send_stream_boundary_payload(
    socket: &mut WebSocket,
    payload: Value,
    send_timeout: std::time::Duration,
) -> Result<(), ()> {
    send_stream_message(
        socket,
        Message::Text(payload.to_string().into()),
        send_timeout,
    )
    .await?;
    send_stream_message(
        socket,
        Message::Close(Some(CloseFrame {
            code: 1013,
            reason: "resume with pull from last_cursor".into(),
        })),
        send_timeout,
    )
    .await
}

fn stream_boundary_payload(kind: &str, code: &str, cursor: Option<&str>) -> Value {
    json!({
        "kind": kind,
        "code": code,
        "last_cursor": cursor,
        "recovery": "pull"
    })
}

pub(super) async fn send_protocol_close(
    socket: &mut WebSocket,
    code: u16,
    reason: &'static str,
    send_timeout: std::time::Duration,
) -> Result<(), ()> {
    send_stream_message(
        socket,
        Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })),
        send_timeout,
    )
    .await
}

async fn send_stream_message(
    socket: &mut WebSocket,
    message: Message,
    send_timeout: std::time::Duration,
) -> Result<(), ()> {
    enforce_stream_send_deadline(send_timeout, socket.send(message)).await
}

pub(super) async fn enforce_stream_send_deadline<F, E>(
    send_timeout: std::time::Duration,
    send: F,
) -> Result<(), ()>
where
    F: std::future::Future<Output = Result<(), E>>,
{
    tokio::time::timeout(send_timeout, send)
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}
