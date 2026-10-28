//! CLI-owned ESS validation and deterministic conformance projection.
use crate::report::Report;
use anyhow::{Context, Result, ensure};
use std::{fs, path::Path, process::Command};

/// The `b10x-substrate` presentation binding over `spec/client`.
const CLI_BINDING: &str = "spec/client/cli.yaml";
/// The package `ess generate cli` projects from [`CLI_BINDING`], committed with its output record.
const CLI_CONTRACT: &str = "spec/generated/cli";
/// The workspace projection (ADR 0035), its committed suite and its generated wire types.
const WORKSPACES: &str = "spec/workspaces";
const WORKSPACES_SUITE: &str = "spec/workspaces-suite.json";
const WORKSPACE_TYPES: &str = "crates/substrate-wire/src/generated/workspace.rs";
fn run(root: &Path, args: &[&str]) -> Result<()> {
    let result = Command::new("ess")
        .args(args)
        .current_dir(root)
        .output()
        .context("ESS 0.75.0 must be installed; see spec/README.md")?;
    print!("{}", String::from_utf8_lossy(&result.stdout));
    eprint!("{}", String::from_utf8_lossy(&result.stderr));
    ensure!(result.status.success(), "ESS failed: {}", args.join(" "));
    Ok(())
}
/// `spec/workspaces` (ADR 0035): the root-name set, the workspace origin, `workspace.adopt` and
/// `workspace.destroy`. The committed suite is the synthesized obligations plus the authored
/// scenarios; it is drift-checked and run against the interpreted reference, which checks the
/// model, not the daemon. The daemon's own tests carry the runtime claims.
fn check_workspaces(root: &Path, scratch: &Path) -> Result<()> {
    run(
        root,
        &[
            "specify",
            "validate",
            "--path",
            WORKSPACES,
            "--strict-requires",
        ],
    )?;
    let suite = scratch.join("workspaces-suite.json");
    run(
        root,
        &[
            "verify",
            "conform",
            "synthesize",
            "--path",
            WORKSPACES,
            "--scenarios",
            WORKSPACES,
            "--strict-requires",
            "--out",
            suite.to_str().context("UTF-8 output path")?,
        ],
    )?;
    ensure!(
        fs::read(&suite)? == fs::read(root.join(WORKSPACES_SUITE))?,
        "ESS workspaces suite drift: regenerate {WORKSPACES_SUITE} with the pinned ESS release"
    );
    run(
        root,
        &[
            "verify",
            "conform",
            "run",
            "--path",
            WORKSPACES,
            "--suite",
            WORKSPACES_SUITE,
            "--target",
            "interpreted",
            "--report-format",
            "2",
            "--strict-requires",
        ],
    )?;
    let types = scratch.join("workspace-types");
    run(
        root,
        &[
            "generate",
            "types",
            "--path",
            WORKSPACES,
            "--root",
            "substrate.workspaces.RootName",
            "--root",
            "substrate.workspaces.Origin",
            "--target",
            "rust",
            "--package",
            "b10x-substrate-workspace-types",
            "--out",
            types.to_str().context("UTF-8 type output")?,
            "--strict-requires",
        ],
    )?;
    ensure!(
        fs::read(types.join("types.rs"))? == fs::read(root.join(WORKSPACE_TYPES))?,
        "ESS workspace type drift: regenerate {WORKSPACE_TYPES} with the pinned ESS release"
    );
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
    check_host_processes(root, dir.path())?;
    check_capture_types(root, dir.path())?;
    check_workspaces(root, dir.path())?;
    check_confinement(root, dir.path())?;
    run(
        root,
        &[
            "specify",
            "validate",
            "--path",
            "spec/client",
            "--strict-requires",
        ],
    )?;
    run(
        root,
        &[
            "specify",
            "cli",
            "--path",
            "spec/client",
            "--binding",
            CLI_BINDING,
            "--strict-requires",
        ],
    )?;
    // `--check` regenerates in memory and exits 1 naming each edited, missing or stale file.
    run(
        root,
        &[
            "generate",
            "cli",
            "--path",
            "spec/client",
            "--binding",
            CLI_BINDING,
            "--out",
            CLI_CONTRACT,
            "--check",
            "--strict-requires",
        ],
    )
    .context(format!(
        "ESS CLI contract drift: regenerate {CLI_CONTRACT} from {CLI_BINDING} with the pinned ESS release"
    ))?;
    Ok(Report::passed(
        "ESS specification, committed suite and CLI contract are current; production execution runs in the substrate-store test suite",
    ))
}

/// The sessions domain and the one capture type generated from it.
fn check_capture_types(root: &Path, dir: &Path) -> Result<()> {
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
    let types = dir.join("capture-types");
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
    Ok(())
}

/// The host-process domain (ADR 0036) and its authored acceptance scenarios.
const HOST_PROCESSES: &str = "spec/host-processes";
/// The suite `ess verify conform synthesize` derives from [`HOST_PROCESSES`], committed.
const HOST_PROCESSES_SUITE: &str = "spec/host-processes-suite.json";
/// The host-process wire types (ADR 0036 S1-S7, S11, S12) generated from [`HOST_PROCESSES`], committed.
const HOST_PROCESS_TYPES: &str = "crates/substrate-wire/src/generated/host_process.rs";
/// The roots of [`HOST_PROCESS_TYPES`]: every request, record, page and tail of the surface. Their
/// transitive closure brings the closed vocabularies (state, `ended_by`, profile, confinement).
const HOST_PROCESS_TYPE_ROOTS: [&str; 8] = [
    "substrate.host_processes.ProcessStartInput",
    "substrate.host_processes.ProcessRecord",
    "substrate.host_processes.ProcessPage",
    "substrate.host_processes.ProcessStopByOwnerInput",
    "substrate.host_processes.ProcessStopByOwnerResult",
    "substrate.host_processes.ProcessLogTail",
    "substrate.host_processes.ProcessTransition",
    "substrate.host_processes.ProcessStreamItem",
];

