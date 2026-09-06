//! Linux privilege transition, isolated from the daemon library's unsafe-code prohibition.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, ensure};

const ROOT: &str = "/sys/fs/cgroup";
const INIT: &str = "/usr/bin/tini";
const UID: u32 = 65532;
const SYS_ADMIN: u32 = 21;
const BOOTSTRAP_CAPS: u64 = (1 << 0) | (1 << 6) | (1 << 7) | (1 << 8) | (1 << SYS_ADMIN);
const LIMITS: [&str; 4] = ["cpu.max", "memory.max", "memory.swap.max", "pids.max"];

pub fn run(quotas: bool, arguments: Vec<OsString>) -> Result<()> {
    validate_arguments(&arguments)?;
    let executable = if quotas {
        "/usr/local/bin/substrate-daemon-quota"
    } else {
        "/usr/local/bin/substrate-daemon"
    };
    for path in [INIT, executable] {
        let metadata = fs::symlink_metadata(path).context("fixed init/daemon executable")?;
        ensure!(
            safe_root_file(&metadata),
            "init/daemon executable must be a root-owned regular file without setuid/setgid or group/other write"
        );
    }
    let before = preflight(quotas)?;
    make_delegation()?;
    prepare_private_proc()?;
    ensure!(limits()? == before, "enclosing resource limits changed");
    drop_bootstrap_authority(quotas)?;
    for name in LIMITS {
        let result = fs::OpenOptions::new()
            .write(true)
            .open(Path::new(ROOT).join(name));
        ensure!(
            matches!(result, Err(ref error) if error.kind() == std::io::ErrorKind::PermissionDenied),
            "non-root write access to enclosing {name} was not denied"
        );
    }
    ensure!(
        limits()? == before,
        "enclosing resource limits changed after UID drop"
    );
    Err(Command::new(INIT)
        .env_remove("TINI_SUBREAPER")
        .env_remove("TINI_KILL_PROCESS_GROUP")
        .env_remove("TINI_VERBOSITY")
        .args(["--", executable])
        .args(arguments)
        .args(["--cgroup-root", ROOT])
        .exec())
    .context("exec fixed non-root init and daemon")
}

fn validate_arguments(arguments: &[OsString]) -> Result<()> {
    ensure!(
        !arguments.iter().any(|argument| {
            let bytes = argument.as_encoded_bytes();
            bytes == b"--cgroup-root" || bytes.starts_with(b"--cgroup-root=")
        }),
        "the delegation root cannot be overridden"
    );
    Ok(())
}

fn preflight(quotas: bool) -> Result<BTreeMap<String, String>> {
    // SAFETY: these identity queries take no pointers and change no process state.
    ensure!(
        unsafe { libc::getpid() == 1 && libc::geteuid() == 0 },
        "requires container PID 1 and UID 0"
    );
    ensure!(
        fs::read_to_string("/proc/self/cgroup")?.trim() == "0::/",
        "private cgroup namespace root required"
    );
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")?;
    validate_mount(&mountinfo)?;
    validate_proc_mount(&mountinfo)?;
    let label = fs::read_to_string("/proc/self/attr/apparmor/current")
        .or_else(|_| fs::read_to_string("/proc/self/attr/current"))?;
    ensure!(
        label.trim() == "substrate-host-exec-v1 (enforce)",
        "the exact enforced container execution AppArmor profile is required"
    );
    ensure!(
        prctl(libc::PR_GET_SECUREBITS, 0)? == 0,
        "unsupported securebits"
    );
    ensure!(
        !quotas || prctl(libc::PR_GET_NO_NEW_PRIVS, 0)? == 0,
        "quota file capability requires privilege acquisition at exec"
    );
    let caps = capabilities()?;
    for name in ["CapEff", "CapPrm", "CapBnd"] {
        ensure!(
            caps.get(name) == Some(&BOOTSTRAP_CAPS),
            "bootstrap requires exactly SYS_ADMIN, CHOWN, SETUID, SETGID and SETPCAP"
        );
    }
    ensure!(
        caps.get("CapInh") == Some(&0) && caps.get("CapAmb") == Some(&0),
        "inheritable and ambient capabilities must be empty"
    );
    let root = Path::new(ROOT);
    ensure!(
        fs::read_to_string(root.join("cgroup.procs"))?.trim() == "1",
        "bootstrap must be the only direct cgroup member"
    );
    for entry in fs::read_dir(root)? {
        ensure!(
            !entry?.file_type()?.is_dir(),
            "delegation already contains a child group"
        );
    }
    let controllers = fs::read_to_string(root.join("cgroup.controllers"))?;
    ensure!(
        ["cpu", "memory", "pids"]
            .iter()
            .all(|wanted| controllers.split_whitespace().any(|value| value == *wanted)),
        "cpu/memory/pids delegation is required"
    );
    let limits = limits()?;
    validate_enclosing_limits(&limits)?;
    Ok(limits)
}

