//! ADR 0036 S1-S7 and S11: the host-process wire types generated from `spec/host-processes/`, and
//! the hand-written constants, refusal codes, queries and capability fact beside them.

use serde_json::{Value, json};

use super::{
    CapabilityFacts, LEASE_EXPIRED, MAX_LEASE_TTL_MS, MAX_PROCESS_ARGV_BYTES,
    MAX_PROCESS_ARGV_ITEMS, MAX_PROCESS_ENV_BYTES, MAX_PROCESS_ENV_ITEMS, MAX_PROCESS_LIST_ITEMS,
    MAX_PROCESS_LOG_BYTES, MAX_PROCESS_LOG_LINE_BYTES, MAX_PROCESS_LOG_TAIL_BYTES,
    MAX_PROCESS_OWNER_BYTES, MAX_PROCESS_REASON_BYTES, MAX_PROCESS_TIMEOUT_MS,
    MAX_PROCESSES_PER_SUBJECT, MIN_LEASE_TTL_MS, PROCESS_HOST_FACT, PROCESS_REFUSALS,
    PROCESS_RETAIN_ENDED_COUNT, PROCESS_RETAIN_ENDED_MS, PROCESS_SHARED_REFUSALS,
    PROCESS_STOP_GRACE_MS, ProcessConfinement, ProcessCpu, ProcessDeadline, ProcessDeadlineSource,
    ProcessEndedBy, ProcessEnvDigest, ProcessExit, ProcessKind, ProcessLease, ProcessLeaseState,
    ProcessListQuery, ProcessLogLine, ProcessLogStream, ProcessLogStreamSelection, ProcessLogTail,
    ProcessLogsQuery, ProcessOwner, ProcessPresence, ProcessProfile, ProcessRecord,
    ProcessStartInput, ProcessState, ProcessStopByOwnerInput, ProcessStopByOwnerResult,
    ProcessStopped, ProcessTimestamp, process_refusal,
};

fn at(text: &str) -> ProcessTimestamp {
    ProcessTimestamp::from(
        text.parse::<chrono::DateTime<chrono::Utc>>()
            .expect("fixed instant"),
    )
}

fn running_record() -> ProcessRecord {
    ProcessRecord {
        id: "proc_01".to_owned(),
        kind: Box::new(ProcessKind::Process),
        owner: ProcessPresence::Present(Box::new(ProcessOwner {
            session: "s-1".to_owned(),
            name: "dev-server".to_owned(),
        })),
        reason: "serve the preview".to_owned(),
        profile: Box::new(ProcessProfile::Host),
        confinement: Box::new(ProcessConfinement::None),
        argv: vec!["npm".to_owned(), "run".to_owned(), "dev".to_owned()],
        cwd: "/srv/app".to_owned(),
        env_names: vec!["HOME".to_owned(), "PATH".to_owned()],
        env_digest: Box::new(ProcessEnvDigest(format!("sha256:{}", "a".repeat(64)))),
        state: Box::new(ProcessState::Running),
        started_at: at("2026-10-17T09:00:00Z"),
        observed_at: at("2026-10-17T09:00:05Z"),
        lease: Box::new(ProcessLease {
            keep: false,
            keep_reason: ProcessPresence::Absent,
            ttl_ms: ProcessPresence::Present(3_600_000.into()),
            renew_by: ProcessPresence::Present(at("2026-10-17T10:00:00Z")),
            state: ProcessPresence::Present(Box::new(ProcessLeaseState::Active)),
        }),
        timeout_ms: ProcessPresence::Present(600_000.into()),
        expires_at: ProcessPresence::Absent,
        deadline: ProcessPresence::Present(Box::new(ProcessDeadline {
            at: at("2026-10-17T09:10:00Z"),
            set_by: Box::new(ProcessDeadlineSource::Timeout),
        })),
        ended_by: ProcessPresence::Absent,
        exit: ProcessPresence::Absent,
        cpu: ProcessPresence::Present(Box::new(ProcessCpu {
            usage_usec: 1_250.into(),
            observed_at: at("2026-10-17T09:00:05Z"),
        })),
        memory: ProcessPresence::Absent,
        listening: ProcessPresence::Absent,
    }
}

