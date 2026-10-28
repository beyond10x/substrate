//! ADR 0036 S12: the per-process event stream, `GET /v1/processes/{process_id}/events`.
//!
//! One WebSocket per reader carries one record's lifecycle events, read from the existing event
//! log, interleaved with its S7 log lines, read from the log file and never from the program's
//! pipes. Each transition is placed at the log offset the store recorded beside it
//! (`substrate_store::ProcessStreamEvent::log_offset`), so a line written before a transition
//! precedes it; an end event follows the whole log. After the record's end, every event and the
//! whole retained log, one `ended` item is sent and the socket is closed.
//!
//! **Order of the reads.** Each round reads the record first, then the log, then the events. A
//! record already ended when it is read has a complete log, because the supervisor drains the log
//! before it publishes the end, and the end event is committed with the end. An event read after
//! the log is held until the stream reaches its offset.
//!
//! **Honest boundaries.** Every cursor the daemon cannot honour is refused before the upgrade with
//! the event log's own codes. Bytes the log's bound discarded before the reader reached them are a
//! `gap` item, never skipped. A stream that can no longer be served (a failed read, a record
//! pruned under it, event history removed under it, or the stream lifetime) closes with 1011 and
//! no `ended` item; the reader resumes from its last cursor and is told then what it lost. A
//! reader that does not drain its socket stalls only its own stream: a send that does not
//! complete within the event stream's send bound drops the connection.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use axum::Extension;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse as _, Response};
use futures_util::StreamExt as _;
use parking_lot::Mutex as ParkingMutex;
use serde_json::{Value, json};
use substrate_host::{DriverError, DriverErrorClass, LogLine, LogRead, LogStream, ProcessLogs};
use substrate_store::{
    EventCursorError, HostProcessEndedBy, HostProcessExit, HostProcessRecord, HostProcessState,
    ProcessStreamEvent, ProcessStreamEvents, Scope,
};
use substrate_wire::{
    MAX_PROCESS_EVENT_CURSOR_CHARS, MAX_PROCESS_EVENT_LINE_BYTES, MAX_PROCESS_EVENT_READERS,
    MAX_PROCESS_EVENT_STREAMS_PER_DEPLOYMENT, MAX_PROCESS_EVENT_STREAMS_PER_SUBJECT,
    PROCESS_EVENTS_READERS_EXCEEDED, PROCESS_NOT_FOUND, ProcessEventCursor, ProcessEventsQuery,
    process_refusal,
};

use crate::runtime::{StreamSlot, TransportPermit, hand_over_transport};

use super::events::{
    ClientFrame, ControlRate, EventStreamLimits, EventStreamPermit, EventStreamPolicy,
    EventSubscription, classify_client_frame, enforce_stream_send_deadline, event_cursor_failure,
};
use super::operations::driver_detail;
use super::responses::{failure, failure_detail, request_id, schema_invalid, store_failure};
use super::{App, Identity, SessionTransport};

/// Log bytes one read takes before the stream sends what it read.
const READ_BYTES: u64 = 256 * 1024;
/// How often a waiting stream looks at its log's end, which wakes no event subscriber. It reads
/// the store only when the log grew, an event was committed, or [`STORE_POLL`] has passed.
const LOG_POLL: std::time::Duration = std::time::Duration::from_millis(100);
/// The longest an idle stream goes without reading the store.
const STORE_POLL: std::time::Duration = std::time::Duration::from_secs(1);
const END_TRANSITIONS: [&str; 3] = ["process.exited", "process.stopped", "process.vanished"];

/// The open streams (S12): at most [`MAX_PROCESS_EVENT_READERS`] per record, and, over every
/// record live or ended, at most [`MAX_PROCESS_EVENT_STREAMS_PER_SUBJECT`] per subject and
/// [`MAX_PROCESS_EVENT_STREAMS_PER_DEPLOYMENT`] per deployment, counted by the same per-subject
/// and deployment-wide slots the event and metrics streams use.
pub(super) struct ProcessEventReaders {
    held: ParkingMutex<HashMap<(Scope, String), usize>>,
    streams: Arc<EventStreamLimits>,
}

