//! Final-image checks: Docker's archive API retains file capabilities; `docker cp` extraction
//! alone does not prove them. Process observations come from the local daemon's host procfs.

use std::collections::BTreeMap;
use std::fs;
use std::process::{Command, ExitCode};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail, ensure};
use clap::Args as ClapArgs;
use sha2::{Digest as _, Sha256};
use ulid::Ulid;

const DAEMON: &str = "/usr/local/bin/substrate-daemon";
const QUOTA: &str = "/usr/local/bin/substrate-daemon-quota";
const SYS_ADMIN: u64 = 1 << 21;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Final daemon image, already loaded into a local rootful Linux Docker engine.
    #[arg(long, value_name = "IMAGE")]
    image: String,
}

pub fn run(args: &Args) -> Result<ExitCode> {
    let endpoint = std::env::var("DOCKER_HOST").unwrap_or(docker_text(&[
        "context",
        "inspect",
        "--format",
        "{{.Endpoints.docker.Host}}",
    ])?);
    ensure!(
        endpoint.starts_with("unix://"),
        "image startup requires local Linux Docker for procfs observations"
    );
    let mut passed = 0;
    let result: Result<()> = (|| {
        check_files(&args.image)?;
        passed += 1;
        for (name, quota, bounding) in [
            ("default without capabilities", false, 0),
            ("default with SYS_ADMIN bounding only", false, SYS_ADMIN),
            ("explicit quota startup", true, SYS_ADMIN),
        ] {
            check_process(&args.image, name, quota, bounding)?;
            passed += 1;
        }
        Ok(())
    })();
    println!(
        "image startup: {passed} passed; {} failed",
        usize::from(result.is_err())
    );
    result?;
    Ok(ExitCode::SUCCESS)
}

