//! The result bound and the widest stored form of each result the operation ledger records
//! (ADR 0053 decisions 4, 6 and 7).
//!
//! Each width is held from two sides. Real values of the result type, in every state and
//! variant and with each member a request does not set at its bound in the costliest
//! character, are no wider than the figure; and one of them reaches the figure, so it reserves
//! nothing no result needs. The member bounds are held to the schema files bundle `0.27.0`
//! publishes, which state them as literals.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Number, Value, json};

use super::result_widths::{
    Widest, json_bytes, widest_exec_absence, widest_execs, widest_file_absence,
    widest_file_observation, widest_process_record, widest_session, widest_session_absence,
    widest_workspace, widest_workspace_absence,
};
use super::{
    ApertureBytes, ApertureMechanism, ApertureMode, AppliedAperture, AppliedConfinement,
    AppliedExecutionCapsule, AppliedFilesystem, AppliedNetwork, AppliedWritableRoot, CaptureMode,
    ConfinementFamily, ConfinementProperty, ConfinementRequest, ConfinementRequires,
    EXEC_WORKDIR_MISSING, ErrorClass, Exec, ExecAbsence, ExecEnvironment, ExecExit, ExecKind,
    ExecLimits, ExecMeasurement, ExecRefusal, ExecStartInput, ExecState, ExecUsage,
    ExecutionCapsuleInput, FileAbsence, FileKind, FileObservation, LeaseObservation,
    LeaseRenewInput, LeaseState, MAX_APERTURE_DESTINATION_CHARS, MAX_APERTURE_LISTEN_CHARS,
    MAX_APERTURE_NAME_CHARS, MAX_APPLIED_CGROUP_CHARS, MAX_EGRESS_APERTURES,
    MAX_EXEC_REFUSAL_CODE_CHARS, MAX_EXEC_REFUSAL_MESSAGE_CHARS, MAX_EXEC_USAGE_CODE_CHARS,
    MAX_EXEC_USAGE_MESSAGE_CHARS, MAX_LABEL_KEY_CHARS, MAX_LABEL_VALUE_CHARS, MAX_LABELS,
    MAX_LEASE_ACTOR_CHARS, MAX_LEASE_PRINCIPAL_CHARS, MAX_OPERATION_ID_CHARS,
    MAX_PROCESS_EXIT_SIGNAL_CHARS, MAX_PROCESS_REASON_BYTES, MAX_READ_ONLY_FILE_PATH_BYTES,
    MAX_RESOURCE_ID_CHARS, MAX_ROOT_NAME_BYTES, MAX_TIMESTAMP_CHARS, MAX_WRITABLE_ROOTS,
    NetworkMode, OPERATION_RESULT_MAX_JSON_BYTES, PipeSession, PipeSessionLimits,
    ProcessConfinement, ProcessDeadline, ProcessDeadlineSource, ProcessEndedBy, ProcessEnvDigest,
    ProcessExit, ProcessKind, ProcessLease, ProcessLeaseState, ProcessOwner, ProcessPresence,
    ProcessProfile, ProcessRecord, ProcessState, ProcessTimestamp, ReadOnlyFile, ReadOnlyRoot,
    ResourceUsage, RootName, SESSION_OUTPUT_BACKPRESSURE, SESSION_OUTPUT_LIMIT, SandboxProfile,
    SecretSlotRequest, SessionAbsence, SessionAttachmentState, SessionKind, SessionMode,
    SessionState, Signal, StorageLimit, StorageUsage, UnrecordedOutput, Workspace,
    WorkspaceAbsence, WorkspaceAccess, WorkspaceKind, WorkspaceOrigin, WorkspaceState,
    WritableGitRoot, WritableHostRoot, WritableRoot, json_char_bytes, valid_aperture_name,
    validate_operation_id,
};

/// The characters the serializer writes for an instant held as `DateTime<Utc>` with
/// nanoseconds; `the_widest_forms_count_every_timestamp_at_its_published_length` holds it.
const WRITTEN_INSTANT_CHARS: usize = 30;
/// What a figure counts for each such instant beyond what the serializer writes.
const INSTANT_SLACK: usize = MAX_TIMESTAMP_CHARS - WRITTEN_INSTANT_CHARS;

fn schema(text: &str) -> Value {
    serde_json::from_str(text).expect("a released schema is JSON")
}

fn common() -> Value {
    schema(include_str!(
        "../../../contracts/substrate-wire/0.27.0/schemas/common.json"
    ))
}

fn resource() -> Value {
    schema(include_str!(
        "../../../contracts/substrate-wire/0.27.0/schemas/resource.json"
    ))
}

fn usize_of(bytes: u64) -> usize {
    usize::try_from(bytes).expect("u64 fits usize")
}

fn published_bound() -> usize {
    usize_of(OPERATION_RESULT_MAX_JSON_BYTES)
}

/// The longest instant the serializer writes for a `DateTime<Utc>`.
fn instant() -> DateTime<Utc> {
    DateTime::from_timestamp_nanos(1_999_999_999_123_456_789)
}

/// `chars` characters that cost six bytes each in a JSON string.
fn costliest(chars: usize) -> String {
    "\u{1}".repeat(chars)
}

/// A text with a character of every JSON cost: one byte, a two-byte escape, a six-byte escape,
/// and two, three and four UTF-8 bytes.
const EVERY_COST: &str = "a\"\\\n\u{1}\u{1f}\u{e9}\u{20ac}\u{1f600}";

fn json_text_bytes(text: &str) -> usize {
    text.chars().map(json_char_bytes).sum()
}

fn digest_name(digit: &str) -> String {
    format!("sha256:{}", digit.repeat(64))
}

fn value_of<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("a result is JSON")
}

// ---------------------------------------------------------------------------------------------
// The published bounds.

#[test]
#[allow(clippy::too_many_lines)]
fn the_result_member_bounds_are_the_lengths_bundle_0_27_0_publishes() {
    let common = common();
    let resource = resource();
    let published = |schema: &Value, pointer: &str| {
        schema
            .pointer(pointer)
            .unwrap_or_else(|| panic!("{pointer} is published"))
            .clone()
    };
    for (pointer, bound) in [
        ("/$defs/timestamp/maxLength", MAX_TIMESTAMP_CHARS),
        ("/$defs/operation-id/maxLength", MAX_OPERATION_ID_CHARS),
        (
            "/$defs/host-absolute-path/maxLength",
            MAX_READ_ONLY_FILE_PATH_BYTES,
        ),
        ("/$defs/labels/maxProperties", MAX_LABELS),
        ("/$defs/labels/propertyNames/maxLength", MAX_LABEL_KEY_CHARS),
        (
            "/$defs/labels/additionalProperties/maxLength",
            MAX_LABEL_VALUE_CHARS,
        ),
    ] {
        assert_eq!(published(&common, pointer), json!(bound), "{pointer}");
    }
    // The host path of a bound writable root, of either kind, is a host absolute path.
    for kind in 0..2 {
        assert_eq!(
            published(
                &common,
                &format!("/$defs/applied-writable-root/oneOf/{kind}/properties/host_path")
            ),
            json!({ "$ref": "#/$defs/host-absolute-path" })
        );
    }
    assert!(
        common
            .pointer("/$defs/applied-writable-root/oneOf/2")
            .is_none()
    );

    for (pointer, bound) in [
        (
            "/$defs/lease/properties/actor/maxLength",
            MAX_LEASE_ACTOR_CHARS,
        ),
        (
            "/$defs/lease/properties/principal/maxLength",
            MAX_LEASE_PRINCIPAL_CHARS,
        ),
        (
            "/$defs/confinement-applied/oneOf/0/properties/cgroup/maxLength",
            MAX_APPLIED_CGROUP_CHARS,
        ),
        (
            "/$defs/confinement-applied/oneOf/1/properties/cgroup/maxLength",
            MAX_APPLIED_CGROUP_CHARS,
        ),
        (
            "/$defs/applied-network/oneOf/1/properties/name/maxLength",
            MAX_APERTURE_NAME_CHARS,
        ),
        (
            "/$defs/applied-network/oneOf/1/properties/destination/maxLength",
            MAX_APERTURE_DESTINATION_CHARS,
        ),
        (
            "/$defs/applied-network/oneOf/2/items/properties/name/maxLength",
            MAX_APERTURE_NAME_CHARS,
        ),
        (
            "/$defs/applied-network/oneOf/2/items/properties/destination/maxLength",
            MAX_APERTURE_DESTINATION_CHARS,
        ),
        (
            "/$defs/applied-network/oneOf/2/items/properties/listen/maxLength",
            MAX_APERTURE_LISTEN_CHARS,
        ),
        (
            "/$defs/applied-network/oneOf/2/maxItems",
            usize_of(u64::from(MAX_EGRESS_APERTURES)),
        ),
        (
            "/$defs/exec-usage/oneOf/2/properties/code/maxLength",
            MAX_EXEC_USAGE_CODE_CHARS,
        ),
        (
            "/$defs/exec-usage/oneOf/2/properties/message/maxLength",
            MAX_EXEC_USAGE_MESSAGE_CHARS,
        ),
        (
            "/$defs/exec-refusal/oneOf/0/properties/message/maxLength",
            MAX_EXEC_REFUSAL_MESSAGE_CHARS,
        ),
        (
            "/$defs/exec-refusal/oneOf/1/properties/message/maxLength",
            MAX_EXEC_REFUSAL_MESSAGE_CHARS,
        ),
        (
            "/$defs/process-exit/properties/signal/maxLength",
            MAX_PROCESS_EXIT_SIGNAL_CHARS,
        ),
        (
            "/$defs/process-lease/properties/keep_reason/maxLength",
            MAX_PROCESS_REASON_BYTES,
        ),
        (
            "/$defs/workspace/properties/root_name/maxLength",
            MAX_ROOT_NAME_BYTES,
        ),
    ] {
        assert_eq!(published(&resource, pointer), json!(bound), "{pointer}");
    }
    // Each list of alternatives ends where the cases above end.
    for (pointer, alternatives) in [
        ("/$defs/confinement-applied/oneOf", 2),
        ("/$defs/applied-network/oneOf", 3),
        ("/$defs/exec-usage/oneOf", 3),
        ("/$defs/exec-refusal/oneOf", 2),
    ] {
        assert_eq!(
            published(&resource, pointer).as_array().map(Vec::len),
            Some(alternatives),
            "{pointer}"
        );
    }
    assert_eq!(
        published(&resource, "/$defs/exec-usage/oneOf/2/properties/status"),
        json!({ "const": "unavailable" })
    );
    assert_eq!(
        published(&resource, "/$defs/workspace/properties/labels"),
        json!({ "$ref": "common.json#/$defs/labels" })
    );
}

