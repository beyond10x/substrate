//! Design 37 § 2 to § 6 and § 8: the executable lists on the `sandbox` of an exec start, their
//! echo on the applied record, the deny-list's property, the refusals a start that names a list
//! can earn, and the capability snapshot's `withheld`.

use chrono::DateTime;

use super::{
    AppliedConfinement, AppliedFilesystem, AppliedNetwork, CONFINEMENT_EXEC_DENIED_PATHS_FACT,
    CONFINEMENT_EXEC_DENIED_PATHS_WITHOUT_SUBPATHS, CONFINEMENT_FACT_UNSERVED, CapabilityFacts,
    CapabilitySnapshot, ConfinementProperty, ConfinementRequest, EXEC_DENIED_PATHS_ADDRESS,
    EXEC_LIST_REFUSALS, EXEC_SANDBOX_EXEC_SUBPATH_UNDECLARED, EXEC_SANDBOX_EXEC_SUBPATHS_EMPTY,
    EXEC_SANDBOX_PATH_UNREPRESENTABLE, EXEC_SANDBOX_PLACE_UNSERVED, EXEC_SANDBOX_PROGRAM_UNLISTED,
    EXEC_SUBPATHS_ADDRESS, ExecListRefusal, HostDriverKind, MAX_EXEC_DENIED_PATHS,
    MAX_EXEC_LIST_ENTRY_BYTES, MAX_EXEC_SUBPATHS, MAX_WITHHELD_FACTS, SESSION_PTY_UNSERVED,
    SandboxProfile, WireValidationError, WithheldAddress, WithheldCause, WithheldFact,
    WithheldFactName, WorkspaceAccess, exec_lists_fact_refusal, validate_exec_lists,
    withheld_facts,
};

fn digest() -> String {
    format!("sha256:{}", "7".repeat(64))
}

/// A sandbox with the members a start must carry, and `extra` beside them.
fn sandbox(extra: &serde_json::Value) -> serde_json::Value {
    let mut sandbox = serde_json::json!({
        "capability_snapshot": digest(),
        "network": "none",
        "profile": "workspace",
        "require": true,
    });
    for (member, value) in extra.as_object().expect("an object") {
        sandbox[member] = value.clone();
    }
    sandbox
}

fn read(extra: &serde_json::Value) -> Result<ConfinementRequest, serde_json::Error> {
    serde_json::from_value(sandbox(extra))
}

fn listed(allow: Option<Vec<String>>, deny: Vec<String>) -> ConfinementRequest {
    ConfinementRequest {
        exec_subpaths: allow,
        exec_denied_paths: deny,
        ..read(&serde_json::json!({})).expect("the sandbox reads")
    }
}

fn entries(count: usize) -> Vec<String> {
    (0..count).map(|entry| format!("tools/{entry}")).collect()
}

/// § 2 and § 3: an absent allow-list and a present, empty one are two values, and each is written
/// as it was read.
#[test]
fn an_absent_allow_list_is_told_from_a_present_and_empty_one() {
    let absent = read(&serde_json::json!({})).expect("reads");
    assert_eq!(absent.exec_subpaths, None);
    let empty = read(&serde_json::json!({"exec_subpaths": []})).expect("reads");
    assert_eq!(empty.exec_subpaths, Some(Vec::new()));
    let named =
        read(&serde_json::json!({"exec_subpaths": ["/usr/bin/git", "tools"]})).expect("reads");
    assert_eq!(
        named.exec_subpaths,
        Some(vec!["/usr/bin/git".to_owned(), "tools".to_owned()])
    );
    let written = serde_json::to_value(&absent).expect("serialises");
    assert!(written.get("exec_subpaths").is_none());
    let written = serde_json::to_value(&empty).expect("serialises");
    assert_eq!(written["exec_subpaths"], serde_json::json!([]));
}

