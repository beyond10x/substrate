//! The widest stored form of every result the operation ledger records (ADR 0053, decisions 6
//! and 7).
//!
//! A caller that can record at most [`OPERATION_RESULT_MAX_JSON_BYTES`] bytes for a result
//! decides with these figures, before a request has any effect, whether its result can be
//! recorded at all. Each `widest_json_bytes` builds the widest value of its result type and
//! counts the compact JSON this crate's serializer writes for it, by three rules:
//!
//! - a string the request sets is written as sent, so each character costs what it costs in a
//!   JSON string;
//! - a member the request does not set is present and as wide as it is admitted: a string holds
//!   its bound in characters that cost six bytes each, or one byte each where only ASCII
//!   letters, digits and punctuation are admitted (an identifier, a digest name, a label key, a
//!   root name); an integer is the largest its type holds, twenty digits for a 64-bit one; a
//!   timestamp has [`MAX_TIMESTAMP_CHARS`] characters; a minted identifier has
//!   [`MAX_RESOURCE_ID_CHARS`]; an enumeration has its longest name; a boolean is `false`; a
//!   list is at its bound; and a member whose every value is written shorter than `null` is
//!   `null` where the type admits it (the `code` of an exit);
//! - the figure covers every form the same resource is stored in afterwards, in every state.
//!
//! The bounds of the members a request does not set are the constants of this module. Each is
//! the length bundle `0.27.0` publishes where the bundle publishes one, and
//! `result_width_tests` holds it to the released schema file.
//!
//! [`OPERATION_RESULT_MAX_JSON_BYTES`]: super::OPERATION_RESULT_MAX_JSON_BYTES

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{
    ApertureBytes, ApertureMechanism, ApertureMode, AppliedAperture, AppliedConfinement,
    AppliedExecutionCapsule, AppliedFilesystem, AppliedNetwork, AppliedWritableRoot, CaptureMode,
    EXECUTION_CAPSULE_MOUNT, ErrorClass, Exec, ExecAbsence, ExecExit, ExecKind, ExecRefusal,
    ExecStartInput, ExecState, ExecUsage, FileAbsence, FileKind, FileObservation, LeaseObservation,
    LeaseState, MAX_EGRESS_APERTURES, MAX_PROCESS_REASON_BYTES, MAX_READ_ONLY_FILE_PATH_BYTES,
    MAX_ROOT_NAME_BYTES, MAX_WRITABLE_ROOTS, PipeSession, PipeSessionLimits, ProcessConfinement,
    ProcessDeadline, ProcessDeadlineSource, ProcessEndedBy, ProcessEnvDigest, ProcessExit,
    ProcessKind, ProcessLease, ProcessLeaseState, ProcessOwner, ProcessPresence, ProcessProfile,
    ProcessRecord, ProcessState, ProcessTimestamp, ResourceUsage, RootName, SessionAbsence,
    SessionAttachmentState, SessionKind, SessionMode, SessionState, Signal, StorageLimit,
    StorageUsage, UnrecordedOutput, Workspace, WorkspaceAbsence, WorkspaceKind, WorkspaceOrigin,
    WorkspaceState, WritableRoot,
};

