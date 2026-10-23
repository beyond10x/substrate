//! ADR 0042: the caps, lifetime and refusal of each upgraded stream family, published as facts.

use super::{
    AttachmentBoundsFacts, CapabilityFacts, EVENT_STREAM_CAPACITY, EVENT_STREAMS_GLOBAL,
    EVENT_STREAMS_PER_SUBJECT, EVENTS_STREAM_BOUNDS_FACT, METRICS_STREAM_BOUNDS_FACT,
    METRICS_STREAM_CAPACITY, METRICS_STREAMS_GLOBAL, METRICS_STREAMS_PER_SUBJECT,
    SESSION_ATTACHMENT_CAPACITY, SESSION_ATTACHMENTS_GLOBAL, SESSION_ATTACHMENTS_PER_SESSION,
    SESSION_ATTACHMENTS_PER_SUBJECT, SESSIONS_ATTACHMENT_BOUNDS_FACT, StreamBoundsFacts,
    UPGRADED_STREAM_LIFETIME_MS,
};

/// The values ADR 0042 decisions 4 and 6 publish, stated once here so a constant that moves without
/// its ADR fails.
#[test]
fn the_published_stream_bounds_are_the_ones_adr_0042_states() {
    assert_eq!(EVENTS_STREAM_BOUNDS_FACT, "events.stream-bounds");
    assert_eq!(METRICS_STREAM_BOUNDS_FACT, "metrics.stream-bounds");
    assert_eq!(
        SESSIONS_ATTACHMENT_BOUNDS_FACT,
        "sessions.attachment-bounds"
    );
    assert_eq!((EVENT_STREAMS_PER_SUBJECT, EVENT_STREAMS_GLOBAL), (4, 64));
    assert_eq!(
        (METRICS_STREAMS_PER_SUBJECT, METRICS_STREAMS_GLOBAL),
        (4, 64)
    );
    assert_eq!(
        (
            SESSION_ATTACHMENTS_PER_SUBJECT,
            SESSION_ATTACHMENTS_PER_SESSION,
            SESSION_ATTACHMENTS_GLOBAL
        ),
        (16, 1, 32)
    );
    assert_eq!(UPGRADED_STREAM_LIFETIME_MS, 3_600_000);
    assert_eq!(EVENT_STREAM_CAPACITY, "event.stream-capacity");
    assert_eq!(METRICS_STREAM_CAPACITY, "metrics.stream-capacity");
    assert_eq!(SESSION_ATTACHMENT_CAPACITY, "session.attachment-capacity");
}

/// Each fact serializes under its name as the closed object the bundle declares, and is absent
/// unless published.
#[test]
fn the_stream_bound_facts_serialize_under_their_names_and_read_back() {
    let absent = serde_json::to_value(CapabilityFacts::default()).expect("facts");
    for name in [
        EVENTS_STREAM_BOUNDS_FACT,
        METRICS_STREAM_BOUNDS_FACT,
        SESSIONS_ATTACHMENT_BOUNDS_FACT,
    ] {
        assert!(absent.get(name).is_none(), "{name} is absent by default");
    }

    let facts = CapabilityFacts {
        events_stream_bounds: Some(StreamBoundsFacts::events()),
        metrics_stream_bounds: Some(StreamBoundsFacts::metrics()),
        sessions_attachment_bounds: Some(AttachmentBoundsFacts::sessions()),
        ..CapabilityFacts::default()
    };
    let value = serde_json::to_value(&facts).expect("facts");
    assert_eq!(
        value[EVENTS_STREAM_BOUNDS_FACT],
        serde_json::json!({
            "per_subject": 4,
            "global": 64,
            "lifetime_ms": 3_600_000,
            "refusal": "event.stream-capacity",
        })
    );
    assert_eq!(
        value[METRICS_STREAM_BOUNDS_FACT],
        serde_json::json!({
            "per_subject": 4,
            "global": 64,
            "lifetime_ms": 3_600_000,
            "refusal": "metrics.stream-capacity",
        })
    );
    assert_eq!(
        value[SESSIONS_ATTACHMENT_BOUNDS_FACT],
        serde_json::json!({
            "per_subject": 16,
            "per_session": 1,
            "global": 32,
            "lifetime_ms": 3_600_000,
            "refusal": "session.attachment-capacity",
        })
    );
    let back: CapabilityFacts = serde_json::from_value(value).expect("facts read back");
    assert_eq!(back, facts);
}

/// The fact objects are closed: a member the bundle does not declare is refused on read.
#[test]
fn a_stream_bound_fact_with_an_undeclared_member_is_refused() {
    let mut value = serde_json::to_value(StreamBoundsFacts::events()).expect("fact");
    value["per_source"] = serde_json::json!(16);
    assert!(serde_json::from_value::<StreamBoundsFacts>(value).is_err());
    let mut value = serde_json::to_value(AttachmentBoundsFacts::sessions()).expect("fact");
    value["per_source"] = serde_json::json!(16);
    assert!(serde_json::from_value::<AttachmentBoundsFacts>(value).is_err());
}