fn docker(args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("docker")
        .args(args)
        .output()
        .context("run Docker")?;
    ensure!(
        output.status.success(),
        "docker {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn docker_text(args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(docker(args)?)?.trim().to_owned())
}

struct Container(String);

impl Container {
    fn create(image: &str, quota: bool, bounding: u64) -> Result<Self> {
        let name = format!(
            "substrate-image-startup-{}",
            Ulid::generate().to_string().to_lowercase()
        );
        let mut argv = vec![
            "create",
            "--name",
            &name,
            "--network",
            "none",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--pids-limit",
            "64",
            "--memory",
            "256m",
            "--env",
            "TOKIO_WORKER_THREADS=2",
            "--tmpfs",
            "/var/lib/b10x-substrate:rw,nosuid,nodev,uid=65532,gid=65532,mode=0700",
        ];
        if bounding == SYS_ADMIN {
            argv.extend(["--cap-add", "SYS_ADMIN"]);
        }
        if quota {
            argv.extend(["--entrypoint", QUOTA]);
        }
        argv.extend([
            "--",
            image,
            "--socket",
            "/var/lib/b10x-substrate/substrate.sock",
            "--state",
            "/var/lib/b10x-substrate/state.db",
            "--workspaces",
            "/var/lib/b10x-substrate/workspaces",
            "--deployment",
            "image-startup",
            "--allow-uid",
            "65532",
        ]);
        docker(&argv)?;
        Ok(Self(name))
    }

    fn file(&self, path: &str) -> Result<ImageFile> {
        let source = format!("{}:{path}", self.0);
        image_file(&docker(&["cp", &source, "-"])?)
            .with_context(|| format!("inspect final-image {path}"))
    }

    fn remove(mut self) -> Result<()> {
        docker(&["rm", "--force", "--volumes", &self.0])?;
        self.0.clear();
        Ok(())
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        if self.0.is_empty() {
            return;
        }
        if let Err(error) = docker(&["rm", "--force", "--volumes", &self.0]) {
            eprintln!("image startup cleanup {}: {error:#}", self.0);
        }
    }
}

fn check_files(image: &str) -> Result<()> {
    let entrypoint: Vec<String> = serde_json::from_str(&docker_text(&[
        "image",
        "inspect",
        "--format",
        "{{json .Config.Entrypoint}}",
        image,
    ])?)?;
    ensure!(
        entrypoint == [DAEMON],
        "default entrypoint changed: {entrypoint:?}"
    );
    let container = Container::create(image, false, 0)?;
    let daemon = container.file(DAEMON)?;
    let quota = container.file(QUOTA)?;
    verify_files(&daemon, &quota)?;
    container.remove()?;
    println!(
        "PASS final-image files: root:root 0755, byte-identical sha256={}, default has no file capabilities, quota has only cap_sys_admin=ep",
        hex::encode(Sha256::digest(&daemon.bytes))
    );
    Ok(())
}

#[derive(Debug)]
struct ImageFile {
    uid: u64,
    gid: u64,
    mode: u64,
    capability: Option<Vec<u8>>,
    bytes: Vec<u8>,
}

fn verify_files(daemon: &ImageFile, quota: &ImageFile) -> Result<()> {
    for file in [daemon, quota] {
        ensure!(
            (file.uid, file.gid, file.mode) == (0, 0, 0o755),
            "daemon file must be root:root mode 0755"
        );
    }
    ensure!(
        daemon.capability.is_none(),
        "ordinary daemon has a file capability"
    );
    let expected: Vec<_> = [0x0200_0001_u32, 1 << 21, 0, 0, 0]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let mut namespaced = expected.clone();
    namespaced[3] = 3;
    namespaced.extend(0_u32.to_le_bytes());
    ensure!(
        quota
            .capability
            .as_ref()
            .is_some_and(|value| value == &expected || value == &namespaced),
        "quota daemon must carry only root-owned cap_sys_admin=ep"
    );
    ensure!(
        !daemon.bytes.is_empty() && daemon.bytes == quota.bytes,
        "daemon executables differ or are empty"
    );
    Ok(())
}

// Accept the bounded regular-file/PAX archive returned for one exact Docker path. Preserve the
// binary security.capability value instead of unpacking it under the check runner's uid.
fn image_file(archive: &[u8]) -> Result<ImageFile> {
    let mut offset = 0;
    let mut capability = None;
    let mut file = None;
    while let Some(header) = archive.get(offset..offset + 512) {
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let checksum = header
            .iter()
            .enumerate()
            .map(|(index, byte)| {
                if (148..156).contains(&index) {
                    32
                } else {
                    u64::from(*byte)
                }
            })
            .sum::<u64>();
        ensure!(
            octal(&header[148..156])? == checksum,
            "invalid Docker tar header checksum"
        );
        let size = usize::try_from(octal(&header[124..136])?)?;
        ensure!(
            size <= 128 * 1024 * 1024,
            "unexpected Docker archive member size"
        );
        let start = offset + 512;
        let data = archive
            .get(start..start + size)
            .context("truncated Docker archive")?;
        match header[156] {
            b'x' => capability = pax_capability(data)?,
            b'0' | 0 => {
                ensure!(file.is_none(), "multiple files in Docker archive");
                file = Some(ImageFile {
                    uid: octal(&header[108..116])?,
                    gid: octal(&header[116..124])?,
                    mode: octal(&header[100..108])?,
                    capability: capability.take(),
                    bytes: data.to_vec(),
                });
            }
            kind => bail!("unexpected Docker archive member type {kind}"),
        }
        offset = start + size.div_ceil(512) * 512;
    }
    file.context("Docker archive contains no regular file")
}

fn octal(bytes: &[u8]) -> Result<u64> {
    Ok(u64::from_str_radix(
        std::str::from_utf8(bytes)?.trim_matches(['\0', ' ']),
        8,
    )?)
}

fn pax_capability(mut data: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut capability = None;
    while !data.is_empty() {
        let space = data
            .iter()
            .position(|byte| *byte == b' ')
            .context("invalid PAX length")?;
        let length: usize = std::str::from_utf8(&data[..space])?.parse()?;
        ensure!(
            length > space + 2 && length <= data.len() && data[length - 1] == b'\n',
            "invalid PAX record length"
        );
        let record = &data[space + 1..length - 1];
        let equal = record
            .iter()
            .position(|byte| *byte == b'=')
            .context("invalid PAX record")?;
        if &record[..equal] == b"SCHILY.xattr.security.capability" {
            ensure!(capability.is_none(), "duplicate file capability");
            capability = Some(record[equal + 1..].to_vec());
        }
        data = &data[length..];
    }
    Ok(capability)
}

fn check_process(image: &str, name: &str, quota: bool, bounding: u64) -> Result<()> {
    let container = Container::create(image, quota, bounding)?;
    docker(&["start", &container.0])?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let state: serde_json::Value = serde_json::from_str(&docker_text(&[
            "inspect",
            "--format",
            "{{json .State}}",
            &container.0,
        ])?)?;
        ensure!(
            state["Running"] == true,
            "{name} stopped: {}",
            docker_text(&["logs", &container.0])?
        );
        if docker_text(&["logs", &container.0])?.contains("substrate ready") {
            break;
        }
        ensure!(Instant::now() < deadline, "{name} did not become ready");
        thread::sleep(Duration::from_millis(100));
    }
    let pid: u32 =
        docker_text(&["inspect", "--format", "{{.State.Pid}}", &container.0])?.parse()?;
    let mut observed = 0;
    let mut workers = 0;
    for entry in fs::read_dir(format!("/proc/{pid}/task"))? {
        let status = fs::read_to_string(entry?.path().join("status"))?;
        verify_status(&status, if quota { SYS_ADMIN } else { 0 }, bounding)?;
        observed += 1;
        workers += usize::from(status.lines().any(|line| {
            line.strip_prefix("Name:")
                .is_some_and(|name| name.trim() == "tokio-rt-worker")
        }));
    }
    ensure!(
        observed >= 3 && workers >= 2,
        "{name}: expected main plus two Tokio workers, observed {observed} threads, {workers} workers"
    );
    container.remove()?;
    println!(
        "PASS {name}: {observed} threads ({workers} Tokio workers), UID/GID 65532, CapPrm/Eff={:#x}, CapBnd={bounding:#x}, CapInh/Amb=0",
        if quota { SYS_ADMIN } else { 0 }
    );
    Ok(())
}

