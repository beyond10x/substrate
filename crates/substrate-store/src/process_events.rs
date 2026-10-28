//! ADR 0036 S12: a host process's lifecycle transitions in the existing event log, and the read
//! the per-process event stream makes of them.
//!
//! Each `process.*` event is an ordinary event-log row (`events`), so `GET /v1/events` pages it
//! with every other family's. Beside it, `process_events` names the record the event belongs to
//! and the log byte offset the record's S7 log had reached when the event was recorded, which is
//! where the per-process stream places it among the record's output lines. An end event
//! (`exited`, `stopped`, `vanished`) has no offset: the supervisor drained the log before it
//! published the end, so it follows every line. Rows go with their events: a row whose event the
//! event-log retention removed, or of an earlier generation, is pruned at the next append.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension as _, params};
use serde_json::{Value, json};
use substrate_wire::Event;

use crate::events::{append_event, ensure_stream, stream_position};
use crate::host_processes::{HostProcessEndedBy, HostProcessExit, HostProcessRecord};
use crate::{Scope, Store, StoreError, to_i64, to_u64};

/// One `process.*` event of a record, with where the stream places it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessStreamEvent {
    pub event: Event,
    /// The log byte offset the record's log had reached when the event was recorded; `None` for
    /// an end event, which follows the whole log.
    pub log_offset: Option<u64>,
}

/// What the per-process stream reads of the event log at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessStreamEvents {
    pub source_scope: String,
    pub generation: u64,
    /// The last seq the subject's event log has assigned.
    pub through_seq: u64,
    /// The oldest seq the event log still retains, when it retains any.
    pub first_retained_seq: Option<u64>,
    /// The record's events after the requested seq, oldest first.
    pub events: Vec<ProcessStreamEvent>,
}

pub(crate) fn migrate_process_events(connection: &Connection) -> Result<(), StoreError> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS process_events (
            deployment TEXT NOT NULL,
            subject TEXT NOT NULL,
            generation INTEGER NOT NULL,
            seq INTEGER NOT NULL,
            process TEXT NOT NULL CHECK (length(process) > 0),
            log_offset INTEGER CHECK (log_offset IS NULL OR log_offset >= 0),
            PRIMARY KEY (deployment, subject, seq)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS process_events_by_process
            ON process_events (deployment, subject, process, seq);
        ",
    )?;
    // The stop operation a caller's stop recorded, the cause of the `process.stopped` its end
    // appends. A record written before it has none and names its start instead.
    let mut statement = connection.prepare("PRAGMA table_info(host_processes)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    if !columns.iter().any(|column| column == "stop_operation") {
        connection.execute(
            "ALTER TABLE host_processes ADD COLUMN stop_operation TEXT",
            [],
        )?;
    }
    Ok(())
}

/// The observation every `process.*` event carries: the record, the state it entered, its cause
/// once it has one and its exit once observed.
fn observation(record: &HostProcessRecord) -> Value {
    let mut observation = json!({
        "process": record.id,
        "state": record.state.as_str(),
    });
    if let Some(cause) = record.ended_by {
        observation["ended_by"] = json!(cause.as_str());
    }
    match &record.exit {
        Some(HostProcessExit::Code(code)) => observation["exit"] = json!({ "code": code }),
        Some(HostProcessExit::Signal(signal)) => observation["exit"] = json!({ "signal": signal }),
        None => {}
    }
    observation
}