impl Default for ProcessEventReaders {
    fn default() -> Self {
        Self {
            held: ParkingMutex::default(),
            streams: EventStreamLimits::new(
                MAX_PROCESS_EVENT_STREAMS_PER_DEPLOYMENT,
                MAX_PROCESS_EVENT_STREAMS_PER_SUBJECT,
            ),
        }
    }
}

/// One open stream of one record. Dropping it, on every path out of the stream, returns its
/// place and its subject and deployment slots.
pub(super) struct ProcessEventReader {
    readers: Arc<ProcessEventReaders>,
    key: (Scope, String),
    _stream: EventStreamPermit,
}

impl ProcessEventReaders {
    fn acquire(self: &Arc<Self>, scope: &Scope, process: &str) -> Option<ProcessEventReader> {
        let key = (scope.clone(), process.to_owned());
        let mut held = self.held.lock();
        if held.get(&key).copied().unwrap_or(0) >= MAX_PROCESS_EVENT_READERS {
            return None;
        }
        let stream = self.streams.acquire(scope)?;
        *held.entry(key.clone()).or_insert(0) += 1;
        drop(held);
        Some(ProcessEventReader {
            readers: Arc::clone(self),
            key,
            _stream: stream,
        })
    }
}

impl Drop for ProcessEventReader {
    fn drop(&mut self) {
        let mut held = self.readers.held.lock();
        if let Some(count) = held.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                held.remove(&self.key);
            }
        }
    }
}

/// A process event stream's slot: the per-record readers bound and the per-subject and deployment
/// stream caps the route publishes (ADR 0042).
impl StreamSlot for ProcessEventReader {}

/// Where a stream stands: the record, the event-log source and generation it reads, the seq
/// through which the record's events are delivered, and the log offset after the last delivered
/// line. Its text is the item's cursor, `pe1.{process}.{source}.{generation}.{seq}.{offset}`.
type Position = ProcessEventCursor;

/// The published cursor form, and nothing else (`ProcessEventCursor::parse`).
fn parse_cursor(cursor: &str) -> Option<Position> {
    ProcessEventCursor::parse(cursor)
}

/// The log reader the stream uses: the driver's when this run opened one, else the stored logs
/// an earlier run left. Like `process.logs.get`, the route carries no capability predicate.
fn log_reader(app: &App) -> Option<&dyn ProcessLogs> {
    match app.host_processes.as_deref() {
        Some(supervisor) => Some(supervisor as &dyn ProcessLogs),
        None => app.host_process_logs.as_deref(),
    }
}

/// Reads the record's log forward from `from`. A record with no log (its start never wrote one,
/// or no reader exists) has an empty log ending at 0. `Ok(None)` is an offset the log cannot
/// honour; `Err` a read that failed.
fn read_log(
    app: &App,
    process: &str,
    from: u64,
    max_bytes: u64,
) -> Result<Option<LogRead>, DriverError> {
    let empty = |from: u64| {
        (from == 0).then(|| LogRead {
            gap: 0,
            lines: Vec::new(),
            end: 0,
        })
    };
    let Some(reader) = log_reader(app) else {
        return Ok(empty(from));
    };
    match reader.log_read(process, from, max_bytes) {
        Ok(read) => Ok(read),
        Err(error) if error.class == DriverErrorClass::NotFound => Ok(empty(from)),
        Err(error) => Err(error),
    }
}

fn is_end(event: &ProcessStreamEvent) -> bool {
    END_TRANSITIONS.contains(&event.event.transition.as_str())
}

