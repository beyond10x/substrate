//! Design 26: the working directory a start names, and the aperture list one run selects.
//!
//! Lexical rules only. Whether the directory exists in the sandbox is the driver's question,
//! because this crate performs no I/O; `substrate-host` proves that half.

use std::collections::BTreeSet;

use super::{
    APERTURE_LIST_REFUSALS, ApertureBytes, ApertureMechanism, ApertureMode, AppliedAperture,
    AppliedConfinement, AppliedFilesystem, AppliedNetwork, CapabilityFacts, ErrorClass,
    ExecStartInput, MAX_EGRESS_APERTURES, MAX_WORKDIR_BYTES, NetworkMode, ReadOnlyRoot,
    SandboxProfile, StorageLimit, WORKDIR_REFUSALS, WireValidationError, WorkspaceAccess,
    canonical_request_hash_v2, exec_start_refusal, exec_start_refusal_for,
    validate_aperture_selection, validate_workdir,
};

fn exec_start() -> serde_json::Value {
    serde_json::json!({
        "workspace": "ws_test",
        "argv": ["pwd"],
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

fn with_workdir(workdir: &str) -> ExecStartInput {
    let mut input = input();
    input.workdir = Some(workdir.to_owned());
    input
}

fn selecting(aperture: Option<&str>, apertures: &[&str]) -> super::ConfinementRequest {
    let mut sandbox = input().sandbox;
    sandbox.network = NetworkMode::Aperture;
    sandbox.aperture = aperture.map(str::to_owned);
    sandbox.apertures = apertures.iter().map(|name| (*name).to_owned()).collect();
    sandbox
}

fn applied(name: &str, listen: Option<&str>) -> AppliedAperture {
    AppliedAperture {
        mode: ApertureMode::Aperture,
        name: name.to_owned(),
        destination: "203.0.113.7:443".to_owned(),
        mechanism: ApertureMechanism::LoopbackForwarder,
        bytes: ApertureBytes {
            to_destination: 12,
            from_destination: 34,
        },
        max_bytes: None,
        listen: listen.map(str::to_owned),
    }
}

#[test]
fn an_unchanged_start_keeps_its_bytes_and_its_hash() {
    let value = exec_start();
    let input: ExecStartInput = serde_json::from_value(value.clone()).expect("decodes");
    assert_eq!(input.workdir, None);
    assert!(input.sandbox.apertures.is_empty());
    let encoded = serde_json::to_value(&input).expect("encodes");
    assert!(encoded.get("workdir").is_none(), "absent is skipped");
    assert!(
        encoded["sandbox"].get("apertures").is_none(),
        "an empty list is skipped"
    );
    assert_eq!(encoded, value);
    assert_eq!(
        canonical_request_hash_v2("POST", "/v1/execs", &encoded, None),
        canonical_request_hash_v2("POST", "/v1/execs", &value, None)
    );
}

#[test]
fn a_workdir_and_an_aperture_list_round_trip() {
    let mut value = exec_start();
    value["workdir"] = serde_json::json!("/workspace/crates/gate");
    value["sandbox"]["network"] = serde_json::json!("aperture");
    value["sandbox"]["apertures"] = serde_json::json!(["github_api", "github"]);
    let input: ExecStartInput = serde_json::from_value(value.clone()).expect("decodes");
    assert_eq!(input.workdir.as_deref(), Some("/workspace/crates/gate"));
    assert_eq!(input.sandbox.apertures, vec!["github_api", "github"]);
    assert_eq!(serde_json::to_value(&input).expect("encodes"), value);
}

#[test]
fn a_workdir_at_or_beneath_the_workspace_is_admitted() {
    for workdir in [
        "/workspace",
        "/workspace/a",
        "/workspace/a/b-c/d.e",
        "/workspace/..a",
    ] {
        assert_eq!(
            validate_workdir(&with_workdir(workdir)),
            Ok(()),
            "{workdir}"
        );
    }
    assert_eq!(validate_workdir(&input()), Ok(()), "absent is the default");
}

#[test]
fn a_workdir_that_is_not_absolute_and_normal_is_invalid() {
    let long = format!(
        "/workspace/{}",
        "a".repeat(MAX_WORKDIR_BYTES - "/workspace/".len() + 1)
    );
    assert_eq!(long.len(), MAX_WORKDIR_BYTES + 1);
    for workdir in [
        "",
        "workspace",
        "workspace/a",
        "/workspace/",
        "/workspace//a",
        "/workspace/./a",
        "/workspace/.",
        "/workspace/../etc",
        "/workspace/a/..",
        "/workspace/a\0b",
        "//workspace",
        long.as_str(),
    ] {
        assert_eq!(
            validate_workdir(&with_workdir(workdir)),
            Err(WireValidationError::WorkdirInvalid),
            "{workdir:?}"
        );
    }
    let longest = format!(
        "/workspace/{}",
        "a".repeat(MAX_WORKDIR_BYTES - "/workspace/".len())
    );
    assert_eq!(longest.len(), MAX_WORKDIR_BYTES);
    assert_eq!(validate_workdir(&with_workdir(&longest)), Ok(()));
}

#[test]
fn the_length_bound_counts_bytes_not_characters() {
    let wide = "\u{00e9}".repeat((MAX_WORKDIR_BYTES - "/workspace/".len()) / 2 + 1);
    let workdir = format!("/workspace/{wide}");
    assert!(workdir.chars().count() <= MAX_WORKDIR_BYTES);
    assert!(workdir.len() > MAX_WORKDIR_BYTES);
    assert_eq!(
        validate_workdir(&with_workdir(&workdir)),
        Err(WireValidationError::WorkdirInvalid)
    );
}

#[test]
fn a_workdir_outside_every_placed_mount_is_refused() {
    for workdir in [
        "/",
        "/tmp",
        "/usr",
        "/usr/bin",
        "/etc",
        "/workspacex",
        "/workspace-other/a",
        "/scratch",
        "/scratch/a",
        "/runtime",
        "/runtime/bin",
        "/opt",
        "/opt/toolx",
    ] {
        let mut input = with_workdir(workdir);
        input.read_only_roots = vec![ReadOnlyRoot {
            host_path: "/srv/tool".to_owned(),
            mount: "/opt/tool".to_owned(),
        }];
        assert_eq!(
            validate_workdir(&input),
            Err(WireValidationError::WorkdirOutsideMounts),
            "{workdir}"
        );
    }
}

#[test]
fn a_workdir_beneath_a_mount_this_start_places_is_admitted() {
    let mut input = with_workdir("/opt/tool/bin");
    input.read_only_roots = vec![ReadOnlyRoot {
        host_path: "/srv/tool".to_owned(),
        mount: "/opt/tool".to_owned(),
    }];
    assert_eq!(validate_workdir(&input), Ok(()));
    input.workdir = Some("/opt/tool".to_owned());
    assert_eq!(validate_workdir(&input), Ok(()), "the mount itself");

    let mut input = with_workdir("/scratch");
    input.scratch = Some(StorageLimit {
        max_bytes: 1_048_576,
        max_inodes: 16,
    });
    assert_eq!(validate_workdir(&input), Ok(()), "scratch is placed");
    input.workdir = Some("/scratch/build".to_owned());
    assert_eq!(
        validate_workdir(&input),
        Ok(()),
        "lexically beneath scratch"
    );

    let mut input = with_workdir("/runtime/lib");
    input.capsule = Some(super::ExecutionCapsuleInput {
        manifest_sha256: "0".repeat(64),
        entrypoint: "/runtime/run".to_owned(),
        files: Vec::new(),
    });
    assert_eq!(
        validate_workdir(&input),
        Ok(()),
        "a capsule places /runtime"
    );
}

#[test]
fn an_invalid_workdir_is_reported_before_where_it_lies() {
    assert_eq!(
        validate_workdir(&with_workdir("/tmp/../workspace")),
        Err(WireValidationError::WorkdirInvalid)
    );
}

#[test]
fn a_single_aperture_keeps_its_exact_rules() {
    assert_eq!(
        validate_aperture_selection(&selecting(Some("model"), &[])),
        Ok(())
    );
    let mut none = input().sandbox;
    assert_eq!(validate_aperture_selection(&none), Ok(()));
    none.aperture = Some("model".to_owned());
    assert_eq!(
        validate_aperture_selection(&none),
        Err(WireValidationError::ApertureModeMismatch)
    );
    assert_eq!(
        validate_aperture_selection(&selecting(None, &[])),
        Err(WireValidationError::ApertureModeMismatch),
        "network aperture names one"
    );
    assert_eq!(
        validate_aperture_selection(&selecting(Some("api.example.com:443"), &[])),
        Err(WireValidationError::ApertureDestinationInRequest)
    );
}

#[test]
fn an_aperture_list_of_two_to_four_distinct_names_is_admitted() {
    assert_eq!(
        validate_aperture_selection(&selecting(None, &["github_api", "github"])),
        Ok(())
    );
    let four: Vec<String> = (0..MAX_EGRESS_APERTURES).map(|n| format!("a{n}")).collect();
    let four: Vec<&str> = four.iter().map(String::as_str).collect();
    assert_eq!(validate_aperture_selection(&selecting(None, &four)), Ok(()));
}

#[test]
fn an_aperture_list_is_refused_by_the_rule_it_breaks() {
    let five: Vec<String> = (0..=MAX_EGRESS_APERTURES)
        .map(|n| format!("a{n}"))
        .collect();
    let five: Vec<&str> = five.iter().map(String::as_str).collect();
    for (aperture, apertures, expected) in [
        (
            Some("model"),
            vec!["github_api", "github"],
            WireValidationError::ApertureModeMismatch,
        ),
        (None, vec!["github"], WireValidationError::AperturesTooFew),
        (None, five, WireValidationError::AperturesTooMany),
        (
            None,
            vec!["github", "github"],
            WireValidationError::ApertureListedTwice,
        ),
        (
            None,
            vec!["github", "api.github.com:443"],
            WireValidationError::ApertureDestinationInRequest,
        ),
        (
            None,
            vec!["github", "model/max=1MiB"],
            WireValidationError::ApertureCeilingInRequest,
        ),
        (
            None,
            vec!["github", "GitHub"],
            WireValidationError::InvalidApertureName,
        ),
    ] {
        assert_eq!(
            validate_aperture_selection(&selecting(aperture, &apertures)),
            Err(expected.clone()),
            "{aperture:?} {apertures:?}"
        );
    }
    let mut none = input().sandbox;
    none.apertures = vec!["github_api".to_owned(), "github".to_owned()];
    assert_eq!(
        validate_aperture_selection(&none),
        Err(WireValidationError::ApertureModeMismatch),
        "a list needs network aperture"
    );
}

#[test]
fn the_eight_design_codes_carry_their_status_and_class() {
    let expected = [
        ("exec.workdir-invalid", 422, ErrorClass::Refused),
        ("exec.workdir-outside-mounts", 422, ErrorClass::Refused),
        ("exec.workdir-missing", 422, ErrorClass::Refused),
        ("exec.workdir-unserved", 501, ErrorClass::Unserved),
        ("exec.apertures-too-few", 422, ErrorClass::Refused),
        ("exec.apertures-too-many", 422, ErrorClass::Refused),
        ("exec.aperture-listed-twice", 422, ErrorClass::Refused),
        ("exec.aperture-host-listed-twice", 422, ErrorClass::Refused),
    ];
    let table: Vec<_> = WORKDIR_REFUSALS
        .iter()
        .chain(APERTURE_LIST_REFUSALS.iter())
        .map(|refusal| (refusal.code, refusal.status, refusal.class))
        .collect();
    assert_eq!(table, expected);
    let distinct: BTreeSet<&str> = table.iter().map(|(code, _, _)| *code).collect();
    assert_eq!(distinct.len(), 8);
    let host = exec_start_refusal("exec.aperture-host-listed-twice").expect("named");
    assert_eq!(
        host.message,
        "Each aperture in a list declares a distinct host name; two for one name cannot both be reached.",
        "the message names the rule and never the request's names or the operator's hosts"
    );
    for (code, _, _) in expected {
        assert_eq!(
            exec_start_refusal(code).map(|refusal| refusal.code),
            Some(code)
        );
    }
    assert_eq!(exec_start_refusal("exec.aperture-undeclared"), None);
    assert!(
        exec_start_refusal("exec.workdir-invalid")
            .expect("named")
            .message
            .contains(&MAX_WORKDIR_BYTES.to_string()),
        "the message states the bound"
    );
    assert!(
        exec_start_refusal("exec.apertures-too-many")
            .expect("named")
            .message
            .contains(&MAX_EGRESS_APERTURES.to_string())
    );
}

#[test]
fn each_new_validation_error_names_its_design_code() {
    for (error, code) in [
        (WireValidationError::WorkdirInvalid, "exec.workdir-invalid"),
        (
            WireValidationError::WorkdirOutsideMounts,
            "exec.workdir-outside-mounts",
        ),
        (
            WireValidationError::AperturesTooFew,
            "exec.apertures-too-few",
        ),
        (
            WireValidationError::AperturesTooMany,
            "exec.apertures-too-many",
        ),
        (
            WireValidationError::ApertureListedTwice,
            "exec.aperture-listed-twice",
        ),
    ] {
        assert_eq!(
            exec_start_refusal_for(&error).map(|refusal| refusal.code),
            Some(code)
        );
    }
    assert_eq!(
        exec_start_refusal_for(&WireValidationError::InvalidApertureName),
        None,
        "the per-name codes keep their own names"
    );
}

#[test]
fn a_single_applied_aperture_keeps_its_object_without_listen() {
    let single = AppliedNetwork::Aperture(applied("model", None));
    let rendered = serde_json::to_value(&single).expect("encodes");
    assert!(rendered.is_object());
    assert!(rendered.get("listen").is_none(), "no 0.20.0 record changes");
    assert_eq!(
        serde_json::from_value::<AppliedNetwork>(rendered).expect("decodes"),
        single
    );
}

#[test]
fn an_applied_aperture_list_is_an_array_whose_elements_carry_listen() {
    let several = AppliedNetwork::Apertures(vec![
        applied("github_api", Some("127.0.0.1:443")),
        applied("github", Some("127.0.0.2:443")),
    ]);
    let rendered = serde_json::to_value(&several).expect("encodes");
    let elements = rendered.as_array().expect("the third form is an array");
    assert_eq!(elements.len(), 2);
    assert_eq!(elements[0]["listen"], "127.0.0.1:443");
    assert_eq!(elements[1]["name"], "github");
    assert_eq!(
        serde_json::from_value::<AppliedNetwork>(rendered).expect("decodes"),
        several
    );
    assert_eq!(
        serde_json::from_value::<AppliedNetwork>(serde_json::json!("none")).expect("decodes"),
        AppliedNetwork::None
    );
}

#[test]
fn the_applied_confinement_reports_the_workdir_only_when_one_was_set() {
    let mut confinement = AppliedConfinement {
        capability_snapshot: format!("sha256:{}", "7".repeat(64)),
        cgroup: "substrate/exec".to_owned(),
        filesystem: AppliedFilesystem::WorkspaceReadWriteSystemReadOnly,
        network: AppliedNetwork::None,
        profile: SandboxProfile::Workspace,
        workspace_access: WorkspaceAccess::ReadWrite,
        capsule: None,
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        workdir: None,
    };
    let rendered = serde_json::to_value(&confinement).expect("encodes");
    assert!(rendered.get("workdir").is_none());
    confinement.workdir = Some("/workspace/sub".to_owned());
    let rendered = serde_json::to_value(&confinement).expect("encodes");
    assert_eq!(rendered["workdir"], "/workspace/sub");
    assert_eq!(
        serde_json::from_value::<AppliedConfinement>(rendered).expect("decodes"),
        confinement
    );
}

#[test]
fn the_workdir_fact_is_absent_by_default_and_carries_its_wire_name() {
    let facts = CapabilityFacts::default();
    assert_eq!(facts.exec_workdir, None);
    assert!(
        serde_json::to_value(&facts)
            .expect("encodes")
            .get("exec.workdir")
            .is_none()
    );
    let facts = CapabilityFacts {
        exec_workdir: Some(true),
        ..CapabilityFacts::default()
    };
    assert_eq!(
        serde_json::to_value(&facts).expect("encodes")["exec.workdir"],
        serde_json::json!(true)
    );
}
