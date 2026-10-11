//! ADR 0033: the lexical rules for a declared read-only host file.
//!
//! The filesystem rules (absent, symlinked, not a regular file) are the driver's, because this
//! crate performs no I/O; `substrate-host` proves those.

use std::collections::BTreeSet;

use super::{
    APERTURE_CA_BUNDLE_PATH, APERTURE_HOSTS_PATH, EXEC_SCRATCH_MOUNT, EXECUTION_CAPSULE_MOUNT,
    ExecStartInput, MAX_READ_ONLY_FILES, READ_ONLY_FILE_REFUSAL_CODES, ReadOnlyFile, ReadOnlyRoot,
    WireValidationError, canonical_request_hash_v2, read_only_file_refusal,
    validate_read_only_files,
};

fn file(host: &str, mount: &str) -> ReadOnlyFile {
    ReadOnlyFile {
        host_path: host.to_owned(),
        mount: mount.to_owned(),
    }
}

fn root(host: &str, mount: &str) -> ReadOnlyRoot {
    ReadOnlyRoot {
        host_path: host.to_owned(),
        mount: mount.to_owned(),
    }
}

const PROGRAM: &str = "/srv/someone/.local/bin/gate";

#[test]
fn a_declared_file_is_admitted_when_both_paths_are_absolute_and_canonical() {
    assert_eq!(
        validate_read_only_files(
            &[
                file(PROGRAM, "/opt/gates/bin/gate"),
                file(
                    "/srv/someone/.config/gate/config.toml",
                    "/opt/gates/gate.toml"
                ),
            ],
            &[root("/srv/someone/.cargo", "/toolchain/cargo")],
        ),
        Ok(())
    );
    assert_eq!(
        validate_read_only_files(&[], &[]),
        Ok(()),
        "none is the default"
    );
    assert_eq!(
        validate_read_only_files(&[file(PROGRAM, PROGRAM)], &[]),
        Ok(()),
        "a file may appear at its own host path"
    );
}

#[test]
fn a_file_path_that_could_escape_or_be_re_pointed_is_refused_rather_than_normalised() {
    for (host, mount) in [
        ("relative/gate", "/opt/gate"),
        ("/srv/../etc/shadow", "/opt/gate"),
        ("/srv/someone/./gate", "/opt/gate"),
        ("/srv//someone/gate", "/opt/gate"),
        ("/srv/someone/gate/", "/opt/gate"),
        (PROGRAM, "opt/gate"),
        (PROGRAM, "/opt/../gate"),
        (PROGRAM, "/opt/gate/"),
        (PROGRAM, "/opt//gate"),
        ("/", "/opt/gate"),
        (PROGRAM, "/"),
    ] {
        assert_eq!(
            validate_read_only_files(&[file(host, mount)], &[]),
            Err(WireValidationError::InvalidReadOnlyFilePath),
            "{host} -> {mount}"
        );
    }
}

#[test]
fn a_file_may_not_take_or_shadow_a_mount_substrate_owns() {
    for mount in [
        "/usr",
        "/usr/local/bin/gate",
        "/bin",
        "/bin/gate",
        "/lib",
        "/lib64/gate",
        "/proc/gate",
        "/dev/gate",
        "/tmp/gate",
        "/workspace",
        "/workspace/gate",
        EXECUTION_CAPSULE_MOUNT,
        "/runtime/gate",
        EXEC_SCRATCH_MOUNT,
        "/scratch/gate",
        APERTURE_HOSTS_PATH,
        "/etc",
        APERTURE_CA_BUNDLE_PATH,
        "/etc/ssl",
    ] {
        assert_eq!(
            validate_read_only_files(&[file(PROGRAM, mount)], &[]),
            Err(WireValidationError::ReservedReadOnlyFileMount),
            "{mount}"
        );
    }
    assert_eq!(
        validate_read_only_files(&[file(PROGRAM, "/runtime2/gate")], &[]),
        Ok(()),
        "component comparison must not reject a similarly prefixed name"
    );
    assert_eq!(
        validate_read_only_files(&[file(PROGRAM, "/etc/gate.toml")], &[]),
        Ok(()),
        "a sibling of an aperture file is not the aperture file"
    );
}

#[test]
fn a_file_may_not_collide_with_a_declared_root_in_either_direction() {
    let roots = [root("/srv/someone/.cargo", "/toolchain/cargo")];
    for mount in [
        // The root's own mount point.
        "/toolchain/cargo",
        // Beneath the root: the file would shadow part of a directory the caller declared whole.
        "/toolchain/cargo/bin/gate",
        // A parent of the root: the root would be mounted inside a file.
        "/toolchain",
    ] {
        assert_eq!(
            validate_read_only_files(&[file(PROGRAM, mount)], &roots),
            Err(WireValidationError::OverlappingReadOnlyFileMount),
            "{mount}"
        );
    }
    assert_eq!(
        validate_read_only_files(&[file(PROGRAM, "/toolchain/cargo2")], &roots),
        Ok(()),
        "component comparison, not prefix comparison"
    );
}