/// The S7 line shape, with `text` at most [`MAX_PROCESS_EVENT_LINE_BYTES`] cut at a character
/// boundary. A line the log stored cut may end inside a character; that partial character is
/// dropped before the line is judged, so a cut never makes a valid line lossy.
fn wire_line(line: &LogLine) -> Value {
    let mut bytes = line.bytes.as_slice();
    let mut truncated = line.truncated;
    if truncated
        && let Err(error) = std::str::from_utf8(bytes)
        && error.error_len().is_none()
    {
        bytes = &bytes[..error.valid_up_to()];
    }
    let (mut text, lossy) = match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_owned(), false),
        Err(_) => (String::from_utf8_lossy(bytes).into_owned(), true),
    };
    if text.len() > MAX_PROCESS_EVENT_LINE_BYTES {
        let mut cut = MAX_PROCESS_EVENT_LINE_BYTES;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        truncated = true;
    }
    json!({
        "stream": match line.stream {
            LogStream::Stdout => "stdout",
            LogStream::Stderr => "stderr",
        },
        "text": text,
        "lossy": lossy,
        "truncated": truncated,
        "full_bytes": line.full_len,
    })
}

/// The `ended` item's member: the record's cause and, once observed, its exit.
fn ended(record: &HostProcessRecord) -> Option<Value> {
    let cause: HostProcessEndedBy = record.ended_by?;
    let mut value = json!({ "ended_by": cause.as_str() });
    match &record.exit {
        Some(HostProcessExit::Code(code)) => value["exit"] = json!({ "code": code }),
        Some(HostProcessExit::Signal(signal)) => value["exit"] = json!({ "signal": signal }),
        None => {}
    }
    Some(value)
}

fn readers_exceeded(request_id: &str) -> Response {
    let named = process_refusal(PROCESS_EVENTS_READERS_EXCEEDED)
        .expect("the readers refusal is a published process refusal");
    failure(
        StatusCode::from_u16(named.status).unwrap_or(StatusCode::TOO_MANY_REQUESTS),
        request_id,
        None,
        named.class,
        named.code,
        named.message,
        Some("process"),
        true,
    )
}

fn not_found(request_id: &str) -> Response {
    let named = process_refusal(PROCESS_NOT_FOUND).expect("published process refusal");
    failure(
        StatusCode::from_u16(named.status).unwrap_or(StatusCode::NOT_FOUND),
        request_id,
        None,
        named.class,
        named.code,
        named.message,
        Some("process"),
        false,
    )
}

/// Where a stream starts: the cursor's position once it names this record and this event log, a
/// retained seq and a readable log offset. Without a cursor, what the event log and the log still
/// hold (ADR 0036 S12): the oldest retained event and log byte. Retention never refuses a stream
/// without a cursor, which promised its reader nothing yet; `event.retention-gap` is only for a
/// cursor whose seq retention has passed.
fn start_position(
    app: &App,
    request_id: &str,
    record: &HostProcessRecord,
    requested: Option<Position>,
    read: &ProcessStreamEvents,
) -> Result<Position, Response> {
    let first = read.first_retained_seq.unwrap_or(read.through_seq + 1);
    let retention = EventCursorError::Retention {
        first,
        last: read.through_seq,
    };
    let Some(position) = requested else {
        return Ok(Position {
            process: record.id.clone(),
            source: read.source_scope.clone(),
            generation: read.generation,
            seq: first.saturating_sub(1),
            offset: 0,
        });
    };
    // A cursor another record's stream carried states nothing about this one (S12: the cursor is
    // per process); its seq would skip this record's events silently.
    if position.process != record.id {
        return Err(event_cursor_failure(request_id, &EventCursorError::Invalid));
    }
    if position.source != read.source_scope || position.generation != read.generation {
        return Err(event_cursor_failure(request_id, &EventCursorError::Source));
    }
    if position.seq > read.through_seq {
        return Err(event_cursor_failure(request_id, &EventCursorError::Invalid));
    }
    if position.seq.saturating_add(1) < first {
        return Err(event_cursor_failure(request_id, &retention));
    }
    match read_log(app, &record.id, position.offset, 1) {
        Ok(Some(_)) => Ok(position),
        Ok(None) => Err(event_cursor_failure(request_id, &EventCursorError::Invalid)),
        Err(error) => {
            let (status, detail) = driver_detail(None, &error);
            Err(failure_detail(status, request_id, detail))
        }
    }
}