/// § 2: `null` for either member is refused when the start is read; the daemon answers
/// `request.schema-invalid`.
#[test]
fn null_for_either_list_is_refused_by_the_decoder() {
    for extra in [
        serde_json::json!({"exec_subpaths": null}),
        serde_json::json!({"exec_denied_paths": null}),
        serde_json::json!({"exec_subpaths": "tools"}),
        serde_json::json!({"exec_denied_paths": [null]}),
    ] {
        assert!(read(&extra).is_err(), "{extra} was read");
    }
}

/// § 3 and § 10: an empty deny-list reads as an absent one, and a start without either member,
/// or with an empty deny-list, is written byte for byte as before the members existed.
#[test]
fn a_start_without_the_lists_keeps_its_bytes_and_an_empty_deny_list_is_not_written() {
    let before = sandbox(&serde_json::json!({}));
    for extra in [
        serde_json::json!({}),
        serde_json::json!({"exec_denied_paths": []}),
    ] {
        let start = read(&extra).expect("reads");
        assert!(start.exec_denied_paths.is_empty());
        assert_eq!(
            serde_json::to_string(&start).expect("serialises"),
            serde_json::to_string(&before).expect("serialises"),
            "{extra}"
        );
    }
    let start = read(&serde_json::json!({"exec_denied_paths": ["tools/unsafe"]})).expect("reads");
    assert_eq!(
        serde_json::to_value(&start).expect("serialises")["exec_denied_paths"],
        serde_json::json!(["tools/unsafe"])
    );
}

fn applied(allow: Option<Vec<String>>, deny: Vec<String>) -> AppliedConfinement {
    AppliedConfinement {
        capability_snapshot: digest(),
        cgroup: None,
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
        requires: None,
        exec_subpaths: allow,
        exec_denied_paths: deny,
    }
}

/// § 4: the applied record echoes each list exactly when the start named it, in the request's
/// order and spelling, and reads back what it wrote.
#[test]
fn the_applied_record_echoes_each_list_only_when_the_start_named_it() {
    let without = serde_json::to_value(applied(None, Vec::new())).expect("serialises");
    assert!(without.get("exec_subpaths").is_none());
    assert!(without.get("exec_denied_paths").is_none());
    let allow = vec!["tools".to_owned(), "/usr/bin/git".to_owned()];
    let deny = vec!["tools/unsafe".to_owned()];
    let record = applied(Some(allow.clone()), deny.clone());
    let echoed = serde_json::to_value(&record).expect("serialises");
    assert_eq!(echoed["exec_subpaths"], serde_json::json!(allow));
    assert_eq!(echoed["exec_denied_paths"], serde_json::json!(deny));
    let read: AppliedConfinement = serde_json::from_value(echoed).expect("reads the echo");
    assert_eq!(read, record);
    let mut refused = serde_json::to_value(applied(None, Vec::new())).expect("serialises");
    refused["exec_subpaths"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<AppliedConfinement>(refused).is_err());
}

#[test]
fn the_list_bounds_are_design_37s() {
    assert_eq!(MAX_EXEC_SUBPATHS, 64);
    assert_eq!(MAX_EXEC_DENIED_PATHS, 64);
    assert_eq!(MAX_EXEC_LIST_ENTRY_BYTES, 4_095);
    assert_eq!(MAX_WITHHELD_FACTS, 12);
    assert_eq!(EXEC_SUBPATHS_ADDRESS, "sandbox.exec_subpaths");
    assert_eq!(EXEC_DENIED_PATHS_ADDRESS, "sandbox.exec_denied_paths");
}

