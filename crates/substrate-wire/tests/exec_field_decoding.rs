#![forbid(unsafe_code)]
//! Design 27, amendment 3: the wire type refuses the exec-field values the published schema
//! refuses, so the daemon answers them `request.schema-invalid` on exec start and session start.

use serde_json::{Value, json};
use substrate_wire::{ExecStartInput, PipeSessionStartInput, StorageLimit};

fn exec() -> Value {
    json!({
        "workspace": "ws_check",
        "argv": ["/usr/bin/true"],
        "env": { "allow": [], "set": {} },
        "sandbox": {
            "capability_snapshot": format!("sha256:{}", "0".repeat(64)),
            "profile": "workspace",
            "network": "none",
            "require": true
        },
        "limits": {
            "timeout_ms": 1000,
            "output_bytes": 1024,
            "processes": 1,
            "memory_bytes": 1_048_576,
            "cpu_millis": 1000
        },
        "wait": false,
        "lease_ttl_ms": 60_000
    })
}

fn with(field: &str, value: Value) -> Value {
    let mut exec = exec();
    exec[field] = value;
    exec
}

fn session(exec: &Value) -> Value {
    json!({
        "exec": exec,
        "input_limit_bytes": 1024,
        "frame_limit_bytes": 1024,
        "queued_frames": 1
    })
}

/// Decodes on both starts; both must agree.
fn decodes(exec: &Value) -> bool {
    let start = serde_json::from_value::<ExecStartInput>(exec.clone()).is_ok();
    let in_session = serde_json::from_value::<PipeSessionStartInput>(session(exec)).is_ok();
    assert_eq!(
        start, in_session,
        "exec and session starts disagree on {exec}"
    );
    start
}

#[test]
fn control_an_absent_or_well_formed_field_decodes() {
    assert!(decodes(&exec()));
    assert!(decodes(&with(
        "scratch",
        json!({ "max_bytes": 1_048_576, "max_inodes": 16 })
    )));
    assert!(decodes(&with("measurements", json!([]))));
    assert!(decodes(&with("measurements", json!(["resource-usage"]))));
}

#[test]
fn a_null_scratch_is_refused_rather_than_read_as_absent() {
    assert!(!decodes(&with("scratch", Value::Null)));
}

#[test]
fn a_repeated_measurement_is_refused_rather_than_collapsed() {
    assert!(!decodes(&with(
        "measurements",
        json!(["resource-usage", "resource-usage"])
    )));
}

#[test]
fn a_null_measurements_list_stays_refused() {
    assert!(!decodes(&with("measurements", Value::Null)));
}

#[test]
fn a_scratch_quota_off_the_1024_byte_unit_is_outside_the_contract_bounds() {
    let aligned = StorageLimit {
        max_bytes: 1_048_576,
        max_inodes: 16,
    };
    assert!(aligned.within_contract_bounds());
    let misaligned = StorageLimit {
        max_bytes: 1_048_577,
        max_inodes: 16,
    };
    assert!(!misaligned.within_contract_bounds());
}