/// Most characters of a timestamp: the length bundle `0.27.0` publishes for one, which is an
/// RFC 3339 date-time with nanoseconds and a numeric offset.
pub const MAX_TIMESTAMP_CHARS: usize = 35;
/// Most characters of an identifier the daemon mints for a workspace, an exec, a session, a
/// snapshot or a host process: its prefix and the identifier. Every such identifier is ASCII.
pub const MAX_RESOURCE_ID_CHARS: usize = 32;
/// Most characters of an operation identifier, the length bundle `0.27.0` publishes and
/// [`validate_operation_id`](super::validate_operation_id) admits. Every one is ASCII.
pub const MAX_OPERATION_ID_CHARS: usize = 128;
/// Most labels of one workspace, the count bundle `0.27.0` publishes.
pub const MAX_LABELS: usize = 64;
/// Most characters of a label key, the length bundle `0.27.0` publishes. Every key is ASCII.
pub const MAX_LABEL_KEY_CHARS: usize = 128;
/// Most characters of a label value, the length bundle `0.27.0` publishes.
pub const MAX_LABEL_VALUE_CHARS: usize = 256;
/// Most characters of a lease's `actor`, the length bundle `0.27.0` publishes.
pub const MAX_LEASE_ACTOR_CHARS: usize = 256;
/// Most characters of a lease's `principal`, the length bundle `0.27.0` publishes.
pub const MAX_LEASE_PRINCIPAL_CHARS: usize = 256;
/// Most characters of the `cgroup` an applied record names, the length bundle `0.27.0`
/// publishes.
pub const MAX_APPLIED_CGROUP_CHARS: usize = 512;
/// Most characters of an applied aperture's `name`, the length bundle `0.27.0` publishes and
/// [`valid_aperture_name`](super::valid_aperture_name) admits.
pub const MAX_APERTURE_NAME_CHARS: usize = 64;
/// Most characters of an applied aperture's `destination`, the length bundle `0.27.0`
/// publishes.
pub const MAX_APERTURE_DESTINATION_CHARS: usize = 64;
/// Most characters of an applied aperture's `listen`, the length bundle `0.27.0` publishes.
pub const MAX_APERTURE_LISTEN_CHARS: usize = 21;
/// Most characters of the `code` of a usage that is unavailable, the length bundle `0.27.0`
/// publishes.
pub const MAX_EXEC_USAGE_CODE_CHARS: usize = 128;
/// Most characters of the `message` of a usage that is unavailable, the length bundle `0.27.0`
/// publishes.
pub const MAX_EXEC_USAGE_MESSAGE_CHARS: usize = 2_048;
/// Most characters of the `code` of the refusal an exec records. Bundle `0.27.0` publishes the
/// codes themselves and no length; this is the length a usage `code` has.
pub const MAX_EXEC_REFUSAL_CODE_CHARS: usize = MAX_EXEC_USAGE_CODE_CHARS;
/// Most characters of the `message` of the refusal an exec records, the length bundle `0.27.0`
/// publishes.
pub const MAX_EXEC_REFUSAL_MESSAGE_CHARS: usize = 512;
/// Most characters of the `signal` a host process's `exit` names, the length bundle `0.27.0`
/// publishes.
pub const MAX_PROCESS_EXIT_SIGNAL_CHARS: usize = 32;

/// The characters this crate's serializer writes for an instant held as `DateTime<Utc>` with
/// nanoseconds: the date, the time, nine fraction digits and `Z`. A numeric offset in place of
/// `Z` makes the [`MAX_TIMESTAMP_CHARS`] a timestamp may have.
const WRITTEN_INSTANT_CHARS: usize = 30;

/// The bytes `value` occupies as compact JSON, every escape counted: what the operation ledger
/// records for it. Should serialization ever fail, the answer is `usize::MAX`, which fits no
/// bound.
pub(crate) fn json_bytes<T: Serialize + ?Sized>(value: &T) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, value).map_or(usize::MAX, |()| count.0)
}

/// One widest form of a result: the value, and how many of its members are instants held as
/// `DateTime<Utc>`. The serializer writes each of those in [`WRITTEN_INSTANT_CHARS`]
/// characters, and the figure counts each at [`MAX_TIMESTAMP_CHARS`].
pub(crate) struct Widest<T> {
    pub(crate) value: T,
    pub(crate) instants: usize,
}

impl<T: Serialize> Widest<T> {
    /// The bytes of the value as compact JSON with every instant at [`MAX_TIMESTAMP_CHARS`].
    pub(crate) fn json_bytes(&self) -> usize {
        json_bytes(&self.value).saturating_add(
            self.instants
                .saturating_mul(MAX_TIMESTAMP_CHARS - WRITTEN_INSTANT_CHARS),
        )
    }
}