/// Where bundle `0.27.0` publishes a pattern and no length, the bound is the one this crate
/// admits or mints.
#[test]
fn the_bounds_bundle_0_27_0_publishes_no_length_for_are_the_ones_this_crate_admits() {
    // A minted identifier is its published prefix and one ULID.
    let common = common();
    for name in [
        "workspace-id",
        "exec-id",
        "session-id",
        "snapshot-id",
        "process-id",
    ] {
        let definition = common
            .pointer(&format!("/$defs/{name}"))
            .expect("the identifier is published");
        assert!(definition.get("maxLength").is_none(), "{name}");
        let pattern = definition
            .get("pattern")
            .and_then(Value::as_str)
            .expect("its pattern");
        let prefix = pattern
            .strip_prefix('^')
            .and_then(|pattern| pattern.strip_suffix("[A-Za-z0-9]+$"))
            .expect("a prefix and ASCII alphanumerics");
        assert!(
            prefix.len() + ulid::ULID_LEN <= MAX_RESOURCE_ID_CHARS,
            "{name}"
        );
    }

    assert_eq!(
        validate_operation_id(&"a".repeat(MAX_OPERATION_ID_CHARS)),
        Ok(())
    );
    assert!(validate_operation_id(&"a".repeat(MAX_OPERATION_ID_CHARS + 1)).is_err());
    assert!(valid_aperture_name(&"a".repeat(MAX_APERTURE_NAME_CHARS)));
    assert!(!valid_aperture_name(
        &"a".repeat(MAX_APERTURE_NAME_CHARS + 1)
    ));

    // The refusal an exec records has a published code and no published length: every
    // published code, and every code of this crate a run records, is within the bound counted.
    let resource = resource();
    let mut codes: Vec<String> = resource
        .pointer("/$defs/exec-refusal/oneOf")
        .and_then(Value::as_array)
        .expect("the refusal alternatives")
        .iter()
        .map(|alternative| {
            alternative
                .pointer("/properties/code/const")
                .and_then(Value::as_str)
                .expect("a published code")
                .to_owned()
        })
        .collect();
    assert!(codes.iter().any(|code| code == EXEC_WORKDIR_MISSING));
    codes.push(SESSION_OUTPUT_LIMIT.to_owned());
    codes.push(SESSION_OUTPUT_BACKPRESSURE.to_owned());
    for code in codes {
        assert!(
            code.chars().count() <= MAX_EXEC_REFUSAL_CODE_CHARS,
            "{code}"
        );
    }
    assert_eq!(MAX_EXEC_REFUSAL_CODE_CHARS, MAX_EXEC_USAGE_CODE_CHARS);
}

/// No character costs more in a JSON string than the one the widest forms are filled with.
#[test]
fn the_character_the_widest_forms_are_filled_with_is_the_costliest_there_is() {
    assert_eq!(json_char_bytes('\u{1}'), 6);
    assert_eq!(json_bytes("\u{1}"), 6 + 2);
    assert_eq!(
        (0..=0x0010_ffff_u32)
            .filter_map(char::from_u32)
            .map(json_char_bytes)
            .max(),
        Some(6)
    );
}

// ---------------------------------------------------------------------------------------------
// Exec.

fn minimal_start() -> ExecStartInput {
    ExecStartInput {
        workspace: "ws_01".to_owned(),
        argv: vec!["true".to_owned()],
        env: ExecEnvironment {
            allow: Vec::new(),
            set: BTreeMap::new(),
        },
        sandbox: ConfinementRequest {
            capability_snapshot: digest_name("a"),
            network: NetworkMode::None,
            aperture: None,
            apertures: Vec::new(),
            profile: SandboxProfile::Workspace,
            required: true,
            requires: None,
        },
        limits: ExecLimits {
            timeout_ms: 1_000,
            output_bytes: 1_024,
            processes: 1,
            memory_bytes: 1_024,
            cpu_millis: 1_000,
        },
        wait: false,
        workspace_access: WorkspaceAccess::ReadWrite,
        scratch: None,
        measurements: BTreeSet::new(),
        read_only_roots: Vec::new(),
        read_only_files: Vec::new(),
        secret_slots: Vec::new(),
        capsule: None,
        lease_ttl_ms: None,
        workdir: None,
        writable_roots: Vec::new(),
    }
}

fn host_root(name: &str) -> WritableRoot {
    WritableRoot::Host(WritableHostRoot {
        host: name.to_owned(),
        mount: format!("/mnt/{name}"),
    })
}

/// A start that sets every member an exec's result follows, with characters of every cost.
fn full_start() -> ExecStartInput {
    let mut start = minimal_start();
    start.workspace = format!("ws_{EVERY_COST}");
    // Wider than the name of a digest, so an applied record that echoes it is the wider one.
    start.sandbox.capability_snapshot = format!("{}{EVERY_COST}", digest_name("a"));
    start.sandbox.network = NetworkMode::Aperture;
    start.sandbox.apertures = vec!["first".to_owned(), format!("second{EVERY_COST}")];
    start.sandbox.requires = Some(ConfinementRequires {
        families: Some(vec![
            ConfinementFamily::Workspace,
            ConfinementFamily::NetworkEgress,
        ]),
        facts: Some(vec![ConfinementProperty::WholeTreeKill]),
    });
    start.workspace_access = WorkspaceAccess::Scoped {
        writable_subtrees: vec![format!("out/{EVERY_COST}")],
    };
    start.scratch = Some(StorageLimit {
        max_bytes: 1_048_576,
        max_inodes: 1_024,
    });
    start.measurements = BTreeSet::from([ExecMeasurement::ResourceUsage]);
    start.read_only_roots = vec![ReadOnlyRoot {
        host_path: format!("/opt/{EVERY_COST}"),
        mount: "/opt/tools".to_owned(),
    }];
    start.read_only_files = vec![ReadOnlyFile {
        host_path: "/etc/tool.conf".to_owned(),
        mount: format!("/etc/{EVERY_COST}"),
    }];
    start.secret_slots = vec![SecretSlotRequest {
        slot: "vendor_api_key".to_owned(),
        fd: 3,
    }];
    start.capsule = Some(ExecutionCapsuleInput {
        manifest_sha256: "b".repeat(64),
        entrypoint: format!("bin/{EVERY_COST}"),
        files: Vec::new(),
    });
    start.lease_ttl_ms = Some(60_000);
    start.workdir = Some(format!("/workspace/{EVERY_COST}"));
    start.writable_roots = vec![
        WritableRoot::Host(WritableHostRoot {
            host: "cache".to_owned(),
            mount: format!("/cache/{EVERY_COST}"),
        }),
        WritableRoot::Git(WritableGitRoot {
            workspace: "ws_02".to_owned(),
            mount: "/checkout".to_owned(),
        }),
    ];
    start
}