#[allow(clippy::too_many_arguments)] // One axum handler's extractors.
pub(super) async fn process_events(
    State(app): State<Arc<App>>,
    Extension(identity): Extension<Identity>,
    Extension(transport): Extension<SessionTransport>,
    transport_permit: Option<Extension<TransportPermit>>,
    headers: HeaderMap,
    Path(process_id): Path<String>,
    RawQuery(raw_query): RawQuery,
    ws: WebSocketUpgrade,
) -> Response {
    let request_id = request_id(&app, &headers);
    if let Err(code) = super::processes::admission(&app, &identity, transport) {
        return super::processes::refusal(&request_id, None, code, "process");
    }
    let Ok(query) =
        serde_urlencoded::from_str::<ProcessEventsQuery>(raw_query.as_deref().unwrap_or(""))
    else {
        return schema_invalid(&request_id, None, "query");
    };
    if query.cursor.as_ref().is_some_and(|cursor| {
        cursor.is_empty() || cursor.chars().count() > MAX_PROCESS_EVENT_CURSOR_CHARS
    }) {
        return schema_invalid(&request_id, None, "query");
    }
    let scope = app.scope(&identity);
    // Register before the first durable read, as the event stream does: a commit after it either
    // appears in the first round's read or wakes the stream for another.
    let subscription = app.event_wakeups.subscribe(&scope);
    let record = match app
        .store_io(|| app.store.host_process(&scope, &process_id))
        .await
    {
        Ok(Some(record)) => record,
        Ok(None) => return not_found(&request_id),
        Err(error) => return store_failure(&request_id, None, &error),
    };
    let requested = match query.cursor.as_deref() {
        None => None,
        Some(cursor) => match parse_cursor(cursor) {
            Some(position) => Some(position),
            None => return event_cursor_failure(&request_id, &EventCursorError::Invalid),
        },
    };
    let after_seq = requested.as_ref().map_or(0, |position| position.seq);
    let read = match app
        .store_io(|| {
            app.store
                .process_stream_events(&scope, &process_id, after_seq)
        })
        .await
    {
        Ok(read) => read,
        Err(error) => return store_failure(&request_id, None, &error),
    };
    let from_held = requested.is_none();
    let start = match start_position(&app, &request_id, &record, requested, &read) {
        Ok(start) => start,
        Err(response) => return response,
    };
    let Some(reader) = app.process_event_readers.acquire(&scope, &process_id) else {
        return readers_exceeded(&request_id);
    };
    let policy = app.event_stream_policy;
    // Handed over to the reader slot in the upgraded task (ADR 0042): from the upgrade on this
    // socket is counted by the per-record readers bound and the subject and deployment caps.
    let transport_admission = transport_permit.map(|Extension(permit)| permit);
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
            hand_over_transport(transport_admission, &reader);
            let deadline = tokio::time::Instant::now() + policy.lifetime;
            let stream = Stream {
                app,
                scope,
                process: process_id,
                position: start,
                from_held,
                seen_log_end: None,
                queue: VecDeque::new(),
                policy,
                socket,
            };
            stream.run(subscription, deadline).await;
            drop(reader);
        })
        .into_response()
}

