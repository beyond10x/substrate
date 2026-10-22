//! ADR 0038: writable roots, a named host path or a Git checkout at one mount.
//!
//! Lexical rules and the capability answer only. Whether a declared host path exists, or a named
//! workspace is a Ready Git checkout of the caller, is the driver's and the daemon's question,
//! because this crate performs no I/O.

use super::{
    AppliedWritableRoot, CapabilityFacts, EXEC_WRITABLE_ROOT_MOUNT_DUPLICATE,
    EXEC_WRITABLE_ROOT_MOUNT_INVALID, EXEC_WRITABLE_ROOT_MOUNT_OVERLAP,
    EXEC_WRITABLE_ROOT_MOUNT_RESERVED, EXEC_WRITABLE_ROOT_NAME_INVALID,
    EXEC_WRITABLE_ROOTS_EXCEEDED, EXEC_WRITABLE_ROOTS_GIT_FACT, EXEC_WRITABLE_ROOTS_GIT_UNSERVED,
    EXEC_WRITABLE_ROOTS_HOST_FACT, EXEC_WRITABLE_ROOTS_HOST_UNSERVED, ErrorClass, ExecStartInput,
    MAX_WRITABLE_ROOTS, ReadOnlyFile, ReadOnlyRoot, WRITABLE_ROOT_REFUSALS,
    WRITABLE_ROOT_RESERVED_MOUNTS, WireValidationError, WritableRoot, admit_writable_roots,
    canonical_request_hash_v2, exec_start_refusal, validate_workdir, validate_writable_roots,
    writable_root_refusal,
};

fn exec_start() -> serde_json::Value {
    serde_json::json!({
        "workspace": "ws_test",
        "argv": ["make"],
        "env": {"allow": [], "set": {}},
        "sandbox": {
            "capability_snapshot": format!("sha256:{}", "7".repeat(64)),
            "network": "none",
            "profile": "workspace",
            "require": true
        },
        "limits": {
            "timeout_ms": 5_000,
            "output_bytes": 65_536,
            "processes": 16,
            "memory_bytes": 67_108_864,
            "cpu_millis": 1_000
        },
        "wait": true
    })
}

fn input() -> ExecStartInput {
    serde_json::from_value(exec_start()).expect("decodes")
}

fn host(name: &str, mount: &str) -> WritableRoot {
    serde_json::from_value(serde_json::json!({"host": name, "mount": mount})).expect("decodes")
}

fn git(workspace: &str, mount: &str) -> WritableRoot {
    serde_json::from_value(serde_json::json!({"workspace": workspace, "mount": mount}))
        .expect("decodes")
}

fn with_roots(roots: Vec<WritableRoot>) -> ExecStartInput {
    let mut input = input();
    input.writable_roots = roots;
    input
}

fn code_of(input: &ExecStartInput) -> Option<&'static str> {
    validate_writable_roots(input)
        .err()
        .and_then(|error| writable_root_refusal(&error))
        .map(|(refusal, _)| refusal.code)
}

#[test]
fn a_start_without_writable_roots_keeps_its_bytes_and_its_hash() {
    let value = exec_start();
    let input: ExecStartInput = serde_json::from_value(value.clone()).expect("decodes");
    assert!(input.writable_roots.is_empty());
    let encoded = serde_json::to_value(&input).expect("encodes");
    assert!(
        encoded.get("writable_roots").is_none(),
        "an empty list is skipped"
    );
    assert_eq!(encoded, value);
    assert_eq!(
        canonical_request_hash_v2("POST", "/v1/execs", &encoded, None),
        canonical_request_hash_v2("POST", "/v1/execs", &value, None)
    );
    assert_eq!(validate_writable_roots(&input), Ok(()));
    assert_eq!(
        admit_writable_roots(&input, &CapabilityFacts::default()),
        Ok(())
    );
}

#[test]
fn both_kinds_round_trip() {
    let mut value = exec_start();
    value["writable_roots"] = serde_json::json!([
        {"host": "cargo-home", "mount": "/srv/builder/.cargo"},
        {"workspace": "ws_01JPOLICY", "mount": "/src/policy"}
    ]);
    let input: ExecStartInput = serde_json::from_value(value.clone()).expect("decodes");
    assert_eq!(
        input.writable_roots,
        vec![
            host("cargo-home", "/srv/builder/.cargo"),
            git("ws_01JPOLICY", "/src/policy")
        ]
    );
    assert_eq!(input.writable_roots[0].mount(), "/srv/builder/.cargo");
    assert_eq!(input.writable_roots[1].mount(), "/src/policy");
    assert_eq!(serde_json::to_value(&input).expect("encodes"), value);
}

