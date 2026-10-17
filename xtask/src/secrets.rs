use std::collections::HashMap;
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
    let roots = root_commits(root)?;
    let scanned = log.lines().find_map(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        let at = words.iter().position(|word| *word == "commits")?;
        words.get(at.checked_sub(1)?)?.parse::<u64>().ok()
    });
    // Gitleaks counts the commits whose `git log -m --root -p` emits a patch, roots included.
    // A commit with no patch carries no bytes to scan, so it is named instead of counted.
    let empty = treeless_commits(root)?;
    let expected_diffs = expected.saturating_sub(empty.len() as u64);
    if scanned != Some(expected_diffs) {
        return Ok(Report::failed(vec![format!(
            "Gitleaks reported {scanned:?} scanned commits; Git reports {expected_diffs} with a patch"
        )]));
    }
    for commit in &roots {
        scan_root_commit(root, directory.path(), commit)?;
    }
    let unchanged = if empty.is_empty() {
        String::new()
    } else {
        format!(
            "; {} commits carry no patch: {}",
            empty.len(),
            empty.join(", ")
        )
    };
    Ok(Report::passed(format!(
        "Gitleaks scanned all {expected} reachable commits ({expected_diffs} with a patch and {} root trees{unchanged})",
        roots.len()
    )))
}

fn root_commits(root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["rev-list", "--all", "--max-parents=0"])
        .current_dir(root)
        .output()
        .context("listing reachable root commits")?;
    if !output.status.success() {
        bail!(
            "git rev-list roots failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// The tree of a commit that holds no files.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Commits without a patch: a root with the empty tree, or a commit whose tree is identical to
/// the tree of every parent.
fn treeless_commits(root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args(["log", "--all", "--format=%H %T %P"])
        .current_dir(root)
        .output()
        .context("listing commit trees")?;
    if !output.status.success() {
        bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let text = String::from_utf8(output.stdout)?;
    let rows: Vec<Vec<&str>> = text
        .lines()
        .map(|line| line.split_whitespace().collect())
        .filter(|words: &Vec<&str>| words.len() >= 2)
        .collect();
    let trees: HashMap<&str, &str> = rows.iter().map(|words| (words[0], words[1])).collect();
    let mut empty = Vec::new();
    for words in &rows {
        let parents = &words[2..];
        let mut same = !parents.is_empty() || words[1] == EMPTY_TREE;
        for parent in parents {
            let Some(tree) = trees.get(parent) else {
                bail!("parent {parent} of {} is not reachable", words[0]);
            };
            same &= *tree == words[1];
        }
        if same {
            empty.push(words[0].to_owned());
        }
    }
    empty.sort();
    Ok(empty)
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
