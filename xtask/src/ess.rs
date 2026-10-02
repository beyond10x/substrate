//! CLI-owned ESS validation and deterministic conformance projection.
use crate::report::Report;
use anyhow::{Context, Result, ensure};
use std::{fs, path::Path, process::Command};
fn run(root: &Path, args: &[&str]) -> Result<()> {
    let result = Command::new("ess")
        .args(args)
        .current_dir(root)
        .output()
        .context("ESS 0.50.0 must be installed; see spec/README.md")?;
    print!("{}", String::from_utf8_lossy(&result.stdout));
    eprint!("{}", String::from_utf8_lossy(&result.stderr));
    ensure!(result.status.success(), "ESS failed: {}", args.join(" "));
    Ok(())
}
pub fn check(root: &Path) -> Result<Report> {
    run(
        root,
        &["specify", "validate", "--path", "spec", "--strict-requires"],
    )?;
    let dir = tempfile::tempdir()?;
    let suite = dir.path().join("suite.json");
    run(
        root,
        &[
            "verify",
            "conform",
            "synthesize",
            "--path",
            "spec",
            "--strict-requires",
            "--out",
            suite.to_str().context("UTF-8 output path")?,
        ],
    )?;
    ensure!(
        fs::read(suite)? == fs::read(root.join("spec/operations-suite.json"))?,
        "ESS suite drift: regenerate spec/operations-suite.json with the pinned ESS release"
    );
    run(
        root,
        &[
            "specify",
            "validate",
            "--path",
            "spec/sessions",
            "--strict-requires",
        ],
    )?;
    let types = dir.path().join("capture-types");
    run(
        root,
        &[
            "generate",
            "types",
            "--path",
            "spec/sessions",
            "--root",
            "substrate.sessions.CaptureMode",
            "--target",
            "rust",
            "--package",
            "b10x-substrate-capture-types",
            "--out",
            types.to_str().context("UTF-8 type output")?,
            "--strict-requires",
        ],
    )?;
    ensure!(
        fs::read(types.join("types.rs"))?
            == fs::read(root.join("crates/substrate-wire/src/generated/capture.rs"))?,
        "ESS capture type drift: regenerate the selected type with the pinned ESS release"
    );
    Ok(Report::passed(
        "ESS specification and committed suite are current; production execution runs in the substrate-store test suite",
    ))
}
