//! ADR 0053 decision 4: the capability fact `operation.result-max-json-bytes`. Every snapshot
//! states it, a snapshot of bundle `0.28.0` reads it, and a machine an earlier bundle states,
//! which has no such member, still reads, so no check of an earlier bundle changes its answer.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{ADVERTISED_CONTRACT_BUNDLE, CapabilityFacts, OPERATION_RESULT_MAX_JSON_BYTES};

const FACT: &str = "operation.result-max-json-bytes";

fn bundle_file(version: &str, path: &str) -> Value {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts/substrate-wire")
        .join(version)
        .join(path);
    serde_json::from_slice(&std::fs::read(&file).expect("bundle file")).expect("bundle JSON")
}

fn advertised_version() -> &'static str {
    ADVERTISED_CONTRACT_BUNDLE
        .strip_prefix("substrate-wire/")
        .expect("advertised contract prefix")
}

/// The fact is always stated, at the wire constant, under its wire name.
#[test]
fn every_snapshot_states_the_result_bound_at_the_wire_constant() {
    let rendered = serde_json::to_value(CapabilityFacts::default()).expect("facts serialise");
    assert_eq!(rendered[FACT], json!(OPERATION_RESULT_MAX_JSON_BYTES));
    let lowered = CapabilityFacts {
        operation_result_max_json_bytes: 64,
        ..CapabilityFacts::default()
    };
    let rendered = serde_json::to_value(&lowered).expect("facts serialise");
    assert_eq!(rendered[FACT], json!(64));
    let read: CapabilityFacts = serde_json::from_value(rendered).expect("facts read back");
    assert_eq!(read, lowered);
}

/// The bundle the wire crate advertises requires the fact and publishes it at the constant, and
/// its probe vector's snapshot reads back to the facts it states, the fact among them.
#[test]
fn the_advertised_bundle_requires_the_fact_at_the_wire_constant() {
    let version = advertised_version();
    let capability = bundle_file(version, "schemas/capability.json");
    let facts = &capability["properties"]["facts"];
    assert_eq!(
        facts["properties"][FACT]["const"],
        json!(OPERATION_RESULT_MAX_JSON_BYTES),
        "{version}"
    );
    assert!(
        facts["required"]
            .as_array()
            .expect("required facts")
            .contains(&json!(FACT)),
        "{version}"
    );
    let probe = bundle_file(version, "vectors/http/machine-probe.json");
    let stated = probe["expected"]["response"]["body"]["result"]["facts"].clone();
    let read: CapabilityFacts = serde_json::from_value(stated.clone()).expect("probe facts read");
    assert_eq!(
        read.operation_result_max_json_bytes,
        OPERATION_RESULT_MAX_JSON_BYTES
    );
    assert_eq!(serde_json::to_value(&read).expect("serialise"), stated);
}

/// A machine of bundle `0.27.0` states no such member. It reads, as the checks of `0.26.0` and
/// `0.27.0` read their vectors' machines, with the fact at the wire constant, which is what a
/// daemon of that source served.
#[test]
fn a_machine_of_an_earlier_bundle_reads_with_the_fact_at_the_wire_constant() {
    let probe = bundle_file("0.27.0", "vectors/http/machine-probe.json");
    let stated = probe["expected"]["response"]["body"]["result"]["facts"].clone();
    assert!(stated.get(FACT).is_none());
    let read: CapabilityFacts = serde_json::from_value(stated).expect("0.27.0 facts read");
    assert_eq!(
        read.operation_result_max_json_bytes,
        OPERATION_RESULT_MAX_JSON_BYTES
    );
}