/// § 2: each bound is admitted and one past it is refused, at the member's address; an empty
/// entry and one entry twice are refused; a present, empty allow-list is not this function's
/// (row 2 is the daemon's).
#[test]
fn the_lexical_rules_admit_each_bound_and_refuse_one_past_it() {
    let at =
        |allow: Option<Vec<String>>, deny: Vec<String>| validate_exec_lists(&listed(allow, deny));
    assert_eq!(at(None, Vec::new()), Ok(()));
    assert_eq!(at(Some(Vec::new()), Vec::new()), Ok(()));
    assert_eq!(at(Some(entries(MAX_EXEC_SUBPATHS)), Vec::new()), Ok(()));
    assert_eq!(
        at(Some(entries(MAX_EXEC_SUBPATHS + 1)), Vec::new()),
        Err(WireValidationError::ExecListExceeded {
            address: EXEC_SUBPATHS_ADDRESS
        })
    );
    assert_eq!(at(Some(entries(1)), entries(MAX_EXEC_DENIED_PATHS)), Ok(()));
    assert_eq!(
        at(Some(entries(1)), entries(MAX_EXEC_DENIED_PATHS + 1)),
        Err(WireValidationError::ExecListExceeded {
            address: EXEC_DENIED_PATHS_ADDRESS
        })
    );
    // The bound counts bytes: 4095 one-byte characters are admitted, and 2048 characters of two
    // bytes each, 4096 bytes, are not.
    let longest = "a".repeat(MAX_EXEC_LIST_ENTRY_BYTES);
    let over = "\u{e9}".repeat(MAX_EXEC_LIST_ENTRY_BYTES.div_ceil(2));
    assert!(over.chars().count() < MAX_EXEC_LIST_ENTRY_BYTES);
    assert_eq!(
        at(Some(vec![longest.clone()]), vec![longest.clone()]),
        Ok(())
    );
    for (allow, deny, address) in [
        (
            vec![format!("{longest}a")],
            Vec::new(),
            EXEC_SUBPATHS_ADDRESS,
        ),
        (vec![over.clone()], Vec::new(), EXEC_SUBPATHS_ADDRESS),
        (vec![String::new()], Vec::new(), EXEC_SUBPATHS_ADDRESS),
        (
            vec!["tools".to_owned()],
            vec![over.clone()],
            EXEC_DENIED_PATHS_ADDRESS,
        ),
        (
            vec!["tools".to_owned()],
            vec![String::new()],
            EXEC_DENIED_PATHS_ADDRESS,
        ),
    ] {
        assert_eq!(
            at(Some(allow.clone()), deny.clone()),
            Err(WireValidationError::ExecListEntryInvalid { address }),
            "{allow:?} {deny:?}"
        );
    }
    assert_eq!(
        at(
            Some(vec!["tools".to_owned(), "tools".to_owned()]),
            Vec::new()
        ),
        Err(WireValidationError::ExecListEntryDuplicate {
            address: EXEC_SUBPATHS_ADDRESS
        })
    );
    assert_eq!(
        at(
            Some(vec!["tools".to_owned()]),
            vec!["tools/a".to_owned(), "tools/a".to_owned()]
        ),
        Err(WireValidationError::ExecListEntryDuplicate {
            address: EXEC_DENIED_PATHS_ADDRESS
        })
    );
    // The allow-list is checked first.
    assert_eq!(
        at(
            Some(vec![String::new()]),
            entries(MAX_EXEC_DENIED_PATHS + 1)
        ),
        Err(WireValidationError::ExecListEntryInvalid {
            address: EXEC_SUBPATHS_ADDRESS
        })
    );
    // A deny-list without an allow-list is row 3, the daemon's, not a lexical rule.
    assert_eq!(at(None, entries(1)), Ok(()));
}

/// § 5: the deny-list's property stands directly after `exec-subpaths`, its fact is
/// `confinement.exec-denied-paths`, and it is read from its own member.
#[test]
fn the_deny_list_property_stands_directly_after_the_allow_lists() {
    assert_eq!(ConfinementProperty::ALL.len(), 8);
    let at = ConfinementProperty::ALL
        .iter()
        .position(|property| *property == ConfinementProperty::ExecSubpaths)
        .expect("exec-subpaths");
    assert_eq!(
        ConfinementProperty::ALL[at + 1],
        ConfinementProperty::ExecDeniedPaths
    );
    assert_eq!(
        ConfinementProperty::ExecDeniedPaths.name(),
        "exec-denied-paths"
    );
    assert_eq!(
        ConfinementProperty::ExecDeniedPaths.fact(),
        CONFINEMENT_EXEC_DENIED_PATHS_FACT
    );
    assert_eq!(
        CONFINEMENT_EXEC_DENIED_PATHS_FACT,
        "confinement.exec-denied-paths"
    );
    let facts = CapabilityFacts {
        confinement_exec_denied_paths: Some(true),
        ..CapabilityFacts::default()
    };
    assert!(facts.proves(ConfinementProperty::ExecDeniedPaths));
    assert!(!facts.proves(ConfinementProperty::ExecSubpaths));
}