#[test]
fn start_input_reads_the_adr_shape() {
    let input: ProcessStartInput = serde_json::from_value(json!({
        "profile": "host",
        "argv": ["npm", "run", "dev"],
        "cwd": "/srv/app",
        "env": {"PATH": "/usr/bin", "HOME": "/var/lib/dev"},
        "owner": {"session": "s-1", "name": "dev-server"},
        "reason": "serve the preview",
        "lease": {"ttl_ms": 3_600_000},
        "timeout_ms": 600_000,
        "expires_at": "2026-10-17T12:00:00Z"
    }))
    .expect("the ADR 0036 S1 request shape");
    assert_eq!(input.profile, ProcessPresence::Present("host".to_owned()));
    let ProcessPresence::Present(env) = &input.env else {
        panic!("env is present");
    };
    assert_eq!(env.ess_extra.len(), 2);
    let ProcessPresence::Present(lease) = &input.lease else {
        panic!("lease is present");
    };
    assert_eq!(lease.ttl_ms, ProcessPresence::Present(3_600_000.into()));
    assert!(lease.keep.is_absent());
}

#[test]
fn start_input_keep_lease_carries_its_own_reason() {
    let input: ProcessStartInput = serde_json::from_value(json!({
        "lease": {"keep": true, "keep_reason": "shared database for the week"}
    }))
    .expect("a keep lease");
    let ProcessPresence::Present(lease) = input.lease else {
        panic!("lease is present");
    };
    assert_eq!(lease.keep, ProcessPresence::Present(true));
    assert_eq!(
        lease.keep_reason,
        ProcessPresence::Present("shared database for the week".to_owned())
    );
    assert!(lease.ttl_ms.is_absent());
}

/// Absence must decode, so that the daemon answers the named refusal of each member rather than
/// the generic schema refusal (`process.profile-required` above all: S2 is requested, never
/// defaulted).
#[test]
fn start_input_without_members_decodes_so_named_refusals_are_reachable() {
    let input: ProcessStartInput = serde_json::from_value(json!({})).expect("empty input");
    assert!(input.profile.is_absent());
    assert!(input.argv.is_absent());
    assert!(input.cwd.is_absent());
    assert!(input.env.is_absent());
    assert!(input.owner.is_absent());
    assert!(input.reason.is_absent());
    assert!(input.lease.is_absent());
    // An unknown profile is text, so it reaches `process.profile-unknown`.
    let input: ProcessStartInput =
        serde_json::from_value(json!({"profile": "sandbox"})).expect("unknown profile");
    assert_eq!(
        input.profile,
        ProcessPresence::Present("sandbox".to_owned())
    );
    // An expires_at that is not an RFC 3339 instant is text, so it reaches
    // `process.expires-at-invalid`.
    let input: ProcessStartInput =
        serde_json::from_value(json!({"expires_at": "tomorrow"})).expect("unparsed instant");
    assert_eq!(
        input.expires_at,
        ProcessPresence::Present("tomorrow".to_owned())
    );
}

#[test]
fn start_input_refuses_an_unknown_member() {
    let error = serde_json::from_value::<ProcessStartInput>(json!({
        "profile": "host",
        "shell": true
    }))
    .expect_err("closed input");
    assert!(error.to_string().contains("unknown field"), "{error}");
    let error = serde_json::from_value::<ProcessStartInput>(json!({
        "owner": {"session": "s-1", "name": "n", "uid": 0}
    }))
    .expect_err("closed owner");
    assert!(error.to_string().contains("unknown field"), "{error}");
}