fn verify_status(status: &str, active: u64, bounding: u64) -> Result<()> {
    let fields: BTreeMap<_, _> = status
        .lines()
        .filter_map(|line| line.split_once(':'))
        .collect();
    for key in ["Uid", "Gid"] {
        let values: Vec<u32> = fields
            .get(key)
            .with_context(|| format!("missing {key}"))?
            .split_whitespace()
            .map(str::parse)
            .collect::<std::result::Result<_, _>>()?;
        ensure!(values == [65532; 4], "unexpected {key}: {values:?}");
    }
    for (key, expected) in [
        ("CapInh", 0),
        ("CapPrm", active),
        ("CapEff", active),
        ("CapBnd", bounding),
        ("CapAmb", 0),
    ] {
        let observed = u64::from_str_radix(
            fields
                .get(key)
                .with_context(|| format!("missing {key}"))?
                .trim(),
            16,
        )?;
        ensure!(
            observed == expected,
            "unexpected {key}: {observed:#x}, expected {expected:#x}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(capability: Option<Vec<u8>>) -> ImageFile {
        ImageFile {
            uid: 0,
            gid: 0,
            mode: 0o755,
            capability,
            bytes: b"daemon".to_vec(),
        }
    }

    #[test]
    fn file_checks_reject_missing_excess_and_default_capabilities() {
        let cap: Vec<_> = [0x0200_0001_u32, 1 << 21, 0, 0, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        let daemon = file(None);
        let mut quota = file(Some(cap.clone()));
        verify_files(&daemon, &quota).unwrap();
        assert!(verify_files(&daemon, &file(None)).is_err());
        assert!(verify_files(&file(Some(cap)), &quota).is_err());
        quota.capability.as_mut().unwrap()[7] |= 1; // CAP_SYS_RESOURCE must never pass.
        assert!(verify_files(&daemon, &quota).is_err());
    }

    #[test]
    fn file_checks_reject_mutable_or_different_executables() {
        let cap: Vec<_> = [0x0200_0001_u32, 1 << 21, 0, 0, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        for alteration in 0..4 {
            let mut quota = file(Some(cap.clone()));
            match alteration {
                0 => quota.uid = 65532,
                1 => quota.gid = 65532,
                2 => quota.mode = 0o777,
                _ => quota.bytes.push(0),
            }
            assert!(verify_files(&file(None), &quota).is_err());
        }
    }

    #[test]
    fn process_checks_reject_dropped_or_inherited_quota_authority() {
        let status = "Uid:\t65532\t65532\t65532\t65532\nGid:\t65532\t65532\t65532\t65532\nCapInh:\t0\nCapPrm:\t200000\nCapEff:\t200000\nCapBnd:\t200000\nCapAmb:\t0\n";
        verify_status(status, SYS_ADMIN, SYS_ADMIN).unwrap();
        for bad in [
            status.replace("CapEff:\t200000", "CapEff:\t0"),
            status.replace("CapInh:\t0", "CapInh:\t200000"),
            status.replace("CapAmb:\t0", "CapAmb:\t200000"),
            status.replace("CapPrm:\t200000", "CapPrm:\t1200000"),
            status.replace("65532", "0"),
        ] {
            assert!(verify_status(&bad, SYS_ADMIN, SYS_ADMIN).is_err());
        }
    }

    #[test]
    fn pax_preserves_binary_capabilities_and_rejects_truncation() {
        let record = b"38 SCHILY.xattr.security.capability=\0\n";
        assert_eq!(pax_capability(record).unwrap(), Some(vec![0]));
        assert!(pax_capability(&record[..record.len() - 1]).is_err());
        assert!(image_file(b"not a tar file").is_err());
    }
}