#[test]
fn the_union_is_closed() {
    for entry in [
        serde_json::json!({"host": "cargo", "workspace": "ws_a", "mount": "/a"}),
        serde_json::json!({"host": "cargo", "mount": "/a", "host_path": "/srv/cargo"}),
        serde_json::json!({"workspace": "ws_a", "mount": "/a", "read_only": false}),
        serde_json::json!({"host": "cargo"}),
        serde_json::json!({"mount": "/a"}),
        serde_json::json!({"path": "/srv/cargo", "mount": "/a"}),
    ] {
        assert!(
            serde_json::from_value::<WritableRoot>(entry.clone()).is_err(),
            "{entry} must not decode"
        );
    }
}

#[test]
fn the_bound_is_eight() {
    assert_eq!(MAX_WRITABLE_ROOTS, 8);
    let roots = |count: usize| {
        (0..count)
            .map(|index| host(&format!("root{index}"), &format!("/w/{index}")))
            .collect::<Vec<_>>()
    };
    assert_eq!(validate_writable_roots(&with_roots(roots(8))), Ok(()));
    assert_eq!(
        code_of(&with_roots(roots(9))),
        Some(EXEC_WRITABLE_ROOTS_EXCEEDED)
    );
}

#[test]
fn a_host_root_is_named_by_a_root_name_and_a_git_root_by_a_workspace_id() {
    for name in ["cargo-home", "_x", "a", "Z9"] {
        assert_eq!(
            validate_writable_roots(&with_roots(vec![host(name, "/w")])),
            Ok(()),
            "{name}"
        );
    }
    for name in ["", "-x", ".x", "a/b", "a.b", "a b", &"a".repeat(256)] {
        assert_eq!(
            code_of(&with_roots(vec![host(name, "/w")])),
            Some(EXEC_WRITABLE_ROOT_NAME_INVALID),
            "{name:?}"
        );
    }
    assert_eq!(
        validate_writable_roots(&with_roots(vec![git("ws_01ABC", "/w")])),
        Ok(())
    );
    for workspace in ["", "ws_", "ws-01", "ex_01", "ws_01/..", "WS_01"] {
        assert_eq!(
            code_of(&with_roots(vec![git(workspace, "/w")])),
            Some(EXEC_WRITABLE_ROOT_NAME_INVALID),
            "{workspace:?}"
        );
    }
}

#[test]
fn a_mount_is_absolute_and_canonical() {
    for mount in [
        "", "/", "w", "w/x", "/w/", "//w", "/w//x", "/w/.", "/w/./x", "/w/..", "/w/../x", "/w\0x",
    ] {
        assert_eq!(
            code_of(&with_roots(vec![host("cargo", mount)])),
            Some(EXEC_WRITABLE_ROOT_MOUNT_INVALID),
            "{mount:?}"
        );
    }
    let long = format!("/{}", "a/".repeat(2_000));
    assert_eq!(
        code_of(&with_roots(vec![host("cargo", long.trim_end_matches('/'))])),
        Some(EXEC_WRITABLE_ROOT_MOUNT_INVALID)
    );
    let component = format!("/{}", "a".repeat(256));
    assert_eq!(
        code_of(&with_roots(vec![host("cargo", &component)])),
        Some(EXEC_WRITABLE_ROOT_MOUNT_INVALID)
    );
}

#[test]
fn a_mount_never_takes_a_reserved_path() {
    assert_eq!(
        WRITABLE_ROOT_RESERVED_MOUNTS,
        [
            "/",
            "/usr",
            "/bin",
            "/lib",
            "/lib64",
            "/proc",
            "/dev",
            "/tmp",
            "/etc",
            "/workspace",
            "/runtime",
            "/scratch"
        ]
    );
    for mount in [
        "/usr",
        "/usr/local/cargo",
        "/bin",
        "/lib",
        "/lib64",
        "/proc",
        "/dev/shm",
        "/tmp",
        "/tmp/build",
        "/etc",
        "/etc/cargo",
        "/workspace",
        "/workspace/target",
        "/runtime",
        "/scratch/cache",
    ] {
        assert_eq!(
            code_of(&with_roots(vec![host("cargo", mount)])),
            Some(EXEC_WRITABLE_ROOT_MOUNT_RESERVED),
            "{mount}"
        );
        assert_eq!(
            code_of(&with_roots(vec![git("ws_01", mount)])),
            Some(EXEC_WRITABLE_ROOT_MOUNT_RESERVED),
            "{mount}"
        );
    }
    for mount in [
        "/usrx",
        "/srv/builder",
        "/srv/tmp",
        "/opt/etc",
        "/var/cache",
    ] {
        assert_eq!(
            validate_writable_roots(&with_roots(vec![host("cargo", mount)])),
            Ok(()),
            "{mount}"
        );
    }
}