#[test]
fn record_reports_host_profile_and_no_confinement() {
    let value = serde_json::to_value(running_record()).expect("record serializes");
    assert_eq!(value["kind"], "process");
    assert_eq!(value["profile"], "host");
    assert_eq!(value["confinement"], "none");
    assert_eq!(value["state"], "running");
    assert_eq!(
        value["owner"],
        json!({"session": "s-1", "name": "dev-server"})
    );
    assert_eq!(value["lease"]["state"], "active");
    assert_eq!(value["deadline"]["set_by"], "timeout");
    assert_eq!(value["deadline"]["at"], "2026-10-17T09:10:00Z");
    assert_eq!(value["cpu"]["usage_usec"], 1_250);
    let object = value.as_object().expect("object");
    // Absent, never zero: an observation the daemon could not make is not on the record.
    assert!(!object.contains_key("memory"));
    assert!(!object.contains_key("listening"));
    assert!(!object.contains_key("ended_by"));
    assert!(!object.contains_key("exit"));
    // Environment values are never on a record; only the names and one digest.
    assert!(!object.contains_key("env"));
    assert_eq!(value["env_names"], json!(["HOME", "PATH"]));
    let back: ProcessRecord = serde_json::from_value(value).expect("record reads back");
    assert_eq!(back, running_record());
}

#[test]
fn ended_record_names_one_cause_and_a_code_or_a_signal() {
    let mut record = running_record();
    record.state = Box::new(ProcessState::Ended);
    record.ended_by = ProcessPresence::Present(Box::new(ProcessEndedBy::Timeout));
    record.exit = ProcessPresence::Present(Box::new(ProcessExit {
        code: ProcessPresence::Absent,
        signal: ProcessPresence::Present("TERM".to_owned()),
    }));
    record.cpu = ProcessPresence::Absent;
    let value = serde_json::to_value(record).expect("record serializes");
    assert_eq!(value["state"], "ended");
    assert_eq!(value["ended_by"], "timeout");
    assert_eq!(value["exit"], json!({"signal": "TERM"}));
}

#[test]
fn closed_vocabularies_have_the_adr_spelling() {
    let spell = |value: Value| value.as_str().expect("string").to_owned();
    let states = [
        ProcessState::Starting,
        ProcessState::Running,
        ProcessState::Stopping,
        ProcessState::Ended,
    ]
    .map(|state| spell(serde_json::to_value(state).expect("state")));
    assert_eq!(states, ["starting", "running", "stopping", "ended"]);
    let causes = [
        ProcessEndedBy::Exit,
        ProcessEndedBy::Stop,
        ProcessEndedBy::LeaseExpiry,
        ProcessEndedBy::Timeout,
        ProcessEndedBy::ExpiresAt,
        ProcessEndedBy::Vanished,
    ]
    .map(|cause| spell(serde_json::to_value(cause).expect("cause")));
    assert_eq!(
        causes,
        [
            "exit",
            "stop",
            "lease-expiry",
            "timeout",
            "expires-at",
            "vanished"
        ]
    );
    for state in [
        ProcessState::Starting,
        ProcessState::Running,
        ProcessState::Stopping,
        ProcessState::Ended,
    ] {
        assert_eq!(serde_json::to_value(state).expect("state"), state.as_str());
    }
    for cause in [
        ProcessEndedBy::Exit,
        ProcessEndedBy::Stop,
        ProcessEndedBy::LeaseExpiry,
        ProcessEndedBy::Timeout,
        ProcessEndedBy::ExpiresAt,
        ProcessEndedBy::Vanished,
    ] {
        assert_eq!(serde_json::to_value(cause).expect("cause"), cause.as_str());
    }
    let selections = [
        ProcessLogStreamSelection::Stdout,
        ProcessLogStreamSelection::Stderr,
        ProcessLogStreamSelection::Both,
    ]
    .map(|selection| spell(serde_json::to_value(selection).expect("selection")));
    assert_eq!(selections, ["stdout", "stderr", "both"]);
    assert_eq!(
        serde_json::to_value(ProcessDeadlineSource::ExpiresAt).expect("source"),
        "expires-at"
    );
    assert_eq!(
        serde_json::to_value(ProcessLeaseState::Expiring).expect("lease state"),
        "expiring"
    );
}

