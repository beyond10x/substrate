//! The provisioned runner for the real project-quota lane.
//!
//! `cargo xtask quota-lane` runs the seven `real_quota_*` cases, but only on a filesystem mounted
//! with project quotas and with `SYS_ADMIN` in the inheritable and ambient sets for `quotactl`.
//! A developer host generally has neither, and a hosted runner never does. This verb is the one
//! documented command that provides both:
//!
//! ```console
//! cargo xtask quota-fixture run
//! ```
//!
//! It builds the checked-in recipe `fixtures/quota-lab/Dockerfile`, then runs one container with
//! only what the recorded 2026-09-05 execution used (`SYS_ADMIN` added to Docker's default set,
//! never `--privileged`) plus the loop-device access the filesystem needs. Inside, `quota-fixture
//! inside` (this same binary, built in the container) creates a sparse ext4 image with the
//! `quota` and `project` features, attaches it through one loop device with automatic detach,
//! mounts it `prjquota` at `/quota`, and drops to the
//! invoking user's identity with `SYS_ADMIN` inheritable and ambient before it runs
//! `quota-lane` against the read-only bind of this checkout.
//!
//! The container never sees the live checkout. The run reads `HEAD` once, extracts that commit with
//! `git archive` into a fresh directory under the target directory, and builds the recipe and runs
//! the lane from that snapshot; the snapshot is removed afterwards. Its image tag and both volumes
//! are named per checkout, so two checkouts never share a container build target.
//!
//! A green run ends with the release record `Quota-lane: <source-sha> <case-count> <RFC 3339 UTC>`,
//! the line `.github/workflows/release.yml` requires in the annotated tag (AGENTS.md,
//! § *Releases*), only when the checkout was clean before the run, is still clean after it, and
//! `HEAD` did not move. A dirty checkout, a moved `HEAD`, a lane that ran fewer cases than the
//! snapshot declares, or any failure prints no record.

use std::fs;
use std::io::{BufRead as _, BufReader};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::SystemTime;

use anyhow::{Context as _, Result, bail, ensure};
use clap::{Args as ClapArgs, Subcommand};
use sha2::{Digest as _, Sha256};

#[cfg(test)]
use crate::lane_record::rfc3339_utc;
use crate::lane_record::{git_text, record_line};

/// The checked-in recipe, relative to the workspace root.
pub const RECIPE: &str = "fixtures/quota-lab";
/// The local image repository the recipe is built under, tagged per checkout. Never pushed.
pub const IMAGE_REPOSITORY: &str = "b10x-substrate-quota-lab";
/// Prefix of the two per-checkout volumes: the container's build output and its crate cache.
const VOLUME_PREFIX: &str = "b10x-substrate-quota-lab";
/// Set by the recipe; `inside` refuses to provision anything without it.
const LAB_MARKER: &str = "B10X_SUBSTRATE_QUOTA_LAB";
/// The exclusive identity range, as in the recorded run.
pub const IDS: &str = "200000-200511";
/// Where the project-quota filesystem is mounted inside the container.
pub const QUOTA_ROOT: &str = "/quota";
const SOURCE_MOUNT: &str = "/src";
const TARGET_MOUNT: &str = "/target";
const REGISTRY_MOUNT: &str = "/usr/local/cargo/registry";
const LAB_DIRECTORY: &str = "/lab";
const IMAGE_FILE: &str = "/lab/quota.img";
/// Sparse; the cases set limits of a few MiB.
///
/// ext4, not XFS: XFS reports an exceeded project quota as `ENOSPC`, and `ProjectQuotas::probe`
/// (`crates/substrate-host/src/quota.rs`) requires `EDQUOT`, so an XFS fixture refuses every case.
/// The recorded 2026-09-05 run was ext4 as well.
const IMAGE_BYTES: u64 = 1 << 30;
const LOOP_CTL_GET_FREE: libc::c_ulong = 0x4C82;
const LOOP_MAJOR: u32 = 7;

#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    pub command: Verb,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Verb {
    /// Build the recipe and run the real project-quota lane inside it.
    Run(RunArgs),
    /// Inside the recipe's container only: provision the filesystem and run the lane.
    #[command(hide = true)]
    Inside(InsideArgs),
}