/// An instant with nanoseconds, the longest form the serializer writes for a `DateTime<Utc>`.
fn instant() -> DateTime<Utc> {
    DateTime::from_timestamp_nanos(999_999_999)
}

/// A string of `chars` characters that cost six bytes each in JSON, the most a character costs.
fn costliest(chars: usize) -> String {
    "\u{1}".repeat(chars)
}

/// An identifier as long as the daemon mints one.
fn minted_id() -> String {
    "0".repeat(MAX_RESOURCE_ID_CHARS)
}

/// The name of a SHA-256 digest: `sha256:` and sixty-four hexadecimal digits.
fn digest_name() -> String {
    format!("sha256:{}", "0".repeat(64))
}

/// The name among `first` and `rest` whose JSON form is longest.
fn longest<T: Serialize + Copy>(first: T, rest: &[T]) -> T {
    rest.iter().copied().fold(first, |longest, name| {
        if json_bytes(&name) > json_bytes(&longest) {
            name
        } else {
            longest
        }
    })
}

/// The longest name of a closed vocabulary. Every variant is listed, and the `match` has no
/// other arm: a variant that is added fails to compile here until it is listed.
macro_rules! longest_name {
    ($type:ident { $first:ident $(, $rest:ident)* $(,)? }) => {
        match $type::$first {
            $type::$first $(| $type::$rest)* => longest($type::$first, &[$($type::$rest),*]),
        }
    };
}

fn widest_lease() -> Widest<LeaseObservation> {
    Widest {
        value: LeaseObservation {
            ttl_ms: u64::MAX,
            renew_by: instant(),
            state: longest_name!(LeaseState {
                Active,
                Expiring,
                Expired
            }),
            clock_tolerance_ms: u64::MAX,
            authorizing_operation: "0".repeat(MAX_OPERATION_ID_CHARS),
            actor: costliest(MAX_LEASE_ACTOR_CHARS),
            principal: Some(costliest(MAX_LEASE_PRINCIPAL_CHARS)),
        },
        instants: 1,
    }
}

fn widest_storage() -> Widest<StorageUsage> {
    Widest {
        value: StorageUsage {
            limit: StorageLimit {
                max_bytes: u64::MAX,
                max_inodes: u64::MAX,
            },
            used_bytes: u64::MAX,
            used_inodes: u64::MAX,
            observed_at: instant(),
        },
        instants: 1,
    }
}

/// The widest exit the type holds. A code has at most three digits and `null` has four
/// characters, so the widest exit is the wider of the one with the largest code and the one
/// without a code.
pub(crate) fn widest_exit() -> ExecExit {
    let signal = Some(longest_name!(Signal { Int, Term, Kill }));
    let with_code = ExecExit {
        code: Some(u8::MAX),
        signal,
    };
    let without_code = ExecExit { code: None, signal };
    if json_bytes(&without_code) > json_bytes(&with_code) {
        without_code
    } else {
        with_code
    }
}

/// Every variant of [`ExecUsage`] at its widest.
fn widest_usages() -> Vec<Widest<ExecUsage>> {
    let pending = ExecUsage::Pending {
        observed_at: instant(),
    };
    // Every variant is named, and the `match` has no other arm: a variant that is added fails to
    // compile here until its widest form is listed below.
    match pending {
        ExecUsage::Pending { .. } | ExecUsage::Observed(_) | ExecUsage::Unavailable { .. } => {}
    }
    let scratch = widest_storage();
    vec![
        Widest {
            value: pending,
            instants: 1,
        },
        Widest {
            value: ExecUsage::Observed(ResourceUsage {
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
                scratch: Some(scratch.value),
            }),
            instants: 1 + scratch.instants,
        },
        Widest {
            value: ExecUsage::Unavailable {
                observed_at: instant(),
                code: costliest(MAX_EXEC_USAGE_CODE_CHARS),
                message: costliest(MAX_EXEC_USAGE_MESSAGE_CHARS),
            },
            instants: 1,
        },
    ]
}