/// Appends one `process.*` event for `record`, caused by `operation`, inside the caller's
/// transaction, and the row that places it in the record's stream.
#[allow(clippy::too_many_arguments)] // One event row and its placement row, in one transaction.
pub(crate) fn append_process_event(
    connection: &Connection,
    store: &Store,
    scope: &Scope,
    record: &HostProcessRecord,
    transition: &str,
    operation: &str,
    at: DateTime<Utc>,
    log_offset: Option<u64>,
) -> Result<Event, StoreError> {
    // The operation that caused it names who acted. An operation the ledger no longer holds (a
    // long-lived record's start) leaves the subject itself as the actor.
    let (actor, principal) = connection
        .query_row(
            "SELECT actor, principal FROM operations
             WHERE deployment = ?1 AND subject = ?2 AND operation = ?3",
            params![scope.deployment, scope.subject, operation],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?
        .unwrap_or_else(|| (scope.subject.clone(), None));
    let event = append_event(
        connection,
        store.event_retention,
        scope,
        &record.id,
        "process",
        transition,
        &at.to_rfc3339(),
        &actor,
        principal.as_deref(),
        operation,
        Some(observation(record)),
    )?;
    connection.execute(
        "DELETE FROM process_events
         WHERE deployment = ?1 AND subject = ?2
           AND (generation != ?3
             OR seq < (SELECT COALESCE(MIN(seq), 0) FROM events
                       WHERE deployment = ?1 AND subject = ?2))",
        params![scope.deployment, scope.subject, to_i64(event.generation)?],
    )?;
    connection.execute(
        "INSERT INTO process_events (deployment, subject, generation, seq, process, log_offset)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            scope.deployment,
            scope.subject,
            to_i64(event.generation)?,
            to_i64(event.seq)?,
            record.id,
            log_offset.map(to_i64).transpose()?,
        ],
    )?;
    Ok(event)
}

/// The end event a record appends when it ends, and the operation that caused it: an exit and a
/// disappearance name the start, a completed stop of any cause the stop.
pub(crate) fn end_event(
    connection: &Connection,
    scope: &Scope,
    record: &HostProcessRecord,
) -> Result<(&'static str, String), StoreError> {
    Ok(match record.ended_by {
        Some(HostProcessEndedBy::Exit) | None => ("process.exited", record.start_operation.clone()),
        Some(HostProcessEndedBy::Vanished) => ("process.vanished", record.start_operation.clone()),
        Some(_) => {
            let stop: Option<String> = connection.query_row(
                "SELECT stop_operation FROM host_processes
                 WHERE deployment = ?1 AND subject = ?2 AND id = ?3",
                params![scope.deployment, scope.subject, record.id],
                |row| row.get(0),
            )?;
            (
                "process.stopped",
                stop.unwrap_or_else(|| record.start_operation.clone()),
            )
        }
    })
}

impl Store {
    /// The `process.*` events of one record after `after_seq`, oldest first, with the event-log
    /// position they were read at (S12).
    pub fn process_stream_events(
        &self,
        scope: &Scope,
        process: &str,
        after_seq: u64,
    ) -> Result<ProcessStreamEvents, StoreError> {
        let connection = self.connection.lock();
        ensure_stream(&connection, scope)?;
        let (source_scope, generation, through_seq) = stream_position(&connection, scope)?;
        let first_retained_seq = connection
            .query_row(
                "SELECT MIN(seq) FROM events WHERE deployment = ?1 AND subject = ?2",
                params![scope.deployment, scope.subject],
                |row| row.get::<_, Option<i64>>(0),
            )?
            .map(to_u64)
            .transpose()?;
        let mut statement = connection.prepare(
            "SELECT events.event_json, process_events.log_offset
             FROM process_events JOIN events
               ON events.deployment = process_events.deployment
              AND events.subject = process_events.subject
              AND events.seq = process_events.seq
              AND events.generation = process_events.generation
             WHERE process_events.deployment = ?1 AND process_events.subject = ?2
               AND process_events.process = ?3 AND process_events.generation = ?4
               AND process_events.seq > ?5
             ORDER BY process_events.seq",
        )?;
        let events = statement
            .query_map(
                params![
                    scope.deployment,
                    scope.subject,
                    process,
                    to_i64(generation)?,
                    to_i64(after_seq)?
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
            )?
            .map(|row| -> Result<ProcessStreamEvent, StoreError> {
                let (json, offset) = row?;
                let mut event: Event = serde_json::from_str(&json)?;
                event.source_scope.clone_from(&source_scope);
                Ok(ProcessStreamEvent {
                    event,
                    log_offset: offset.map(to_u64).transpose()?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ProcessStreamEvents {
            source_scope,
            generation,
            through_seq,
            first_retained_seq,
            events,
        })
    }
}