#[derive(Debug, Clone, ClapArgs)]
pub struct RunArgs {
    /// Build and run the cases in the release profile, as the recorded 2026-09-05 execution did.
    #[arg(long)]
    pub release: bool,
    /// Remove this checkout's image tag and its two volumes after the run, and nothing else.
    #[arg(long)]
    pub clean: bool,
}

#[derive(Debug, Clone, ClapArgs)]
pub struct InsideArgs {
    /// The identity the lane runs as; never root, which quota limits do not bind.
    #[arg(long)]
    pub uid: u32,
    #[arg(long)]
    pub gid: u32,
    #[arg(long)]
    pub release: bool,
}

pub fn run(args: &Args) -> Result<ExitCode> {
    match &args.command {
        Verb::Run(run) => host(run),
        Verb::Inside(inside) => in_container(inside),
    }
}

fn host(args: &RunArgs) -> Result<ExitCode> {
    let root = crate::repo::root()?;
    // `HEAD` and the clean state are read once, here; everything below runs from this commit.
    let sha = git_text(&root, &["rev-parse", "HEAD"])?;
    let clean_before = git_text(&root, &["status", "--porcelain"])?.is_empty();
    let source = git_text(
        &root,
        &["show", &format!("{sha}:{}", crate::quota_lane::SOURCE)],
    )
    .with_context(|| format!("reading {} at {sha}", crate::quota_lane::SOURCE))?;
    let declared = crate::quota_lane::declared_cases(&source).len();
    ensure!(
        declared > 0,
        "{} declares no ignored real-quota case; there is nothing to run",
        crate::quota_lane::SOURCE
    );

    let docker = Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match docker {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            eprintln!(
                "quota fixture: refusing — `docker info` failed for this user, so the recipe cannot \
                 run and none of the {declared} real-quota cases ran: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return Ok(ExitCode::FAILURE);
        }
        Err(error) => {
            eprintln!(
                "quota fixture: refusing — docker is not runnable ({error}); none of the \
                 {declared} real-quota cases ran"
            );
            return Ok(ExitCode::FAILURE);
        }
    }

    let (uid, gid) = identity(&fs::read_to_string("/proc/self/status")?)?;
    if uid == 0 {
        eprintln!(
            "quota fixture: refusing — run as an ordinary user; the lane runs the cases as the \
             invoking identity, and project quota limits do not bind root"
        );
        return Ok(ExitCode::FAILURE);
    }
    let lab = Lab::for_checkout(&root);
    let snapshot = Snapshot::of(&root, &sha)?;
    let outcome = build_and_run(&lab, &snapshot.directory, uid, gid, args.release);
    drop(snapshot);
    if args.clean {
        clean_up(&lab);
    }
    let (summary, success) = outcome?;
    let verdict = verdict(declared, summary, success);

    // Read again after a run that takes minutes: a moved `HEAD` or an edited checkout means the
    // record would name a commit nobody is looking at any more.
    let head_after = git_text(&root, &["rev-parse", "HEAD"])?;
    let clean_after = git_text(&root, &["status", "--porcelain"])?.is_empty();
    match verdict {
        Err(reason) => {
            eprintln!("quota fixture: no record — {reason}");
            Ok(ExitCode::FAILURE)
        }
        Ok(cases) if head_after != sha => {
            println!(
                "quota fixture: {cases} cases green at {sha}, but HEAD moved to {head_after} \
                 during the run; this run is not a release record for any commit"
            );
            Ok(ExitCode::SUCCESS)
        }
        Ok(cases) if clean_before && clean_after => {
            println!("quota fixture: green at {sha}; record this line in the annotated tag:");
            println!("{}", evidence_line(&sha, cases, SystemTime::now()));
            Ok(ExitCode::SUCCESS)
        }
        Ok(cases) => {
            let when = if clean_before { "after" } else { "before" };
            println!(
                "quota fixture: {cases} cases green at the committed {sha}, but the checkout was \
                 not clean {when} the run; this run is not a release record for any commit"
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Build the recipe from the snapshot and run the lane on it; the container's lane summary, if it
/// printed one, and whether the container exited successfully.
fn build_and_run(
    lab: &Lab,
    snapshot: &Path,
    uid: u32,
    gid: u32,
    release: bool,
) -> Result<(Option<LaneSummary>, bool)> {
    let recipe = snapshot.join(RECIPE);
    let status = Command::new("docker")
        .args(["build", "--tag", &lab.image, "--file"])
        .arg(recipe.join("Dockerfile"))
        .arg(&recipe)
        .status()
        .context("docker build")?;
    ensure!(
        status.success(),
        "building {} from {RECIPE} failed: {status}",
        lab.image
    );

    let name = format!(
        "substrate-quota-lab-{}",
        ulid::Ulid::generate().to_string().to_lowercase()
    );
    let invocation = run_argv(lab, snapshot, uid, gid, release, &name);
    let mut child = Command::new("docker")
        .args(&invocation)
        .stdout(Stdio::piped())
        .spawn()
        .context("docker run")?;
    let stdout = child.stdout.take().context("piped container stdout")?;
    let mut summary = None;
    for line in BufReader::new(stdout).lines() {
        let line = line?;
        if let Some(found) = lane_summary(&line) {
            summary = Some(found);
        }
        println!("{line}");
    }
    let exit = child.wait().context("awaiting the quota-lab container")?;
    Ok((summary, exit.success()))
}

/// The image tag and the two volumes one checkout owns, keyed by its canonical path.
///
/// Keyed per checkout, not per commit: two checkouts of byte-identical commits still get separate
/// build targets, and one checkout reuses its own across commits instead of leaving a volume per
/// commit behind. Reuse is sound because every snapshot is extracted with fresh modification times
/// (`tar -m`), so Cargo rebuilds every workspace crate and reuses only registry dependencies.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lab {
    image: String,
    target_volume: String,
    registry_volume: String,
}

impl Lab {
    fn for_checkout(root: &Path) -> Self {
        let digest = Sha256::digest(root.as_os_str().as_bytes());
        let key = hex::encode(&digest[..8]);
        Self {
            image: format!("{IMAGE_REPOSITORY}:{key}"),
            target_volume: format!("{VOLUME_PREFIX}-target-{key}"),
            registry_volume: format!("{VOLUME_PREFIX}-cargo-registry-{key}"),
        }
    }
}

/// `git archive` of one commit, extracted into a fresh directory under the target directory and
/// removed when dropped. The container builds and runs from it, never from the live checkout.
struct Snapshot {
    directory: PathBuf,
}

impl Snapshot {
    fn of(root: &Path, sha: &str) -> Result<Self> {
        let parent = target_directory(root).join("quota-fixture");
        fs::create_dir_all(&parent).with_context(|| format!("creating {}", parent.display()))?;
        let directory = parent.join(format!(
            "snapshot-{}",
            ulid::Ulid::generate().to_string().to_lowercase()
        ));
        fs::create_dir(&directory).with_context(|| format!("creating {}", directory.display()))?;
        let snapshot = Self { directory };

        let mut archive = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["archive", "--format=tar", sha])
            .stdout(Stdio::piped())
            .spawn()
            .context("git archive")?;
        let tar = archive.stdout.take().context("piped git archive")?;
        // `-m`: every file gets the extraction time, newer than anything already in the volume.
        let extracted = Command::new("tar")
            .args(["-x", "-m", "-f", "-", "-C"])
            .arg(&snapshot.directory)
            .stdin(tar)
            .status()
            .context("tar")?;
        let archived = archive.wait().context("awaiting git archive")?;
        ensure!(
            archived.success() && extracted.success(),
            "snapshotting {sha} into {} failed: git archive {archived}, tar {extracted}",
            snapshot.directory.display()
        );
        Ok(snapshot)
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            eprintln!(
                "quota fixture: could not remove the snapshot {}: {error}",
                self.directory.display()
            );
        }
    }
}

fn target_directory(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .filter(|value| !value.is_empty())
        .map_or_else(|| root.join("target"), |value| root.join(value))
}

fn clean_up(lab: &Lab) {
    for argv in [
        vec!["image", "rm", lab.image.as_str()],
        vec![
            "volume",
            "rm",
            lab.target_volume.as_str(),
            lab.registry_volume.as_str(),
        ],
    ] {
        match Command::new("docker").args(&argv).output() {
            Ok(output) if output.status.success() => {}
            Ok(output) => eprintln!(
                "quota fixture: docker {} failed: {}",
                argv.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Err(error) => eprintln!("quota fixture: docker {}: {error}", argv.join(" ")),
        }
    }
}

/// The real uid and gid from `/proc/self/status`.
fn identity(status: &str) -> Result<(u32, u32)> {
    let field = |key: &str| -> Result<u32> {
        let line = status
            .lines()
            .find_map(|line| line.strip_prefix(key))
            .with_context(|| format!("no {key} line in /proc/self/status"))?;
        let real = line
            .split_whitespace()
            .next()
            .with_context(|| format!("empty {key} line"))?;
        Ok(real.parse()?)
    };
    Ok((field("Uid:")?, field("Gid:")?))
}

/// The one `docker run` this verb issues. Docker's default capability set plus `SYS_ADMIN`, as the
/// recorded run had; the loop device needs `/dev/loop-control`, block major 7 in the device cgroup,
/// and an `AppArmor` profile that does not deny `mount`. Nothing else, and never `--privileged`.
fn run_argv(
    lab: &Lab,
    source: &Path,
    uid: u32,
    gid: u32,
    release: bool,
    name: &str,
) -> Vec<String> {
    let mut argv: Vec<String> = [
        "run",
        "--rm",
        "--name",
        name,
        "--cap-add",
        "SYS_ADMIN",
        "--device",
        "/dev/loop-control",
        "--device-cgroup-rule",
        "b 7:* rmw",
        "--security-opt",
        "apparmor=unconfined",
        "--mount",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    argv.push(format!(
        "type=bind,source={},target={SOURCE_MOUNT},readonly",
        source.display()
    ));
    argv.extend([
        "--mount".to_owned(),
        format!(
            "type=volume,source={},target={TARGET_MOUNT}",
            lab.target_volume
        ),
        "--mount".to_owned(),
        format!(
            "type=volume,source={},target={REGISTRY_MOUNT}",
            lab.registry_volume
        ),
        "--workdir".to_owned(),
        SOURCE_MOUNT.to_owned(),
        "--env".to_owned(),
        format!("CARGO_TARGET_DIR={TARGET_MOUNT}/root"),
        lab.image.clone(),
        "cargo".to_owned(),
        "run".to_owned(),
        "--locked".to_owned(),
        "--quiet".to_owned(),
        "--package".to_owned(),
        "xtask".to_owned(),
        "--".to_owned(),
        "quota-fixture".to_owned(),
        "inside".to_owned(),
        "--uid".to_owned(),
        uid.to_string(),
        "--gid".to_owned(),
        gid.to_string(),
    ]);
    if release {
        argv.push("--release".to_owned());
    }
    argv
}

/// `(declared, ran, failed)` from a `quota lane:` summary line.
type LaneSummary = (usize, usize, usize);

/// `quota lane: <declared> declared, <ran> ran, <failed> failed`, as `quota-lane` prints it.
fn lane_summary(line: &str) -> Option<LaneSummary> {
    let rest = line.strip_prefix("quota lane: ")?;
    let mut numbers = Vec::new();
    for (part, label) in rest.split(", ").zip(["declared", "ran", "failed"]) {
        let (number, word) = part.split_once(' ')?;
        if word != label {
            return None;
        }
        numbers.push(number.parse().ok()?);
    }
    match numbers[..] {
        [declared, ran, failed] => Some((declared, ran, failed)),
        _ => None,
    }
}

/// Green only when the container succeeded and its lane ran every case the source declares.
fn verdict(declared: usize, summary: Option<LaneSummary>, success: bool) -> Result<usize, String> {
    let Some((counted, ran, failed)) = summary else {
        return Err(
            "the container printed no `quota lane:` summary, so no case is known to have \
                    run"
            .to_owned(),
        );
    };
    if failed > 0 {
        return Err(format!("{failed} of {counted} real-quota cases failed"));
    }
    if counted != declared || ran != declared {
        return Err(format!(
            "the lane counted {counted} and ran {ran}; this checkout declares {declared}"
        ));
    }
    if !success {
        return Err("the container exited unsuccessfully after a green summary".to_owned());
    }
    Ok(declared)
}

fn evidence_line(sha: &str, cases: usize, at: SystemTime) -> String {
    record_line("Quota-lane", sha, cases, at)
}

/// `inside` provisions block devices and mounts; it must never act outside the recipe's container.
fn lab_guard(marker: Option<&str>, in_container: bool, euid: u32) -> Result<(), String> {
    if marker != Some("1") {
        return Err(format!(
            "{LAB_MARKER} is not set; `quota-fixture inside` runs only in the {RECIPE} container \
             (use `cargo xtask quota-fixture run`)"
        ));
    }
    if !in_container {
        return Err("/.dockerenv is absent; this is not a Docker container".to_owned());
    }
    if euid != 0 {
        return Err(format!(
            "running as uid {euid}; provisioning the loop filesystem needs the container's root"
        ));
    }
    Ok(())
}

fn in_container(args: &InsideArgs) -> Result<ExitCode> {
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    if let Err(reason) = lab_guard(
        std::env::var(LAB_MARKER).ok().as_deref(),
        Path::new("/.dockerenv").exists(),
        euid,
    ) {
        eprintln!("quota fixture: refusing — {reason}");
        return Ok(ExitCode::FAILURE);
    }
    ensure!(
        args.uid != 0,
        "the lane must not run as root, which quota limits do not bind"
    );

    fs::create_dir_all(LAB_DIRECTORY)?;
    fs::File::create(IMAGE_FILE)?.set_len(IMAGE_BYTES)?;
    command(
        "mkfs.ext4",
        &[
            "-q",
            "-F",
            "-O",
            "quota,project",
            "-E",
            "quotatype=prjquota",
            IMAGE_FILE,
        ],
    )?;
    fs::create_dir_all(QUOTA_ROOT)?;
    mount_loop()?;
    let lane = run_lane(args);
    let unmount = command("umount", &[QUOTA_ROOT]);
    let code = lane?;
    unmount?;
    Ok(code)
}

/// Attach the image to one free loop device with automatic detach, and mount it `prjquota`.
///
/// The container's `/dev` carries only `/dev/loop-control`; the device node for the free index is
/// created here, inside the device cgroup rule the run was granted. `loop=<device>` makes
/// `mount` attach with `LO_FLAGS_AUTOCLEAR`, so the host's loop device is released when the
/// filesystem is unmounted or the container's mount namespace ends.
fn mount_loop() -> Result<()> {
    let mut last = None;
    for _ in 0..3 {
        let index = free_loop_index()?;
        let device = format!("/dev/loop{index}");
        if !Path::new(&device).exists() {
            let path = std::ffi::CString::new(device.as_bytes())?;
            // SAFETY: the path is a live NUL-terminated string; mknod reads it and returns.
            let made = unsafe {
                libc::mknod(
                    path.as_ptr(),
                    libc::S_IFBLK | 0o660,
                    libc::makedev(LOOP_MAJOR, index),
                )
            };
            ensure!(
                made == 0,
                "mknod {device}: {}",
                std::io::Error::last_os_error()
            );
        }
        let options = format!("loop={device},prjquota");
        match command(
            "mount",
            &["-t", "ext4", "-o", &options, IMAGE_FILE, QUOTA_ROOT],
        ) {
            Ok(()) => {
                println!(
                    "quota fixture: {IMAGE_FILE} on {device} mounted at {QUOTA_ROOT} ({options})"
                );
                return Ok(());
            }
            // Another host process may take the same free index between the query and the attach.
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("no loop device could be attached")))
}

fn free_loop_index() -> Result<u32> {
    let control = fs::File::open("/dev/loop-control").context("/dev/loop-control")?;
    // SAFETY: LOOP_CTL_GET_FREE takes no argument and returns an index or -1.
    let index = unsafe {
        libc::ioctl(
            std::os::fd::AsRawFd::as_raw_fd(&control),
            LOOP_CTL_GET_FREE as _,
        )
    };
    ensure!(
        index >= 0,
        "LOOP_CTL_GET_FREE: {}",
        std::io::Error::last_os_error()
    );
    Ok(u32::try_from(index)?)
}

/// Hand the filesystem and the lane's own caches to the invoking identity, then run `quota-lane`
/// as that identity with `SYS_ADMIN` inheritable and ambient, exactly as the recorded run did.
fn run_lane(args: &InsideArgs) -> Result<ExitCode> {
    let owner = format!("{}:{}", args.uid, args.gid);
    let lane_target = PathBuf::from(TARGET_MOUNT).join("lane");
    let home = PathBuf::from(TARGET_MOUNT).join("home");
    for directory in [&lane_target, &home] {
        fs::create_dir_all(directory)?;
    }
    command("chown", &[&owner, QUOTA_ROOT])?;
    fs::set_permissions(QUOTA_ROOT, fs::Permissions::from_mode(0o755))?;
    let chown_tree = |path: &Path| -> Result<()> {
        let path = path.to_str().context("non-UTF-8 cache path")?;
        command("chown", &["-R", &owner, path])
    };
    chown_tree(&lane_target)?;
    chown_tree(&home)?;
    chown_tree(Path::new(REGISTRY_MOUNT))?;

    let xtask = std::env::current_exe().context("this xtask binary")?;
    let mut lane = Command::new("setpriv");
    lane.arg(format!("--reuid={}", args.uid))
        .arg(format!("--regid={}", args.gid))
        .args([
            "--clear-groups",
            "--inh-caps=+sys_admin",
            "--ambient-caps=+sys_admin",
        ])
        .arg(&xtask)
        .arg("quota-lane");
    if args.release {
        lane.arg("--release");
    }
    lane.current_dir(SOURCE_MOUNT)
        .env("SUBSTRATE_TEST_QUOTA_ROOT", QUOTA_ROOT)
        .env("SUBSTRATE_TEST_PROJECT_QUOTA_IDS", IDS)
        .env("CARGO_TARGET_DIR", &lane_target)
        .env("HOME", &home);
    let status = lane.status().context("setpriv quota-lane")?;
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn command(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("running {program}"))?;
    if !output.status.success() {
        bail!(
            "{program} {}: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim(),
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        IMAGE_REPOSITORY, Lab, RECIPE, evidence_line, identity, lab_guard, lane_summary,
        rfc3339_utc, run_argv, verdict,
    };
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    fn root() -> std::path::PathBuf {
        crate::repo::root().expect("workspace root")
    }

    fn lab() -> Lab {
        Lab::for_checkout(Path::new("/checkout"))
    }

    #[test]
    fn every_checkout_owns_its_image_tag_and_both_volumes() {
        let one = Lab::for_checkout(Path::new("/work/one/substrate"));
        let other = Lab::for_checkout(Path::new("/work/other/substrate"));
        assert_eq!(one, Lab::for_checkout(Path::new("/work/one/substrate")));
        assert_ne!(one.image, other.image);
        assert_ne!(one.target_volume, other.target_volume);
        assert_ne!(one.registry_volume, other.registry_volume);
        assert!(
            one.image.starts_with(&format!("{IMAGE_REPOSITORY}:")),
            "{one:?}"
        );
        let argv = run_argv(&one, Path::new("/snapshot"), 1000, 1000, false, "lab");
        assert!(argv.contains(&format!(
            "type=volume,source={},target=/target",
            one.target_volume
        )));
        assert!(!argv.iter().any(|arg| arg.contains(&other.target_volume)));
    }

    fn base_image(dockerfile: &str) -> Option<String> {
        dockerfile.lines().find_map(|line| {
            let rest = line.strip_prefix("FROM rust:")?;
            Some(rest.split_whitespace().next()?.to_owned())
        })
    }

    #[test]
    fn the_recipe_builds_on_the_release_builders_exact_pinned_image() {
        let release = std::fs::read_to_string(root().join("Dockerfile")).expect("Dockerfile");
        let recipe = std::fs::read_to_string(root().join(RECIPE).join("Dockerfile"))
            .expect("recipe Dockerfile");
        let pinned = base_image(&release).expect("release builder image");
        assert!(pinned.contains("@sha256:"), "{pinned}");
        assert_eq!(base_image(&recipe), Some(pinned));
    }

    #[test]
    fn the_recipe_installs_the_toolchain_the_repository_pins() {
        let toolchain =
            std::fs::read_to_string(root().join("rust-toolchain.toml")).expect("toolchain");
        let channel = toolchain
            .lines()
            .find_map(|line| line.strip_prefix("channel = "))
            .expect("channel")
            .trim_matches('"');
        let recipe = std::fs::read_to_string(root().join(RECIPE).join("Dockerfile"))
            .expect("recipe Dockerfile");
        assert!(
            recipe.contains(&format!("rustup toolchain install {channel} ")),
            "the recipe must install toolchain {channel} so no run downloads one"
        );
        assert!(recipe.contains("ENV B10X_SUBSTRATE_QUOTA_LAB=1"));
        assert!(recipe.contains("e2fsprogs"));
    }

    #[test]
    fn the_container_gets_sys_admin_and_loop_access_and_nothing_privileged() {
        let argv = run_argv(&lab(), Path::new("/snapshot"), 1000, 1000, false, "lab");
        assert!(
            !argv.iter().any(|arg| arg.contains("privileged")),
            "{argv:?}"
        );
        let added: Vec<_> = argv
            .windows(2)
            .filter(|pair| pair[0] == "--cap-add")
            .map(|pair| pair[1].as_str())
            .collect();
        assert_eq!(added, ["SYS_ADMIN"]);
        assert!(!argv.iter().any(|arg| arg.starts_with("--cap-add=")));
        let devices: Vec<_> = argv
            .windows(2)
            .filter(|pair| pair[0] == "--device")
            .map(|pair| pair[1].as_str())
            .collect();
        assert_eq!(devices, ["/dev/loop-control"]);
        assert!(argv.contains(&"type=bind,source=/snapshot,target=/src,readonly".to_owned()));
        let image = argv
            .iter()
            .position(|arg| *arg == lab().image)
            .expect("image");
        assert_eq!(
            argv[image + 1..],
            [
                "cargo",
                "run",
                "--locked",
                "--quiet",
                "--package",
                "xtask",
                "--",
                "quota-fixture",
                "inside",
                "--uid",
                "1000",
                "--gid",
                "1000"
            ]
        );
        let release = run_argv(&lab(), Path::new("/snapshot"), 1000, 1000, true, "lab");
        assert_eq!(release.last().map(String::as_str), Some("--release"));
    }

    #[test]
    fn only_a_complete_green_lane_is_a_record() {
        let line = "quota lane: 7 declared, 7 ran, 0 failed";
        assert_eq!(lane_summary(line), Some((7, 7, 0)));
        assert_eq!(
            lane_summary("quota lane: 7 declared, 6 ran, 1 failed"),
            Some((7, 6, 1))
        );
        assert_eq!(lane_summary("quota lane:   ran  x"), None);
        assert_eq!(lane_summary("quota lane: root /quota ids 1-200"), None);

        assert_eq!(verdict(7, Some((7, 7, 0)), true), Ok(7));
        for (summary, success) in [
            (None, true),
            (Some((7, 6, 1)), false),
            (Some((6, 6, 0)), true),
            (Some((7, 6, 0)), true),
            (Some((7, 7, 0)), false),
        ] {
            assert!(
                verdict(7, summary, success).is_err(),
                "{summary:?} {success} must not be a record"
            );
        }
    }

    #[test]
    fn the_record_is_the_form_the_release_workflow_accepts() {
        let at = UNIX_EPOCH + Duration::from_hours(497_986);
        assert_eq!(rfc3339_utc(at), "2026-10-23T10:00:00Z");
        assert_eq!(
            rfc3339_utc(UNIX_EPOCH + Duration::from_secs(951_868_799)),
            "2000-02-29T23:59:59Z"
        );
        assert_eq!(rfc3339_utc(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(
            evidence_line("abc", 7, at),
            "Quota-lane: abc 7 2026-10-23T10:00:00Z"
        );
    }

    #[test]
    fn provisioning_refuses_outside_the_recipe_container() {
        assert!(lab_guard(None, true, 0).is_err());
        assert!(lab_guard(Some("1"), false, 0).is_err());
        assert!(lab_guard(Some("1"), true, 1000).is_err());
        assert!(lab_guard(Some("0"), true, 0).is_err());
        assert_eq!(lab_guard(Some("1"), true, 0), Ok(()));
    }

    #[test]
    fn the_invoking_identity_is_the_real_uid_and_gid() {
        let status = "Name:\txtask\nUid:\t1000\t1000\t1000\t1000\nGid:\t100\t100\t100\t100\n";
        assert_eq!(identity(status).expect("identity"), (1000, 100));
        assert!(identity("Name:\txtask\n").is_err());
    }
}
