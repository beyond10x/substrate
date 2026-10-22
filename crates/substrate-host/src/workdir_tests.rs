//! Design 26 § 1 on the host: the fact gate, the existence check against the objects the mounts
//! bind, and the bubblewrap diagnostic the backstop recognises.

use std::os::fd::OwnedFd;
use std::path::Path;

use substrate_wire::{
    EXEC_WORKDIR_INVALID, EXEC_WORKDIR_MISSING, EXEC_WORKDIR_OUTSIDE_MOUNTS, EXEC_WORKDIR_UNSERVED,
    ExecStartInput, ExecutionCapsuleInput, ReadOnlyRoot, StorageLimit,
};

use super::{admit, chdir_target, check_present, is_chdir_failure};
use crate::DriverErrorClass;

fn input(workdir: Option<&str>) -> ExecStartInput {
    let mut input: ExecStartInput = serde_json::from_value(serde_json::json!({
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
    }))
    .expect("decodes");
    input.workdir = workdir.map(str::to_owned);
    input
}

fn directory(path: &Path) -> OwnedFd {
    OwnedFd::from(std::fs::File::open(path).expect("open the fixture directory"))
}

/// A workspace with a directory, a nested directory, a file and two symlinks: one at the last
/// component and one above it.
fn workspace() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("fixture");
    std::fs::create_dir_all(root.path().join("crates/gate")).expect("directories");
    std::fs::write(root.path().join("Cargo.toml"), "").expect("file");
    std::os::unix::fs::symlink("crates", root.path().join("link")).expect("symlink");
    std::os::unix::fs::symlink("/", root.path().join("escape")).expect("symlink");
    root
}

fn code(result: Result<(), crate::DriverError>) -> Option<&'static str> {
    result.err().map(|error| error.code)
}

#[test]
fn absent_starts_in_the_workspace_and_a_named_directory_is_the_chdir_target() {
    assert_eq!(chdir_target(&input(None)), "/workspace");
    assert_eq!(
        chdir_target(&input(Some("/workspace/crates/gate"))),
        "/workspace/crates/gate"
    );
}

#[test]
fn a_start_without_a_workdir_needs_no_fact() {
    assert_eq!(code(admit(&input(None), None)), None);
}

#[test]
fn shape_is_refused_before_the_fact_is_asked() {
    for (workdir, expected) in [
        ("workspace/a", EXEC_WORKDIR_INVALID),
        ("/workspace/../etc", EXEC_WORKDIR_INVALID),
        ("/tmp", EXEC_WORKDIR_OUTSIDE_MOUNTS),
    ] {
        let error = admit(&input(Some(workdir)), None).expect_err("refused");
        assert_eq!(error.code, expected, "{workdir}");
        assert_eq!(error.class, DriverErrorClass::Refused);
        assert_eq!(error.address.as_deref(), Some("workdir"));
    }
}

#[test]
fn a_well_formed_workdir_on_a_host_without_the_fact_is_unserved_by_name() {
    let error = admit(&input(Some("/workspace/a")), None).expect_err("unserved");
    assert_eq!(error.code, EXEC_WORKDIR_UNSERVED);
    assert_eq!(error.class, DriverErrorClass::Unserved);
    assert_eq!(code(admit(&input(Some("/workspace/a")), Some(true))), None);
}

#[test]
fn a_directory_beneath_the_pinned_workspace_is_present() {
    let fixture = workspace();
    let pinned = directory(fixture.path());
    for workdir in ["/workspace", "/workspace/crates", "/workspace/crates/gate"] {
        assert_eq!(
            code(check_present(&input(Some(workdir)), &pinned, None)),
            None,
            "{workdir}"
        );
    }
    assert_eq!(code(check_present(&input(None), &pinned, None)), None);
}

#[test]
fn a_missing_entry_a_file_or_a_symlink_at_any_component_is_missing() {
    let fixture = workspace();
    let pinned = directory(fixture.path());
    for workdir in [
        "/workspace/absent",
        "/workspace/crates/absent",
        "/workspace/Cargo.toml",
        "/workspace/link",
        "/workspace/link/gate",
        "/workspace/escape",
        "/workspace/escape/usr",
    ] {
        let error = check_present(&input(Some(workdir)), &pinned, None).expect_err("refused");
        assert_eq!(error.code, EXEC_WORKDIR_MISSING, "{workdir}");
        assert_eq!(error.class, DriverErrorClass::Refused);
        assert_eq!(error.address.as_deref(), Some("workdir"));
        assert!(
            !error.message.contains(workdir),
            "the refusal does not echo the path"
        );
    }
}

