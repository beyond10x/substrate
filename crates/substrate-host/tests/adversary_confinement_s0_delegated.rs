//! Adversary cases for `story:confinement-contract-on-linux-and-local` (delegated lane).
//!
//! Absent, never passed, without `SUBSTRATE_VECTORS_CGROUP_ROOT` naming a delegated cgroup v2
//! subtree this process is inside: each case prints `ABSENT` and returns.

#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use substrate_host::HostConfig;
use substrate_host::confinement::{
    self, AdapterRequest, Command, ConfinementAdapter, ConfinementError, ConfinementProfile,
    ContractFamily, linux::LinuxRequest,
};

fn delegated() -> Result<PathBuf, &'static str> {
    let root = PathBuf::from(
        std::env::var_os("SUBSTRATE_VECTORS_CGROUP_ROOT")
            .ok_or("SUBSTRATE_VECTORS_CGROUP_ROOT is unset")?,
    );
    std::fs::write(root.join("cgroup.subtree_control"), "+cpu +memory +pids")
        .map_err(|_| "the delegated cgroup root does not accept controllers")?;
    if !Path::new("/usr/bin/bwrap").is_file() {
        return Err("no bubblewrap at /usr/bin/bwrap");
    }
    Ok(root)
}

fn open_linux(host: &Path, cgroup_root: &Path) -> Arc<dyn ConfinementAdapter> {
    let config = HostConfig::minimum(host.join("workspaces"));
    confinement::open(AdapterRequest::Linux(Box::new(
        LinuxRequest::new(config).with_cgroup_root(Some(cgroup_root.to_path_buf())),
    )))
    .expect("linux opens")
}

/// `profile.rs` maps `writable_roots` to `filesystem_view`, and `mod.rs`: "A profile that needs
/// anything the adapter lacks is refused by name". `linux` serves `filesystem_view` from the
/// read-only-roots and read-only-files facts alone, so on a host that declared no writable root
/// (`exec.writable-roots.host` absent) a profile naming one is admitted, and only its start is
/// refused, by a driver code, with no audit record.
#[tokio::test(flavor = "multi_thread")]
async fn adv_linux_refuses_at_admission_a_writable_root_it_cannot_bind() {
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("adversary writable root: ABSENT ({reason})");
            return;
        }
    };
    let host = tempfile::tempdir().expect("host root");
    let adapter = open_linux(host.path(), &cgroup_root);
    assert!(
        adapter.facts().serves(&ContractFamily::FilesystemView),
        "{:?}",
        adapter.facts()
    );
    let workspace = host.path().join("workspaces").join("ws_writable");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let profile = ConfinementProfile {
        workspace: Some(workspace),
        writable_roots: vec![substrate_wire::WritableHostRoot {
            host: "tools".into(),
            mount: "/opt/tools".into(),
        }],
        ..ConfinementProfile::default()
    };
    let admitted = adapter.admit(&profile);
    let start = match &admitted {
        Ok(admitted) => Some(
            adapter
                .start(
                    admitted,
                    &Command {
                        argv: vec!["/bin/true".into()],
                        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
                    },
                )
                .await
                .map_err(|error| error.code().to_owned()),
        ),
        Err(_) => None,
    };
    assert!(
        matches!(
            &admitted,
            Err(ConfinementError::FamilyUnserved {
                family: ContractFamily::FilesystemView,
                ..
            })
        ),
        "a writable root the host cannot bind was admitted ({}); its start answered {start:?}",
        admitted.is_ok()
    );
    println!("adversary writable root: executed, refused at admission");
}

/// The story: `render` is "the program and argv the adapter would run ... used by `--dry-run` and
/// the parity test". No parity test exists. This one renders, starts the same command, reads the
/// bubblewrap process's argv from `/proc`, and compares the two with descriptor numbers masked.
#[tokio::test(flavor = "multi_thread")]
async fn adv_linux_render_is_the_argv_start_runs() {
    let cgroup_root = match delegated() {
        Ok(root) => root,
        Err(reason) => {
            println!("adversary render parity: ABSENT ({reason})");
            return;
        }
    };
    let host = tempfile::tempdir().expect("host root");
    let adapter = open_linux(host.path(), &cgroup_root);
    let workspace = host.path().join("workspaces").join("ws_parity");
    std::fs::create_dir_all(workspace.join("sub")).expect("workspace");
    let admitted = adapter
        .admit(&ConfinementProfile {
            workspace: Some(workspace),
            workdir: Some("sub".into()),
            read_only_roots: vec![substrate_wire::ReadOnlyRoot {
                host_path: "/etc".into(),
                mount: "/opt/etc".into(),
            }],
            ..ConfinementProfile::default()
        })
        .expect("admitted");
    let marker = format!(
        "adversary-parity-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    let command = Command {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            "/bin/sleep 20".into(),
            marker.clone(),
        ],
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
    };
    let rendered = adapter.render(&admitted, &command).expect("rendered");
    let run = adapter.start(&admitted, &command).await.expect("started");
    let program = rendered.program.to_string_lossy().into_owned();
    let mut observed = None;
    for _ in 0..300 {
        observed = std::fs::read_dir("/proc")
            .expect("proc")
            .filter_map(Result::ok)
            .filter_map(|entry| std::fs::read(entry.path().join("cmdline")).ok())
            .map(|bytes| {
                bytes
                    .split(|byte| *byte == 0)
                    .filter(|part| !part.is_empty())
                    .map(|part| String::from_utf8_lossy(part).into_owned())
                    .collect::<Vec<_>>()
            })
            .find(|argv| argv.first() == Some(&program) && argv.contains(&marker));
        if observed.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let _ = adapter.signal(&run, substrate_wire::Signal::Kill).await;
    let _ = tokio::time::timeout(Duration::from_secs(20), adapter.wait(&run)).await;
    let observed = observed.expect("the bubblewrap process carrying the marker");
    let mask = |argv: &[String]| {
        let mut masked = Vec::new();
        let mut previous: Option<&str> = None;
        for part in argv {
            let descriptor = matches!(
                previous,
                Some(
                    "--seccomp"
                        | "--block-fd"
                        | "--sync-fd"
                        | "--info-fd"
                        | "--bind-fd"
                        | "--ro-bind-fd"
                )
            );
            masked.push(if descriptor && part.parse::<u32>().is_ok() {
                "<fd>".to_owned()
            } else {
                part.clone()
            });
            previous = Some(part.as_str());
        }
        masked
    };
    let rendered_argv = mask(&rendered.args);
    let started_argv = mask(&observed[1..]);
    assert_eq!(
        rendered_argv, started_argv,
        "render differs from the argv start ran"
    );
    println!(
        "adversary render parity: executed, {} args",
        started_argv.len()
    );
}