/// A start that names one aperture, which a driver records as the single object.
fn one_aperture_start() -> ExecStartInput {
    let mut start = minimal_start();
    start.sandbox.network = NetworkMode::Aperture;
    start.sandbox.aperture = Some("model".to_owned());
    start
}

/// A start with as many apertures and writable roots as one start may have.
fn start_at_the_list_bounds() -> ExecStartInput {
    let mut start = minimal_start();
    start.sandbox.network = NetworkMode::Aperture;
    start.sandbox.apertures = (0..MAX_EGRESS_APERTURES)
        .map(|aperture| format!("aperture_{aperture}"))
        .collect();
    start.writable_roots = (0..MAX_WRITABLE_ROOTS)
        .map(|root| host_root(&format!("root_{root}")))
        .collect();
    start
}

fn stored_lease(state: LeaseState) -> LeaseObservation {
    LeaseObservation {
        ttl_ms: u64::MAX,
        renew_by: instant(),
        state,
        clock_tolerance_ms: u64::MAX,
        authorizing_operation: "Z".repeat(MAX_OPERATION_ID_CHARS),
        actor: costliest(MAX_LEASE_ACTOR_CHARS),
        principal: Some(costliest(MAX_LEASE_PRINCIPAL_CHARS)),
    }
}

fn stored_storage() -> StorageUsage {
    StorageUsage {
        limit: StorageLimit {
            max_bytes: u64::MAX,
            max_inodes: u64::MAX,
        },
        used_bytes: u64::MAX,
        used_inodes: u64::MAX,
        observed_at: instant(),
    }
}

fn stored_aperture(listen: bool) -> AppliedAperture {
    AppliedAperture {
        mode: ApertureMode::Aperture,
        name: costliest(MAX_APERTURE_NAME_CHARS),
        destination: costliest(MAX_APERTURE_DESTINATION_CHARS),
        mechanism: ApertureMechanism::LoopbackForwarder,
        bytes: ApertureBytes {
            to_destination: u64::MAX,
            from_destination: u64::MAX,
        },
        max_bytes: Some(u64::MAX),
        listen: listen.then(|| costliest(MAX_APERTURE_LISTEN_CHARS)),
    }
}

fn stored_root(root: &WritableRoot) -> AppliedWritableRoot {
    let host_path = costliest(MAX_READ_ONLY_FILE_PATH_BYTES);
    match root {
        WritableRoot::Host(root) => AppliedWritableRoot::Host {
            name: root.host.clone(),
            host_path,
            mount: root.mount.clone(),
        },
        WritableRoot::Git(root) => AppliedWritableRoot::Git {
            workspace: root.workspace.clone(),
            host_path,
            mount: root.mount.clone(),
        },
    }
}

/// The applied record a driver writes for `start`, every member it sets at its bound. With
/// `echo` the snapshot is the one the start names, otherwise the name of a digest; with
/// `listed` every selected aperture is an element of the array form.
fn stored_applied(start: &ExecStartInput, echo: bool, listed: bool) -> AppliedConfinement {
    let selected = start.sandbox.aperture.iter().count() + start.sandbox.apertures.len();
    AppliedConfinement {
        capability_snapshot: if echo {
            start.sandbox.capability_snapshot.clone()
        } else {
            digest_name("f")
        },
        cgroup: Some(costliest(MAX_APPLIED_CGROUP_CHARS)),
        filesystem: if start.capsule.is_some() {
            AppliedFilesystem::WorkspaceReadWriteCapsuleReadOnlySystemReadOnly
        } else {
            AppliedFilesystem::WorkspaceReadWriteSystemReadOnly
        },
        network: match selected {
            0 => AppliedNetwork::None,
            1 if !listed => AppliedNetwork::Aperture(stored_aperture(false)),
            _ => AppliedNetwork::Apertures(vec![stored_aperture(true); selected]),
        },
        profile: start.sandbox.profile,
        workspace_access: start.workspace_access.clone(),
        capsule: start
            .capsule
            .as_ref()
            .map(|capsule| AppliedExecutionCapsule {
                manifest_sha256: capsule.manifest_sha256.clone(),
                entrypoint: format!("/runtime/{}", capsule.entrypoint),
                mount: "/runtime".to_owned(),
                file_count: u32::MAX,
                total_bytes: u64::MAX,
            }),
        read_only_roots: start.read_only_roots.clone(),
        read_only_files: start.read_only_files.clone(),
        secret_slots: start.secret_slots.clone(),
        workdir: start.workdir.clone(),
        writable_roots: start.writable_roots.iter().map(stored_root).collect(),
        requires: start.sandbox.requires.clone(),
    }
}

fn stored_usages() -> Vec<Option<ExecUsage>> {
    let observed = ResourceUsage {
        complete: false,
        observed_at: instant(),
        wall_time_us: u64::MAX,
        cpu_time_us: u64::MAX,
        memory_current_bytes: Some(u64::MAX),
        memory_peak_bytes: u64::MAX,
        processes_current: Some(u64::MAX),
        processes_peak: u64::MAX,
        process_limit_hits: u64::MAX,
        memory_oom_kills: u64::MAX,
        io_read_bytes: Some(u64::MAX),
        io_write_bytes: Some(u64::MAX),
        scratch: Some(stored_storage()),
    };
    vec![
        None,
        Some(ExecUsage::Pending {
            observed_at: instant(),
        }),
        Some(ExecUsage::Observed(ResourceUsage {
            complete: true,
            memory_current_bytes: None,
            processes_current: None,
            io_read_bytes: None,
            io_write_bytes: None,
            scratch: None,
            ..observed.clone()
        })),
        Some(ExecUsage::Observed(observed)),
        Some(stored_unavailable_usage()),
    ]
}

fn stored_unavailable_usage() -> ExecUsage {
    ExecUsage::Unavailable {
        observed_at: instant(),
        code: costliest(MAX_EXEC_USAGE_CODE_CHARS),
        message: costliest(MAX_EXEC_USAGE_MESSAGE_CHARS),
    }
}

fn stored_refusal(class: ErrorClass) -> ExecRefusal {
    ExecRefusal {
        class,
        code: costliest(MAX_EXEC_REFUSAL_CODE_CHARS),
        message: costliest(MAX_EXEC_REFUSAL_MESSAGE_CHARS),
    }
}

fn stored_unrecorded_output() -> UnrecordedOutput {
    UnrecordedOutput {
        stdout_observed_bytes: u64::MAX,
        stderr_observed_bytes: u64::MAX,
        stdout_queued_bytes: u64::MAX,
        stderr_queued_bytes: u64::MAX,
        queue_high_water_frames: u32::MAX,
    }
}

const EXEC_STATES: [ExecState; 6] = [
    ExecState::Accepted,
    ExecState::Running,
    ExecState::Exited,
    ExecState::Cancelled,
    ExecState::Expired,
    ExecState::Unknown,
];
const ERROR_CLASSES: [ErrorClass; 5] = [
    ErrorClass::Refused,
    ErrorClass::Conflict,
    ErrorClass::Unserved,
    ErrorClass::Exhausted,
    ErrorClass::Failed,
];
const LEASE_STATES: [LeaseState; 3] = [
    LeaseState::Active,
    LeaseState::Expiring,
    LeaseState::Expired,
];
const SIGNALS: [Signal; 3] = [Signal::Int, Signal::Term, Signal::Kill];

/// The widest exit: no code, because `null` is longer than the three digits a code has.
fn widest_stored_exit() -> ExecExit {
    ExecExit {
        code: None,
        signal: Some(Signal::Kill),
    }
}

/// Every exit the type holds, for the smallest and the largest code: with and without a code,
/// with each signal and without one.
fn stored_exits() -> Vec<ExecExit> {
    let mut exits = Vec::new();
    for code in [None, Some(0), Some(u8::MAX)] {
        exits.push(ExecExit { code, signal: None });
        for signal in SIGNALS {
            exits.push(ExecExit {
                code,
                signal: Some(signal),
            });
        }
    }
    exits
}

/// The stored exec of `start` with every member present and at its bound: the form that
/// reaches the figure when the start selects no aperture or lists its apertures.
fn widest_stored_exec(start: &ExecStartInput) -> Exec {
    Exec {
        unrecorded_output: Some(stored_unrecorded_output()),
        id: "Z".repeat(MAX_RESOURCE_ID_CHARS),
        kind: ExecKind::Exec,
        workspace: start.workspace.clone(),
        state: ExecState::Cancelled,
        observed_at: instant(),
        requested: start.sandbox.clone(),
        applied: Some(AppliedConfinement {
            // The longer of the two names, which the type admits for any start.
            filesystem: AppliedFilesystem::WorkspaceReadWriteCapsuleReadOnlySystemReadOnly,
            ..stored_applied(start, true, true)
        }),
        exit: Some(widest_stored_exit()),
        usage: Some(stored_unavailable_usage()),
        lease: Some(stored_lease(LeaseState::Expiring)),
        refusal: Some(stored_refusal(ErrorClass::Exhausted)),
    }
}

