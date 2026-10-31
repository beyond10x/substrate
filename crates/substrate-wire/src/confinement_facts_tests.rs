//! Design 30 and ADR 0048: the wire half of bundle `0.26.0`. The families and mechanism-neutral
//! properties a confinement adapter proved, as optional facts; the `sandbox.requires` a start may
//! carry; the applied record's optional `cgroup` and its `requires` echo; and the two refusals a
//! daemon answers by name for a requirement the admitted snapshot does not meet.

use super::{
    AppliedConfinement, AppliedFilesystem, AppliedNetwork, CONFINEMENT_FACT_UNSERVED,
    CONFINEMENT_FAMILY_UNSERVED, CONFINEMENT_UNSERVED_REFUSALS, CapabilityFacts, ConfinementFamily,
    ConfinementProperty, ConfinementRequest, ConfinementRequires, SandboxProfile, UnservedRefusal,
    WireValidationError, WorkspaceAccess, confinement_requires_refusal,
    validate_confinement_requires,
};

/// ADR 0044 decision 1's order, kebab-case on the wire, each with its fact name.
const FAMILIES: [(&str, &str); 10] = [
    ("process-run", "confinement.family.process-run"),
    ("process-watch", "confinement.family.process-watch"),
    ("output-read", "confinement.family.output-read"),
    ("wait", "confinement.family.wait"),
    ("workspace", "confinement.family.workspace"),
    ("filesystem-view", "confinement.family.filesystem-view"),
    ("network-egress", "confinement.family.network-egress"),
    ("network-ingress", "confinement.family.network-ingress"),
    ("limits", "confinement.family.limits"),
    ("audit", "confinement.family.audit"),
];

/// Design 30 § 4.3's seven, in the `Fact` order of `spec/confinement/confinement.yaml`.
const PROPERTIES: [(&str, &str); 7] = [
    ("paths-are-host-paths", "confinement.paths-are-host-paths"),
    ("whole-tree-kill", "confinement.whole-tree-kill"),
    ("ipc-socket-allowlist", "confinement.ipc-socket-allowlist"),
    ("ipc-signals-contained", "confinement.ipc-signals-contained"),
    ("sealed-paths", "confinement.sealed-paths"),
    ("exec-subpaths", "confinement.exec-subpaths"),
    ("private-tmpdir", "confinement.private-tmpdir"),
];

#[test]
fn every_family_has_its_wire_word_and_fact_name_in_adr_0044_order() {
    let published: Vec<(String, &str)> = ConfinementFamily::ALL
        .iter()
        .map(|family| {
            let word = serde_json::to_value(family).expect("serialises");
            assert_eq!(word, serde_json::json!(family.name()));
            (family.name().to_owned(), family.fact())
        })
        .collect();
    let expected: Vec<(String, &str)> = FAMILIES
        .iter()
        .map(|(name, fact)| ((*name).to_owned(), *fact))
        .collect();
    assert_eq!(published, expected);
    // The match fails to compile when a variant is added without this test.
    let counted = ConfinementFamily::ALL
        .iter()
        .map(|family| match family {
            ConfinementFamily::ProcessRun
            | ConfinementFamily::ProcessWatch
            | ConfinementFamily::OutputRead
            | ConfinementFamily::Wait
            | ConfinementFamily::Workspace
            | ConfinementFamily::FilesystemView
            | ConfinementFamily::NetworkEgress
            | ConfinementFamily::NetworkIngress
            | ConfinementFamily::Limits
            | ConfinementFamily::Audit => 1,
        })
        .sum::<usize>();
    assert_eq!(counted, FAMILIES.len());
}