#[test]
fn two_writable_roots_neither_share_nor_nest() {
    assert_eq!(
        code_of(&with_roots(vec![host("a", "/w"), git("ws_01", "/w")])),
        Some(EXEC_WRITABLE_ROOT_MOUNT_DUPLICATE)
    );
    assert_eq!(
        code_of(&with_roots(vec![host("a", "/w"), host("b", "/w/x")])),
        Some(EXEC_WRITABLE_ROOT_MOUNT_OVERLAP)
    );
    assert_eq!(
        code_of(&with_roots(vec![git("ws_01", "/w/x"), host("b", "/w")])),
        Some(EXEC_WRITABLE_ROOT_MOUNT_OVERLAP)
    );
    assert_eq!(
        validate_writable_roots(&with_roots(vec![host("a", "/w"), host("b", "/wx")])),
        Ok(())
    );
}

#[test]
fn a_writable_root_neither_shares_nor_nests_with_a_read_only_mount() {
    let mut start = with_roots(vec![host("cargo", "/srv/builder/.cargo")]);
    for mount in [
        "/srv/builder/.cargo",
        "/srv/builder",
        "/srv/builder/.cargo/bin",
    ] {
        start.read_only_roots = vec![ReadOnlyRoot {
            host_path: "/opt/toolchain".to_owned(),
            mount: mount.to_owned(),
        }];
        start.read_only_files = Vec::new();
        assert_eq!(
            code_of(&start),
            Some(EXEC_WRITABLE_ROOT_MOUNT_OVERLAP),
            "root {mount}"
        );
        start.read_only_roots = Vec::new();
        start.read_only_files = vec![ReadOnlyFile {
            host_path: "/opt/toolchain/config.toml".to_owned(),
            mount: mount.to_owned(),
        }];
        assert_eq!(
            code_of(&start),
            Some(EXEC_WRITABLE_ROOT_MOUNT_OVERLAP),
            "file {mount}"
        );
    }
    start.read_only_files = vec![ReadOnlyFile {
        host_path: "/opt/toolchain/config.toml".to_owned(),
        mount: "/srv/builder/.cargo-config.toml".to_owned(),
    }];
    assert_eq!(validate_writable_roots(&start), Ok(()));
}

#[test]
fn every_lexical_refusal_is_a_named_422_and_both_unserved_are_named_501s() {
    let codes: Vec<&str> = WRITABLE_ROOT_REFUSALS
        .iter()
        .map(|refusal| refusal.code)
        .collect();
    assert_eq!(
        codes,
        [
            EXEC_WRITABLE_ROOTS_EXCEEDED,
            EXEC_WRITABLE_ROOT_NAME_INVALID,
            EXEC_WRITABLE_ROOT_MOUNT_INVALID,
            EXEC_WRITABLE_ROOT_MOUNT_RESERVED,
            EXEC_WRITABLE_ROOT_MOUNT_DUPLICATE,
            EXEC_WRITABLE_ROOT_MOUNT_OVERLAP,
            EXEC_WRITABLE_ROOTS_HOST_UNSERVED,
            EXEC_WRITABLE_ROOTS_GIT_UNSERVED,
        ]
    );
    for refusal in &WRITABLE_ROOT_REFUSALS[..6] {
        assert_eq!((refusal.status, refusal.class), (422, ErrorClass::Refused));
        assert_eq!(exec_start_refusal(refusal.code), Some(*refusal));
    }
    for refusal in &WRITABLE_ROOT_REFUSALS[6..] {
        assert_eq!((refusal.status, refusal.class), (501, ErrorClass::Unserved));
        assert_eq!(exec_start_refusal(refusal.code), Some(*refusal));
    }
}

#[test]
fn a_kind_whose_fact_is_absent_is_unserved_with_the_name() {
    let facts = CapabilityFacts::default();
    let start = with_roots(vec![host("cargo-home", "/srv/builder/.cargo")]);
    let error = admit_writable_roots(&start, &facts).expect_err("unserved");
    let (refusal, message) = writable_root_refusal(&error).expect("a named refusal");
    assert_eq!(refusal.code, EXEC_WRITABLE_ROOTS_HOST_UNSERVED);
    assert_eq!(
        message,
        "Writable host root cargo-home is not served by this host."
    );

    let start = with_roots(vec![git("ws_01JPOLICY", "/src/policy")]);
    let error = admit_writable_roots(&start, &facts).expect_err("unserved");
    let (refusal, message) = writable_root_refusal(&error).expect("a named refusal");
    assert_eq!(refusal.code, EXEC_WRITABLE_ROOTS_GIT_UNSERVED);
    assert_eq!(
        message,
        "Writable Git root ws_01JPOLICY is not served by this host."
    );
}