/// Stored forms of the exec of `start`: every state, every lease state, every refusal class and
/// every exit on the widest form, and then every combination of an absent or present applied
/// record (with either snapshot and either aperture form), usage variant, exit, lease, refusal
/// and unrecorded output.
fn stored_execs(start: &ExecStartInput) -> Vec<Exec> {
    let widest = widest_stored_exec(start);
    let mut forms = Vec::new();
    for state in EXEC_STATES {
        forms.push(Exec {
            state,
            ..widest.clone()
        });
    }
    for state in LEASE_STATES {
        forms.push(Exec {
            lease: Some(stored_lease(state)),
            ..widest.clone()
        });
    }
    for class in ERROR_CLASSES {
        forms.push(Exec {
            refusal: Some(stored_refusal(class)),
            ..widest.clone()
        });
    }
    for exit in stored_exits() {
        forms.push(Exec {
            exit: Some(exit),
            ..widest.clone()
        });
    }
    let applied = [
        None,
        Some(stored_applied(start, false, false)),
        Some(stored_applied(start, true, false)),
        Some(stored_applied(start, false, true)),
        Some(stored_applied(start, true, true)),
    ];
    let exits = [
        None,
        Some(ExecExit {
            code: Some(0),
            signal: None,
        }),
        Some(ExecExit {
            code: Some(u8::MAX),
            signal: Some(Signal::Term),
        }),
        Some(widest_stored_exit()),
    ];
    for applied in &applied {
        for usage in stored_usages() {
            for exit in &exits {
                for present in 0..8_u8 {
                    forms.push(Exec {
                        unrecorded_output: (present & 1 != 0).then(stored_unrecorded_output),
                        applied: applied.clone(),
                        exit: exit.clone(),
                        usage: usage.clone(),
                        lease: (present & 2 != 0).then(|| stored_lease(LeaseState::Active)),
                        refusal: (present & 4 != 0).then(|| stored_refusal(ErrorClass::Refused)),
                        ..widest.clone()
                    });
                }
            }
        }
    }
    forms
}

/// How many members of `value` are instants held as `DateTime<Utc>`: the strings of the form
/// the serializer writes for one. No request-set string of these cases has that form.
fn written_instants(value: &Value) -> usize {
    match value {
        Value::String(text) => usize::from(
            text.len() == WRITTEN_INSTANT_CHARS
                && text.ends_with('Z')
                && DateTime::parse_from_rfc3339(text).is_ok(),
        ),
        Value::Array(items) => items.iter().map(written_instants).sum(),
        Value::Object(members) => members.values().map(written_instants).sum(),
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
    }
}

/// The bytes of `value` with each of its instants counted at the published timestamp length.
fn at_the_timestamp_bound<T: Serialize>(value: &T) -> usize {
    json_bytes(value) + written_instants(&value_of(value)) * INSTANT_SLACK
}

#[test]
fn the_widest_exec_result_bounds_every_stored_form_of_its_exec() {
    for (name, start, reached) in [
        ("minimal", minimal_start(), true),
        ("full", full_start(), true),
        // A driver records one named aperture as the single object without `listen`; the figure
        // counts the array form, which is wider.
        ("one aperture", one_aperture_start(), false),
        ("at the list bounds", start_at_the_list_bounds(), true),
    ] {
        let widest = Exec::widest_json_bytes(&start);
        let forms = stored_execs(&start);
        assert!(forms.len() > 600, "{name}: {} forms", forms.len());
        for form in &forms {
            assert!(
                at_the_timestamp_bound(form) <= widest,
                "{name}: {} bytes over the figure {widest}: {form:?}",
                at_the_timestamp_bound(form)
            );
        }
        let stored = widest_stored_exec(&start);
        if reached {
            assert_eq!(
                at_the_timestamp_bound(&stored),
                widest,
                "{name}: the figure is reached, so it reserves nothing no result needs"
            );
        } else {
            assert!(at_the_timestamp_bound(&stored) <= widest, "{name}");
        }
    }

    // The figures of two starts: one that sets nothing a result follows but its workspace and
    // its snapshot, and one with as many apertures and writable roots as a start may have.
    assert_eq!(Exec::widest_json_bytes(&minimal_start()), 24_325);
    let at_the_list_bounds = Exec::widest_json_bytes(&start_at_the_list_bounds());
    assert_eq!(at_the_list_bounds, 225_869);
    assert!(at_the_list_bounds < published_bound());
}

/// A list of the start that is longer than its wire bound is counted entry by entry: the figure
/// is the one of the exec with every entry built.
#[test]
fn an_exec_width_counts_every_entry_of_a_list_longer_than_its_wire_bound() {
    let mut start = start_at_the_list_bounds();
    start.sandbox.apertures.extend(
        ["ninth", "tenth", "eleventh"]
            .into_iter()
            .map(str::to_owned),
    );
    start.writable_roots.push(host_root("ninth"));
    start
        .writable_roots
        .push(WritableRoot::Git(WritableGitRoot {
            workspace: format!("ws_{EVERY_COST}"),
            mount: "/tenth".to_owned(),
        }));
    let stored = widest_stored_exec(&start);
    let applied = stored.applied.as_ref().expect("applied");
    assert_eq!(applied.writable_roots.len(), 10);
    assert!(
        matches!(&applied.network, AppliedNetwork::Apertures(apertures) if apertures.len() == 11)
    );
    assert_eq!(
        Exec::widest_json_bytes(&start),
        at_the_timestamp_bound(&stored)
    );
    for form in stored_execs(&start).iter().take(32) {
        assert!(at_the_timestamp_bound(form) <= Exec::widest_json_bytes(&start));
    }
}

// ---------------------------------------------------------------------------------------------
// Host process.

struct ProcessStart {
    argv: Vec<String>,
    cwd: String,
    env_names: Vec<String>,
    owner: ProcessOwner,
    reason: String,
}

impl ProcessStart {
    fn widest_json_bytes(&self) -> usize {
        ProcessRecord::widest_json_bytes(
            &self.argv,
            &self.cwd,
            &self.env_names,
            &self.owner,
            &self.reason,
        )
    }
}

fn minimal_process_start() -> ProcessStart {
    ProcessStart {
        argv: vec!["sleep".to_owned()],
        cwd: "/".to_owned(),
        env_names: Vec::new(),
        owner: ProcessOwner {
            name: "n".to_owned(),
            session: "s".to_owned(),
        },
        reason: "r".to_owned(),
    }
}

fn full_process_start() -> ProcessStart {
    ProcessStart {
        argv: vec![
            "serve".to_owned(),
            format!("--name={EVERY_COST}"),
            String::new(),
        ],
        cwd: format!("/srv/{EVERY_COST}"),
        env_names: vec!["PATH".to_owned(), format!("NAME_{EVERY_COST}")],
        owner: ProcessOwner {
            name: format!("owner {EVERY_COST}"),
            session: format!("session {EVERY_COST}"),
        },
        reason: format!("because {EVERY_COST}"),
    }
}

/// A host-process timestamp as the daemon writes one, from a UTC instant.
fn written_process_timestamp() -> ProcessTimestamp {
    ProcessTimestamp::from(instant())
}

/// A host-process timestamp of the published length, with a numeric offset.
fn longest_process_timestamp() -> ProcessTimestamp {
    let text = "2026-11-24T23:59:59.123456789-23:59";
    assert_eq!(text.len(), MAX_TIMESTAMP_CHARS);
    serde_json::from_value(json!(text)).expect("an RFC 3339 timestamp")
}

fn ttl_lease(state: ProcessLeaseState, at: ProcessTimestamp) -> ProcessLease {
    ProcessLease {
        keep: false,
        keep_reason: ProcessPresence::Absent,
        renew_by: ProcessPresence::Present(at),
        state: ProcessPresence::Present(Box::new(state)),
        ttl_ms: ProcessPresence::Present(Number::from(u64::MAX)),
    }
}

fn keep_lease() -> ProcessLease {
    ProcessLease {
        keep: true,
        keep_reason: ProcessPresence::Present(costliest(MAX_PROCESS_REASON_BYTES)),
        renew_by: ProcessPresence::Absent,
        state: ProcessPresence::Absent,
        ttl_ms: ProcessPresence::Absent,
    }
}