/// The six code constants, in the order the daemon's `REFUSAL_CODES` names them.
#[test]
fn the_six_codes_are_design_37s() {
    assert_eq!(
        [
            EXEC_SANDBOX_EXEC_SUBPATHS_EMPTY,
            CONFINEMENT_EXEC_DENIED_PATHS_WITHOUT_SUBPATHS,
            EXEC_SANDBOX_PATH_UNREPRESENTABLE,
            EXEC_SANDBOX_EXEC_SUBPATH_UNDECLARED,
            EXEC_SANDBOX_PROGRAM_UNLISTED,
            EXEC_SANDBOX_PLACE_UNSERVED,
        ],
        [
            "exec.sandbox-exec-subpaths-empty",
            "confinement.exec-denied-paths-without-subpaths",
            "exec.sandbox-path-unrepresentable",
            "exec.sandbox-exec-subpath-undeclared",
            "exec.sandbox-program-unlisted",
            "exec.sandbox-place-unserved",
        ]
    );
}

/// § 6, amended: one row for each of rows 2 to 9, with code, status and the wire's addresses;
/// rows 2, 3 and 5 hold a fixed message, row 4's is computed and rows 6 to 9 hold none.
#[test]
fn the_refusal_table_has_rows_2_to_9_of_section_6() {
    let rows: Vec<(&str, u16, &[&str], bool)> = EXEC_LIST_REFUSALS
        .iter()
        .map(|row| (row.code, row.status, row.addresses, row.message.is_some()))
        .collect();
    let member: &[&str] = &[EXEC_SUBPATHS_ADDRESS, EXEC_DENIED_PATHS_ADDRESS];
    assert_eq!(
        rows,
        [
            (
                EXEC_SANDBOX_EXEC_SUBPATHS_EMPTY,
                422,
                &[EXEC_SUBPATHS_ADDRESS][..],
                true
            ),
            (
                CONFINEMENT_EXEC_DENIED_PATHS_WITHOUT_SUBPATHS,
                422,
                &[EXEC_DENIED_PATHS_ADDRESS][..],
                true
            ),
            (CONFINEMENT_FACT_UNSERVED, 501, member, false),
            (SESSION_PTY_UNSERVED, 501, &["mode"][..], true),
            (
                EXEC_SANDBOX_PATH_UNREPRESENTABLE,
                422,
                &[EXEC_SUBPATHS_ADDRESS, EXEC_DENIED_PATHS_ADDRESS, "argv"][..],
                false
            ),
            (EXEC_SANDBOX_EXEC_SUBPATH_UNDECLARED, 422, member, false),
            (EXEC_SANDBOX_PROGRAM_UNLISTED, 422, &["argv"][..], false),
            (
                EXEC_SANDBOX_PLACE_UNSERVED,
                501,
                &[
                    EXEC_SUBPATHS_ADDRESS,
                    EXEC_DENIED_PATHS_ADDRESS,
                    "argv",
                    "read_only_roots",
                    "read_only_files",
                    "writable_roots",
                ][..],
                false
            ),
        ]
    );
    let messages: Vec<Option<&str>> = EXEC_LIST_REFUSALS.iter().map(|row| row.message).collect();
    assert_eq!(
        messages[..2],
        [
            Some("An executable allow-list names at least one entry."),
            Some("An executable deny-list holds only beside an executable allow-list."),
        ]
    );
    // Row 5 is the text the host driver answers today (`listed_terminal_unserved`).
    assert_eq!(
        messages[3],
        Some(
            "A run held to an executable list is given no controlling terminal: taking one needs \
             a program before the helper, and nothing runs in the run's view before the list is \
             applied."
        )
    );
    for row in EXEC_LIST_REFUSALS {
        assert!(!row.addresses.is_empty(), "{}", row.code);
        if let Some(message) = row.message {
            assert!(message.ends_with('.'), "{message}");
            assert!(message.len() <= 512, "{message}");
        }
    }
}

