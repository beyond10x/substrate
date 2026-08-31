use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use sha2::{Digest as _, Sha256};

use crate::report::Report;

const VERSION: &str = "8.30.1";
const ARCHIVE: &str = "gitleaks_8.30.1_linux_x64.tar.gz";
const SHA256: &str = "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb";

pub fn check(root: &Path) -> Result<Report> {
    require_success(root, "git", &["fsck", "--full"])?;
    let count_output = Command::new("git")
        .args(["rev-list", "--all", "--count"])
        .current_dir(root)
        .output()
        .context("counting reachable commits")?;
    if !count_output.status.success() {
        bail!(
            "git rev-list failed: {}",
            String::from_utf8_lossy(&count_output.stderr)
        );
    }
    let expected: u64 = String::from_utf8(count_output.stdout)?.trim().parse()?;
    if expected == 0 {
        return Ok(Report::failed(vec![
            "secret scan refused an empty history".to_owned(),
        ]));
    }

    let directory = tempfile::tempdir().context("creating Gitleaks tool directory")?;
    let archive = directory.path().join(ARCHIVE);
    let url =
        format!("https://github.com/gitleaks/gitleaks/releases/download/v{VERSION}/{ARCHIVE}");
    require_success_at(
        root,
        "curl",
        &["-fsSL", &url, "-o", archive.to_str().unwrap_or("")],
    )?;
    let bytes = fs::read(&archive)?;
    if hex::encode(Sha256::digest(bytes)) != SHA256 {
        bail!("downloaded Gitleaks archive checksum disagrees with the repository pin");
    }
    require_success_at(
        root,
        "tar",
        &[
            "-xzf",
            archive.to_str().unwrap_or(""),
            "-C",
            directory.path().to_str().unwrap_or(""),
            "gitleaks",
        ],
    )?;
    let output = Command::new(directory.path().join("gitleaks"))
        .args([
            "git",
            "--redact=100",
            "--no-banner",
            "--no-color",
            "--log-level=info",
            "--log-opts=-m --root --all",
        ])
        .current_dir(root)
        .output()
        .context("running Gitleaks")?;
    let log = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return Ok(Report::failed(vec![format!(
            "Gitleaks refused the history:\n{log}"
        )]));
    }
    let roots_output = Command::new("git")
        .args(["rev-list", "--all", "--max-parents=0"])
        .current_dir(root)
        .output()
        .context("listing reachable root commits")?;
    if !roots_output.status.success() {
        bail!(
            "git rev-list roots failed: {}",
            String::from_utf8_lossy(&roots_output.stderr)
        );
    }
    let roots: Vec<&str> = std::str::from_utf8(&roots_output.stdout)?
        .lines()
        .filter(|line| !line.is_empty())
        .collect();
    let scanned = log.lines().find_map(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        let at = words.iter().position(|word| *word == "commits")?;
        words.get(at.checked_sub(1)?)?.parse::<u64>().ok()
    });
    let expected_diffs = expected.saturating_sub(roots.len() as u64);
    if scanned != Some(expected_diffs) {
        return Ok(Report::failed(vec![format!(
            "Gitleaks reported {scanned:?} scanned non-root commits; Git reports {expected_diffs}"
        )]));
    }
    for commit in &roots {
        scan_root_commit(root, directory.path(), commit)?;
    }
    Ok(Report::passed(format!(
        "Gitleaks scanned all {expected} reachable commits ({expected_diffs} diffs and {} root trees)",
        roots.len()
    )))
}

fn scan_root_commit(root: &Path, tool_directory: &Path, commit: &str) -> Result<()> {
    let tree = tempfile::tempdir().context("creating root-commit scan directory")?;
    let archive = tree.path().join("root.tar");
    require_success_at(
        root,
        "git",
        &[
            "archive",
            "--format=tar",
            &format!("--output={}", archive.display()),
            commit,
        ],
    )?;
    let extracted = tree.path().join("tree");
    fs::create_dir(&extracted)?;
    require_success_at(
        root,
        "tar",
        &[
            "-xf",
            archive.to_str().unwrap_or(""),
            "-C",
            extracted.to_str().unwrap_or(""),
        ],
    )?;
    let output = Command::new(tool_directory.join("gitleaks"))
        .args([
            "dir",
            "--redact=100",
            "--no-banner",
            "--no-color",
            "--log-level=info",
            "--config",
        ])
        .arg(root.join(".gitleaks.toml"))
        .arg(&extracted)
        .output()
        .context("running Gitleaks on a root commit tree")?;
    if !output.status.success() {
        bail!(
            "Gitleaks refused root commit {commit}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn require_success(root: &Path, program: &str, args: &[&str]) -> Result<()> {
    require_success_at(root, program, args)
}

fn require_success_at(root: &Path, program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        bail!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