/// ADR 0036 § Named refusals, every slice of this surface (S1-S7, S11), with its status. S8, S9
/// and S12 add their codes with their routes; none is reachable here.
#[test]
fn process_refusals_are_the_adr_table_for_this_surface() {
    let table: Vec<(&str, u16)> = PROCESS_REFUSALS
        .iter()
        .map(|refusal| (refusal.code, refusal.status))
        .collect();
    assert_eq!(
        table,
        [
            ("process.host-unserved", 501),
            ("process.host-posture-refused", 403),
            ("process.host-subject-mismatch", 403),
            ("process.profile-required", 422),
            ("process.profile-unknown", 422),
            ("process.argv-invalid", 422),
            ("process.cwd-invalid", 422),
            ("process.env-invalid", 422),
            ("process.owner-invalid", 422),
            ("process.reason-required", 422),
            ("process.name-conflict", 409),
            ("process.lease-invalid", 422),
            ("process.keep-reason-required", 422),
            ("process.limit-exceeded", 429),
            ("process.not-found", 404),
            ("process.timeout-invalid", 422),
            ("process.expires-at-invalid", 422),
        ]
    );
    for refusal in PROCESS_REFUSALS {
        assert_eq!(process_refusal(refusal.code), Some(refusal));
        let class = serde_json::to_value(refusal.class).expect("class");
        let expected = match refusal.status {
            501 => "unserved",
            409 => "conflict",
            429 => "exhausted",
            _ => "refused",
        };
        assert_eq!(class, expected, "{}", refusal.code);
        assert!(!refusal.message.is_empty());
    }
    for deferred in [
        "process.container-unserved",
        "process.container-restart-refused",
        "process.container-label-reserved",
        "process.adopt-target-absent",
        "process.adopt-target-foreign",
        "process.adopt-unmovable",
        "process.events-readers-exceeded",
    ] {
        assert_eq!(process_refusal(deferred), None, "{deferred}");
    }
}

/// `process.lease.renew` declares the shared `lease.expired` with the status, class and message the
/// workspace and exec lease renewals already answer; it is not a `process.*` code.
#[test]
fn process_shared_refusals_are_the_lease_renewal_codes() {
    let table: Vec<(&str, u16, Value)> = PROCESS_SHARED_REFUSALS
        .iter()
        .map(|refusal| {
            (
                refusal.code,
                refusal.status,
                serde_json::to_value(refusal.class).expect("class"),
            )
        })
        .collect();
    assert_eq!(table, [(LEASE_EXPIRED, 409, json!("conflict"))]);
    assert_eq!(LEASE_EXPIRED, "lease.expired");
    assert_eq!(
        PROCESS_SHARED_REFUSALS[0].message,
        "An expired lease cannot be renewed."
    );
    assert_eq!(process_refusal(LEASE_EXPIRED), None);
}

#[test]
fn process_host_fact_is_absent_unless_published() {
    assert_eq!(PROCESS_HOST_FACT, "process.host");
    let absent = serde_json::to_value(CapabilityFacts::default()).expect("facts");
    assert!(absent.get(PROCESS_HOST_FACT).is_none());
    let facts = CapabilityFacts {
        process_host: Some(true),
        ..CapabilityFacts::default()
    };
    let value = serde_json::to_value(&facts).expect("facts");
    assert_eq!(value[PROCESS_HOST_FACT], true);
    let back: CapabilityFacts = serde_json::from_value(value).expect("facts read back");
    assert_eq!(back.process_host, Some(true));
}

