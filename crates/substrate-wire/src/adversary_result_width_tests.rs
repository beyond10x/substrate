//! Adversary cases for the widths of the results the operation ledger records (ADR 0053
//! decisions 6 and 7): the rules a width rests on that no other case of this crate states.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde_json::Number;

use super::result_widths::json_bytes;
use super::{
    ApertureBytes, ApertureMechanism, ApertureMode, AppliedAperture, AppliedNetwork,
    AppliedWritableRoot, ConfinementRequest, Exec, ExecEnvironment, ExecKind, ExecLimits,
    ExecStartInput, FileKind, MAX_EGRESS_APERTURES, MAX_READ_ONLY_FILE_PATH_BYTES,
    MAX_WRITABLE_ROOTS, NetworkMode, SandboxProfile, SessionKind, WorkspaceAccess, WorkspaceKind,
    WritableHostRoot, WritableRoot, is_root_name, json_char_bytes, validate_operation_id,
};

/// A start that selects `apertures` apertures of one name and declares `roots` writable host
/// roots of one name and mount, and sets nothing else a result follows.
fn start(apertures: usize, roots: usize) -> ExecStartInput {
    ExecStartInput {
        workspace: "ws_01".to_owned(),
        argv: vec!["true".to_owned()],
        env: ExecEnvironment {
            allow: Vec::new(),
            set: BTreeMap::new(),
        },
        sandbox: ConfinementRequest {
            capability_snapshot: format!("sha256:{}", "a".repeat(64)),
            network: if apertures == 0 {
                NetworkMode::None
            } else {
                NetworkMode::Aperture
            },
            aperture: None,
            apertures: vec!["model".to_owned(); apertures],
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
        writable_roots: vec![
            WritableRoot::Host(WritableHostRoot {
                host: "cache".to_owned(),
                mount: "/cache".to_owned(),
            });
            roots
        ],
    }
}

fn width(apertures: usize, roots: usize) -> usize {
    Exec::widest_json_bytes(&start(apertures, roots))
}

fn count(bound: u32) -> usize {
    usize::try_from(bound).expect("a count")
}

/// The lease's `authorizing_operation` and the workspace's `root_name` are counted at one byte
/// for each character. Every character an operation identifier admits, and every character a
/// root name admits, costs one byte in a JSON string.
#[test]
fn a_member_counted_in_one_byte_characters_admits_no_costlier_character() {
    let mut identifier = String::new();
    let mut name = String::new();
    let mut admitted = (0_usize, 0_usize);
    for character in (0..=0x0010_ffff_u32).filter_map(char::from_u32) {
        identifier.clear();
        identifier.extend(std::iter::repeat_n(character, 16));
        if validate_operation_id(&identifier).is_ok() {
            assert_eq!(json_char_bytes(character), 1, "{character:?}");
            admitted.0 += 1;
        }
        name.clear();
        name.push('a');
        name.push(character);
        if is_root_name(&name) {
            assert_eq!(json_char_bytes(character), 1, "{character:?}");
            admitted.1 += 1;
        }
    }
    // The ASCII letters and digits, `_` and `-`.
    assert_eq!(admitted, (64, 64));
}

/// An integer is counted at twenty characters. No integer type a stored result holds, signed
/// or not, is written in more; a boolean is counted at the longer of its two values; and a
/// number that is present is wider than `null`.
#[test]
fn no_integer_a_stored_result_holds_is_written_in_more_than_twenty_characters() {
    assert_eq!(json_bytes(&u64::MAX), 20);
    assert_eq!(json_bytes(&i64::MIN), 20);
    assert_eq!(json_bytes(&Number::from(u64::MAX)), 20);
    assert_eq!(json_bytes(&Number::from(i64::MIN)), 20);
    assert!(json_bytes(&Number::from(i32::MIN)) < 20);
    assert!(json_bytes(&u32::MAX) < 20);
    assert!(json_bytes(&false) > json_bytes(&true));
    assert!(json_bytes(&None::<u64>) < json_bytes(&u64::MAX));
}

/// The bytes of the one name of a vocabulary. The `match` has no other arm: a second variant
/// does not compile here until the width that names the vocabulary counts its longest name.
macro_rules! the_one_name {
    ($type:ident :: $variant:ident) => {
        match $type::$variant {
            $type::$variant => json_bytes(&$type::$variant),
        }
    };
}

/// A width names each of these vocabularies by its one variant, so each has no other.
#[test]
fn a_vocabulary_a_width_names_by_one_variant_has_no_other() {
    assert_eq!(the_one_name!(ApertureMode::Aperture), "\"aperture\"".len());
    assert_eq!(
        the_one_name!(ApertureMechanism::LoopbackForwarder),
        "\"loopback-forwarder\"".len()
    );
    assert_eq!(the_one_name!(ExecKind::Exec), "\"exec\"".len());
    assert_eq!(the_one_name!(SessionKind::Session), "\"session\"".len());
    assert_eq!(
        the_one_name!(WorkspaceKind::Workspace),
        "\"workspace\"".len()
    );
    assert_eq!(the_one_name!(FileKind::File), "\"file\"".len());
    assert_eq!(
        the_one_name!(SandboxProfile::Workspace),
        "\"workspace\"".len()
    );
}

/// Which of its forms an applied network has. Every form is named: one that is added does not
/// compile here until the width of an exec counts it.
fn form_of(network: &AppliedNetwork) -> &'static str {
    match network {
        AppliedNetwork::None => "none",
        AppliedNetwork::Aperture(_) => "one",
        AppliedNetwork::Apertures(_) => "list",
    }
}