/// One applied aperture with every member present and at its bound.
fn widest_aperture() -> AppliedAperture {
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
        listen: Some(costliest(MAX_APERTURE_LISTEN_CHARS)),
    }
}

/// A requested writable root as it is bound: its name or workspace and its mount as sent, and
/// a host path at the bound of a host absolute path, [`MAX_READ_ONLY_FILE_PATH_BYTES`].
fn widest_applied_root(root: &WritableRoot) -> AppliedWritableRoot {
    applied_root(root, costliest(MAX_READ_ONLY_FILE_PATH_BYTES))
}

/// A requested writable root as it is bound at `host_path`.
fn applied_root(root: &WritableRoot, host_path: String) -> AppliedWritableRoot {
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

/// How many apertures the start selects: the one it names and those it lists.
fn selected_apertures(input: &ExecStartInput) -> usize {
    input
        .sandbox
        .aperture
        .iter()
        .count()
        .saturating_add(input.sandbox.apertures.len())
}

fn bound(count: u32) -> usize {
    usize::try_from(count).unwrap_or(usize::MAX)
}

/// The applied record of the start at its widest. A list with one entry for each aperture or
/// writable root of the start is built up to the bound of that list;
/// [`beyond_the_list_bounds`] counts the entries past it.
fn widest_applied(input: &ExecStartInput) -> AppliedConfinement {
    let digest = digest_name();
    let apertures = selected_apertures(input).min(bound(MAX_EGRESS_APERTURES));
    AppliedConfinement {
        // The snapshot the start names or the name of a digest, whichever is wider.
        capability_snapshot: if json_bytes(&input.sandbox.capability_snapshot) > json_bytes(&digest)
        {
            input.sandbox.capability_snapshot.clone()
        } else {
            digest
        },
        cgroup: Some(costliest(MAX_APPLIED_CGROUP_CHARS)),
        filesystem: longest_name!(AppliedFilesystem {
            WorkspaceReadWriteSystemReadOnly,
            WorkspaceReadWriteCapsuleReadOnlySystemReadOnly,
        }),
        // The array form with `listen` on every element is the widest of the three forms.
        network: if apertures == 0 {
            AppliedNetwork::None
        } else {
            AppliedNetwork::Apertures(vec![widest_aperture(); apertures])
        },
        profile: input.sandbox.profile,
        workspace_access: input.workspace_access.clone(),
        capsule: input
            .capsule
            .as_ref()
            .map(|capsule| AppliedExecutionCapsule {
                manifest_sha256: capsule.manifest_sha256.clone(),
                entrypoint: format!("{EXECUTION_CAPSULE_MOUNT}/{}", capsule.entrypoint),
                mount: EXECUTION_CAPSULE_MOUNT.to_owned(),
                file_count: u32::MAX,
                total_bytes: u64::MAX,
            }),
        read_only_roots: input.read_only_roots.clone(),
        read_only_files: input.read_only_files.clone(),
        secret_slots: input.secret_slots.clone(),
        workdir: input.workdir.clone(),
        writable_roots: input
            .writable_roots
            .iter()
            .take(bound(MAX_WRITABLE_ROOTS))
            .map(widest_applied_root)
            .collect(),
        requires: input.sandbox.requires.clone(),
    }
}

/// The bytes of the applied entries [`widest_applied`] does not build: each aperture the start
/// selects past [`MAX_EGRESS_APERTURES`] and each writable root past [`MAX_WRITABLE_ROOTS`],
/// with the comma before it. They are counted one at a time, so the figure of a start with a
/// list of any length is found without building that many entries at their bounds: a root past
/// the bound is written with an empty host path, and the bytes of a host path at its bound are
/// added as a number, so the work for a root follows the length of its own strings.
fn beyond_the_list_bounds(input: &ExecStartInput) -> usize {
    // What a host path at its bound costs between its quotes: six bytes for each character.
    let host_path = MAX_READ_ONLY_FILE_PATH_BYTES.saturating_mul(6);
    let apertures = selected_apertures(input).saturating_sub(bound(MAX_EGRESS_APERTURES));
    let apertures = if apertures == 0 {
        0
    } else {
        json_bytes(&widest_aperture())
            .saturating_add(1)
            .saturating_mul(apertures)
    };
    input
        .writable_roots
        .iter()
        .skip(bound(MAX_WRITABLE_ROOTS))
        .map(|root| {
            json_bytes(&applied_root(root, String::new()))
                .saturating_add(host_path)
                .saturating_add(1)
        })
        .fold(apertures, usize::saturating_add)
}

/// The exec of the start at its widest, once for each variant of its `usage`.
pub(crate) fn widest_execs(input: &ExecStartInput) -> Vec<Widest<Exec>> {
    widest_usages()
        .into_iter()
        .map(|usage| {
            let lease = widest_lease();
            Widest {
                value: Exec {
                    unrecorded_output: Some(UnrecordedOutput {
                        stdout_observed_bytes: u64::MAX,
                        stderr_observed_bytes: u64::MAX,
                        stdout_queued_bytes: u64::MAX,
                        stderr_queued_bytes: u64::MAX,
                        queue_high_water_frames: u32::MAX,
                    }),
                    id: minted_id(),
                    kind: ExecKind::Exec,
                    workspace: input.workspace.clone(),
                    state: longest_name!(ExecState {
                        Accepted,
                        Running,
                        Exited,
                        Cancelled,
                        Expired,
                        Unknown,
                    }),
                    observed_at: instant(),
                    requested: input.sandbox.clone(),
                    applied: Some(widest_applied(input)),
                    exit: Some(widest_exit()),
                    usage: Some(usage.value),
                    lease: Some(lease.value),
                    refusal: Some(ExecRefusal {
                        class: longest_name!(ErrorClass {
                            Refused,
                            Conflict,
                            Unserved,
                            Exhausted,
                            Failed,
                        }),
                        code: costliest(MAX_EXEC_REFUSAL_CODE_CHARS),
                        message: costliest(MAX_EXEC_REFUSAL_MESSAGE_CHARS),
                    }),
                },
                instants: 1 + lease.instants + usage.instants,
            }
        })
        .collect()
}

impl Exec {
    /// The bytes the widest exec the start `input` can lead to occupies as compact JSON: the
    /// workspace, the requested confinement and every applied member that follows the start
    /// (the workspace access, the read-only roots and files, the secret slots, the working
    /// directory, the capsule's digest and entrypoint, each writable root's name and mount, the
    /// requirement) as sent, and every other member present and at its bound. One applied
    /// aperture is counted for each aperture the start selects, and one bound root for each
    /// writable root it declares. The `usage` is the widest of its variants.
    ///
    /// No stored form of an exec this start creates is wider, in any state, with or without an
    /// `exit`, a `usage`, a `lease`, a `refusal` and an `unrecorded_output`. A caller that can
    /// record at most some number of bytes for a result decides with this, before it dispatches
    /// the start to any driver, whether every later result of the exec can be recorded.
    /// `the_widest_exec_result_bounds_every_stored_form_of_its_exec` holds the figure.
    #[must_use]
    pub fn widest_json_bytes(input: &ExecStartInput) -> usize {
        widest_execs(input)
            .iter()
            .map(Widest::json_bytes)
            .max()
            .map_or(usize::MAX, |bytes| {
                bytes.saturating_add(beyond_the_list_bounds(input))
            })
    }
}

/// The session at its widest.
pub(crate) fn widest_session() -> Widest<PipeSession> {
    let lease = widest_lease();
    Widest {
        value: PipeSession {
            capture: longest_name!(CaptureMode {
                Recorded,
                Unrecorded
            }),
            id: minted_id(),
            kind: SessionKind::Session,
            mode: longest_name!(SessionMode { Pipes, Pty }),
            exec: minted_id(),
            workspace: minted_id(),
            state: longest_name!(SessionState {
                Accepted,
                Ready,
                Attached,
                Exited,
                Cancelled,
                Expired,
                Unknown,
            }),
            attachment: longest_name!(SessionAttachmentState {
                Pending,
                Available,
                Attached,
                Consumed,
                Uncertain,
            }),
            observed_at: instant(),
            capability_snapshot: digest_name(),
            limits: PipeSessionLimits {
                input_bytes: u64::MAX,
                frame_bytes: u64::MAX,
                queued_frames: u32::MAX,
            },
            exit: Some(widest_exit()),
            lease: lease.value,
        },
        instants: 1 + lease.instants,
    }
}

impl PipeSession {
    /// The bytes the widest session occupies as compact JSON: every member present and at its
    /// bound. The figure is the same for every session, in every state, and is under
    /// [`OPERATION_RESULT_MAX_JSON_BYTES`]
    /// (`the_widest_session_result_is_under_the_published_bound`).
    ///
    /// Two members of a session are strings of its start: `workspace` and
    /// `capability_snapshot`. They are counted as a minted identifier and as the name of a
    /// digest, which is what each is once the start's input is validated and its workspace is
    /// admitted. A caller compares this figure for a start that passed both.
    ///
    /// [`OPERATION_RESULT_MAX_JSON_BYTES`]: super::OPERATION_RESULT_MAX_JSON_BYTES
    #[must_use]
    pub fn widest_json_bytes() -> usize {
        widest_session().json_bytes()
    }
}

/// The workspace at its widest.
pub(crate) fn widest_workspace() -> Widest<Workspace> {
    let storage = widest_storage();
    let lease = widest_lease();
    Widest {
        value: Workspace {
            id: minted_id(),
            kind: WorkspaceKind::Workspace,
            // Distinct keys of the longest admitted length, each with the costliest value.
            labels: (0..MAX_LABELS)
                .map(|label| {
                    (
                        format!("{label:0>MAX_LABEL_KEY_CHARS$}"),
                        costliest(MAX_LABEL_VALUE_CHARS),
                    )
                })
                .collect(),
            observed_at: instant(),
            state: longest_name!(WorkspaceState {
                Ready,
                Destroying,
                Destroyed,
                Expired,
                Unknown,
            }),
            origin: longest_name!(WorkspaceOrigin { Adopted, Created }),
            root_name: RootName("0".repeat(MAX_ROOT_NAME_BYTES)),
            storage: Some(storage.value),
            lease: Some(lease.value),
        },
        instants: 1 + storage.instants + lease.instants,
    }
}

impl Workspace {
    /// The bytes the widest workspace occupies as compact JSON: [`MAX_LABELS`] labels with keys
    /// of [`MAX_LABEL_KEY_CHARS`] and values of [`MAX_LABEL_VALUE_CHARS`] characters, a root
    /// name of [`MAX_ROOT_NAME_BYTES`], its storage and its lease present, and every other
    /// member at its bound. The figure is the same for every workspace, in every state, and is
    /// under [`OPERATION_RESULT_MAX_JSON_BYTES`]
    /// (`the_widest_workspace_result_is_under_the_published_bound`).
    ///
    /// [`OPERATION_RESULT_MAX_JSON_BYTES`]: super::OPERATION_RESULT_MAX_JSON_BYTES
    #[must_use]
    pub fn widest_json_bytes() -> usize {
        widest_workspace().json_bytes()
    }
}

/// The observation of the file at `path` in `workspace` at its widest.
pub(crate) fn widest_file_observation(workspace: &str, path: &str) -> Widest<FileObservation> {
    Widest {
        value: FileObservation {
            kind: FileKind::File,
            workspace: workspace.to_owned(),
            path: path.to_owned(),
            size: u64::MAX,
            sha256: "0".repeat(64),
            atomic_replacement: false,
            observed_at: instant(),
        },
        instants: 1,
    }
}

impl FileObservation {
    /// The bytes the widest observation of the file at `path` in `workspace` occupies as
    /// compact JSON: both as sent, a size of twenty digits, a digest, a boolean at `false`
    /// (the longer of its two values), and a timestamp at its bound. No observation for this
    /// workspace and path is wider, and the figure depends on nothing the file holds. A caller
    /// decides with this, before it dispatches a write to any driver, whether its result can be
    /// recorded.
    #[must_use]
    pub fn widest_json_bytes(workspace: &str, path: &str) -> usize {
        widest_file_observation(workspace, path).json_bytes()
    }
}

/// The absence of the file at `path` in `workspace` at its widest.
pub(crate) fn widest_file_absence(workspace: &str, path: &str) -> Widest<FileAbsence> {
    Widest {
        value: FileAbsence {
            kind: FileKind::File,
            workspace: workspace.to_owned(),
            path: path.to_owned(),
            absent: false,
            observed_at: instant(),
        },
        instants: 1,
    }
}

impl FileAbsence {
    /// The bytes the widest absence of the file at `path` in `workspace` occupies as compact
    /// JSON: both as sent, a boolean at `false` (the longer of its two values), and a timestamp
    /// at its bound. No absence for this workspace and path is wider. A caller decides with
    /// this, before it dispatches a delete to any driver, whether its result can be recorded.
    #[must_use]
    pub fn widest_json_bytes(workspace: &str, path: &str) -> usize {
        widest_file_absence(workspace, path).json_bytes()
    }
}

/// The absence of an exec at its widest.
pub(crate) fn widest_exec_absence() -> Widest<ExecAbsence> {
    Widest {
        value: ExecAbsence {
            kind: ExecKind::Exec,
            id: minted_id(),
            absent: false,
            observed_at: instant(),
        },
        instants: 1,
    }
}

impl ExecAbsence {
    /// The bytes the widest absence of an exec occupies as compact JSON: an identifier of
    /// [`MAX_RESOURCE_ID_CHARS`] characters, a boolean at `false` (the longer of its two
    /// values), and a timestamp at its bound. The figure is under
    /// [`OPERATION_RESULT_MAX_JSON_BYTES`]
    /// (`the_widest_absence_is_under_the_published_bound`).
    ///
    /// [`OPERATION_RESULT_MAX_JSON_BYTES`]: super::OPERATION_RESULT_MAX_JSON_BYTES
    #[must_use]
    pub fn widest_json_bytes() -> usize {
        widest_exec_absence().json_bytes()
    }
}

/// The absence of a session at its widest.
pub(crate) fn widest_session_absence() -> Widest<SessionAbsence> {
    Widest {
        value: SessionAbsence {
            kind: SessionKind::Session,
            id: minted_id(),
            absent: false,
            observed_at: instant(),
        },
        instants: 1,
    }
}

impl SessionAbsence {
    /// The bytes the widest absence of a session occupies as compact JSON, counted as
    /// [`ExecAbsence::widest_json_bytes`] counts an exec's.
    #[must_use]
    pub fn widest_json_bytes() -> usize {
        widest_session_absence().json_bytes()
    }
}

/// The absence of a workspace at its widest.
pub(crate) fn widest_workspace_absence() -> Widest<WorkspaceAbsence> {
    Widest {
        value: WorkspaceAbsence {
            kind: WorkspaceKind::Workspace,
            id: minted_id(),
            absent: false,
            observed_at: instant(),
        },
        instants: 1,
    }
}

impl WorkspaceAbsence {
    /// The bytes the widest absence of a workspace occupies as compact JSON, counted as
    /// [`ExecAbsence::widest_json_bytes`] counts an exec's.
    #[must_use]
    pub fn widest_json_bytes() -> usize {
        widest_workspace_absence().json_bytes()
    }
}

/// A host-process timestamp of [`MAX_TIMESTAMP_CHARS`] characters: nanoseconds and a numeric
/// offset. This type writes its offset, so the value itself has the widest form.
fn widest_process_timestamp() -> ProcessTimestamp {
    let instant = time::OffsetDateTime::UNIX_EPOCH + time::Duration::nanoseconds(999_999_999);
    ProcessTimestamp(
        time::UtcOffset::from_hms(23, 59, 0).map_or(instant, |offset| instant.to_offset(offset)),
    )
}

/// The record of a host process started with these members, at its widest.
pub(crate) fn widest_process_record(
    argv: &[String],
    cwd: &str,
    env_names: &[String],
    owner: &ProcessOwner,
    reason: &str,
) -> ProcessRecord {
    ProcessRecord {
        argv: argv.to_vec(),
        confinement: Box::new(longest_name!(ProcessConfinement { None })),
        // `cpu`, `memory` and `listening` are what a read observes of a live process; a stored
        // record has none of the three.
        cpu: ProcessPresence::Absent,
        cwd: cwd.to_owned(),
        deadline: ProcessPresence::Present(Box::new(ProcessDeadline {
            at: widest_process_timestamp(),
            set_by: Box::new(longest_name!(ProcessDeadlineSource { ExpiresAt, Timeout })),
        })),
        ended_by: ProcessPresence::Present(Box::new(longest_name!(ProcessEndedBy {
            Exit,
            ExpiresAt,
            LeaseExpiry,
            Stop,
            Timeout,
            Vanished,
        }))),
        env_digest: Box::new(ProcessEnvDigest(digest_name())),
        env_names: env_names.to_vec(),
        exit: ProcessPresence::Present(Box::new(ProcessExit {
            code: ProcessPresence::Present(serde_json::Number::from(u64::MAX)),
            signal: ProcessPresence::Present(costliest(MAX_PROCESS_EXIT_SIGNAL_CHARS)),
        })),
        expires_at: ProcessPresence::Present(widest_process_timestamp()),
        id: minted_id(),
        kind: Box::new(longest_name!(ProcessKind { Process })),
        lease: Box::new(ProcessLease {
            keep: false,
            keep_reason: ProcessPresence::Present(costliest(MAX_PROCESS_REASON_BYTES)),
            renew_by: ProcessPresence::Present(widest_process_timestamp()),
            state: ProcessPresence::Present(Box::new(longest_name!(ProcessLeaseState {
                Active,
                Expired,
                Expiring,
            }))),
            ttl_ms: ProcessPresence::Present(serde_json::Number::from(u64::MAX)),
        }),
        listening: ProcessPresence::Absent,
        memory: ProcessPresence::Absent,
        observed_at: widest_process_timestamp(),
        owner: ProcessPresence::Present(Box::new(owner.clone())),
        profile: Box::new(longest_name!(ProcessProfile { Host })),
        reason: reason.to_owned(),
        started_at: widest_process_timestamp(),
        state: Box::new(longest_name!(ProcessState {
            Ended,
            Running,
            Starting,
            Stopping,
        })),
        timeout_ms: ProcessPresence::Present(serde_json::Number::from(u64::MAX)),
    }
}

impl ProcessRecord {
    /// The bytes the widest stored record of a host process started with these members occupies
    /// as compact JSON: `argv`, `cwd`, the environment names, the `owner` and the `reason` as
    /// sent, and every other member of a stored record present and at its bound, among them the
    /// `exit` with a code and a signal, `ended_by`, `expires_at`, the `deadline`, `timeout_ms`
    /// and a lease with a `keep_reason` of [`MAX_PROCESS_REASON_BYTES`].
    ///
    /// No stored form of a record this start creates is wider, in any state. The figure leaves
    /// out `cpu`, `memory` and `listening`, which a read observes of a live process and no
    /// stored record carries. A caller that can record at most some number of bytes for a
    /// result decides with this, before anything is started, whether every later result of the
    /// process can be recorded.
    /// `the_widest_process_record_bounds_every_stored_form_of_its_process` holds the figure.
    #[must_use]
    pub fn widest_json_bytes(
        argv: &[String],
        cwd: &str,
        env_names: &[String],
        owner: &ProcessOwner,
        reason: &str,
    ) -> usize {
        json_bytes(&widest_process_record(argv, cwd, env_names, owner, reason))
    }
}
