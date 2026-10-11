//! Adversarial cases against ADR 0033's lexical rules.
//!
//! The `0.18.0` bundle declares both halves of a read-only file as `host-absolute-path`, whose
//! pattern excludes U+0000. The wire crate is the daemon's and the driver's only lexical check, so
//! a path the published schema rejects must be refused here by name, not carried to the spawn.

use substrate_wire::{
    MAX_PATH_DEPTH, MAX_READ_ONLY_FILE_COMPONENT_BYTES, MAX_READ_ONLY_FILE_PATH_BYTES,
    READ_ONLY_FILE_PATH_INVALID, ReadOnlyFile, ReadOnlyRoot, WireValidationError,
    read_only_file_refusal, validate_read_only_files, validate_read_only_roots,
};

fn refusal(host_path: &str, mount: &str) -> Option<&'static str> {
    let file = ReadOnlyFile {
        host_path: host_path.to_owned(),
        mount: mount.to_owned(),
    };
    validate_read_only_files(&[file], &[])
        .err()
        .and_then(|error| read_only_file_refusal(&error))
}

/// An absolute path of exactly `bytes` bytes under `/opt`, in components of at most `component`
/// bytes, so only the total length is at stake.
fn path_of(bytes: usize, component: usize) -> String {
    let mut path = String::from("/opt");
    while path.len() < bytes {
        let room = bytes - path.len() - 1;
        path.push('/');
        path.push_str(&"a".repeat(room.min(component)));
    }
    assert_eq!(path.len(), bytes);
    path
}

#[test]
fn a_path_at_the_published_byte_bound_is_admitted_and_one_byte_over_is_refused() {
    let at = path_of(MAX_READ_ONLY_FILE_PATH_BYTES, 200);
    let over = path_of(MAX_READ_ONLY_FILE_PATH_BYTES + 1, 200);
    assert!(at.split('/').count() - 1 <= MAX_PATH_DEPTH);
    assert_eq!(refusal(&at, "/opt/gate"), None, "host_path at the bound");
    assert_eq!(refusal("/opt/tools/gate", &at), None, "mount at the bound");
    assert_eq!(
        refusal(&over, "/opt/gate"),
        Some(READ_ONLY_FILE_PATH_INVALID)
    );
    assert_eq!(
        refusal("/opt/tools/gate", &over),
        Some(READ_ONLY_FILE_PATH_INVALID)
    );
}

#[test]
fn the_byte_bound_counts_bytes_not_characters() {
    // Fewer characters than the bound, more bytes: the kernel counts bytes.
    let wide = format!(
        "/opt/{}",
        "\u{e9}".repeat(MAX_READ_ONLY_FILE_PATH_BYTES / 2)
    );
    assert!(wide.chars().count() <= MAX_READ_ONLY_FILE_PATH_BYTES);
    assert!(wide.len() > MAX_READ_ONLY_FILE_PATH_BYTES);
    assert_eq!(
        refusal(&wide, "/opt/gate"),
        Some(READ_ONLY_FILE_PATH_INVALID)
    );
}

#[test]
fn a_component_over_the_name_bound_is_refused() {
    let at = format!("/opt/{}", "a".repeat(MAX_READ_ONLY_FILE_COMPONENT_BYTES));
    let over = format!(
        "/opt/{}",
        "a".repeat(MAX_READ_ONLY_FILE_COMPONENT_BYTES + 1)
    );
    assert_eq!(refusal(&at, "/opt/gate"), None);
    assert_eq!(refusal("/opt/tools/gate", &at), None);
    assert_eq!(
        refusal(&over, "/opt/gate"),
        Some(READ_ONLY_FILE_PATH_INVALID)
    );
    assert_eq!(
        refusal("/opt/tools/gate", &over),
        Some(READ_ONLY_FILE_PATH_INVALID)
    );
}

#[test]
fn a_path_deeper_than_the_published_depth_is_refused() {
    let at = "/a".repeat(MAX_PATH_DEPTH);
    let over = "/a".repeat(MAX_PATH_DEPTH + 1);
    assert_eq!(refusal(&at, "/opt/gate"), None);
    assert_eq!(
        refusal(&over, "/opt/gate"),
        Some(READ_ONLY_FILE_PATH_INVALID)
    );
    assert_eq!(
        refusal("/opt/tools/gate", &over),
        Some(READ_ONLY_FILE_PATH_INVALID)
    );
}

/// The same lexical helper serves ADR 0010's roots, which carried a NUL to the spawn as well.
#[test]
fn a_nul_byte_in_a_read_only_root_is_refused_as_an_invalid_root_path() {
    for (host_path, mount) in [
        ("/opt/tools\u{0}x", "/toolchain"),
        ("/opt/tools", "/tool\u{0}x"),
    ] {
        let root = ReadOnlyRoot {
            host_path: host_path.to_owned(),
            mount: mount.to_owned(),
        };
        assert_eq!(
            validate_read_only_roots(&[root]),
            Err(WireValidationError::InvalidReadOnlyRootPath),
            "{host_path:?} -> {mount:?}"
        );
    }
}

#[test]
fn a_nul_byte_in_a_mount_is_refused_as_an_invalid_path() {
    let file = ReadOnlyFile {
        host_path: "/opt/tools/gate".to_owned(),
        mount: "/opt/gate\u{0}x".to_owned(),
    };
    let refusal = validate_read_only_files(&[file], &[])
        .err()
        .and_then(|error| read_only_file_refusal(&error));
    assert_eq!(refusal, Some(READ_ONLY_FILE_PATH_INVALID));
}

#[test]
fn a_nul_byte_in_a_host_path_is_refused_as_an_invalid_path() {
    let file = ReadOnlyFile {
        host_path: "/opt/tools/gate\u{0}x".to_owned(),
        mount: "/opt/gate".to_owned(),
    };
    let refusal = validate_read_only_files(&[file], &[])
        .err()
        .and_then(|error| read_only_file_refusal(&error));
    assert_eq!(refusal, Some(READ_ONLY_FILE_PATH_INVALID));
}