/// Validates the host-process specification, checks its committed suite for drift and runs the
/// synthesized and authored scenarios against the ESS interpreted reference, then regenerates the
/// host-process wire types and compares them with [`HOST_PROCESS_TYPES`]. The interpreted run
/// checks the model only; it is not evidence that the daemon conforms.
fn check_host_processes(root: &Path, scratch: &Path) -> Result<()> {
    run(
        root,
        &[
            "specify",
            "validate",
            "--path",
            HOST_PROCESSES,
            "--strict-requires",
        ],
    )?;
    let suite = scratch.join("host-processes-suite.json");
    run(
        root,
        &[
            "verify",
            "conform",
            "synthesize",
            "--path",
            HOST_PROCESSES,
            "--strict-requires",
            "--out",
            suite.to_str().context("UTF-8 output path")?,
        ],
    )?;
    ensure!(
        fs::read(suite)? == fs::read(root.join(HOST_PROCESSES_SUITE))?,
        "ESS suite drift: regenerate {HOST_PROCESSES_SUITE} from {HOST_PROCESSES} with the pinned ESS release"
    );
    run(
        root,
        &[
            "verify",
            "conform",
            "run",
            "--path",
            HOST_PROCESSES,
            "--scenarios",
            HOST_PROCESSES,
            "--target",
            "interpreted",
            "--report-format",
            "2",
            "--strict-requires",
        ],
    )?;
    let types = scratch.join("host-process-types");
    let mut args = vec!["generate", "types", "--path", HOST_PROCESSES];
    for root in HOST_PROCESS_TYPE_ROOTS {
        args.extend(["--root", root]);
    }
    args.extend([
        "--names",
        "short",
        "--target",
        "rust",
        "--package",
        "b10x-substrate-host-process-types",
        "--out",
        types.to_str().context("UTF-8 type output")?,
        "--strict-requires",
    ]);
    run(root, &args)?;
    ensure!(
        fs::read(types.join("types.rs"))? == fs::read(root.join(HOST_PROCESS_TYPES))?,
        "ESS host-process type drift: regenerate {HOST_PROCESS_TYPES} from {HOST_PROCESSES} with the pinned ESS release"
    );
    Ok(())
}

/// The confinement contract (atlas ADR 0109, Substrate ADR 0044) and every adapter's authored
/// scenarios, each named for the lane that runs it.
const CONFINEMENT: &str = "spec/confinement";
/// The suite `ess verify conform synthesize` derives from [`CONFINEMENT`], committed.
const CONFINEMENT_SUITE: &str = "spec/confinement-suite.json";
/// The contract's closed vocabularies generated from [`CONFINEMENT`], committed.
const CONFINEMENT_TYPES: &str = "crates/substrate-host/src/generated/confinement.rs";
/// The roots of [`CONFINEMENT_TYPES`].
const CONFINEMENT_TYPE_ROOTS: [&str; 3] = [
    "substrate.confinement.ContractFamily",
    "substrate.confinement.AdapterName",
    "substrate.confinement.Fact",
];

/// Validates the confinement specification, checks its committed suite for drift and runs it
/// against the ESS interpreted reference, then regenerates the contract's types and compares them
/// with [`CONFINEMENT_TYPES`]. The interpreted run checks the model only; each adapter's own lane
/// runs the same scenarios against the adapter.
fn check_confinement(root: &Path, scratch: &Path) -> Result<()> {
    run(
        root,
        &[
            "specify",
            "validate",
            "--path",
            CONFINEMENT,
            "--strict-requires",
        ],
    )?;
    let suite = scratch.join("confinement-suite.json");
    run(
        root,
        &[
            "verify",
            "conform",
            "synthesize",
            "--path",
            CONFINEMENT,
            "--scenarios",
            CONFINEMENT,
            "--strict-requires",
            "--out",
            suite.to_str().context("UTF-8 output path")?,
        ],
    )?;
    ensure!(
        fs::read(suite)? == fs::read(root.join(CONFINEMENT_SUITE))?,
        "ESS confinement suite drift: regenerate {CONFINEMENT_SUITE} from {CONFINEMENT} with the pinned ESS release"
    );
    run(
        root,
        &[
            "verify",
            "conform",
            "run",
            "--path",
            CONFINEMENT,
            "--suite",
            CONFINEMENT_SUITE,
            "--target",
            "interpreted",
            "--report-format",
            "2",
            "--strict-requires",
        ],
    )?;
    let types = scratch.join("confinement-types");
    let mut args = vec!["generate", "types", "--path", CONFINEMENT];
    for root in CONFINEMENT_TYPE_ROOTS {
        args.extend(["--root", root]);
    }
    args.extend([
        "--names",
        "short",
        "--target",
        "rust",
        "--package",
        "b10x-substrate-confinement-types",
        "--out",
        types.to_str().context("UTF-8 type output")?,
        "--strict-requires",
    ]);
    run(root, &args)?;
    ensure!(
        fs::read(types.join("types.rs"))? == fs::read(root.join(CONFINEMENT_TYPES))?,
        "ESS confinement type drift: regenerate {CONFINEMENT_TYPES} from {CONFINEMENT} with the pinned ESS release"
    );
    Ok(())
}