/// Why a round ended without the stream being complete.
enum Stop {
    /// The stream can no longer be served: closed 1011 with no `ended` item.
    Unserved(&'static str),
    /// The connection is gone, or a send did not complete; nothing more can be sent.
    Gone,
}

struct Stream {
    app: Arc<App>,
    scope: Scope,
    process: String,
    position: Position,
    /// Opened without a cursor and no event read yet: the first read starts from what the event
    /// log still holds, whatever retention removed before it (S12).
    from_held: bool,
    /// The log end the last round saw; a waiting stream reads the store again once it moves.
    seen_log_end: Option<u64>,
    /// The record's events read and not yet delivered, oldest first.
    queue: VecDeque<ProcessStreamEvent>,
    policy: EventStreamPolicy,
    socket: WebSocket,
}

/// Why a waiting stream stopped waiting.
enum Wait {
    /// Something may be due: read the store again.
    Round,
    /// The stream is over; nothing more is sent.
    Done,
}

impl Stream {
    async fn run(mut self, mut subscription: EventSubscription, deadline: tokio::time::Instant) {
        let mut fetched_seq = self.position.seq;
        let mut control_rate = ControlRate::new();
        loop {
            let outcome = tokio::select! {
                outcome = self.round(&mut fetched_seq) => outcome,
                () = tokio::time::sleep_until(deadline) => Err(Stop::Unserved("process event stream lifetime reached")),
            };
            match outcome {
                Ok(true) | Err(Stop::Gone) => return,
                Ok(false) => {}
                Err(Stop::Unserved(reason)) => {
                    let _ = self.close(1011, reason).await;
                    return;
                }
            }
            match self
                .wait(&mut subscription, &mut control_rate, deadline)
                .await
            {
                Wait::Round => {}
                Wait::Done => return,
            }
        }
    }

    /// Waits until a round may have something to send: an event committed in the subject, the
    /// log grew past what the last round saw, or [`STORE_POLL`] passed. An idle stream reads only
    /// the log's end every [`LOG_POLL`], and the store at most once per [`STORE_POLL`].
    async fn wait(
        &mut self,
        subscription: &mut EventSubscription,
        control_rate: &mut ControlRate,
        deadline: tokio::time::Instant,
    ) -> Wait {
        let store_due = tokio::time::Instant::now() + STORE_POLL;
        loop {
            tokio::select! {
                wakeup = subscription.changed() => {
                    if wakeup.is_err() {
                        let _ = self.close(1011, "process event stream wakeups ended").await;
                        return Wait::Done;
                    }
                    return Wait::Round;
                }
                () = tokio::time::sleep(LOG_POLL) => {
                    if tokio::time::Instant::now() >= store_due || self.log_grew() {
                        return Wait::Round;
                    }
                }
                () = tokio::time::sleep_until(deadline) => {
                    let _ = self.close(1011, "process event stream lifetime reached").await;
                    return Wait::Done;
                }
                incoming = self.socket.next() => match incoming {
                    Some(Err(_)) | None => return Wait::Done,
                    Some(Ok(message)) => match classify_client_frame(&message) {
                        ClientFrame::Close => return Wait::Done,
                        ClientFrame::Data => {
                            let _ = self.close(1003, "process event streams accept control frames only").await;
                            return Wait::Done;
                        }
                        ClientFrame::Control => {
                            if control_rate.exceeded(
                                self.policy.max_controls_per_window,
                                self.policy.control_window,
                            ) {
                                let _ = self.close(1008, "process event stream control-frame rate exceeded").await;
                                return Wait::Done;
                            }
                        }
                    },
                },
            }
        }
    }

    /// Whether the record's log ends somewhere other than the last round saw. A reader that
    /// cannot say leaves the store poll to find out.
    fn log_grew(&self) -> bool {
        let end = log_reader(&self.app).and_then(|reader| reader.log_end(&self.process).ok());
        end.is_some() && end != self.seen_log_end
    }

