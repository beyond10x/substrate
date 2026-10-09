//! The exec CPU ceiling is operator configuration (`story:configurable-exec-cpu-ceiling`).
//!
//! A ceiling the host cannot honour is refused when the host opens, by name and before anything is
//! written, never clamped to something it can. Portable: the bubblewrap path names nothing, so no
//! case needs a delegated cgroup; the quota each ceiling produces is unit-tested beside
//! `exec_cpu_quota` in `src/process.rs`.

use substrate_host::{DriverErrorClass, HostConfig, HostDriver};
use tempfile::TempDir;

fn config(directory: &TempDir, exec_cpu_cores: u32) -> HostConfig {
    let mut config = HostConfig::minimum(directory.path().join("workspaces"));
    config.bubblewrap = directory.path().join("no-sandbox-binary");
    config.exec_cpu_cores = exec_cpu_cores;
    config
}

fn usable_cpus() -> u32 {
    let available = std::thread::available_parallelism().expect("usable CPU count");
    u32::try_from(available.get()).expect("CPU count fits u32")
}

fn assert_refused(exec_cpu_cores: u32) {
    let directory = tempfile::tempdir().expect("test-owned root");
    let config = config(&directory, exec_cpu_cores);
    let workspace_root = config.workspace_root.clone();

    let Err(error) = HostDriver::open(config) else {
        panic!("a ceiling of {exec_cpu_cores} cores must be refused");
    };

    assert_eq!(error.code, "config.exec-cpu-cores-invalid");
    assert_eq!(error.class, DriverErrorClass::Refused);
    assert!(
        !workspace_root.exists(),
        "a refused configuration must write nothing on the host"
    );
}

#[test]
fn host_open_refuses_an_exec_cpu_ceiling_of_zero() {
    assert_refused(0);
}

#[test]
fn host_open_refuses_an_exec_cpu_ceiling_above_the_usable_cpus() {
    assert_refused(usable_cpus().saturating_add(1));
    assert_refused(u32::MAX);
}

#[test]
fn host_open_admits_the_default_and_every_usable_cpu_count() {
    for exec_cpu_cores in [1, usable_cpus()] {
        let directory = tempfile::tempdir().expect("test-owned root");
        HostDriver::open(config(&directory, exec_cpu_cores))
            .unwrap_or_else(|error| panic!("{exec_cpu_cores} cores must be admitted: {error}"));
    }
}