#[test]
fn a_directory_beneath_a_declared_root_is_checked_beneath_that_root() {
    let workspace = tempfile::tempdir().expect("workspace");
    let pinned = directory(workspace.path());
    let root = self::workspace();
    let mut start = input(Some("/opt/tool/crates/gate"));
    start.read_only_roots = vec![ReadOnlyRoot {
        host_path: root.path().to_str().expect("UTF-8").to_owned(),
        mount: "/opt/tool".to_owned(),
    }];
    assert_eq!(code(check_present(&start, &pinned, None)), None);
    start.workdir = Some("/opt/tool".to_owned());
    assert_eq!(code(check_present(&start, &pinned, None)), None);
    for workdir in ["/opt/tool/link", "/opt/tool/absent", "/opt/tool/Cargo.toml"] {
        start.workdir = Some(workdir.to_owned());
        assert_eq!(
            code(check_present(&start, &pinned, None)),
            Some(EXEC_WORKDIR_MISSING),
            "{workdir}"
        );
    }
}

#[test]
fn scratch_is_fresh_so_only_its_mount_point_exists() {
    let workspace = tempfile::tempdir().expect("workspace");
    let pinned = directory(workspace.path());
    let mut start = input(Some("/scratch"));
    start.scratch = Some(StorageLimit {
        max_bytes: 1_048_576,
        max_inodes: 16,
    });
    assert_eq!(code(check_present(&start, &pinned, None)), None);
    start.workdir = Some("/scratch/build".to_owned());
    assert_eq!(
        code(check_present(&start, &pinned, None)),
        Some(EXEC_WORKDIR_MISSING)
    );
}

#[test]
fn a_directory_in_the_capsule_is_checked_beneath_the_materialized_capsule() {
    let workspace = tempfile::tempdir().expect("workspace");
    let pinned = directory(workspace.path());
    let capsule = self::workspace();
    let mut start = input(Some("/runtime/crates"));
    start.capsule = Some(ExecutionCapsuleInput {
        manifest_sha256: "0".repeat(64),
        entrypoint: "run".to_owned(),
        files: Vec::new(),
    });
    assert_eq!(
        code(check_present(&start, &pinned, Some(capsule.path()))),
        None
    );
    start.workdir = Some("/runtime/link".to_owned());
    assert_eq!(
        code(check_present(&start, &pinned, Some(capsule.path()))),
        Some(EXEC_WORKDIR_MISSING)
    );
}

#[test]
fn a_root_whose_host_directory_is_gone_holds_no_working_directory() {
    let workspace = tempfile::tempdir().expect("workspace");
    let pinned = directory(workspace.path());
    let root = tempfile::tempdir().expect("root");
    let host_path = root.path().join("gone");
    let mut start = input(Some("/opt/tool"));
    start.read_only_roots = vec![ReadOnlyRoot {
        host_path: host_path.to_str().expect("UTF-8").to_owned(),
        mount: "/opt/tool".to_owned(),
    }];
    assert_eq!(
        code(check_present(&start, &pinned, None)),
        Some(EXEC_WORKDIR_MISSING)
    );
}

#[test]
fn a_start_without_a_workdir_is_never_checked() {
    let workspace = tempfile::tempdir().expect("workspace");
    let pinned = directory(workspace.path());
    let mut start = input(None);
    start.read_only_roots = vec![ReadOnlyRoot {
        host_path: "/nonexistent-substrate-root".to_owned(),
        mount: "/opt/tool".to_owned(),
    }];
    assert_eq!(code(check_present(&start, &pinned, None)), None);
}

#[test]
fn the_backstop_recognises_exactly_the_chdir_diagnostic_for_the_requested_directory() {
    for output in [
        "bwrap: Can't chdir to /workspace/gone: No such file or directory\n",
        "bwrap: Can't chdir to /workspace/gone: Not a directory\n",
        "bwrap: Can't chdir to /workspace/gone: No such file or directory\r\n",
    ] {
        assert!(
            is_chdir_failure(output.as_bytes(), "/workspace/gone"),
            "{output:?}"
        );
    }
    for output in [
        "",
        "bwrap: Can't chdir to /workspace/other: No such file or directory\n",
        "bwrap: Can't chdir to /workspace/gone: \n",
        "bwrap: Can't chdir to /workspace/gone: Permission denied\n",
        "bwrap: Can't chdir to /workspace/gone: No such file or directory",
        "noise\nbwrap: Can't chdir to /workspace/gone: No such file or directory\n",
        "bwrap: Can't chdir to /workspace/gone: No such file or directory\nmore\n",
        "bwrap: Can't chdir to /workspace/gone/deeper: No such file or directory\n",
    ] {
        assert!(
            !is_chdir_failure(output.as_bytes(), "/workspace/gone"),
            "{output:?}"
        );
    }
}