#[test]
fn bounds_are_coherent() {
    // Spec point 14: the log bound is bytes, and a tail never asks for more than the log keeps.
    assert_eq!(MAX_PROCESS_LOG_BYTES, 8 * 1024 * 1024);
    const {
        assert!(MAX_PROCESS_LOG_TAIL_BYTES <= MAX_PROCESS_LOG_BYTES);
        assert!(MAX_PROCESS_LOG_LINE_BYTES as u64 <= MAX_PROCESS_LOG_TAIL_BYTES);
        assert!(MAX_PROCESS_TIMEOUT_MS >= MIN_LEASE_TTL_MS);
        assert!(MAX_PROCESS_TIMEOUT_MS <= MAX_LEASE_TTL_MS);
        assert!(MAX_PROCESS_ARGV_ITEMS > 0 && MAX_PROCESS_ARGV_BYTES > 0);
        assert!(MAX_PROCESS_ENV_ITEMS > 0 && MAX_PROCESS_ENV_BYTES > 0);
        assert!(MAX_PROCESS_OWNER_BYTES > 0 && MAX_PROCESS_REASON_BYTES > 0);
        assert!(MAX_PROCESSES_PER_SUBJECT > 0);
        assert!(MAX_PROCESS_LIST_ITEMS > 0);
        assert!(PROCESS_RETAIN_ENDED_COUNT > 0 && PROCESS_RETAIN_ENDED_MS > 0);
        assert!(PROCESS_STOP_GRACE_MS > 0);
    }
}

#[test]
fn list_and_logs_queries_read_from_a_query_string() {
    let query: ProcessListQuery =
        serde_urlencoded::from_str("owner_session=s-1&name=dev&state=running&limit=10")
            .expect("list query");
    assert_eq!(query.owner_session.as_deref(), Some("s-1"));
    assert_eq!(query.name.as_deref(), Some("dev"));
    assert_eq!(query.state, Some(ProcessState::Running));
    assert_eq!(query.limit, Some(10));
    assert_eq!(query.cursor, None);
    let empty: ProcessListQuery = serde_urlencoded::from_str("").expect("no filters");
    assert_eq!(empty, ProcessListQuery::default());
    assert!(serde_urlencoded::from_str::<ProcessListQuery>("state=Running").is_err());
    assert!(serde_urlencoded::from_str::<ProcessListQuery>("owner=s-1").is_err());

    let logs: ProcessLogsQuery =
        serde_urlencoded::from_str("stream=stderr&max_bytes=4096").expect("logs query");
    assert_eq!(logs.stream, Some(ProcessLogStreamSelection::Stderr));
    assert_eq!(logs.max_bytes, Some(4_096));
    let logs: ProcessLogsQuery = serde_urlencoded::from_str("").expect("defaults");
    assert_eq!(logs.selection(), ProcessLogStreamSelection::Both);
    assert_eq!(logs.budget(), MAX_PROCESS_LOG_TAIL_BYTES);
}

#[test]
fn stop_by_owner_and_log_tail_have_the_adr_shape() {
    let input: ProcessStopByOwnerInput =
        serde_json::from_value(json!({"owner_session": "s-1", "except_keep": true}))
            .expect("stop by owner");
    assert_eq!(input.except_keep, ProcessPresence::Present(true));
    let result = ProcessStopByOwnerResult {
        owner_session: "s-1".to_owned(),
        stopped: vec![Box::new(ProcessStopped {
            process: "proc_01".to_owned(),
            operation: "op-1".to_owned(),
        })],
    };
    assert_eq!(
        serde_json::to_value(result).expect("result"),
        json!({"owner_session": "s-1", "stopped": [{"process": "proc_01", "operation": "op-1"}]})
    );
    let tail = ProcessLogTail {
        process: "proc_01".to_owned(),
        profile: Box::new(ProcessProfile::Host),
        confinement: Box::new(ProcessConfinement::None),
        stream: Box::new(ProcessLogStreamSelection::Both),
        lines: vec![Box::new(ProcessLogLine {
            stream: Box::new(ProcessLogStream::Stderr),
            text: "listening on 5173".to_owned(),
            lossy: false,
            truncated: false,
            full_bytes: 17.into(),
        })],
        discarded_bytes: 4_096.into(),
        retained_bytes: 18.into(),
        observed_at: at("2026-10-17T09:00:05Z"),
    };
    let value = serde_json::to_value(tail).expect("tail");
    assert_eq!(value["confinement"], "none");
    assert_eq!(value["lines"][0]["stream"], "stderr");
    assert_eq!(value["discarded_bytes"], 4_096);
}