fn validate_mount(mountinfo: &str) -> Result<()> {
    let mut matches = 0;
    for line in mountinfo.lines() {
        let Some((mount, filesystem)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<_> = mount.split_whitespace().collect();
        if fields.get(4) != Some(&ROOT) {
            continue;
        }
        matches += 1;
        ensure!(
            fields.get(3) == Some(&"/") && filesystem.split_whitespace().next() == Some("cgroup2"),
            "delegation must be the root of a cgroup2 mount"
        );
        let options = fields.get(5).context("missing cgroup mount options")?;
        ensure!(
            ["nosuid", "nodev", "noexec", "relatime"]
                .iter()
                .all(|wanted| options.split(',').any(|value| value == *wanted)),
            "cgroup mount safety flags are required"
        );
        ensure!(
            fields.len() == 6,
            "shared or propagated cgroup mounts are not supported"
        );
    }
    ensure!(matches == 1, "exactly one private cgroup mount is required");
    Ok(())
}

fn limits() -> Result<BTreeMap<String, String>> {
    LIMITS
        .into_iter()
        .map(|name| {
            let path = Path::new(ROOT).join(name);
            ensure!(
                safe_root_file(&fs::symlink_metadata(&path)?),
                "enclosing {name} must remain a protected root-owned regular file"
            );
            Ok((name.to_owned(), fs::read_to_string(path)?.trim().to_owned()))
        })
        .collect()
}

fn validate_proc_mount(mountinfo: &str) -> Result<()> {
    let mut matches = 0;
    for line in mountinfo.lines() {
        let Some((mount, filesystem)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<_> = mount.split_whitespace().collect();
        if fields.get(4) != Some(&"/proc") {
            continue;
        }
        matches += 1;
        ensure!(
            fields.len() == 6
                && fields.get(3) == Some(&"/")
                && filesystem.split_whitespace().next() == Some("proc"),
            "proc must be an unambiguous private procfs mount without propagation"
        );
        let options = fields[5];
        ensure!(
            ["nosuid", "nodev", "noexec"]
                .iter()
                .all(|wanted| options.split(',').any(|value| value == *wanted)),
            "proc mount safety flags are required"
        );
    }
    ensure!(matches == 1, "exactly one private proc mount is required");
    Ok(())
}

fn safe_root_file(metadata: &fs::Metadata) -> bool {
    safe_root_file_attributes(metadata.mode(), metadata.uid())
}

fn safe_root_file_attributes(mode: u32, uid: u32) -> bool {
    mode & libc::S_IFMT == libc::S_IFREG && uid == 0 && mode & 0o6022 == 0
}

fn validate_enclosing_limits(limits: &BTreeMap<String, String>) -> Result<()> {
    let cpu: Vec<_> = limits
        .get("cpu.max")
        .context("missing CPU ceiling")?
        .split_whitespace()
        .collect();
    ensure!(
        cpu.len() == 2
            && cpu
                .iter()
                .all(|value| value.parse::<u64>().is_ok_and(|value| value > 0)),
        "a finite enclosing CPU quota and period are required"
    );
    ensure!(
        limits
            .get("memory.max")
            .is_some_and(|value| value.parse::<u64>().is_ok_and(|value| value > 0)),
        "a finite enclosing memory ceiling is required"
    );
    Ok(())
}

fn make_delegation() -> Result<()> {
    let flags = libc::MS_BIND
        | libc::MS_REMOUNT
        | libc::MS_NOSUID
        | libc::MS_NODEV
        | libc::MS_NOEXEC
        | libc::MS_RELATIME;
    // SAFETY: the fixed NUL-terminated mount target was checked as a private cgroup2 root. A
    // bind-remount changes only this mount's flags, not the shared cgroup filesystem superblock.
    let result = unsafe {
        libc::mount(
            std::ptr::null(),
            c"/sys/fs/cgroup".as_ptr(),
            std::ptr::null(),
            flags,
            std::ptr::null(),
        )
    };
    syscall(result).context("make the private cgroup mount writable")?;
    let root = Path::new(ROOT);
    fs::create_dir(root.join("daemon"))?;
    fs::write(root.join("daemon/cgroup.procs"), "0")?;
    ensure!(
        fs::read_to_string(root.join("cgroup.procs"))?
            .trim()
            .is_empty(),
        "delegation root is not process-free"
    );
    let controllers = fs::read_to_string(root.join("cgroup.controllers"))?;
    let enabled = if controllers.split_whitespace().any(|name| name == "io") {
        "+cpu +memory +pids +io"
    } else {
        "+cpu +memory +pids"
    };
    fs::write(root.join("cgroup.subtree_control"), enabled)?;
    for path in [
        root.to_owned(),
        root.join("cgroup.procs"),
        root.join("cgroup.subtree_control"),
        root.join("daemon"),
        root.join("daemon/cgroup.procs"),
    ] {
        std::os::unix::fs::chown(path, Some(UID), Some(UID))?;
    }
    Ok(())
}

fn prepare_private_proc() -> Result<()> {
    // SAFETY: startup verified container PID 1 and the enforced profile which protects the
    // sensitive proc paths. This fixed mount exposes only the current private PID namespace;
    // no host proc path or namespace descriptor is accepted. It permits bubblewrap to mount
    // its own more restricted procfs without inheriting locked runtime masking submounts.
    syscall(unsafe {
        libc::mount(
            c"proc".as_ptr(),
            c"/proc".as_ptr(),
            c"proc".as_ptr(),
            libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
            std::ptr::null(),
        )
    })
    .context("prepare the private proc mount behind enforced path protections")
}

fn drop_bootstrap_authority(quotas: bool) -> Result<()> {
    let last: u32 = fs::read_to_string("/proc/sys/kernel/cap_last_cap")?
        .trim()
        .parse()?;
    ensure!(last < 64, "unsupported kernel capability width");
    for capability in 0..=last {
        if !quotas || capability != SYS_ADMIN {
            prctl(libc::PR_CAPBSET_DROP, capability.into())?;
        }
    }
    prctl(
        libc::PR_CAP_AMBIENT,
        libc::PR_CAP_AMBIENT_CLEAR_ALL as libc::c_ulong,
    )?;
    // SAFETY: the process is single-threaded PID 1. These calls discard all supplementary groups
    // and permanently replace all root UID/GID identities; keep-caps was refused in preflight.
    syscall(unsafe { libc::setgroups(0, std::ptr::null()) })?;
    syscall(unsafe { libc::setgid(UID) })?;
    syscall(unsafe { libc::setuid(UID) })?;
    let caps = capabilities()?;
    for name in ["CapEff", "CapPrm", "CapInh", "CapAmb"] {
        ensure!(
            caps.get(name) == Some(&0),
            "bootstrap capability drop did not complete"
        );
    }
    let expected = if quotas { 1_u64 << SYS_ADMIN } else { 0 };
    ensure!(
        caps.get("CapBnd") == Some(&expected),
        "unexpected final capability bounding set"
    );
    let status = fs::read_to_string("/proc/self/status")?;
    for name in ["Uid:", "Gid:"] {
        let line = status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .context("missing process identity")?;
        let identities: Vec<_> = line.split_whitespace().collect();
        ensure!(
            identities.len() == 4 && identities.iter().all(|value| *value == "65532"),
            "root identity remains after privilege drop"
        );
    }
    if !quotas {
        prctl(libc::PR_SET_NO_NEW_PRIVS, 1)?;
    }
    Ok(())
}

fn capabilities() -> Result<BTreeMap<String, u64>> {
    fs::read_to_string("/proc/self/status")?
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            ["CapEff", "CapPrm", "CapBnd", "CapInh", "CapAmb"]
                .contains(&name)
                .then(|| Ok((name.to_owned(), u64::from_str_radix(value.trim(), 16)?)))
        })
        .collect()
}