/// An exec's width counts the applied network as a list with `listen` on every element. Of
/// the forms an applied network has for one aperture, that one is the widest.
#[test]
fn the_list_is_the_widest_form_of_an_applied_network() {
    let aperture = |listen: Option<&str>| AppliedAperture {
        mode: ApertureMode::Aperture,
        name: "model".to_owned(),
        destination: "192.0.2.1:443".to_owned(),
        mechanism: ApertureMechanism::LoopbackForwarder,
        bytes: ApertureBytes {
            to_destination: u64::MAX,
            from_destination: u64::MAX,
        },
        max_bytes: Some(u64::MAX),
        listen: listen.map(str::to_owned),
    };
    let forms = [
        AppliedNetwork::None,
        AppliedNetwork::Aperture(aperture(None)),
        AppliedNetwork::Aperture(aperture(Some("127.0.0.1:443"))),
        AppliedNetwork::Apertures(vec![aperture(None)]),
        AppliedNetwork::Apertures(vec![aperture(Some("127.0.0.1:443"))]),
    ];
    assert_eq!(
        forms.iter().map(form_of).collect::<Vec<_>>(),
        ["none", "one", "one", "list", "list"]
    );
    let list = json_bytes(&forms[4]);
    for form in &forms[..4] {
        assert!(json_bytes(form) < list, "{form:?}");
    }
}

/// An entry of a list past the list's wire bound adds what an entry within the bound adds,
/// for an aperture and for a writable root, and a hundred of them add a hundred times that.
#[test]
fn an_exec_width_counts_each_entry_past_a_list_bound_as_each_one_within_it() {
    let roots = count(MAX_WRITABLE_ROOTS);
    let root = width(0, roots) - width(0, roots - 1);
    assert!(root > 6 * MAX_READ_ONLY_FILE_PATH_BYTES);
    assert_eq!(width(0, roots + 1) - width(0, roots), root);
    assert_eq!(width(0, roots + 100), width(0, roots) + 100 * root);

    let apertures = count(MAX_EGRESS_APERTURES);
    let aperture = width(apertures, 0) - width(apertures - 1, 0);
    assert_eq!(width(apertures + 1, 0) - width(apertures, 0), aperture);
    assert_eq!(
        width(apertures + 100, 0),
        width(apertures, 0) + 100 * aperture
    );
}

/// The root counted for a writable root of a start holds a host path of
/// `MAX_READ_ONLY_FILE_PATH_BYTES` characters at six bytes each. A bound root is within it for
/// every host path of at most that many bytes, whatever its characters. A host path one byte
/// longer is outside it, so a host path is held to the bound where a root is declared and
/// where a checkout is materialized (ADR 0053 decision 7).
#[test]
fn limit_a_bound_root_is_counted_for_a_host_path_of_at_most_the_published_bytes() {
    let roots = count(MAX_WRITABLE_ROOTS);
    // What one more root adds, without the comma before it.
    let counted = width(0, roots + 1) - width(0, roots) - 1;
    let bound = |host_path: String| {
        json_bytes(&AppliedWritableRoot::Host {
            name: "cache".to_owned(),
            host_path,
            mount: "/cache".to_owned(),
        })
    };
    assert_eq!(
        bound("\u{1}".repeat(MAX_READ_ONLY_FILE_PATH_BYTES)),
        counted
    );
    assert!(
        bound(format!(
            "/{}",
            "a".repeat(MAX_READ_ONLY_FILE_PATH_BYTES - 1)
        )) < counted
    );
    assert!(bound("\u{10ffff}".repeat(MAX_READ_ONLY_FILE_PATH_BYTES / 4)) < counted);
    assert!(bound("\"".repeat(MAX_READ_ONLY_FILE_PATH_BYTES)) < counted);
    assert_eq!(
        bound("\u{1}".repeat(MAX_READ_ONLY_FILE_PATH_BYTES + 1)),
        counted + 6
    );
}

/// An instant held as `DateTime<Utc>` is written in at most thirty characters, with every
/// length of fraction, through the last second of the year 9999.
#[test]
fn an_instant_is_written_in_at_most_thirty_characters_through_the_year_9999() {
    let last_second = 253_402_300_799_i64;
    for seconds in [0, 1, last_second] {
        for nanoseconds in [0_u32, 1, 1_000, 1_000_000, 999_999_999] {
            let instant: DateTime<Utc> =
                DateTime::from_timestamp(seconds, nanoseconds).expect("an instant");
            assert!(json_bytes(&instant) <= 30 + 2, "{instant:?}");
        }
    }
    let widest: DateTime<Utc> =
        DateTime::from_timestamp(last_second, 999_999_999).expect("an instant");
    assert_eq!(json_bytes(&widest), 30 + 2);
}