const PROCESS_STATES: [ProcessState; 4] = [
    ProcessState::Ended,
    ProcessState::Running,
    ProcessState::Starting,
    ProcessState::Stopping,
];
const PROCESS_ENDED_BY: [ProcessEndedBy; 6] = [
    ProcessEndedBy::Exit,
    ProcessEndedBy::ExpiresAt,
    ProcessEndedBy::LeaseExpiry,
    ProcessEndedBy::Stop,
    ProcessEndedBy::Timeout,
    ProcessEndedBy::Vanished,
];
const PROCESS_LEASE_STATES: [ProcessLeaseState; 3] = [
    ProcessLeaseState::Active,
    ProcessLeaseState::Expired,
    ProcessLeaseState::Expiring,
];
const PROCESS_DEADLINE_SOURCES: [ProcessDeadlineSource; 2] = [
    ProcessDeadlineSource::ExpiresAt,
    ProcessDeadlineSource::Timeout,
];

/// The stored record of `start` with every member of a stored record present and at its bound,
/// every timestamp `at`.
fn widest_stored_record(start: &ProcessStart, at: ProcessTimestamp) -> ProcessRecord {
    ProcessRecord {
        argv: start.argv.clone(),
        confinement: Box::new(ProcessConfinement::None),
        cpu: ProcessPresence::Absent,
        cwd: start.cwd.clone(),
        deadline: ProcessPresence::Present(Box::new(ProcessDeadline {
            at,
            set_by: Box::new(ProcessDeadlineSource::ExpiresAt),
        })),
        ended_by: ProcessPresence::Present(Box::new(ProcessEndedBy::LeaseExpiry)),
        env_digest: Box::new(ProcessEnvDigest(digest_name("f"))),
        env_names: start.env_names.clone(),
        exit: ProcessPresence::Present(Box::new(ProcessExit {
            code: ProcessPresence::Present(Number::from(u64::MAX)),
            signal: ProcessPresence::Present(costliest(MAX_PROCESS_EXIT_SIGNAL_CHARS)),
        })),
        expires_at: ProcessPresence::Present(at),
        id: "Z".repeat(MAX_RESOURCE_ID_CHARS),
        kind: Box::new(ProcessKind::Process),
        lease: Box::new(ProcessLease {
            keep: false,
            keep_reason: ProcessPresence::Present(costliest(MAX_PROCESS_REASON_BYTES)),
            ..ttl_lease(ProcessLeaseState::Expiring, at)
        }),
        listening: ProcessPresence::Absent,
        memory: ProcessPresence::Absent,
        observed_at: at,
        owner: ProcessPresence::Present(Box::new(start.owner.clone())),
        profile: Box::new(ProcessProfile::Host),
        reason: start.reason.clone(),
        started_at: at,
        state: Box::new(ProcessState::Stopping),
        timeout_ms: ProcessPresence::Present(Number::from(u64::MAX)),
    }
}

/// Stored forms of the record of `start`: every state with every lease (each state of a TTL
/// lease, and a keep with its reason at the bound), exit (none, a code, a signal at its bound),
/// cause of the end, deadline, `expires_at` and `timeout_ms`, with timestamps as the daemon
/// writes them and with timestamps of the published length.
fn stored_records(start: &ProcessStart) -> Vec<ProcessRecord> {
    let mut forms = Vec::new();
    for at in [written_process_timestamp(), longest_process_timestamp()] {
        let widest = widest_stored_record(start, at);
        let mut leases: Vec<ProcessLease> = PROCESS_LEASE_STATES
            .into_iter()
            .map(|state| ttl_lease(state, at))
            .collect();
        leases.push(keep_lease());
        let exits = [
            ProcessPresence::Absent,
            ProcessPresence::Present(Box::new(ProcessExit {
                code: ProcessPresence::Present(Number::from(u64::MAX)),
                signal: ProcessPresence::Absent,
            })),
            ProcessPresence::Present(Box::new(ProcessExit {
                code: ProcessPresence::Absent,
                signal: ProcessPresence::Present(costliest(MAX_PROCESS_EXIT_SIGNAL_CHARS)),
            })),
        ];
        let mut ended_by = vec![ProcessPresence::Absent];
        ended_by.extend(
            PROCESS_ENDED_BY
                .into_iter()
                .map(|cause| ProcessPresence::Present(Box::new(cause))),
        );
        let mut deadlines = vec![ProcessPresence::Absent];
        deadlines.extend(PROCESS_DEADLINE_SOURCES.into_iter().map(|source| {
            ProcessPresence::Present(Box::new(ProcessDeadline {
                at,
                set_by: Box::new(source),
            }))
        }));
        for state in PROCESS_STATES {
            for lease in &leases {
                for exit in &exits {
                    for ended_by in &ended_by {
                        for deadline in &deadlines {
                            for bounded in [false, true] {
                                forms.push(ProcessRecord {
                                    deadline: deadline.clone(),
                                    ended_by: ended_by.clone(),
                                    exit: exit.clone(),
                                    expires_at: if bounded {
                                        ProcessPresence::Present(at)
                                    } else {
                                        ProcessPresence::Absent
                                    },
                                    lease: Box::new(lease.clone()),
                                    state: Box::new(state),
                                    timeout_ms: if bounded {
                                        ProcessPresence::Present(Number::from(u64::MAX))
                                    } else {
                                        ProcessPresence::Absent
                                    },
                                    ..widest.clone()
                                });
                            }
                        }
                    }
                }
            }
        }
        forms.push(widest);
    }
    forms
}

#[test]
fn the_widest_process_record_bounds_every_stored_form_of_its_process() {
    for (name, start) in [
        ("minimal", minimal_process_start()),
        ("full", full_process_start()),
    ] {
        let widest = start.widest_json_bytes();
        let forms = stored_records(&start);
        assert!(forms.len() > 4_000, "{name}: {} forms", forms.len());
        for form in &forms {
            assert!(
                json_bytes(form) <= widest,
                "{name}: {} bytes over the figure {widest}: {form:?}",
                json_bytes(form)
            );
        }
        assert_eq!(
            json_bytes(&widest_stored_record(&start, longest_process_timestamp())),
            widest,
            "{name}: the figure is reached, so it reserves nothing no result needs"
        );
    }

    // The figure of a start with one argument, no environment name and one-character texts.
    assert_eq!(minimal_process_start().widest_json_bytes(), 25_539);
    assert!(minimal_process_start().widest_json_bytes() < published_bound());
}

// ---------------------------------------------------------------------------------------------
// The v1 file results.

#[test]
fn the_widest_v1_file_results_bound_every_result_for_their_path() {
    let published_path_chars = common()
        .pointer("/$defs/relative-path/maxLength")
        .and_then(Value::as_u64)
        .map(usize_of)
        .expect("the published path length");
    let longest_path = costliest(published_path_chars);
    for (workspace, path) in [
        ("ws_01", "a"),
        ("ws_01", "dir/\u{1}file \"x\".txt"),
        ("ws_01", EVERY_COST),
        ("ws_01", longest_path.as_str()),
    ] {
        let widest = FileObservation::widest_json_bytes(workspace, path);
        let observation =
            |size: u64, atomic_replacement: bool, observed_at: DateTime<Utc>| FileObservation {
                kind: FileKind::File,
                workspace: workspace.to_owned(),
                path: path.to_owned(),
                size,
                sha256: "f".repeat(64),
                atomic_replacement,
                observed_at,
            };
        for size in [0, 12, u64::MAX] {
            for atomic_replacement in [false, true] {
                for observed_at in [DateTime::UNIX_EPOCH, instant()] {
                    let form = observation(size, atomic_replacement, observed_at);
                    assert!(at_the_timestamp_bound(&form) <= widest, "{form:?}");
                }
            }
        }
        assert_eq!(
            at_the_timestamp_bound(&observation(u64::MAX, false, instant())),
            widest,
            "the figure is reached, so it reserves nothing no result needs"
        );

        let widest = FileAbsence::widest_json_bytes(workspace, path);
        let absence = |absent: bool, observed_at: DateTime<Utc>| FileAbsence {
            kind: FileKind::File,
            workspace: workspace.to_owned(),
            path: path.to_owned(),
            absent,
            observed_at,
        };
        for absent in [false, true] {
            for observed_at in [DateTime::UNIX_EPOCH, instant()] {
                let form = absence(absent, observed_at);
                assert!(at_the_timestamp_bound(&form) <= widest, "{form:?}");
            }
        }
        assert_eq!(
            at_the_timestamp_bound(&absence(false, instant())),
            widest,
            "the figure is reached, so it reserves nothing no result needs"
        );
    }

    // The workspace and the path cost what their characters cost in a JSON string, and nothing
    // else of the figure follows them.
    let empty = (
        FileObservation::widest_json_bytes("", ""),
        FileAbsence::widest_json_bytes("", ""),
    );
    assert_eq!(empty, (223, 107));
    assert_eq!(
        FileObservation::widest_json_bytes(EVERY_COST, &longest_path) - empty.0,
        json_text_bytes(EVERY_COST) + 6 * published_path_chars
    );
    assert_eq!(
        FileAbsence::widest_json_bytes(EVERY_COST, &longest_path) - empty.1,
        json_text_bytes(EVERY_COST) + 6 * published_path_chars
    );
    // A path of the published length, in the costliest character, and a minted workspace
    // identifier leave both figures under the published result bound.
    let workspace = "Z".repeat(MAX_RESOURCE_ID_CHARS);
    assert!(FileObservation::widest_json_bytes(&workspace, &longest_path) < published_bound());
    assert!(FileAbsence::widest_json_bytes(&workspace, &longest_path) < published_bound());
}