#[test]
fn every_property_has_its_wire_word_and_fact_name_in_the_fact_order() {
    let published: Vec<(String, &str)> = ConfinementProperty::ALL
        .iter()
        .map(|property| {
            let word = serde_json::to_value(property).expect("serialises");
            assert_eq!(word, serde_json::json!(property.name()));
            (property.name().to_owned(), property.fact())
        })
        .collect();
    let expected: Vec<(String, &str)> = PROPERTIES
        .iter()
        .map(|(name, fact)| ((*name).to_owned(), *fact))
        .collect();
    assert_eq!(published, expected);
    let counted = ConfinementProperty::ALL
        .iter()
        .map(|property| match property {
            ConfinementProperty::PathsAreHostPaths
            | ConfinementProperty::WholeTreeKill
            | ConfinementProperty::IpcSocketAllowlist
            | ConfinementProperty::IpcSignalsContained
            | ConfinementProperty::SealedPaths
            | ConfinementProperty::ExecSubpaths
            | ConfinementProperty::PrivateTmpdir => 1,
        })
        .sum::<usize>();
    assert_eq!(counted, PROPERTIES.len());
}

/// D2: absent by default and from the document, so a snapshot that states none of them keeps its
/// bytes and its digest.
#[test]
fn no_confinement_fact_is_written_when_none_is_proved() {
    let rendered = serde_json::to_value(CapabilityFacts::default()).expect("facts serialise");
    for (_, fact) in FAMILIES.iter().chain(PROPERTIES.iter()) {
        assert!(rendered.get(*fact).is_none(), "{fact} written while absent");
    }
    for family in ConfinementFamily::ALL {
        assert!(!CapabilityFacts::default().serves_family(family));
    }
    for property in ConfinementProperty::ALL {
        assert!(!CapabilityFacts::default().proves(property));
    }
}

/// Each fact is `true` under its own name and read back into its own field, and no other.
#[test]
fn each_published_fact_round_trips_under_its_own_name() {
    for family in ConfinementFamily::ALL {
        let facts: CapabilityFacts = serde_json::from_value({
            let mut value = serde_json::to_value(CapabilityFacts::default()).expect("serialises");
            value[family.fact()] = serde_json::json!(true);
            value
        })
        .expect("facts deserialise");
        for other in ConfinementFamily::ALL {
            assert_eq!(facts.serves_family(other), other == family, "{other:?}");
        }
        let rendered = serde_json::to_value(&facts).expect("serialises");
        assert_eq!(rendered[family.fact()], serde_json::json!(true));
    }
    for property in ConfinementProperty::ALL {
        let facts: CapabilityFacts = serde_json::from_value({
            let mut value = serde_json::to_value(CapabilityFacts::default()).expect("serialises");
            value[property.fact()] = serde_json::json!(true);
            value
        })
        .expect("facts deserialise");
        for other in ConfinementProperty::ALL {
            assert_eq!(facts.proves(other), other == property, "{other:?}");
        }
        let rendered = serde_json::to_value(&facts).expect("serialises");
        assert_eq!(rendered[property.fact()], serde_json::json!(true));
    }
}

fn sandbox(requires: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "capability_snapshot": format!("sha256:{}", "7".repeat(64)),
        "network": "none",
        "profile": "workspace",
        "require": true,
        "requires": requires,
    })
}

/// D4: the closed object reads, and an absent one is written as before, so an unchanged start
/// hashes as it always did.
#[test]
fn a_start_may_require_families_and_facts_and_one_without_keeps_its_bytes() {
    let request: ConfinementRequest = serde_json::from_value(sandbox(&serde_json::json!({
        "families": ["process-run", "limits"],
        "facts": ["whole-tree-kill"],
    })))
    .expect("requires reads");
    assert_eq!(
        request.requires,
        Some(ConfinementRequires {
            families: Some(vec![
                ConfinementFamily::ProcessRun,
                ConfinementFamily::Limits
            ]),
            facts: Some(vec![ConfinementProperty::WholeTreeKill]),
        })
    );
    assert_eq!(
        validate_confinement_requires(&request.requires.clone().expect("requires")),
        Ok(())
    );
    let without = ConfinementRequest {
        requires: None,
        ..request
    };
    let rendered = serde_json::to_value(&without).expect("serialises");
    assert!(rendered.get("requires").is_none());
    assert_eq!(
        rendered,
        serde_json::json!({
            "capability_snapshot": format!("sha256:{}", "7".repeat(64)),
            "network": "none",
            "profile": "workspace",
            "require": true,
        })
    );
}