    /// Reads the record, its log and its events once and sends what is due. `Ok(true)` when the
    /// stream is complete and closed; `Ok(false)` when it waits for more.
    async fn round(&mut self, fetched_seq: &mut u64) -> Result<bool, Stop> {
        loop {
            let app = Arc::clone(&self.app);
            let record = match app
                .store_io(|| app.store.host_process(&self.scope, &self.process))
                .await
            {
                Ok(Some(record)) => record,
                Ok(None) => return Err(Stop::Unserved("the host-process record was removed")),
                Err(_) => return Err(Stop::Unserved("the event log could not be read")),
            };
            if record.state == HostProcessState::Starting {
                // `process.started` precedes every line (S12) and is committed only when the
                // record is running: hold the lines the program already wrote until then.
                self.seen_log_end =
                    log_reader(&app).and_then(|reader| reader.log_end(&self.process).ok());
                return Ok(false);
            }
            let complete = record.state == HostProcessState::Ended;
            let read = match read_log(&app, &self.process, self.position.offset, READ_BYTES) {
                Ok(Some(read)) => read,
                Ok(None) => return Err(Stop::Unserved("the log offset is no longer readable")),
                Err(_) => return Err(Stop::Unserved("the host-process log could not be read")),
            };
            let after = *fetched_seq;
            let Ok(events) = app
                .store_io(|| {
                    app.store
                        .process_stream_events(&self.scope, &self.process, after)
                })
                .await
            else {
                return Err(Stop::Unserved("the event log could not be read"));
            };
            // A stream that started from what was held and has read nothing yet promised nothing
            // retention can since have removed; from its first read on, it has.
            let removed = !self.from_held
                && events
                    .first_retained_seq
                    .is_some_and(|first| after.saturating_add(1) < first);
            if events.source_scope != self.position.source
                || events.generation != self.position.generation
                || removed
            {
                return Err(Stop::Unserved(
                    "event history this stream needs was removed",
                ));
            }
            self.from_held = false;
            self.queue.extend(events.events);
            *fetched_seq = events.through_seq;

            self.send_due(*fetched_seq).await?;
            if read.gap > 0 {
                self.position.offset = self.position.offset.saturating_add(read.gap);
                self.position.seq = self.delivered_through(*fetched_seq);
                let item = json!({
                    "kind": "gap",
                    "cursor": self.position.to_string(),
                    "gap": { "discarded_bytes": read.gap },
                });
                self.send(&item).await?;
                self.send_due(*fetched_seq).await?;
            }
            for (line, end) in &read.lines {
                self.send_due(*fetched_seq).await?;
                self.position.offset = *end;
                self.position.seq = self.delivered_through(*fetched_seq);
                let item = json!({
                    "kind": "line",
                    "cursor": self.position.to_string(),
                    "line": wire_line(line),
                });
                self.send(&item).await?;
            }
            self.send_due(*fetched_seq).await?;
            if self.position.offset < read.end {
                continue;
            }
            if !complete {
                self.seen_log_end = Some(read.end);
                return Ok(false);
            }
            while let Some(event) = self.queue.pop_front() {
                self.send_event(event, *fetched_seq).await?;
            }
            let Some(ended) = ended(&record) else {
                return Err(Stop::Unserved("the ended record names no cause"));
            };
            self.position.seq = *fetched_seq;
            let item = json!({
                "kind": "ended",
                "cursor": self.position.to_string(),
                "ended": ended,
            });
            self.send(&item).await?;
            let _ = self.close(1000, "process ended").await;
            return Ok(true);
        }
    }

    /// The seq through which every event of the record is delivered: one before the oldest held
    /// event, or everything read when none is held.
    fn delivered_through(&self, fetched_seq: u64) -> u64 {
        self.queue
            .front()
            .map_or(fetched_seq, |event| event.event.seq.saturating_sub(1))
    }

    /// Sends every held event the stream's log position has reached. An end event waits for the
    /// whole log; an event without an offset is due at once.
    async fn send_due(&mut self, fetched_seq: u64) -> Result<(), Stop> {
        while let Some(front) = self.queue.front() {
            let due = !is_end(front)
                && front
                    .log_offset
                    .is_none_or(|offset| offset <= self.position.offset);
            if !due {
                return Ok(());
            }
            let event = self.queue.pop_front().expect("front exists");
            self.send_event(event, fetched_seq).await?;
        }
        Ok(())
    }

    async fn send_event(
        &mut self,
        event: ProcessStreamEvent,
        fetched_seq: u64,
    ) -> Result<(), Stop> {
        self.position.seq = self.delivered_through(fetched_seq).max(event.event.seq);
        let item = json!({
            "kind": "event",
            "cursor": self.position.to_string(),
            "event": event.event,
        });
        self.send(&item).await
    }