// ---------------------------------------------------------------------------------------------
// Workspace, session and the absences.

fn widest_stored_workspace() -> Workspace {
    Workspace {
        id: "Z".repeat(MAX_RESOURCE_ID_CHARS),
        kind: WorkspaceKind::Workspace,
        labels: (0..MAX_LABELS)
            .map(|label| {
                (
                    format!("{label:Z>MAX_LABEL_KEY_CHARS$}"),
                    costliest(MAX_LABEL_VALUE_CHARS),
                )
            })
            .collect(),
        observed_at: instant(),
        state: WorkspaceState::Destroying,
        origin: WorkspaceOrigin::Adopted,
        root_name: RootName("Z".repeat(MAX_ROOT_NAME_BYTES)),
        storage: Some(stored_storage()),
        lease: Some(stored_lease(LeaseState::Expiring)),
    }
}

#[test]
fn the_widest_workspace_result_is_under_the_published_bound() {
    let widest = Workspace::widest_json_bytes();
    assert_eq!(widest, 110_926);
    assert!(widest < published_bound());

    let stored = widest_stored_workspace();
    assert_eq!(stored.labels.len(), MAX_LABELS);
    for state in [
        WorkspaceState::Ready,
        WorkspaceState::Destroying,
        WorkspaceState::Destroyed,
        WorkspaceState::Expired,
        WorkspaceState::Unknown,
    ] {
        for origin in [WorkspaceOrigin::Adopted, WorkspaceOrigin::Created] {
            for storage in [None, Some(stored_storage())] {
                for lease in [None, Some(LeaseState::Active), Some(LeaseState::Expired)] {
                    for labels in [BTreeMap::new(), stored.labels.clone()] {
                        let form = Workspace {
                            labels,
                            state,
                            origin,
                            storage,
                            lease: lease.map(stored_lease),
                            ..stored.clone()
                        };
                        assert!(at_the_timestamp_bound(&form) <= widest, "{form:?}");
                    }
                }
            }
        }
    }
    assert_eq!(
        at_the_timestamp_bound(&stored),
        widest,
        "the figure is reached, so it reserves nothing no result needs"
    );
}

fn widest_stored_session() -> PipeSession {
    PipeSession {
        capture: CaptureMode::Unrecorded,
        id: "Z".repeat(MAX_RESOURCE_ID_CHARS),
        kind: SessionKind::Session,
        mode: SessionMode::Pipes,
        exec: "Z".repeat(MAX_RESOURCE_ID_CHARS),
        workspace: "Z".repeat(MAX_RESOURCE_ID_CHARS),
        state: SessionState::Cancelled,
        attachment: SessionAttachmentState::Available,
        observed_at: instant(),
        capability_snapshot: digest_name("f"),
        limits: PipeSessionLimits {
            input_bytes: u64::MAX,
            frame_bytes: u64::MAX,
            queued_frames: u32::MAX,
        },
        exit: Some(widest_stored_exit()),
        lease: stored_lease(LeaseState::Expiring),
    }
}

#[test]
fn the_widest_session_result_is_under_the_published_bound() {
    let widest = PipeSession::widest_json_bytes();
    assert_eq!(widest, 3_926);
    assert!(widest < published_bound());

    let stored = widest_stored_session();
    let mut exits: Vec<Option<ExecExit>> = stored_exits().into_iter().map(Some).collect();
    exits.push(None);
    for state in [
        SessionState::Accepted,
        SessionState::Ready,
        SessionState::Attached,
        SessionState::Exited,
        SessionState::Cancelled,
        SessionState::Expired,
        SessionState::Unknown,
    ] {
        for attachment in [
            SessionAttachmentState::Pending,
            SessionAttachmentState::Available,
            SessionAttachmentState::Attached,
            SessionAttachmentState::Consumed,
            SessionAttachmentState::Uncertain,
        ] {
            for mode in [SessionMode::Pipes, SessionMode::Pty] {
                for capture in [CaptureMode::Recorded, CaptureMode::Unrecorded] {
                    for exit in &exits {
                        for lease in LEASE_STATES {
                            let form = PipeSession {
                                capture,
                                mode,
                                state,
                                attachment,
                                exit: exit.clone(),
                                lease: stored_lease(lease),
                                ..stored.clone()
                            };
                            assert!(at_the_timestamp_bound(&form) <= widest, "{form:?}");
                        }
                    }
                }
            }
        }
    }
    assert_eq!(
        at_the_timestamp_bound(&stored),
        widest,
        "the figure is reached, so it reserves nothing no result needs"
    );
}

