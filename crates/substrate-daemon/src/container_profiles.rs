//! Deployment-owned node prerequisite; never called by a daemon or workspace request.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, ensure};

const ROOT: &str = "/node-profiles";
const NAME: &str = "substrate-host-exec-v1";
const APPARMOR: &str = "substrate-host-exec-v1.apparmor";
const SECCOMP: &str = "host-exec-v1-amd64.json";
const POLICY: &[u8] = include_bytes!("../../../deploy/apparmor/substrate-host-execution");
const SYSCALLS: &[u8] = include_bytes!("../../../deploy/seccomp/host-exec-v1-amd64.json");

pub fn check() -> Result<()> {
    validate_root()?;
    verify_file(&Path::new(ROOT).join(APPARMOR), POLICY)?;
    verify_file(&Path::new(ROOT).join(SECCOMP), SYSCALLS)?;
    ensure!(
        profile_lines()?
            .lines()
            .any(|line| line == format!("{NAME} (enforce)")),
        "the versioned AppArmor profile is not enforced"
    );
    Ok(())
}

pub fn install() -> Result<()> {
    validate_root()?;
    let root = Path::new(ROOT);
    let policy = root.join(APPARMOR);
    if profile_lines()?
        .lines()
        .any(|line| line.starts_with(&format!("{NAME} (")))
    {
        verify_file(&policy, POLICY)
            .context("refuse a loaded profile without its exact protected source")?;
    }
    install_file(&policy, POLICY)?;
    install_file(&root.join(SECCOMP), SYSCALLS)?;
    let status = Command::new("/sbin/apparmor_parser")
        .env_clear()
        .args(["--replace", "--skip-read-cache"])
        .arg(&policy)
        .status()
        .context("load the compiled-in AppArmor policy")?;
    ensure!(status.success(), "AppArmor profile installation failed");
    check()
}

fn profile_lines() -> Result<String> {
    fs::read_to_string("/sys/kernel/security/apparmor/profiles")
        .context("read the node AppArmor profile registry")
}

fn validate_root() -> Result<()> {
    let metadata = fs::symlink_metadata(ROOT)?;
    ensure!(
        metadata.is_dir() && metadata.uid() == 0 && metadata.mode() & 0o022 == 0,
        "the dedicated node profile directory must be protected and root-owned"
    );
    Ok(())
}

fn verify_file(path: &Path, expected: &[u8]) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && metadata.uid() == 0 && metadata.mode() & 0o6022 == 0,
        "node profile file must be protected, regular and root-owned"
    );
    ensure!(
        metadata.len() == expected.len() as u64,
        "node profile length differs"
    );
    ensure!(
        fs::read(path)? == expected,
        "versioned node profile bytes differ"
    );
    Ok(())
}

fn install_file(path: &Path, bytes: &[u8]) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => return verify_file(path, bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let parent = path.parent().context("profile parent")?;
    let temporary = parent.join(format!(".pending-{}", ulid::Ulid::generate()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        // A same-directory hard link publishes atomically and never overwrites an existing name.
        match fs::hard_link(&temporary, path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        fs::File::open(parent)?.sync_all()?;
        verify_file(path, bytes)
    })();
    let cleanup = fs::remove_file(temporary);
    result?;
    cleanup?;
    Ok(())
}