fn prctl(option: libc::c_int, argument: libc::c_ulong) -> Result<libc::c_int> {
    // SAFETY: callers use only the documented scalar capability/security options. No pointer
    // arguments are passed; all unused variadic arguments are zero machine words.
    let result = unsafe { libc::prctl(option, argument, 0_u64, 0_u64, 0_u64) };
    syscall(result).context("process capability/security control")?;
    Ok(result)
}

fn syscall(result: libc::c_int) -> std::io::Result<()> {
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_delegate_writable_limits_and_identity_changing_executables() {
        for mode in [0o644, 0o755] {
            assert!(safe_root_file_attributes(libc::S_IFREG | mode, 0));
            assert!(!safe_root_file_attributes(libc::S_IFREG | mode, UID));
        }
        for mode in [0o664, 0o646, 0o4755, 0o2755, 0o6755] {
            assert!(!safe_root_file_attributes(libc::S_IFREG | mode, 0));
        }
        assert!(!safe_root_file_attributes(libc::S_IFLNK | 0o755, 0));
        assert!(!safe_root_file_attributes(libc::S_IFDIR | 0o755, 0));
    }

    #[test]
    fn refuses_parent_mounts_and_propagation_instead_of_remounting_them() {
        let valid = "1 2 0:30 / /sys/fs/cgroup ro,nosuid,nodev,noexec,relatime - cgroup2 cgroup rw";
        assert!(validate_mount(valid).is_ok());
        for invalid in [
            valid.replace("0:30 / ", "0:30 /parent "),
            valid.replace(" - ", " shared:9 - "),
            valid.replace(",nodev", ""),
            valid.replace("cgroup2", "tmpfs"),
            format!("{valid}\n{valid}"),
        ] {
            assert!(validate_mount(&invalid).is_err());
        }
    }

    #[test]
    fn refuses_shared_or_ambiguous_proc_before_replacing_runtime_masks() {
        let valid = "1 2 0:31 / /proc rw,nosuid,nodev,noexec,relatime - proc proc rw";
        assert!(validate_proc_mount(valid).is_ok());
        for invalid in [
            valid.replace(" - ", " shared:9 - "),
            valid.replace(" - ", " master:9 - "),
            valid.replace(" - ", " unbindable - "),
            valid.replace("0:31 / ", "0:31 /parent "),
            valid.replace(",nodev", ""),
            valid.replace(" - proc ", " - tmpfs "),
            format!("{valid}\n{valid}"),
            String::new(),
        ] {
            assert!(validate_proc_mount(&invalid).is_err());
        }
    }

    #[test]
    fn refuses_unlimited_parent_and_caller_selected_delegation() {
        let mut limits = BTreeMap::from([
            ("cpu.max".to_owned(), "10000 100000".to_owned()),
            ("memory.max".to_owned(), "67108864".to_owned()),
        ]);
        assert!(validate_enclosing_limits(&limits).is_ok());
        limits.insert("cpu.max".to_owned(), "max 100000".to_owned());
        assert!(validate_enclosing_limits(&limits).is_err());
        for option in ["--cgroup-root", "--cgroup-root=/host"] {
            assert!(validate_arguments(&[option.into()]).is_err());
        }
    }
}
