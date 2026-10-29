//! ADR 0047 and design 34 § 11: the wire half of bundle `0.25.0`. A second driver kind, the
//! mechanism-neutral no-egress fact, and the three refusals a driver that cannot serve a request
//! member or a route answers by name.

use super::{
    CONFINEMENT_NO_EGRESS_FACT, CapabilityFacts, CapabilitySnapshot, DRIVER_UNSERVED_REFUSALS,
    EXEC_INLINE_CAPSULE_UNSERVED, EXEC_SIGNAL_UNSERVED, HostDriverKind, UnservedRefusal,
    WORKSPACE_ADOPT_UNSERVED,
};

/// N7 A: `driver` names the backend that answered, and `kubernetes` is its own word.
#[test]
fn each_driver_kind_has_its_own_wire_word() {
    assert_eq!(
        serde_json::to_value(HostDriverKind::Host).expect("serialises"),
        serde_json::json!("host")
    );
    assert_eq!(
        serde_json::to_value(HostDriverKind::Kubernetes).expect("serialises"),
        serde_json::json!("kubernetes")
    );
    assert_eq!(
        serde_json::from_value::<HostDriverKind>(serde_json::json!("kubernetes"))
            .expect("deserialises"),
        HostDriverKind::Kubernetes
    );
    assert!(serde_json::from_value::<HostDriverKind>(serde_json::json!("pod")).is_err());
}

/// `ALL` is what the bundle check enumerates, so a variant missing from it would be a word the
/// daemon can emit and the bundle never publishes. The match fails to compile when a variant is
/// added without this test, and the length check fails when `ALL` is not extended with it.
#[test]
fn every_driver_kind_is_enumerated() {
    let counted = HostDriverKind::ALL
        .iter()
        .map(|kind| match kind {
            HostDriverKind::Host | HostDriverKind::Kubernetes => 1,
        })
        .sum::<usize>();
    assert_eq!(counted, 2);
    assert_eq!(
        HostDriverKind::ALL,
        [HostDriverKind::Host, HostDriverKind::Kubernetes]
    );
}

/// N5 A: the fact is absent by default and from the document, so a host snapshot's bytes, and so
/// its digest, do not move. Published, it carries its own wire name.
#[test]
fn the_no_egress_fact_is_absent_by_default_and_carries_its_wire_name() {
    assert_eq!(CONFINEMENT_NO_EGRESS_FACT, "confinement.no-egress");
    let facts = CapabilityFacts::default();
    assert_eq!(facts.confinement_no_egress, None);
    let rendered = serde_json::to_value(&facts).expect("facts serialise");
    assert!(rendered.get(CONFINEMENT_NO_EGRESS_FACT).is_none());
    let published = CapabilityFacts {
        confinement_no_egress: Some(true),
        ..CapabilityFacts::default()
    };
    let rendered = serde_json::to_value(&published).expect("facts serialise");
    assert_eq!(
        rendered[CONFINEMENT_NO_EGRESS_FACT],
        serde_json::json!(true)
    );
    let read: CapabilityFacts = serde_json::from_value(rendered).expect("facts deserialise");
    assert_eq!(read.confinement_no_egress, Some(true));
}

/// A namespace snapshot names its driver and states the neutral fact, never a Linux one.
#[test]
fn a_namespace_snapshot_round_trips() {
    let snapshot = CapabilitySnapshot {
        snapshot: format!("sha256:{}", "7".repeat(64)),
        driver: HostDriverKind::Kubernetes,
        driver_version: "fixture-1".to_owned(),
        config_generation: 7,
        probed_at: "2026-08-13T12:00:00Z".parse().expect("timestamp"),
        valid_until: None,
        facts: CapabilityFacts {
            confinement_no_egress: Some(true),
            ..CapabilityFacts::default()
        },
    };
    let rendered = serde_json::to_value(&snapshot).expect("snapshot serialises");
    assert_eq!(rendered["driver"], "kubernetes");
    assert!(rendered["facts"].get("exec.no-egress").is_none());
    let read: CapabilitySnapshot = serde_json::from_value(rendered).expect("snapshot reads");
    assert_eq!(read, snapshot);
}

/// ADR 0047 decision 12 and design 34 § 14 Q3 A: the three codes, each on its own operation and
/// address, unserved and final. The bundle check holds every published vector to this table.
#[test]
fn the_driver_unserved_refusals_are_the_three_design_34_names() {
    assert_eq!(EXEC_INLINE_CAPSULE_UNSERVED, "exec.inline-capsule-unserved");
    assert_eq!(EXEC_SIGNAL_UNSERVED, "exec.signal-unserved");
    assert_eq!(WORKSPACE_ADOPT_UNSERVED, "workspace.adopt-unserved");
    let rows: Vec<(&str, &str, Option<&str>)> = DRIVER_UNSERVED_REFUSALS
        .iter()
        .map(|refusal| (refusal.operation, refusal.code, refusal.address))
        .collect();
    assert_eq!(
        rows,
        [
            ("exec.start", EXEC_INLINE_CAPSULE_UNSERVED, Some("capsule")),
            ("exec.signal", EXEC_SIGNAL_UNSERVED, Some("signal")),
            ("workspace.adopt", WORKSPACE_ADOPT_UNSERVED, None),
        ]
    );
    for refusal in DRIVER_UNSERVED_REFUSALS {
        assert_eq!(UnservedRefusal::STATUS, 501);
        assert_eq!(UnservedRefusal::CLASS, "unserved");
        assert!(!refusal.message.is_empty() && refusal.message.len() <= 1024);
        assert!(refusal.message.ends_with('.'), "{}", refusal.message);
    }
}