    async fn send(&mut self, item: &Value) -> Result<(), Stop> {
        let text = serde_json::to_string(item).map_err(|_| Stop::Gone)?;
        enforce_stream_send_deadline(
            self.policy.send_timeout,
            self.socket.send(Message::Text(text.into())),
        )
        .await
        .map_err(|()| Stop::Gone)
    }

    async fn close(&mut self, code: u16, reason: &'static str) -> Result<(), ()> {
        enforce_stream_send_deadline(
            self.policy.send_timeout,
            self.socket.send(Message::Close(Some(CloseFrame {
                code,
                reason: reason.into(),
            }))),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{Position, parse_cursor, wire_line};
    use substrate_host::{LogLine, LogStream};

    #[test]
    fn a_cursor_round_trips_and_a_foreign_one_is_not_read() {
        let position = Position {
            process: "proc_01ABC".to_owned(),
            source: "scope_abc".to_owned(),
            generation: 1,
            seq: 12,
            offset: 340,
        };
        assert_eq!(position.to_string(), "pe1.proc_01ABC.scope_abc.1.12.340");
        assert_eq!(parse_cursor(&position.to_string()), Some(position));
        for foreign in [
            "ev2.scope_abc.1.12",
            "pe1.scope_abc.1.12.340",
            "pe1.proc_01ABC.scope_abc.1.12",
            "pe1.proc_01ABC.scope_abc.0.12.340",
            "pe1.proc_01ABC.scope_abc.1.012.340",
            "pe1.proc_01ABC.other.1.12.340",
            "pe1.proc_01ABC.scope_.1.12.340",
            "pe1.proc_01ABC.scope_abc.1.12.340.1",
        ] {
            assert_eq!(parse_cursor(foreign), None, "{foreign}");
        }
    }

    fn line(bytes: &[u8], truncated: bool, full_len: u64) -> LogLine {
        LogLine {
            stream: LogStream::Stdout,
            bytes: bytes.to_vec(),
            truncated,
            full_len,
        }
    }

    #[test]
    fn a_line_cut_inside_a_character_is_not_lossy() {
        let mut bytes = vec![b'a'; 3];
        bytes.extend_from_slice(&"é".as_bytes()[..1]);
        let value = wire_line(&line(&bytes, true, 40));
        assert_eq!(value["text"], "aaa");
        assert_eq!(value["lossy"], false);
        assert_eq!(value["truncated"], true);
        assert_eq!(value["full_bytes"], 40);
    }

    #[test]
    fn invalid_utf8_is_lossy_and_the_text_stays_within_the_bound() {
        let value = wire_line(&line(&[0xff; 16 * 1024], false, 16 * 1024));
        assert_eq!(value["lossy"], true);
        let text = value["text"].as_str().expect("text");
        assert!(text.len() <= super::MAX_PROCESS_EVENT_LINE_BYTES);
        assert_eq!(
            value["truncated"], true,
            "replacement grew the text past the bound"
        );
    }

    /// D5: every stream counts against its subject's and the deployment's cap, on whichever
    /// records it reads. Two subjects of 256 streams each fill the deployment; a third subject's
    /// first stream is refused, and a returned place is taken again.
    #[test]
    fn the_deployment_cap_counts_every_subjects_streams() {
        let readers = std::sync::Arc::new(super::ProcessEventReaders::default());
        let scope = |subject: &str| substrate_store::Scope {
            deployment: "dep".to_owned(),
            subject: subject.to_owned(),
        };
        let half = super::MAX_PROCESS_EVENT_STREAMS_PER_DEPLOYMENT / 2;
        let mut held = Vec::new();
        for subject in ["local:1", "local:2"] {
            for index in 0..half {
                let record = format!("proc_{}", index / super::MAX_PROCESS_EVENT_READERS);
                held.push(
                    readers
                        .acquire(&scope(subject), &record)
                        .unwrap_or_else(|| panic!("{subject} stream {index}")),
                );
            }
        }
        assert!(readers.acquire(&scope("local:3"), "proc_x").is_none());
        drop(held.pop());
        assert!(readers.acquire(&scope("local:3"), "proc_x").is_some());
    }
}