/// D4: an unknown name, a member outside the closed object, a prefixed name and a property named
/// as a family are refused when the start is read; the daemon answers `request.schema-invalid`.
#[test]
fn an_unknown_or_prefixed_name_or_an_unknown_member_is_not_read() {
    for requires in [
        serde_json::json!({"families": ["kernel"]}),
        serde_json::json!({"families": ["confinement.family.limits"]}),
        serde_json::json!({"families": ["whole-tree-kill"]}),
        serde_json::json!({"facts": ["process-run"]}),
        serde_json::json!({"facts": ["declared-insecure"]}),
        serde_json::json!({"facts": ["confinement.whole-tree-kill"]}),
        serde_json::json!({"adapter": "linux"}),
        serde_json::json!({"families": "limits"}),
    ] {
        assert!(
            serde_json::from_value::<ConfinementRequest>(sandbox(&requires)).is_err(),
            "{requires} was read"
        );
    }
}

/// D4: the shared validator refuses an empty requirement and a duplicate; the daemon answers
/// both `request.schema-invalid` (422).
#[test]
fn the_validator_refuses_an_empty_object_an_empty_list_and_a_duplicate() {
    for (requires, expected) in [
        (
            serde_json::json!({}),
            WireValidationError::ConfinementRequiresEmpty,
        ),
        (
            serde_json::json!({"families": []}),
            WireValidationError::ConfinementRequiresEmpty,
        ),
        (
            serde_json::json!({"facts": [], "families": ["wait"]}),
            WireValidationError::ConfinementRequiresEmpty,
        ),
        (
            serde_json::json!({"families": ["wait", "audit", "wait"]}),
            WireValidationError::ConfinementRequiresDuplicate,
        ),
        (
            serde_json::json!({"facts": ["sealed-paths", "sealed-paths"]}),
            WireValidationError::ConfinementRequiresDuplicate,
        ),
    ] {
        let request: ConfinementRequest =
            serde_json::from_value(sandbox(&requires)).expect("the shape reads");
        assert_eq!(
            validate_confinement_requires(&request.requires.expect("requires")),
            Err(expected),
            "{requires}"
        );
    }
    let every = ConfinementRequires {
        families: Some(ConfinementFamily::ALL.to_vec()),
        facts: Some(ConfinementProperty::ALL.to_vec()),
    };
    assert_eq!(validate_confinement_requires(&every), Ok(()));
}

#[test]
fn the_two_refusals_are_adr_0044_codes_at_501() {
    assert_eq!(CONFINEMENT_FAMILY_UNSERVED, "confinement.family-unserved");
    assert_eq!(CONFINEMENT_FACT_UNSERVED, "confinement.fact-unserved");
    let rows: Vec<(&str, &str, Option<&str>)> = CONFINEMENT_UNSERVED_REFUSALS
        .iter()
        .map(|refusal| (refusal.operation, refusal.code, refusal.address))
        .collect();
    assert_eq!(
        rows,
        [
            (
                "exec.start",
                CONFINEMENT_FAMILY_UNSERVED,
                Some("sandbox.requires.families")
            ),
            (
                "exec.start",
                CONFINEMENT_FACT_UNSERVED,
                Some("sandbox.requires.facts")
            ),
        ]
    );
    assert_eq!(UnservedRefusal::STATUS, 501);
    assert_eq!(UnservedRefusal::CLASS, "unserved");
    for refusal in CONFINEMENT_UNSERVED_REFUSALS {
        assert!(refusal.message.contains("{name}"), "{}", refusal.message);
        assert!(refusal.message.ends_with('.'), "{}", refusal.message);
        assert!(refusal.message.len() <= 1024);
    }
}