#[test]
fn one_file_mount_may_not_lie_beneath_another() {
    assert_eq!(
        validate_read_only_files(
            &[
                file(PROGRAM, "/opt/gate"),
                file("/srv/someone/.config/gate.toml", "/opt/gate/config.toml"),
            ],
            &[],
        ),
        Err(WireValidationError::OverlappingReadOnlyFileMount)
    );
}

#[test]
fn two_files_cannot_name_one_mount_point() {
    assert_eq!(
        validate_read_only_files(
            &[
                file(PROGRAM, "/opt/gate"),
                file("/srv/someone/.local/bin/other", "/opt/gate"),
            ],
            &[],
        ),
        Err(WireValidationError::DuplicateReadOnlyFileMount)
    );
}

#[test]
fn the_bound_is_the_one_the_capability_snapshot_publishes() {
    let at_bound: Vec<ReadOnlyFile> = (0..MAX_READ_ONLY_FILES)
        .map(|index| file(PROGRAM, &format!("/opt/gates/{index}")))
        .collect();
    assert_eq!(validate_read_only_files(&at_bound, &[]), Ok(()));
    let over: Vec<ReadOnlyFile> = (0..=MAX_READ_ONLY_FILES)
        .map(|index| file(PROGRAM, &format!("/opt/gates/{index}")))
        .collect();
    assert_eq!(
        validate_read_only_files(&over, &[]),
        Err(WireValidationError::InvalidReadOnlyFileBounds)
    );
}

#[test]
fn every_lexical_refusal_has_its_own_published_code() {
    let named = [
        (
            WireValidationError::InvalidReadOnlyFileBounds,
            "exec.read-only-files-exceeded",
        ),
        (
            WireValidationError::InvalidReadOnlyFilePath,
            "exec.read-only-file-path-invalid",
        ),
        (
            WireValidationError::ReservedReadOnlyFileMount,
            "exec.read-only-file-mount-reserved",
        ),
        (
            WireValidationError::OverlappingReadOnlyFileMount,
            "exec.read-only-file-mount-overlap",
        ),
        (
            WireValidationError::DuplicateReadOnlyFileMount,
            "exec.read-only-file-mount-duplicate",
        ),
    ];
    for (error, code) in &named {
        assert_eq!(read_only_file_refusal(error), Some(*code), "{error:?}");
        assert!(
            READ_ONLY_FILE_REFUSAL_CODES.contains(code),
            "{code} is not published"
        );
    }
    assert_eq!(
        read_only_file_refusal(&WireValidationError::InvalidReadOnlyRootPath),
        None,
        "a root refusal is not a file refusal"
    );
    let distinct: BTreeSet<&str> = READ_ONLY_FILE_REFUSAL_CODES.iter().copied().collect();
    assert_eq!(distinct.len(), READ_ONLY_FILE_REFUSAL_CODES.len());
    assert_eq!(READ_ONLY_FILE_REFUSAL_CODES.len(), 9);
}

fn exec_start() -> serde_json::Value {
    serde_json::json!({
        "workspace": "ws_test",
        "argv": ["gate"],
        "env": {"allow": [], "set": {"PATH": "/opt/gates/bin:/usr/bin"}},
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

#[test]
fn an_absent_list_keeps_the_start_and_its_hash_byte_identical() {
    let value = exec_start();
    let input: ExecStartInput = serde_json::from_value(value.clone()).expect("decodes");
    assert!(input.read_only_files.is_empty());
    let encoded = serde_json::to_value(&input).expect("encodes");
    assert!(
        encoded.get("read_only_files").is_none(),
        "an empty list is skipped, so the ledger hash of an unchanged start is unchanged"
    );
    assert_eq!(
        canonical_request_hash_v2("POST", "/v1/execs", &encoded, None),
        canonical_request_hash_v2("POST", "/v1/execs", &value, None)
    );
}

#[test]
fn a_declared_list_round_trips_and_refuses_unknown_members() {
    let mut value = exec_start();
    value["read_only_files"] =
        serde_json::json!([{ "host_path": PROGRAM, "mount": "/opt/gates/bin/gate" }]);
    let input: ExecStartInput = serde_json::from_value(value.clone()).expect("decodes");
    assert_eq!(
        input.read_only_files,
        vec![file(PROGRAM, "/opt/gates/bin/gate")]
    );
    assert_eq!(serde_json::to_value(&input).expect("encodes"), value);
    value["read_only_files"][0]["writable"] = serde_json::json!(true);
    assert!(
        serde_json::from_value::<ExecStartInput>(value).is_err(),
        "a declared file has exactly two members"
    );
}