#[test]
fn the_widest_absence_is_under_the_published_bound() {
    let id = "Z".repeat(MAX_RESOURCE_ID_CHARS);
    assert_eq!(WorkspaceAbsence::widest_json_bytes(), 127);
    assert_eq!(SessionAbsence::widest_json_bytes(), 125);
    assert_eq!(ExecAbsence::widest_json_bytes(), 122);
    for widest in [
        WorkspaceAbsence::widest_json_bytes(),
        SessionAbsence::widest_json_bytes(),
        ExecAbsence::widest_json_bytes(),
    ] {
        assert!(widest < published_bound());
    }
    for absent in [false, true] {
        for observed_at in [DateTime::UNIX_EPOCH, instant()] {
            let workspace = WorkspaceAbsence {
                kind: WorkspaceKind::Workspace,
                id: id.clone(),
                absent,
                observed_at,
            };
            assert!(at_the_timestamp_bound(&workspace) <= WorkspaceAbsence::widest_json_bytes());
            let session = SessionAbsence {
                kind: SessionKind::Session,
                id: id.clone(),
                absent,
                observed_at,
            };
            assert!(at_the_timestamp_bound(&session) <= SessionAbsence::widest_json_bytes());
            let exec = ExecAbsence {
                kind: ExecKind::Exec,
                id: id.clone(),
                absent,
                observed_at,
            };
            assert!(at_the_timestamp_bound(&exec) <= ExecAbsence::widest_json_bytes());
            if !absent && observed_at == instant() {
                assert_eq!(
                    at_the_timestamp_bound(&workspace),
                    WorkspaceAbsence::widest_json_bytes()
                );
                assert_eq!(
                    at_the_timestamp_bound(&session),
                    SessionAbsence::widest_json_bytes()
                );
                assert_eq!(
                    at_the_timestamp_bound(&exec),
                    ExecAbsence::widest_json_bytes()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Each member at its bound.

/// The exec of `start` this crate builds as its widest: the form with the widest `usage`.
fn built_widest_exec(start: &ExecStartInput) -> Widest<Exec> {
    widest_execs(start)
        .into_iter()
        .max_by_key(Widest::json_bytes)
        .expect("a widest form for each usage variant")
}

/// What `member` contributes to the JSON of `whole`: the bytes that go when `emptied` empties
/// it.
fn contribution<T: Serialize + Clone>(whole: &T, emptied: impl Fn(&mut T)) -> usize {
    let mut without = whole.clone();
    emptied(&mut without);
    json_bytes(whole) - json_bytes(&without)
}

fn applied(exec: &mut Exec) -> &mut AppliedConfinement {
    exec.applied.as_mut().expect("applied")
}

fn apertures(exec: &mut Exec) -> &mut Vec<AppliedAperture> {
    match &mut applied(exec).network {
        AppliedNetwork::Apertures(apertures) => apertures,
        other => panic!("the array form, not {other:?}"),
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn every_member_a_driver_sets_is_counted_at_its_wire_bound() {
    // An exec: the public figure is the built form's, and each member a driver, the
    // configuration or the identity sets is in it at six bytes for each character of its bound.
    let start = full_start();
    let built = built_widest_exec(&start);
    assert_eq!(Exec::widest_json_bytes(&start), built.json_bytes());
    let exec = &built.value;
    assert!(matches!(exec.usage, Some(ExecUsage::Unavailable { .. })));
    assert_eq!(
        contribution(exec, |exec| applied(exec).cgroup = Some(String::new())),
        6 * MAX_APPLIED_CGROUP_CHARS
    );
    let selected = start.sandbox.apertures.len();
    assert_eq!(apertures(&mut exec.clone()).len(), selected);
    for aperture in 0..selected {
        assert_eq!(
            contribution(exec, |exec| apertures(exec)[aperture].name.clear()),
            6 * MAX_APERTURE_NAME_CHARS
        );
        assert_eq!(
            contribution(exec, |exec| apertures(exec)[aperture].destination.clear()),
            6 * MAX_APERTURE_DESTINATION_CHARS
        );
        assert_eq!(
            contribution(exec, |exec| apertures(exec)[aperture].listen =
                Some(String::new())),
            6 * MAX_APERTURE_LISTEN_CHARS
        );
        assert_eq!(
            apertures(&mut exec.clone())[aperture].max_bytes,
            Some(u64::MAX)
        );
    }
    let roots = start.writable_roots.len();
    assert_eq!(applied(&mut exec.clone()).writable_roots.len(), roots);
    for root in 0..roots {
        assert_eq!(
            contribution(exec, |exec| match &mut applied(exec).writable_roots[root] {
                AppliedWritableRoot::Host { host_path, .. }
                | AppliedWritableRoot::Git { host_path, .. } => host_path.clear(),
            }),
            6 * MAX_READ_ONLY_FILE_PATH_BYTES
        );
    }
    assert_eq!(
        contribution(exec, |exec| match exec.usage.as_mut().expect("usage") {
            ExecUsage::Unavailable { code, .. } => code.clear(),
            other => panic!("unavailable, not {other:?}"),
        }),
        6 * MAX_EXEC_USAGE_CODE_CHARS
    );
    assert_eq!(
        contribution(exec, |exec| match exec.usage.as_mut().expect("usage") {
            ExecUsage::Unavailable { message, .. } => message.clear(),
            other => panic!("unavailable, not {other:?}"),
        }),
        6 * MAX_EXEC_USAGE_MESSAGE_CHARS
    );
    assert_eq!(
        contribution(exec, |exec| exec
            .refusal
            .as_mut()
            .expect("refusal")
            .code
            .clear()),
        6 * MAX_EXEC_REFUSAL_CODE_CHARS
    );
    assert_eq!(
        contribution(exec, |exec| exec
            .refusal
            .as_mut()
            .expect("refusal")
            .message
            .clear()),
        6 * MAX_EXEC_REFUSAL_MESSAGE_CHARS
    );
    assert_eq!(
        contribution(exec, |exec| exec
            .lease
            .as_mut()
            .expect("lease")
            .actor
            .clear()),
        6 * MAX_LEASE_ACTOR_CHARS
    );
    assert_eq!(
        contribution(exec, |exec| exec.lease.as_mut().expect("lease").principal =
            Some(String::new())),
        6 * MAX_LEASE_PRINCIPAL_CHARS
    );
    // The members only ASCII is admitted in are at their bound, one byte a character.
    assert_eq!(exec.id.len(), MAX_RESOURCE_ID_CHARS);
    assert_eq!(
        exec.lease
            .as_ref()
            .expect("lease")
            .authorizing_operation
            .len(),
        MAX_OPERATION_ID_CHARS
    );

    // A host process.
    let start = full_process_start();
    let record = widest_process_record(
        &start.argv,
        &start.cwd,
        &start.env_names,
        &start.owner,
        &start.reason,
    );
    assert_eq!(start.widest_json_bytes(), json_bytes(&record));
    assert_eq!(
        contribution(&record, |record| record.exit =
            ProcessPresence::Present(Box::new(ProcessExit {
                code: ProcessPresence::Present(Number::from(u64::MAX)),
                signal: ProcessPresence::Present(String::new()),
            }))),
        6 * MAX_PROCESS_EXIT_SIGNAL_CHARS
    );
    assert_eq!(
        contribution(&record, |record| record.lease.keep_reason =
            ProcessPresence::Present(String::new())),
        6 * MAX_PROCESS_REASON_BYTES
    );
    assert_eq!(record.id.len(), MAX_RESOURCE_ID_CHARS);

    // A workspace.
    let built = widest_workspace();
    assert_eq!(Workspace::widest_json_bytes(), built.json_bytes());
    let workspace = &built.value;
    assert_eq!(workspace.labels.len(), MAX_LABELS);
    for (key, value) in &workspace.labels {
        assert_eq!(key.len(), MAX_LABEL_KEY_CHARS);
        assert_eq!(json_bytes(value), 6 * MAX_LABEL_VALUE_CHARS + 2);
    }
    assert_eq!(workspace.id.len(), MAX_RESOURCE_ID_CHARS);
    assert_eq!(workspace.root_name.as_str().len(), MAX_ROOT_NAME_BYTES);
    assert!(workspace.root_name.is_valid());
    assert_eq!(
        contribution(workspace, |workspace| workspace
            .lease
            .as_mut()
            .expect("lease")
            .actor
            .clear()),
        6 * MAX_LEASE_ACTOR_CHARS
    );
    assert_eq!(
        contribution(workspace, |workspace| workspace
            .lease
            .as_mut()
            .expect("lease")
            .principal = Some(String::new())),
        6 * MAX_LEASE_PRINCIPAL_CHARS
    );

    // A session.
    let built = widest_session();
    assert_eq!(PipeSession::widest_json_bytes(), built.json_bytes());
    let session = &built.value;
    for id in [&session.id, &session.exec, &session.workspace] {
        assert_eq!(id.len(), MAX_RESOURCE_ID_CHARS);
    }
    assert_eq!(
        contribution(session, |session| session.lease.actor.clear()),
        6 * MAX_LEASE_ACTOR_CHARS
    );
    assert_eq!(
        contribution(session, |session| session.lease.principal =
            Some(String::new())),
        6 * MAX_LEASE_PRINCIPAL_CHARS
    );
    assert_eq!(
        session.lease.authorizing_operation.len(),
        MAX_OPERATION_ID_CHARS
    );

    // The absences.
    assert_eq!(widest_exec_absence().value.id.len(), MAX_RESOURCE_ID_CHARS);
    assert_eq!(
        widest_session_absence().value.id.len(),
        MAX_RESOURCE_ID_CHARS
    );
    assert_eq!(
        widest_workspace_absence().value.id.len(),
        MAX_RESOURCE_ID_CHARS
    );
}

/// `form` states as many instants as its value holds, and counts each at the published length.
fn held<T: Serialize>(name: &str, form: &Widest<T>, instants: usize) {
    assert_eq!(form.instants, instants, "{name}");
    assert_eq!(written_instants(&value_of(&form.value)), instants, "{name}");
    assert_eq!(
        form.json_bytes(),
        json_bytes(&form.value) + instants * INSTANT_SLACK,
        "{name}"
    );
}

/// Every timestamp of a widest form is counted at the published length: an instant held as
/// `DateTime<Utc>`, which the serializer writes shorter, through the count its form states,
/// and a host-process timestamp by its own value.
#[test]
fn the_widest_forms_count_every_timestamp_at_its_published_length() {
    assert_eq!(
        json_bytes(&DateTime::from_timestamp_nanos(999_999_999)),
        WRITTEN_INSTANT_CHARS + 2
    );
    assert_eq!(json_bytes(&instant()), WRITTEN_INSTANT_CHARS + 2);
    // No instant the type holds in the years a timestamp has four digits for is written longer.
    for nanos in [0, 1, 999_999_999, i64::MAX] {
        assert!(json_bytes(&DateTime::from_timestamp_nanos(nanos)) <= WRITTEN_INSTANT_CHARS + 2);
    }

    let start = full_start();
    let execs = widest_execs(&start);
    // Pending, observed with its scratch, unavailable: each with the exec's own instant and its
    // lease's.
    assert_eq!(execs.len(), 3);
    for (form, instants) in execs.iter().zip([3, 4, 3]) {
        held("exec", form, instants);
    }
    held("workspace", &widest_workspace(), 3);
    held("session", &widest_session(), 2);
    held(
        "file observation",
        &widest_file_observation("ws_01", "a"),
        1,
    );
    held("file absence", &widest_file_absence("ws_01", "a"), 1);
    held("exec absence", &widest_exec_absence(), 1);
    held("session absence", &widest_session_absence(), 1);
    held("workspace absence", &widest_workspace_absence(), 1);

    let start = full_process_start();
    let record = value_of(&widest_process_record(
        &start.argv,
        &start.cwd,
        &start.env_names,
        &start.owner,
        &start.reason,
    ));
    for pointer in [
        "/deadline/at",
        "/expires_at",
        "/lease/renew_by",
        "/observed_at",
        "/started_at",
    ] {
        let text = record
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{pointer} is a timestamp"));
        assert_eq!(text.len(), MAX_TIMESTAMP_CHARS, "{pointer}: {text}");
    }
}

/// A string the request sets is counted as it is sent: each of its characters costs what it
/// costs in a JSON string, once for every member that carries it.
#[test]
fn a_string_the_request_sets_is_counted_as_it_is_sent() {
    let cost = json_text_bytes(EVERY_COST);
    assert_eq!(cost, 1 + 2 + 2 + 2 + 6 + 6 + 2 + 3 + 4);

    let base = minimal_start();
    let figure = Exec::widest_json_bytes(&base);
    let with = |change: &dyn Fn(&mut ExecStartInput)| {
        let mut start = base.clone();
        change(&mut start);
        Exec::widest_json_bytes(&start) - figure
    };
    assert_eq!(with(&|start| start.workspace.push_str(EVERY_COST)), cost);
    // The requested confinement is recorded as sent; the applied record names the wider of
    // that snapshot and a digest's name, which this snapshot now is.
    assert_eq!(
        with(&|start| start.sandbox.capability_snapshot.push_str(EVERY_COST)),
        2 * cost
    );
    // The working directory is in the applied record: `,"workdir":` and the quoted text.
    assert_eq!(
        with(&|start| start.workdir = Some(EVERY_COST.to_owned())),
        r#","workdir":"""#.len() + cost
    );
    // What the result does not carry costs nothing.
    assert_eq!(with(&|start| start.argv.push(EVERY_COST.to_owned())), 0);
    assert_eq!(with(&|start| start.lease_ttl_ms = Some(u64::MAX)), 0);

    let base = minimal_process_start();
    let figure = base.widest_json_bytes();
    let with = |change: &dyn Fn(&mut ProcessStart)| {
        let mut start = minimal_process_start();
        change(&mut start);
        start.widest_json_bytes() - figure
    };
    assert_eq!(with(&|start| start.argv[0].push_str(EVERY_COST)), cost);
    assert_eq!(with(&|start| start.cwd.push_str(EVERY_COST)), cost);
    assert_eq!(
        with(&|start| start.env_names.push(EVERY_COST.to_owned())),
        cost + 2
    );
    assert_eq!(with(&|start| start.owner.name.push_str(EVERY_COST)), cost);
    assert_eq!(
        with(&|start| start.owner.session.push_str(EVERY_COST)),
        cost
    );
    assert_eq!(with(&|start| start.reason.push_str(EVERY_COST)), cost);
}

// ---------------------------------------------------------------------------------------------
// A member a width does not count.

/// The members of a closed type, read from the refusal its decoder gives an unknown member:
/// the decoder names every member it knows.
fn members_of<T: DeserializeOwned>() -> Vec<String> {
    let refusal = match serde_json::from_value::<T>(json!({ "\u{1}": 0 })) {
        Ok(_) => panic!("a closed type refuses an unknown member"),
        Err(refusal) => refusal.to_string(),
    };
    let named: Vec<&str> = refusal.split('`').skip(1).step_by(2).collect();
    assert_eq!(named.first(), Some(&"\u{1}"), "{refusal}");
    assert!(named.len() > 1, "{refusal}");
    named[1..].iter().map(|&member| member.to_owned()).collect()
}

/// Every member of `T` is a member of the widest form at `pointer`, and holds something: not
/// `null`, and not an empty string, list or object.
fn every_member_is_counted<T: DeserializeOwned>(form: &Value, pointer: &str, uncounted: &[&str]) {
    let type_name = std::any::type_name::<T>();
    let object = form
        .pointer(pointer)
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("{type_name} at {pointer:?}"));
    for member in members_of::<T>() {
        if uncounted.contains(&member.as_str()) {
            assert!(!object.contains_key(&member), "{type_name}.{member}");
            continue;
        }
        let counted = match object.get(&member) {
            None | Some(Value::Null) => false,
            Some(Value::String(text)) => !text.is_empty(),
            Some(Value::Array(items)) => !items.is_empty(),
            Some(Value::Object(members)) => !members.is_empty(),
            Some(Value::Bool(_) | Value::Number(_)) => true,
        };
        assert!(
            counted,
            "{type_name}.{member} is not in the widest form at {pointer:?}"
        );
    }
}

/// The exit of a widest form names both members of the type. Its signal is present, and its code
/// is `null`: the wider of the two forms a code has, because `null` is longer than every code.
fn the_exit_is_counted(form: &Value) {
    assert_eq!(members_of::<ExecExit>(), ["code", "signal"]);
    let exit = form
        .pointer("/exit")
        .and_then(Value::as_object)
        .expect("an exit");
    assert_eq!(exit.len(), 2);
    assert_eq!(exit.get("code"), Some(&Value::Null));
    assert!(json_bytes(&u8::MAX) < json_bytes(&None::<u8>));
    assert!(
        exit.get("signal")
            .and_then(Value::as_str)
            .is_some_and(|signal| !signal.is_empty())
    );
}

/// A member that is added to a result type and left absent, `null` or empty in the widest form
/// fails here; one that is added and not named in the form at all does not compile, because
/// every form is built as a literal that names each member.
#[test]
fn every_member_of_a_result_type_is_present_in_its_widest_form() {
    // The decoder's refusal names the members in each of its three forms.
    assert_eq!(members_of::<LeaseRenewInput>(), ["ttl_ms"]);
    assert_eq!(members_of::<ExecExit>(), ["code", "signal"]);
    assert_eq!(
        members_of::<PipeSessionLimits>(),
        ["input_bytes", "frame_bytes", "queued_frames"]
    );

    let start = full_start();
    for form in widest_execs(&start) {
        let observed = matches!(form.value.usage, Some(ExecUsage::Observed(_)));
        let form = value_of(&form.value);
        every_member_is_counted::<Exec>(&form, "", &[]);
        every_member_is_counted::<AppliedConfinement>(&form, "/applied", &[]);
        for aperture in 0..start.sandbox.apertures.len() {
            let pointer = format!("/applied/network/{aperture}");
            every_member_is_counted::<AppliedAperture>(&form, &pointer, &[]);
            every_member_is_counted::<ApertureBytes>(&form, &format!("{pointer}/bytes"), &[]);
        }
        every_member_is_counted::<AppliedExecutionCapsule>(&form, "/applied/capsule", &[]);
        the_exit_is_counted(&form);
        every_member_is_counted::<LeaseObservation>(&form, "/lease", &[]);
        every_member_is_counted::<ExecRefusal>(&form, "/refusal", &[]);
        every_member_is_counted::<UnrecordedOutput>(&form, "/unrecorded_output", &[]);
        if observed {
            every_member_is_counted::<ResourceUsage>(&form, "/usage", &[]);
            every_member_is_counted::<StorageUsage>(&form, "/usage/scratch", &[]);
            every_member_is_counted::<StorageLimit>(&form, "/usage/scratch/limit", &[]);
        }
    }

    let workspace = value_of(&widest_workspace().value);
    every_member_is_counted::<Workspace>(&workspace, "", &[]);
    every_member_is_counted::<StorageUsage>(&workspace, "/storage", &[]);
    every_member_is_counted::<StorageLimit>(&workspace, "/storage/limit", &[]);
    every_member_is_counted::<LeaseObservation>(&workspace, "/lease", &[]);

    let session = value_of(&widest_session().value);
    every_member_is_counted::<PipeSession>(&session, "", &[]);
    every_member_is_counted::<PipeSessionLimits>(&session, "/limits", &[]);
    the_exit_is_counted(&session);
    every_member_is_counted::<LeaseObservation>(&session, "/lease", &[]);

    let observation = value_of(&widest_file_observation("ws_01", "a").value);
    every_member_is_counted::<FileObservation>(&observation, "", &[]);
    let absence = value_of(&widest_file_absence("ws_01", "a").value);
    every_member_is_counted::<FileAbsence>(&absence, "", &[]);
    every_member_is_counted::<ExecAbsence>(&value_of(&widest_exec_absence().value), "", &[]);
    every_member_is_counted::<SessionAbsence>(&value_of(&widest_session_absence().value), "", &[]);
    every_member_is_counted::<WorkspaceAbsence>(
        &value_of(&widest_workspace_absence().value),
        "",
        &[],
    );

    let start = full_process_start();
    let record = value_of(&widest_process_record(
        &start.argv,
        &start.cwd,
        &start.env_names,
        &start.owner,
        &start.reason,
    ));
    // What a read observes of a live process is in no stored record.
    every_member_is_counted::<ProcessRecord>(&record, "", &["cpu", "memory", "listening"]);
    every_member_is_counted::<ProcessLease>(&record, "/lease", &[]);
    every_member_is_counted::<ProcessExit>(&record, "/exit", &[]);
    every_member_is_counted::<ProcessDeadline>(&record, "/deadline", &[]);
    every_member_is_counted::<ProcessOwner>(&record, "/owner", &[]);
}