fn row(code: &str) -> ExecListRefusal {
    *EXEC_LIST_REFUSALS
        .iter()
        .find(|row| row.code == code)
        .expect("a row")
}

/// § 5 and § 6 row 4: a start that names a list requires its fact, the allow-list's first, and
/// the message is the sentence `confinement_requires_refusal` writes.
#[test]
fn a_named_list_whose_fact_is_unproved_is_row_4_the_allow_lists_first() {
    let neither = CapabilityFacts::default();
    let allow_only = CapabilityFacts {
        confinement_exec_subpaths: Some(true),
        ..CapabilityFacts::default()
    };
    let both = CapabilityFacts {
        confinement_exec_subpaths: Some(true),
        confinement_exec_denied_paths: Some(true),
        ..CapabilityFacts::default()
    };
    let refused = |allow: Option<Vec<String>>, deny: Vec<String>, facts: &CapabilityFacts| {
        exec_lists_fact_refusal(&listed(allow, deny), facts)
    };
    let fact_row = row(CONFINEMENT_FACT_UNSERVED);
    assert_eq!(
        refused(Some(entries(1)), entries(1), &neither),
        Some((
            fact_row,
            EXEC_SUBPATHS_ADDRESS,
            "Confinement fact exec-subpaths is not proved by this driver.".to_owned()
        ))
    );
    assert_eq!(
        refused(Some(entries(1)), entries(1), &allow_only),
        Some((
            fact_row,
            EXEC_DENIED_PATHS_ADDRESS,
            "Confinement fact exec-denied-paths is not proved by this driver.".to_owned()
        ))
    );
    assert_eq!(refused(Some(entries(1)), entries(1), &both), None);
    assert_eq!(refused(Some(entries(1)), Vec::new(), &allow_only), None);
    assert_eq!(refused(None, Vec::new(), &neither), None);
    // A deny-list alone is row 3, which the daemon checks before row 4; this function reads only
    // the fact.
    assert_eq!(
        refused(None, entries(1), &allow_only),
        Some((
            fact_row,
            EXEC_DENIED_PATHS_ADDRESS,
            "Confinement fact exec-denied-paths is not proved by this driver.".to_owned()
        ))
    );
}

fn snapshot(withheld: Vec<WithheldFact>) -> CapabilitySnapshot {
    CapabilitySnapshot {
        snapshot: digest(),
        driver: HostDriverKind::Host,
        driver_version: "0.0.0".to_owned(),
        config_generation: 1,
        probed_at: DateTime::from_timestamp(0, 0).expect("an instant"),
        valid_until: None,
        facts: CapabilityFacts::default(),
        withheld,
    }
}

/// § 8: `withheld` is absent when empty, closed, and reads back what it wrote.
#[test]
fn withheld_is_absent_when_empty_and_its_entries_are_closed() {
    let without = serde_json::to_value(snapshot(Vec::new())).expect("serialises");
    assert!(without.get("withheld").is_none());
    let read: CapabilitySnapshot = serde_json::from_value(without).expect("reads without it");
    assert!(read.withheld.is_empty());
    let entry = WithheldFact {
        fact: WithheldFactName::ExecSubpaths,
        cause: WithheldCause::FileSystemUnlisted,
        address: WithheldAddress::WorkspaceRoot,
    };
    let with = serde_json::to_value(snapshot(vec![entry])).expect("serialises");
    assert_eq!(
        with["withheld"],
        serde_json::json!([{
            "fact": "confinement.exec-subpaths",
            "cause": "file-system-unlisted",
            "address": "config.workspace_root",
        }])
    );
    let read: CapabilitySnapshot = serde_json::from_value(with.clone()).expect("reads");
    assert_eq!(read.withheld, vec![entry]);
    for (member, value) in [
        ("fact", serde_json::json!("confinement.sealed-paths")),
        ("cause", serde_json::json!("xfs")),
        ("address", serde_json::json!("/srv/workspaces")),
    ] {
        let mut changed = with.clone();
        changed["withheld"][0][member] = value;
        assert!(serde_json::from_value::<CapabilitySnapshot>(changed).is_err());
    }
    let mut path = with.clone();
    path["withheld"][0]["path"] = serde_json::json!("/srv/workspaces");
    assert!(serde_json::from_value::<CapabilitySnapshot>(path).is_err());
    let mut null = with;
    null["withheld"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<CapabilitySnapshot>(null).is_err());
}

