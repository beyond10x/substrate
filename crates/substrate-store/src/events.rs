use crate::operations::load_operation;
use crate::{Scope, Store, StoreError, to_i64, to_u64};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use serde_json::Value;
use substrate_wire::{Event, EventCause, EventControl, EventPage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitEffect {
    pub scope: Scope,
    pub source_scope: String,
    pub generation: u64,
    pub through_seq: u64,
}

pub trait CommitEffectSink: Send + Sync {
    fn committed(&self, effects: &[CommitEffect]);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventCursorError {
    Source,
    Retention { first: u64, last: u64 },
    Invalid,
}

impl Store {
    pub fn set_commit_effect_sink(&self, sink: std::sync::Arc<dyn CommitEffectSink>) {
        *self.effect_sink.write() = Some(sink);
    }

    pub(crate) fn report_committed(&self, effects: &[CommitEffect]) {
        if !effects.is_empty()
            && let Some(sink) = self.effect_sink.read().as_ref()
        {
            sink.committed(effects);
        }
    }

    pub fn event_retention(&self) -> u64 {
        self.event_retention
    }

    pub fn stream_position(&self, scope: &Scope) -> Result<(String, u64, u64), StoreError> {
        let connection = self.connection.lock();
        ensure_stream(&connection, scope)?;
        stream_position(&connection, scope)
    }

    pub fn events(
        &self,
        scope: &Scope,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<Result<EventPage, EventCursorError>, StoreError> {
        let connection = self.connection.lock();
        ensure_stream(&connection, scope)?;
        let (source_scope, generation, through_seq) = stream_position(&connection, scope)?;
        let first_retained = connection
            .query_row(
                "SELECT MIN(seq) FROM events WHERE deployment = ?1 AND subject = ?2",
                params![scope.deployment, scope.subject],
                |row| row.get::<_, Option<i64>>(0),
            )?
            .map(to_u64)
            .transpose()?;
        let start = match cursor {
            None => first_retained.unwrap_or(1).saturating_sub(1),
            Some(value) => match parse_event_cursor(value) {
                Some((cursor_scope, cursor_generation, sequence))
                    if cursor_scope == source_scope && cursor_generation == generation =>
                {
                    sequence
                }
                Some(_) => {
                    return Ok(Err(EventCursorError::Source));
                }
                None => return Ok(Err(EventCursorError::Invalid)),
            },
        };
        if start > through_seq {
            return Ok(Err(EventCursorError::Invalid));
        }
        if let Some(first) = first_retained
            && start.saturating_add(1) < first
        {
            return Ok(Err(EventCursorError::Retention {
                first,
                last: through_seq,
            }));
        }
        let mut statement = connection.prepare(
            "SELECT event_json FROM events
             WHERE deployment = ?1 AND subject = ?2 AND seq > ?3
             ORDER BY seq LIMIT ?4",
        )?;
        let items = statement
            .query_map(
                params![
                    scope.deployment,
                    scope.subject,
                    to_i64(start)?,
                    i64::from(limit)
                ],
                |row| row.get::<_, String>(0),
            )?
            .map(|row| -> Result<Event, StoreError> {
                let mut event: Event = serde_json::from_str(&row?)?;
                event.source_scope.clone_from(&source_scope);
                Ok(event)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let next_seq = if items.len() < usize::try_from(limit).unwrap_or(usize::MAX) {
            through_seq
        } else {
            items.last().map_or(start, |event| event.seq)
        };
        let next_cursor = event_cursor(&source_scope, generation, next_seq);
        Ok(Ok(EventPage {
            source_scope,
            generation,
            items,
            next_cursor,
            through_seq,
            first_retained_seq: first_retained,
        }))
    }

    pub fn reset_stream_generation(&self, scope: &Scope) -> Result<u64, StoreError> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_stream(&transaction, scope)?;
        let (_, current, _) = stream_position(&transaction, scope)?;
        let replacement = current.checked_add(1).ok_or(StoreError::IntegerRange)?;
        transaction.execute(
            "DELETE FROM events WHERE deployment = ?1 AND subject = ?2",
            params![scope.deployment, scope.subject],
        )?;
        transaction.execute(
            "UPDATE stream_meta SET generation = ?3, next_seq = 1
             WHERE deployment = ?1 AND subject = ?2",
            params![scope.deployment, scope.subject, to_i64(replacement)?],
        )?;
        transaction.commit()?;
        Ok(replacement)
    }
}

pub(crate) fn ensure_stream(connection: &Connection, scope: &Scope) -> Result<(), StoreError> {
    if connection
        .query_row(
            "SELECT 1 FROM stream_meta WHERE deployment = ?1 AND subject = ?2",
            params![scope.deployment, scope.subject],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        return Ok(());
    }
    for _ in 0..8 {
        connection.execute(
            "INSERT OR IGNORE INTO stream_meta (
                deployment, subject, source_scope, generation, next_seq
             ) VALUES (
                ?1, ?2, 'scope_' || lower(hex(randomblob(16))),
                ((random() & 9223372036854775807) % 9007199254740990) + 1, 1
             )",
            params![scope.deployment, scope.subject],
        )?;
        if connection
            .query_row(
                "SELECT 1 FROM stream_meta WHERE deployment = ?1 AND subject = ?2",
                params![scope.deployment, scope.subject],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            return Ok(());
        }
    }
    Err(StoreError::IntegerRange)
}

pub(crate) fn stream_position(
    connection: &Connection,
    scope: &Scope,
) -> Result<(String, u64, u64), StoreError> {
    let (source_scope, generation, next_seq): (String, i64, i64) = connection.query_row(
        "SELECT source_scope, generation, next_seq FROM stream_meta
         WHERE deployment = ?1 AND subject = ?2",
        params![scope.deployment, scope.subject],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok((
        source_scope,
        to_u64(generation)?,
        to_u64(next_seq)?.saturating_sub(1),
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_event(
    connection: &Connection,
    retention: u64,
    scope: &Scope,
    resource: &str,
    resource_kind: &str,
    transition: &str,
    observed_at: &str,
    actor: &str,
    principal: Option<&str>,
    operation: &str,
    observation: Option<Value>,
) -> Result<Event, StoreError> {
    // Sessions are implemented ahead of their event contract. Until a successor bundle owns a
    // session branch, publish only the operation-ledger projection that 0.4 consumers can validate.
    let transition = if resource_kind == "session" {
        match transition {
            "session.unknown" => "operation.unknown",
            "session.cleanup-failed" => "operation.failed",
            _ => "operation.terminal",
        }
    } else {
        transition
    };
    let operation_transition = transition.starts_with("operation.");
    let observation = if operation_transition {
        serde_json::to_value(
            load_operation(connection, scope, operation)?
                .ok_or_else(|| StoreError::NotAccepted(operation.to_owned()))?
                .record,
        )?
    } else {
        observation.ok_or_else(|| {
            StoreError::NotAccepted(format!("event {transition} is missing its observation"))
        })?
    };
    append_event_with_cause(
        connection,
        retention,
        scope,
        if operation_transition {
            operation
        } else {
            resource
        },
        if operation_transition {
            "operation"
        } else {
            resource_kind
        },
        transition,
        observed_at,
        actor,
        principal,
        EventCause::Operation {
            operation: operation.to_owned(),
        },
        observation,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn append_control_event(
    connection: &Connection,
    retention: u64,
    scope: &Scope,
    resource: &str,
    resource_kind: &str,
    transition: &str,
    observed_at: &str,
    actor: &str,
    principal: Option<&str>,
    observation: Value,
) -> Result<Event, StoreError> {
    append_event_with_cause(
        connection,
        retention,
        scope,
        resource,
        resource_kind,
        transition,
        observed_at,
        actor,
        principal,
        EventCause::Control {
            control: EventControl::ReconciliationSnapshotCreate,
        },
        observation,
    )
}

#[allow(clippy::too_many_arguments)]
fn append_event_with_cause(
    connection: &Connection,
    retention: u64,
    scope: &Scope,
    resource: &str,
    resource_kind: &str,
    transition: &str,
    observed_at: &str,
    actor: &str,
    principal: Option<&str>,
    cause: EventCause,
    observation: Value,
) -> Result<Event, StoreError> {
    ensure_stream(connection, scope)?;
    let (source_scope, generation, current) = stream_position(connection, scope)?;
    let sequence = current.checked_add(1).ok_or(StoreError::IntegerRange)?;
    let event = Event {
        source_scope,
        generation,
        seq: sequence,
        resource: resource.to_owned(),
        resource_kind: resource_kind.to_owned(),
        transition: transition.to_owned(),
        observed_at: observed_at.parse()?,
        actor: actor.to_owned(),
        principal: principal.map(ToOwned::to_owned),
        cause,
        observation,
    };
    event
        .validate_closed_shape()
        .map_err(|_| StoreError::InvalidEventShape)?;
    connection.execute(
        "INSERT INTO events (deployment, subject, generation, seq, event_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            scope.deployment,
            scope.subject,
            to_i64(generation)?,
            to_i64(sequence)?,
            serde_json::to_string(&event)?
        ],
    )?;
    connection.execute(
        "UPDATE stream_meta SET next_seq = ?3 WHERE deployment = ?1 AND subject = ?2",
        params![
            scope.deployment,
            scope.subject,
            to_i64(sequence.checked_add(1).ok_or(StoreError::IntegerRange)?)?
        ],
    )?;
    let delete_through = sequence.saturating_sub(retention);
    if delete_through > 0 {
        connection.execute(
            "DELETE FROM events WHERE deployment = ?1 AND subject = ?2 AND seq <= ?3",
            params![scope.deployment, scope.subject, to_i64(delete_through)?],
        )?;
    }
    Ok(event)
}

pub(crate) fn commit_effect(scope: &Scope, event: &Event) -> CommitEffect {
    CommitEffect {
        scope: scope.clone(),
        source_scope: event.source_scope.clone(),
        generation: event.generation,
        through_seq: event.seq,
    }
}

pub(crate) fn event_cursor(source_scope: &str, generation: u64, sequence: u64) -> String {
    format!("ev2.{source_scope}.{generation}.{sequence}")
}

fn parse_event_cursor(value: &str) -> Option<(&str, u64, u64)> {
    let mut parts = value.strip_prefix("ev2.")?.split('.');
    let source_scope = parts.next()?;
    let generation = parse_canonical_u64(parts.next()?)?;
    let sequence = parse_canonical_u64(parts.next()?)?;
    (parts.next().is_none() && !source_scope.is_empty()).then_some((
        source_scope,
        generation,
        sequence,
    ))
}

pub(crate) fn parse_canonical_u64(value: &str) -> Option<u64> {
    if value.is_empty()
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return None;
    }
    let parsed: u64 = value.parse().ok()?;
    (parsed.to_string() == value).then_some(parsed)
}