fn linux_facts() -> CapabilityFacts {
    CapabilityFacts {
        confinement_family_process_run: Some(true),
        confinement_family_process_watch: Some(true),
        confinement_family_output_read: Some(true),
        confinement_family_wait: Some(true),
        confinement_family_workspace: Some(true),
        confinement_family_filesystem_view: Some(true),
        confinement_family_limits: Some(true),
        confinement_family_audit: Some(true),
        confinement_whole_tree_kill: Some(true),
        confinement_ipc_socket_allowlist: Some(true),
        confinement_ipc_signals_contained: Some(true),
        ..CapabilityFacts::default()
    }
}

/// § 4.4: the first unserved family in ADR 0044's order, whatever order the start wrote them in,
/// then the first unproved property in the `Fact` order, each named in the message.
#[test]
fn the_first_unmet_requirement_in_contract_order_is_refused_by_name() {
    let facts = linux_facts();
    let refused = |requires: ConfinementRequires| {
        confinement_requires_refusal(&requires, &facts)
            .map(|(refusal, message)| (refusal.code, refusal.address, message))
    };
    assert_eq!(
        refused(ConfinementRequires {
            families: Some(vec![
                ConfinementFamily::NetworkIngress,
                ConfinementFamily::NetworkEgress
            ]),
            facts: Some(vec![ConfinementProperty::SealedPaths]),
        }),
        Some((
            CONFINEMENT_FAMILY_UNSERVED,
            Some("sandbox.requires.families"),
            "Confinement family network-egress is not served by this driver.".to_owned()
        ))
    );
    assert_eq!(
        refused(ConfinementRequires {
            families: Some(vec![ConfinementFamily::Limits]),
            facts: Some(vec![
                ConfinementProperty::PrivateTmpdir,
                ConfinementProperty::SealedPaths
            ]),
        }),
        Some((
            CONFINEMENT_FACT_UNSERVED,
            Some("sandbox.requires.facts"),
            "Confinement fact sealed-paths is not proved by this driver.".to_owned()
        ))
    );
    assert_eq!(
        refused(ConfinementRequires {
            families: Some(vec![
                ConfinementFamily::ProcessRun,
                ConfinementFamily::Limits
            ]),
            facts: Some(vec![ConfinementProperty::WholeTreeKill]),
        }),
        None
    );
}

fn applied(cgroup: Option<String>, requires: Option<ConfinementRequires>) -> AppliedConfinement {
    AppliedConfinement {
        capability_snapshot: format!("sha256:{}", "7".repeat(64)),
        cgroup,
        filesystem: AppliedFilesystem::WorkspaceReadWriteSystemReadOnly,
        network: AppliedNetwork::None,
        profile: SandboxProfile::Workspace,
        workspace_access: WorkspaceAccess::ReadWrite,
        capsule: None,
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        workdir: None,
        writable_roots: Vec::new(),
        requires,
    }
}

/// D5: `cgroup` is written exactly when one was applied, never invented; `requires` is echoed
/// exactly when the start carried it.
#[test]
fn the_applied_record_writes_cgroup_and_requires_only_when_present() {
    let linux =
        serde_json::to_value(applied(Some("substrate/exec".to_owned()), None)).expect("serialises");
    assert_eq!(linux["cgroup"], "substrate/exec");
    assert!(linux.get("requires").is_none());
    let without = serde_json::to_value(applied(None, None)).expect("serialises");
    assert!(without.get("cgroup").is_none());
    let read: AppliedConfinement = serde_json::from_value(without).expect("reads without cgroup");
    assert_eq!(read.cgroup, None);
    let requires = ConfinementRequires {
        families: Some(vec![ConfinementFamily::Limits]),
        facts: None,
    };
    let echoed = serde_json::to_value(applied(None, Some(requires.clone()))).expect("serialises");
    assert_eq!(
        echoed["requires"],
        serde_json::json!({"families": ["limits"]})
    );
    let read: AppliedConfinement = serde_json::from_value(echoed).expect("reads the echo");
    assert_eq!(read.requires, Some(requires));
}