/// The word a closed value is written as.
fn word<T: serde::Serialize>(value: T) -> String {
    match serde_json::to_value(value).expect("serialises") {
        serde_json::Value::String(word) => word,
        other => panic!("{other} is not a word"),
    }
}

/// § 8: the three enumerations, each in the order the entries are sorted by, and their product is
/// the bound.
#[test]
fn the_withheld_enumerations_are_design_37s_and_bound_the_list() {
    for fact in WithheldFactName::ALL {
        assert_eq!(word(fact), fact.fact());
    }
    assert_eq!(
        WithheldFactName::ALL.map(word),
        ["confinement.exec-subpaths", "confinement.exec-denied-paths"]
    );
    assert_eq!(
        WithheldFactName::ALL.map(WithheldFactName::property),
        [
            ConfinementProperty::ExecSubpaths,
            ConfinementProperty::ExecDeniedPaths
        ]
    );
    for cause in WithheldCause::ALL {
        assert_eq!(word(cause), cause.name());
    }
    assert_eq!(
        WithheldCause::ALL.map(word),
        ["file-system-unlisted", "mount-id-unreported"]
    );
    for address in WithheldAddress::ALL {
        assert_eq!(word(address), address.name());
    }
    assert_eq!(
        WithheldAddress::ALL.map(word),
        [
            "config.exec_gate",
            "config.workspace_root",
            "config.writable_roots"
        ]
    );
    assert_eq!(
        WithheldFactName::ALL.len() * WithheldCause::ALL.len() * WithheldAddress::ALL.len(),
        MAX_WITHHELD_FACTS
    );
}

/// Every entry the three enumerations make.
fn every_entry() -> Vec<WithheldFact> {
    let mut every = Vec::new();
    for fact in WithheldFactName::ALL {
        for cause in WithheldCause::ALL {
            for address in WithheldAddress::ALL {
                every.push(WithheldFact {
                    fact,
                    cause,
                    address,
                });
            }
        }
    }
    every
}

/// § 8: the driver writes each distinct entry once, in the order fact, address, cause, so the list
/// holds at most `MAX_WITHHELD_FACTS` entries whatever the driver kept.
#[test]
fn withheld_holds_each_distinct_entry_once_in_the_order_fact_address_cause() {
    let every = every_entry();
    let mut kept: Vec<WithheldFact> = every.iter().rev().copied().collect();
    kept.extend(every.iter().copied());
    kept.extend(every.iter().step_by(5).copied());
    let written = withheld_facts(kept);
    assert_eq!(written.len(), MAX_WITHHELD_FACTS);
    let mut expected = Vec::new();
    for fact in WithheldFactName::ALL {
        for address in WithheldAddress::ALL {
            for cause in WithheldCause::ALL {
                expected.push(WithheldFact {
                    fact,
                    cause,
                    address,
                });
            }
        }
    }
    assert_eq!(written, expected);
    assert_eq!(withheld_facts(Vec::new()), Vec::new());
    let one = WithheldFact {
        fact: WithheldFactName::ExecDeniedPaths,
        cause: WithheldCause::MountIdUnreported,
        address: WithheldAddress::WritableRoots,
    };
    assert_eq!(withheld_facts([one, one, one]), vec![one]);
}