#[test]
fn an_undeclared_host_name_is_unserved_and_a_declared_one_is_admitted() {
    let mut facts = CapabilityFacts {
        exec_writable_roots_host: Some(vec!["cargo-home".to_owned()]),
        ..CapabilityFacts::default()
    };
    let start = with_roots(vec![host("cargo-home", "/srv/builder/.cargo")]);
    assert_eq!(admit_writable_roots(&start, &facts), Ok(()));
    let start = with_roots(vec![
        host("cargo-home", "/srv/builder/.cargo"),
        host("npm-cache", "/srv/builder/.npm"),
    ]);
    assert_eq!(
        admit_writable_roots(&start, &facts),
        Err(WireValidationError::WritableRootHostUnserved {
            name: "npm-cache".to_owned()
        })
    );
    let start = with_roots(vec![git("ws_01", "/src")]);
    assert!(admit_writable_roots(&start, &facts).is_err());
    facts.exec_writable_roots_git = Some(true);
    assert_eq!(admit_writable_roots(&start, &facts), Ok(()));
}

#[test]
fn every_lexical_rule_is_answered_before_the_capability() {
    let facts = CapabilityFacts::default();
    let start = with_roots(vec![host("cargo-home", "/etc/cargo")]);
    let error = admit_writable_roots(&start, &facts).expect_err("refused");
    assert_eq!(
        writable_root_refusal(&error).map(|(refusal, _)| refusal.code),
        Some(EXEC_WRITABLE_ROOT_MOUNT_RESERVED)
    );
}

#[test]
fn the_facts_are_optional_and_named() {
    assert_eq!(EXEC_WRITABLE_ROOTS_HOST_FACT, "exec.writable-roots.host");
    assert_eq!(EXEC_WRITABLE_ROOTS_GIT_FACT, "exec.writable-roots.git");
    let encoded = serde_json::to_value(CapabilityFacts::default()).expect("encodes");
    assert!(encoded.get(EXEC_WRITABLE_ROOTS_HOST_FACT).is_none());
    assert!(encoded.get(EXEC_WRITABLE_ROOTS_GIT_FACT).is_none());
    let facts = CapabilityFacts {
        exec_writable_roots_host: Some(vec!["cargo-home".to_owned()]),
        exec_writable_roots_git: Some(true),
        ..CapabilityFacts::default()
    };
    let encoded = serde_json::to_value(&facts).expect("encodes");
    assert_eq!(
        encoded[EXEC_WRITABLE_ROOTS_HOST_FACT],
        serde_json::json!(["cargo-home"])
    );
    assert_eq!(
        encoded[EXEC_WRITABLE_ROOTS_GIT_FACT],
        serde_json::json!(true)
    );
    let decoded: CapabilityFacts = serde_json::from_value(encoded).expect("decodes");
    assert_eq!(decoded, facts);
}

#[test]
fn the_applied_record_lists_kind_name_host_path_and_mount() {
    let applied = vec![
        AppliedWritableRoot::Host {
            name: "cargo-home".to_owned(),
            host_path: "/srv/cargo".to_owned(),
            mount: "/srv/builder/.cargo".to_owned(),
        },
        AppliedWritableRoot::Git {
            workspace: "ws_01JPOLICY".to_owned(),
            host_path: "/var/lib/substrate/workspaces/ws_01JPOLICY".to_owned(),
            mount: "/src/policy".to_owned(),
        },
    ];
    let encoded = serde_json::to_value(&applied).expect("encodes");
    assert_eq!(
        encoded,
        serde_json::json!([
            {"kind": "host", "name": "cargo-home", "host_path": "/srv/cargo",
             "mount": "/srv/builder/.cargo"},
            {"kind": "git", "workspace": "ws_01JPOLICY",
             "host_path": "/var/lib/substrate/workspaces/ws_01JPOLICY", "mount": "/src/policy"}
        ])
    );
    let decoded: Vec<AppliedWritableRoot> = serde_json::from_value(encoded).expect("decodes");
    assert_eq!(decoded, applied);
    assert!(
        serde_json::from_value::<AppliedWritableRoot>(serde_json::json!({
            "kind": "host", "name": "a", "host_path": "/a", "mount": "/b", "extra": 1
        }))
        .is_err(),
        "the applied record is closed"
    );
}

#[test]
fn a_workdir_may_lie_at_or_beneath_a_writable_root() {
    let mut start = with_roots(vec![git("ws_01JPOLICY", "/src/policy")]);
    for workdir in ["/src/policy", "/src/policy/crates/gate"] {
        start.workdir = Some(workdir.to_owned());
        assert_eq!(validate_workdir(&start), Ok(()), "{workdir}");
    }
    start.workdir = Some("/src".to_owned());
    assert_eq!(
        validate_workdir(&start),
        Err(WireValidationError::WorkdirOutsideMounts)
    );
    start.writable_roots = Vec::new();
    start.workdir = Some("/src/policy".to_owned());
    assert_eq!(
        validate_workdir(&start),
        Err(WireValidationError::WorkdirOutsideMounts)
    );
}
