//! Offline release-workflow assertions for the contract-bundle publication boundary.
//!
//! The live registry and Sigstore transparency log are necessarily release-time evidence. These
//! tests keep the repository-controlled half fail-closed: the current bundle is explicit, its
//! deterministic OCI layout is the thing copied, canonical tags are checked before publication,
//! all three artifacts are signed and verified by digest before the GitHub release is announced, and
//! protected `main` is never pushed around. A byte-identical write-once bundle may be reused by a
//! later daemon release, while a digest mismatch is always refused.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

const WORKFLOW: &str = include_str!("../../.github/workflows/release.yml");
const GATE_WORKFLOW: &str = include_str!("../../.github/workflows/gate.yml");
const DOCKERFILE: &str = include_str!("../../Dockerfile");

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask has a repository parent")
        .to_path_buf()
}

fn position(needle: &str) -> usize {
    WORKFLOW
        .find(needle)
        .unwrap_or_else(|| panic!("release workflow is missing {needle:?}"))
}

fn occurrences(needle: &str) -> usize {
    WORKFLOW.match_indices(needle).count()
}

fn version_tuple(version: &str) -> (u64, u64, u64) {
    let mut fields = version.split('.').map(|field| {
        field
            .parse::<u64>()
            .unwrap_or_else(|_| panic!("non-numeric bundle version {version:?}"))
    });
    let tuple = (
        fields.next().expect("major"),
        fields.next().expect("minor"),
        fields.next().expect("patch"),
    );
    assert!(
        fields.next().is_none(),
        "non-semver bundle version {version:?}"
    );
    tuple
}

fn current_bundle() -> String {
    let root = repository_root().join("contracts/substrate-wire");
    fs::read_dir(root)
        .expect("released bundle root")
        .map(|entry| {
            entry
                .expect("bundle directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .max_by_key(|version| version_tuple(version))
        .expect("at least one released bundle")
}

#[test]
fn release_pins_the_current_development_bundle_explicitly() {
    let current = current_bundle();
    assert_eq!(
        current, "0.25.0",
        "update this assertion with the successor"
    );
    assert!(
        WORKFLOW.contains(&format!("BUNDLE_VERSION: {current}")),
        "the release workflow must explicitly pin the current development bundle"
    );
    assert!(WORKFLOW.contains("BUNDLE_IMAGE: ghcr.io/beyond10x/b10x-substrate-wire"));
}

#[test]
fn release_copies_the_packagers_exact_manifest() {
    position("cargo xtask package-bundle \"${BUNDLE_VERSION}\" --out \"${BUNDLE_LAYOUT}\"");
    position("oras resolve --oci-layout \"${BUNDLE_LAYOUT}:${BUNDLE_VERSION}\"");
    position("oras cp --from-oci-layout");
    position("oras resolve \"${BUNDLE_IMAGE}:${BUNDLE_VERSION}\" 2>&1");
    position("if [ \"$remote_digest\" != \"$PACKAGE_DIGEST\" ]");
}

#[test]
fn registry_login_and_bundle_readback_use_their_narrow_identities() {
    position("GHCR_USER: ${{ github.actor }}");
    assert!(!WORKFLOW.contains("GHCR_USER: ${{ github.repository_owner }}"));

    let publish = position("Publish or reuse the exact development contract-bundle layout");
    let anonymous = WORKFLOW[publish..]
        .find("anonymous_config=$(mktemp -d)")
        .map(|offset| publish + offset)
        .expect("bundle publication must create a credential-free readback context");
    let readback = WORKFLOW[anonymous..]
        .find("DOCKER_CONFIG=\"$anonymous_config\"")
        .map(|offset| anonymous + offset)
        .expect("bundle readback must be anonymous");
    let bounded_retry = position("for attempt in 1 2 3 4 5");
    let sign = position("Sign the development contract bundle keylessly");
    assert!(publish < anonymous && anonymous <= readback);
    assert!(readback < bounded_retry && bounded_retry < sign);
    position("was not anonymously resolvable after publication");
    position("oras manifest fetch \"${BUNDLE_IMAGE}@${remote_digest}\"");
    assert!(!WORKFLOW.contains("remote_descriptor=$(DOCKER_CONFIG=\"$anonymous_config\""));
    assert!(
        !WORKFLOW.contains("oras manifest fetch --descriptor \"${BUNDLE_IMAGE}@${remote_digest}\"")
    );
}

#[test]
fn a_partial_release_can_recover_only_an_existing_tag_from_protected_main() {
    position("workflow_dispatch:");
    position("Existing annotated release tag to recover after a partial failure");
    position("github.event_name == 'workflow_dispatch' && inputs.version || github.ref_name");
    position("[ \"$WORKFLOW_REF\" != \"refs/heads/main\" ]");
    position("git show \"${source_sha}:Cargo.toml\"");
    position("[ \"$EVENT_NAME\" = \"push\" ] && [ \"$source_sha\" != \"$EVENT_SHA\" ]");
    position("daemon-exists=true");
    position("mcp-exists=true");
    position("executor-exists=true");
    assert_eq!(occurrences("from the same tagged source ${SOURCE_SHA}"), 3);
    assert!(occurrences("org.opencontainers.image.revision") >= 2);
}

#[test]
fn canonical_tags_are_never_overwritten() {
    let refusal = position("Resolve write-once image version tags");
    let daemon_push = position("docker push \"${IMAGE}:${VERSION}\"");
    let mcp_push = position("docker push \"${MCP_IMAGE}:${VERSION}\"");
    let bundle_push = position("oras cp --from-oci-layout");
    assert!(refusal < daemon_push);
    assert!(refusal < mcp_push);
    assert!(refusal < bundle_push);
    assert_eq!(occurrences("docker push \"${IMAGE}:${VERSION}\""), 1);
    assert_eq!(occurrences("docker push \"${MCP_IMAGE}:${VERSION}\""), 1);
    assert!(occurrences("oras resolve \"${BUNDLE_IMAGE}:${BUNDLE_VERSION}\"") >= 3);
    position("reusing byte-identical ${BUNDLE_IMAGE}:${BUNDLE_VERSION}");
    position("not deterministic package ${PACKAGE_DIGEST}; it will not be overwritten");
    position("appeared at ${remote_digest}, not ${PACKAGE_DIGEST}; it will not be overwritten");
}

#[test]
fn all_artifacts_are_signed_and_verified_before_announcement() {
    let daemon_sign = position("cosign sign --yes \"${IMAGE}@${DIGEST}\"");
    let daemon_verify = position("\"${IMAGE}@${DIGEST}\" > cosign-verify.json");
    let bundle_sign = position("cosign sign --yes \"${BUNDLE_IMAGE}@${BUNDLE_DIGEST}\"");
    let bundle_verify =
        position("\"${BUNDLE_IMAGE}@${BUNDLE_DIGEST}\" > bundle-cosign-verify.json");
    let mcp_sign = position("cosign sign --yes \"${MCP_IMAGE}@${MCP_DIGEST}\"");
    let mcp_verify = position("\"${MCP_IMAGE}@${MCP_DIGEST}\" > mcp-cosign-verify.json");
    let announce = position("gh release create \"${VERSION}\"");
    assert!(daemon_sign < daemon_verify && daemon_verify < announce);
    assert!(bundle_sign < bundle_verify && bundle_verify < announce);
    assert!(mcp_sign < mcp_verify && mcp_verify < announce);
    assert!(occurrences("https://token.actions.githubusercontent.com") >= 3);
    assert!(occurrences(".github/workflows/release.yml@${GITHUB_REF}") >= 3);
}

#[test]
fn bundle_visibility_is_proven_before_the_daemon_tag_is_mutated() {
    let anonymous_bundle = position("Prove the contract bundle is anonymously retrievable");
    let daemon_push = position("docker push \"${IMAGE}:${VERSION}\"");
    assert!(anonymous_bundle < daemon_push);
    position("DOCKER_CONFIG=\"$anonymous_config\"");
    position("oras manifest fetch --descriptor \"${BUNDLE_IMAGE}@${BUNDLE_DIGEST}\"");
}

#[test]
fn mcp_image_is_stdio_only_and_its_exact_binary_is_smoke_tested() {
    let mcp = DOCKERFILE
        .split_once(" AS mcp\n")
        .expect("named MCP runtime stage")
        .1
        .split("\nFROM ")
        .next()
        .expect("MCP runtime stage body");
    assert!(DOCKERFILE.contains("gcr.io/distroless/cc-debian12:nonroot@sha256:"));
    assert!(
        DOCKERFILE.contains("install -D /usr/lib/x86_64-linux-gnu/libz.so.1 /out/lib/libz.so.1")
    );
    assert!(
        mcp.contains("COPY --from=builder /out/lib/libz.so.1 /usr/lib/x86_64-linux-gnu/libz.so.1"),
        "the distroless MCP target must carry the shipped binary's zlib"
    );
    let prerequisites = DOCKERFILE
        .split_once(" AS execution-prerequisites\n")
        .expect("packaged daemon dependencies")
        .1
        .split_once("\nFROM execution-prerequisites AS daemon\n")
        .expect("daemon inherits the packaged runtime libraries")
        .0;
    assert!(
        prerequisites
            .split_whitespace()
            .any(|word| word == "zlib1g")
    );
    assert!(mcp.contains("ENTRYPOINT [\"/usr/local/bin/substrate-mcp\"]"));
    assert!(!mcp.contains("EXPOSE"));
    assert!(!mcp.contains("VOLUME"));
    position("--target mcp");
    position("SUBSTRATE_MCP_BINARY=\"${PWD}/target/mcp-image-under-test/substrate-mcp\"");
    position("SUBSTRATE_MCP_DOCKER_IMAGE=\"${MCP_IMAGE}:${VERSION}\"");
    position("-p b10x-substrate-mcp --test stdio -- --nocapture");
}

#[test]
fn release_compiles_both_runtime_binaries_once() {
    let builder = DOCKERFILE
        .split_once(" AS builder\n")
        .expect("the shared builder stage")
        .1
        .split_once("\nFROM ")
        .expect("a stage after the builder")
        .0;
    assert_eq!(builder.matches("cargo build --locked --release").count(), 1);
    assert!(builder.contains("-p b10x-substrate-daemon --bin substrate-daemon"));
    assert!(builder.contains("-p b10x-substrate-mcp --bin substrate-mcp"));
    let executor = DOCKERFILE
        .split_once(" AS executor-builder\n")
        .expect("the executor's own musl stage")
        .1
        .split_once("\nFROM ")
        .expect("a stage after the executor builder")
        .0;
    assert_eq!(
        executor.matches("cargo build --locked --release").count(),
        1
    );
    assert!(executor.contains("--target x86_64-unknown-linux-musl -p b10x-substrate-executor"));
    assert!(!executor.contains("b10x-substrate-daemon"));
    // The third build is the clean-room runner's own `-p` stage (design 31 § 7): the chart lane
    // exports it, and no release target depends on it.
    let runner = DOCKERFILE
        .split_once(" AS conformance-runner-build\n")
        .expect("the clean-room runner's own stage")
        .1
        .split_once("\nFROM ")
        .expect("a stage after the runner's build")
        .0;
    assert_eq!(runner.matches("cargo build --locked --release").count(), 1);
    assert!(runner.contains("cargo build --locked --release -p b10x-substrate-conformance &&"));
    assert_eq!(occurrences("conformance-runner"), 0);
    assert_eq!(
        DOCKERFILE.matches("cargo build --locked --release").count(),
        3
    );
    assert!(DOCKERFILE.contains(" AS daemon\n"));
    assert!(DOCKERFILE.contains(" AS mcp\n"));
    position("--target daemon");
    position("--target mcp");
}

#[test]
fn gate_cache_excludes_workspace_build_outputs() {
    assert!(
        GATE_WORKFLOW
            .contains("uses: Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6")
    );
    assert!(GATE_WORKFLOW.contains("cache-workspace-crates: \"false\""));
    assert!(GATE_WORKFLOW.contains("cache-all-crates: \"false\""));
    assert!(!GATE_WORKFLOW.contains("uses: actions/cache@"));
}

/// The text of one job of `gate.yml`, from its `  <id>:` line to the next job.
fn gate_job(id: &str) -> &'static str {
    let header = format!("\n  {id}:\n");
    let start = GATE_WORKFLOW
        .find(&header)
        .unwrap_or_else(|| panic!("gate.yml has no job {id}"))
        + header.len();
    let body = &GATE_WORKFLOW[start..];
    let end = body
        .match_indices('\n')
        .map(|(index, _)| index)
        .find(|&index| {
            let line = &body[index + 1..];
            line.starts_with("  ") && !line.starts_with("   ")
        })
        .unwrap_or(body.len());
    &body[..end]
}

/// Runs `scripts/gate.sh` with `arguments` against `cargo` and `python3` stubs that only succeed,
/// and returns its exit status and every `gate:` line it printed.
fn gate_steps(arguments: &[&str]) -> (Option<i32>, Vec<String>) {
    let tools = tempfile::tempdir().expect("stub directory");
    for tool in ["cargo", "python3"] {
        let path = tools.path().join(tool);
        fs::write(&path, "#!/bin/sh\nexit 0\n").expect("stub");
        let mut permissions = fs::metadata(&path).expect("stub metadata").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        fs::set_permissions(&path, permissions).expect("stub mode");
    }
    let path = format!(
        "{}:{}",
        tools.path().display(),
        std::env::var("PATH").expect("PATH")
    );
    let output = std::process::Command::new("bash")
        .arg(repository_root().join("scripts/gate.sh"))
        .args(arguments)
        .current_dir(repository_root())
        .env("PATH", path)
        .output()
        .expect("bash runs the gate");
    let lines = String::from_utf8(output.stdout)
        .expect("utf-8")
        .lines()
        .filter(|line| line.starts_with("gate: "))
        .map(str::to_owned)
        .collect();
    (output.status.code(), lines)
}

/// `scripts/gate.sh` with no argument runs every step in its order; each phase runs a part of it,
/// in the same order, and the two parts are disjoint and together the whole.
#[test]
fn the_two_gate_phases_partition_every_gate_step() {
    let (status, all) = gate_steps(&[]);
    assert_eq!(status, Some(0), "{all:?}");
    assert_eq!(all.last().map(String::as_str), Some("gate: passed"));
    let all = &all[..all.len() - 1];
    let mut phases = Vec::new();
    for phase in ["tests", "static"] {
        let (status, lines) = gate_steps(&[phase]);
        assert_eq!(status, Some(0), "{phase}: {lines:?}");
        assert_eq!(
            lines.last().map(String::as_str),
            Some(format!("gate: phase {phase} passed").as_str())
        );
        let lines = lines[..lines.len() - 1].to_vec();
        assert!(!lines.is_empty(), "the {phase} phase runs no step");
        let mut from = 0;
        for line in &lines {
            let at = all[from..]
                .iter()
                .position(|step| step == line)
                .unwrap_or_else(|| panic!("{phase} runs {line} out of the full gate's order"));
            from += at + 1;
        }
        phases.push(lines);
    }
    let mut union: Vec<&String> = phases.iter().flatten().collect();
    union.sort();
    let mut every: Vec<&String> = all.iter().collect();
    every.sort();
    assert_eq!(union, every, "the phases are not a partition of the gate");
    assert!(phases[0].contains(&"gate: cargo test --workspace --release --locked".to_owned()));
    assert!(phases[1].contains(&"gate: cargo xtask check-secrets".to_owned()));
    for refused in [&["unit"][..], &["tests", "static"][..]] {
        let (status, lines) = gate_steps(refused);
        assert_eq!(status, Some(2), "{refused:?} was accepted");
        assert!(lines.is_empty(), "{refused:?} ran {lines:?}");
    }
}

/// `Full gate` is the status context `main`'s ruleset requires, and `release.yml` reads this
/// workflow's run conclusion. The phases run as two jobs side by side; `Full gate` waits for both,
/// runs whatever they concluded, and succeeds only when both succeeded.
#[test]
fn the_full_gate_context_is_green_only_when_both_gate_phases_are() {
    assert_eq!(GATE_WORKFLOW.matches("name: Full gate\n").count(), 1);
    assert!(!GATE_WORKFLOW.contains("bash scripts/gate.sh\n"));
    let tests = gate_job("gate-tests");
    let checks = gate_job("gate-static");
    let full = gate_job("gate");
    assert!(tests.contains("        run: bash scripts/gate.sh tests\n"));
    assert!(checks.contains("        run: bash scripts/gate.sh static\n"));
    assert!(full.contains("    name: Full gate\n"));
    assert!(full.contains("    needs: [gate-tests, gate-static]\n"));
    assert!(full.contains("    if: always()\n"));
    assert!(full.contains("TESTS: ${{ needs.gate-tests.result }}"));
    assert!(full.contains("STATIC: ${{ needs.gate-static.result }}"));
    assert!(full.contains("test \"$TESTS\" = success"));
    assert!(full.contains("test \"$STATIC\" = success"));
    assert!(!full.contains("continue-on-error"));
    assert!(tests.contains("SUBSTRATE_OCI_LANE: \"1\""));
    assert!(tests.contains("- name: Report execution lanes"));
    assert!(tests.contains("- name: Lane summary"));
    assert!(tests.contains("--component clippy"));
    assert!(
        checks.contains("fetch-depth: 0"),
        "check-secrets refuses a shallow history, so the static phase checks out all of it"
    );
    assert!(checks.contains("--component rustfmt"));
    assert!(checks.contains("- name: Install the pinned licence inventory tool"));
    assert!(checks.contains("- name: Install the specification's pinned ESS release"));
    let key = |job: &str| {
        job.lines()
            .find_map(|line| line.trim().strip_prefix("shared-key: "))
            .map(str::to_owned)
    };
    assert_ne!(key(tests), None);
    assert_ne!(key(checks), None);
    assert_ne!(
        key(tests),
        key(checks),
        "two build graphs share one cache key"
    );
}

#[test]
fn a_release_refuses_without_a_recorded_confinement_lane_run() {
    let require = position("Require a recorded confinement-lane run for this commit");
    let announce = position("gh release create \"${VERSION}\"");
    assert!(require < announce);
    position("confinement_evidence:");
    position("Confinement-lane:");
    position("refused: no recorded confinement lane run for");
    position("## Confinement lane");
    assert!(WORKFLOW.contains("confinement-evidence: ${{ steps.confinement.outputs.evidence }}"));

    // The step reads the declared delegated case count out of the *tagged* source with this exact
    // pattern, so a rename or a reformatting of the constant would make every release refuse with
    // a message about the check rather than about the release. Assert the pattern against the file
    // it parses instead of trusting it.
    let vectors = fs::read_to_string(
        repository_root().join("crates/substrate-daemon/tests/runtime_vectors.rs"),
    )
    .expect("the clean-room runner is readable");
    assert!(
        vectors.lines().any(|line| {
            line.starts_with("const DELEGATED_CASES: usize = ") && line.ends_with(';')
        }),
        "the confinement-evidence step's DELEGATED_CASES pattern no longer matches its source"
    );
}

const QUOTA_STEP: &str = "Require a recorded quota-lane run for this commit";
const QUOTA_SOURCE: &str = "crates/substrate-host/src/git/quota_tests.rs";

#[test]
fn a_release_refuses_without_a_recorded_quota_lane_run() {
    let require = position(QUOTA_STEP);
    let announce = position("gh release create \"${VERSION}\"");
    assert!(require < announce);
    position("quota_evidence:");
    position("Quota-lane:");
    position("refused: no recorded quota lane run for");
    position("## Quota lane");
    position("cargo xtask quota-fixture run");
    assert!(WORKFLOW.contains("quota-evidence: ${{ steps.quota.outputs.evidence }}"));
    assert!(WORKFLOW.contains("QUOTA_EVIDENCE: ${{ needs.preflight.outputs.quota-evidence }}"));
}

/// The `run: |` body of the named preflight step, dedented, exactly as the runner would execute it.
fn step_script(name: &str) -> String {
    let start = position(&format!("- name: {name}\n"));
    let mut lines = WORKFLOW[start..].lines().skip(1);
    for line in lines.by_ref() {
        if line.trim() == "run: |" {
            break;
        }
        assert!(
            !line.trim_start().starts_with("- name:"),
            "step {name:?} has no `run: |` block"
        );
    }
    let mut body = Vec::new();
    let mut indent = None;
    for line in lines {
        if line.trim().is_empty() {
            body.push(String::new());
            continue;
        }
        let width = line.len() - line.trim_start().len();
        let indent = *indent.get_or_insert(width);
        if width < indent {
            break;
        }
        body.push(line[indent..].to_owned());
    }
    assert!(
        !body.is_empty(),
        "step {name:?} has an empty `run: |` block"
    );
    body.join("\n") + "\n"
}

/// A throwaway Git repository holding one commit and one annotated tag, isolated from every
/// user-level and system-level Git configuration and hook.
struct TaggedSource {
    directory: tempfile::TempDir,
    sha: String,
}

const TAG: &str = "9.9.9";

fn git(directory: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(directory)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "user.name=release-workflow-test",
            "-c",
            "user.email=release-workflow-test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf-8 git output")
        .trim()
        .to_owned()
}

impl TaggedSource {
    /// `source` becomes the tagged quota-test file; `message` is the annotated tag's message, where
    /// `{sha}` is replaced with the tagged commit.
    fn new(source: &str, message: &str) -> Self {
        Self::with_files(&[(QUOTA_SOURCE, source)], message)
    }

    /// Every `(path, contents)` pair becomes a tracked file of the one tagged commit.
    fn with_files(files: &[(&str, &str)], message: &str) -> Self {
        let directory = tempfile::tempdir().expect("temporary repository");
        let root = directory.path();
        git(root, &["init", "--quiet"]);
        for (path, contents) in files {
            let file = root.join(path);
            fs::create_dir_all(file.parent().expect("source parent")).expect("source tree");
            fs::write(&file, contents).expect("source file");
        }
        git(root, &["add", "--all"]);
        git(root, &["commit", "--quiet", "-m", "tagged source"]);
        let sha = git(root, &["rev-parse", "HEAD"]);
        let message = message.replace("{sha}", &sha);
        git(root, &["tag", "--annotate", TAG, "-m", &message]);
        Self { directory, sha }
    }

    /// Run the quota step against this repository, as a tag push (`input` empty) or a recovery
    /// dispatch. Returns the exit success, stderr and what the step wrote to `GITHUB_OUTPUT`.
    fn run(&self, input: &str) -> (bool, String, String) {
        self.run_step(QUOTA_STEP, input)
    }

    fn run_step(&self, step: &str, input: &str) -> (bool, String, String) {
        self.run_step_on(step, input, "push")
    }

    /// As [`Self::run_step`], for the named workflow event.
    fn run_step_on(&self, step: &str, input: &str, event: &str) -> (bool, String, String) {
        let output_file = self.directory.path().join("github-output");
        fs::write(&output_file, "").expect("GITHUB_OUTPUT");
        let output = std::process::Command::new("bash")
            .current_dir(self.directory.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("EVENT_NAME", event)
            .env("TAG", TAG)
            .env("INPUT_EVIDENCE", input)
            .env("SOURCE_SHA", &self.sha)
            .env("GITHUB_OUTPUT", &output_file)
            .args(["-e", "-o", "pipefail", "-c", &step_script(step)])
            .output()
            .expect("bash runs");
        (
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
            fs::read_to_string(&output_file).expect("GITHUB_OUTPUT readable"),
        )
    }
}

fn quota_source(cases: usize) -> String {
    let mut source = String::from("use super::*;\n\n#[tokio::test]\nasync fn portable() {}\n");
    for case in 0..cases {
        write!(
            source,
            "\n#[tokio::test]\n#[ignore = \"requires the explicitly delegated ext4 project-quota \
             fixture\"]\nasync fn real_quota_case_{case}() {{\n}}\n"
        )
        .expect("writing to a String cannot fail");
    }
    source
}

fn refused(source: &TaggedSource, input: &str, reason: &str) {
    refused_by(QUOTA_STEP, source, input, reason);
}

fn refused_by(step: &str, source: &TaggedSource, input: &str, reason: &str) {
    let (success, stderr, output) = source.run_step(step, input);
    assert!(!success, "the step accepted what it must refuse: {stderr}");
    assert!(
        stderr.contains(reason),
        "refusal does not name {reason:?}: {stderr}"
    );
    assert!(
        !output.contains("evidence="),
        "a refusal must not emit evidence: {output}"
    );
}

#[test]
fn the_quota_lane_record_naming_the_tagged_commit_and_its_case_count_is_accepted() {
    let source = TaggedSource::new(
        &quota_source(7),
        "release\n\nQuota-lane: {sha} 7 2026-10-23T10:00:00Z\n",
    );
    let (success, stderr, output) = source.run("");
    assert!(success, "{stderr}");
    assert_eq!(
        output.trim(),
        format!("evidence={} 7 2026-10-23T10:00:00Z", source.sha)
    );

    // A recovery dispatch carries the same record as an input, for a tag annotated without it.
    let untrailed = TaggedSource::new(&quota_source(7), "release\n");
    let (success, stderr, output) =
        untrailed.run(&format!("{} 7 2026-10-23T10:00:00Z", untrailed.sha));
    assert!(success, "{stderr}");
    assert!(output.contains("evidence="), "{output}");
}

#[test]
fn the_quota_lane_record_is_refused_when_absent_or_only_a_confinement_record() {
    let absent = TaggedSource::new(&quota_source(7), "release\n");
    refused(&absent, "", "refused: no recorded quota lane run for");
    let confinement_only = TaggedSource::new(
        &quota_source(7),
        "release\n\nConfinement-lane: {sha} 167 2026-10-23T10:00:00Z\n",
    );
    refused(
        &confinement_only,
        "",
        "refused: no recorded quota lane run for",
    );
}

#[test]
fn the_quota_lane_record_for_another_commit_is_refused() {
    let source = TaggedSource::new(
        &quota_source(7),
        "release\n\nQuota-lane: 0123456789abcdef0123456789abcdef01234567 7 2026-10-23T10:00:00Z\n",
    );
    refused(
        &source,
        "",
        "records commit '0123456789abcdef0123456789abcdef01234567'",
    );
}

#[test]
fn the_quota_lane_record_with_another_case_count_is_refused() {
    let fewer = TaggedSource::new(
        &quota_source(7),
        "release\n\nQuota-lane: {sha} 6 2026-10-23T10:00:00Z\n",
    );
    refused(&fewer, "", "records 6 real-quota cases;");
    let word = TaggedSource::new(
        &quota_source(7),
        "release\n\nQuota-lane: {sha} seven 2026-10-23T10:00:00Z\n",
    );
    refused(
        &word,
        "",
        "records 'seven' where a real-quota case count belongs",
    );
    let none_declared = TaggedSource::new(
        &quota_source(0),
        "release\n\nQuota-lane: {sha} 0 2026-10-23T10:00:00Z\n",
    );
    refused(&none_declared, "", "declares no real_quota_ case");
}

#[test]
fn the_quota_lane_record_with_a_timestamp_that_is_not_rfc_3339_utc_is_refused() {
    for instant in ["2026-10-23", "2026-10-23T10:00:00+02:00", "yesterday"] {
        let source = TaggedSource::new(
            &quota_source(7),
            &format!("release\n\nQuota-lane: {{sha}} 7 {instant}\n"),
        );
        refused(&source, "", "where an RFC 3339 UTC instant belongs");
    }
    // A space-separated instant or none at all is a record of the wrong shape before it is a
    // wrong instant.
    for (instant, fields) in [
        ("2026-10-23 10:00:00", "has 4 fields"),
        ("", "has 2 fields"),
    ] {
        let source = TaggedSource::new(
            &quota_source(7),
            &format!("release\n\nQuota-lane: {{sha}} 7 {instant}\n"),
        );
        refused(&source, "", fields);
    }
}

#[test]
fn the_quota_step_counts_the_repositorys_own_real_quota_cases() {
    // The step reads the declared case count out of the *tagged* source with a pattern; assert it
    // against the file it parses, so a reformatting cannot make every release refuse with a
    // message about the check rather than about the release.
    let real = fs::read_to_string(repository_root().join(QUOTA_SOURCE)).expect("quota source");
    let ignored = real
        .lines()
        .filter(|line| line.trim_start().starts_with("#[ignore"))
        .count();
    assert!(
        ignored >= 7,
        "{QUOTA_SOURCE} declares {ignored} ignored cases"
    );
    let source = TaggedSource::new(
        &real,
        &format!("release\n\nQuota-lane: {{sha}} {ignored} 2026-10-23T10:00:00Z\n"),
    );
    let (success, stderr, _) = source.run("");
    assert!(success, "{stderr}");
    let off_by_one = TaggedSource::new(
        &real,
        &format!(
            "release\n\nQuota-lane: {{sha}} {} 2026-10-23T10:00:00Z\n",
            ignored - 1
        ),
    );
    refused(&off_by_one, "", "real-quota cases;");
}

#[test]
fn the_quota_step_counts_only_ignored_real_quota_functions() {
    // One definition on both sides: `cargo xtask quota-lane` and the fixture record the ignored
    // `fn real_quota_*` functions; an unignored helper or an ignored function of another name is
    // not a case for either.
    let mut text = quota_source(7);
    text.push_str("\nfn real_quota_fixture_root() {}\n");
    text.push_str("\n#[tokio::test]\n#[ignore = \"slow\"]\nasync fn another_ignored_case() {}\n");
    let source = TaggedSource::new(
        &text,
        "release\n\nQuota-lane: {sha} 7 2026-10-23T10:00:00Z\n",
    );
    let (success, stderr, _) = source.run("");
    assert!(success, "{stderr}");
}

const CONFINEMENT_STEP: &str = "Require a recorded confinement-lane run for this commit";
const RUNTIME_VECTORS: &str = "crates/substrate-daemon/tests/runtime_vectors.rs";
const DECLARED_DELEGATED: &str = "const DELEGATED_CASES: usize = 167;\n";
const AT: &str = "2026-10-23T10:00:00Z";

fn confinement_source(message: &str) -> TaggedSource {
    TaggedSource::with_files(&[(RUNTIME_VECTORS, DECLARED_DELEGATED)], message)
}

/// A lane step: its name, the trailer it reads, the case count its source declares, and a maker
/// for a tagged source with that count.
type LaneStep = (
    &'static str,
    &'static str,
    &'static str,
    fn(&str) -> TaggedSource,
);

/// Every lane-record step, each with a source it accepts and the trailer name it reads.
fn lane_steps() -> [LaneStep; 4] {
    [
        (
            CONFINEMENT_STEP,
            "Confinement-lane",
            "167",
            confinement_source,
        ),
        (QUOTA_STEP, "Quota-lane", "7", |message| {
            TaggedSource::new(&quota_source(7), message)
        }),
        (CHART_RECORD_STEP, "Chart-lane", "10", |message| {
            chart_record_source(&chart_scenarios(10), message)
        }),
        (NAMESPACE_STEP, "Namespace-lane", "8", |message| {
            namespace_source(&namespace_observations(8), message)
        }),
    ]
}

const CHART_RECORD_STEP: &str = "Require a recorded chart-lane run for this commit";
const CHART_LANE_SOURCE: &str = "xtask/src/chart_cluster.rs";
const CHART_YAML: &str = "deploy/helm/substrate/Chart.yaml";

/// A chart-lane source declaring `count` scenarios in the one literal array the step counts.
fn chart_scenarios(count: usize) -> String {
    chart_scenarios_declaring(count, count)
}

fn chart_scenarios_declaring(length: usize, entries: usize) -> String {
    let mut source = String::from("const RELEASE: &str = \"lane\";\n\n");
    writeln!(source, "pub const SCENARIOS: [&str; {length}] = [").expect("String");
    for entry in 0..entries {
        writeln!(source, "    \"scenario {entry}\",").expect("String");
    }
    source.push_str("];\n\nconst OTHER: [&str; 1] = [\n    \"not a scenario\",\n];\n");
    source
}

/// A tagged source carrying the chart and `scenarios` as the chart lane's source.
fn chart_record_source(scenarios: &str, message: &str) -> TaggedSource {
    TaggedSource::with_files(
        &[
            (CHART_LANE_SOURCE, scenarios),
            (CHART_YAML, "apiVersion: v2\nname: b10x-substrate\n"),
        ],
        message,
    )
}

fn chart_refused(source: &TaggedSource, input: &str, reason: &str) {
    refused_by(CHART_RECORD_STEP, source, input, reason);
}

#[test]
fn a_release_refuses_without_a_recorded_chart_lane_run() {
    let require = position(CHART_RECORD_STEP);
    let announce = position("gh release create \"${VERSION}\"");
    assert!(require < announce);
    assert!(position("Require a green chart-lane run for this commit") < require);
    position("chart_evidence:");
    position("Chart-lane:");
    position("refused: no recorded chart lane run for");
    position("## Chart lane");
    position("cargo xtask chart-lane cluster --node-profiles");
    assert!(WORKFLOW.contains("chart-evidence: ${{ steps.chart-record.outputs.evidence }}"));
    assert!(WORKFLOW.contains("CHART_EVIDENCE: ${{ steps.chart-record.outputs.evidence }}"));
    assert!(WORKFLOW.contains("CHART_EVIDENCE: ${{ needs.preflight.outputs.chart-evidence }}"));
    assert_eq!(occurrences("| chart lane record |"), 2);
    assert_eq!(occurrences("| local record |"), 1);
}

#[test]
fn the_chart_lane_record_naming_the_tagged_commit_and_its_scenario_count_is_accepted() {
    let source = chart_record_source(
        &chart_scenarios(10),
        &format!("release\n\nChart-lane: {{sha}} 10 {AT}\n"),
    );
    let (success, stderr, output) = source.run_step(CHART_RECORD_STEP, "");
    assert!(success, "{stderr}");
    assert_eq!(output.trim(), format!("evidence={} 10 {AT}", source.sha));

    let untrailed = chart_record_source(&chart_scenarios(10), "release\n");
    let (success, stderr, output) =
        untrailed.run_step(CHART_RECORD_STEP, &format!("{} 10 {AT}", untrailed.sha));
    assert!(success, "{stderr}");
    assert!(output.contains("evidence="), "{output}");
}

#[test]
fn the_chart_lane_record_is_refused_when_absent_or_only_another_lanes_record() {
    let absent = chart_record_source(&chart_scenarios(10), "release\n");
    chart_refused(&absent, "", "refused: no recorded chart lane run for");
    let others = chart_record_source(
        &chart_scenarios(10),
        &format!("release\n\nQuota-lane: {{sha}} 7 {AT}\nConfinement-lane: {{sha}} 167 {AT}\n"),
    );
    chart_refused(&others, "", "refused: no recorded chart lane run for");
}

#[test]
fn the_chart_lane_record_for_another_commit_is_refused() {
    let source = chart_record_source(
        &chart_scenarios(10),
        &format!("release\n\nChart-lane: 0123456789abcdef0123456789abcdef01234567 10 {AT}\n"),
    );
    chart_refused(
        &source,
        "",
        "records commit '0123456789abcdef0123456789abcdef01234567'",
    );
}

#[test]
fn the_chart_lane_record_with_another_scenario_count_is_refused() {
    let fewer = chart_record_source(
        &chart_scenarios(10),
        &format!("release\n\nChart-lane: {{sha}} 6 {AT}\n"),
    );
    chart_refused(&fewer, "", "records 6 chart-lane scenarios;");
    let word = chart_record_source(
        &chart_scenarios(10),
        &format!("release\n\nChart-lane: {{sha}} ten {AT}\n"),
    );
    chart_refused(
        &word,
        "",
        "records 'ten' where a chart-lane scenario count belongs",
    );
    let none_declared = chart_record_source(
        "const RELEASE: &str = \"lane\";\n",
        &format!("release\n\nChart-lane: {{sha}} 0 {AT}\n"),
    );
    chart_refused(&none_declared, "", "declares no SCENARIOS array");
    let disagreeing = chart_record_source(
        &chart_scenarios_declaring(11, 10),
        &format!("release\n\nChart-lane: {{sha}} 10 {AT}\n"),
    );
    chart_refused(&disagreeing, "", "declares no SCENARIOS array");
    let two_per_line = chart_record_source(
        &chart_scenarios(10).replace("    \"scenario 9\",\n", "    \"scenario 9\", \"x\",\n"),
        &format!("release\n\nChart-lane: {{sha}} 10 {AT}\n"),
    );
    chart_refused(&two_per_line, "", "declares no SCENARIOS array");
}

#[test]
fn the_chart_lane_record_with_a_timestamp_that_is_not_rfc_3339_utc_is_refused() {
    for instant in ["2026-10-23", "2026-10-23T10:00:00+02:00", "yesterday"] {
        let source = chart_record_source(
            &chart_scenarios(10),
            &format!("release\n\nChart-lane: {{sha}} 10 {instant}\n"),
        );
        chart_refused(&source, "", "where an RFC 3339 UTC instant belongs");
    }
}

#[test]
fn a_tagged_source_without_the_chart_needs_no_chart_lane_record() {
    let source = TaggedSource::with_files(&[("README.md", "no chart\n")], "release\n");
    let (success, stderr, output) = source.run_step(CHART_RECORD_STEP, "");
    assert!(success, "{stderr}");
    assert!(!output.contains("evidence="), "{output}");
    let forged = TaggedSource::with_files(
        &[("README.md", "no chart\n")],
        &format!("release\n\nChart-lane: {{sha}} 10 {AT}\n"),
    );
    let (success, stderr, output) = forged.run_step(CHART_RECORD_STEP, "");
    assert!(success, "{stderr}");
    assert!(!output.contains("evidence="), "{output}");
}

#[test]
fn the_chart_record_step_counts_the_repositorys_own_scenarios() {
    // The step counts the entries of one literal array in the *tagged* source; assert that count
    // against the file it parses, so a reformatting cannot make every release refuse with a
    // message about the check rather than about the release.
    let real =
        fs::read_to_string(repository_root().join(CHART_LANE_SOURCE)).expect("chart lane source");
    let start = real
        .lines()
        .position(|line| line.starts_with("pub const SCENARIOS: [&str; "))
        .expect("the SCENARIOS array");
    let declared = real
        .lines()
        .skip(start + 1)
        .take_while(|line| *line != "];")
        .filter(|line| line.starts_with("    \"") && line.ends_with("\","))
        .count();
    // At least the four execution-enabled scenarios the hosted lane reports absent.
    assert!(
        declared >= 4,
        "{CHART_LANE_SOURCE} declares {declared} scenarios"
    );
    let source = chart_record_source(
        &real,
        &format!("release\n\nChart-lane: {{sha}} {declared} {AT}\n"),
    );
    let (success, stderr, output) = source.run_step(CHART_RECORD_STEP, "");
    assert!(success, "{stderr}");
    assert!(output.contains(&format!(" {declared} ")), "{output}");
    let off_by_one = chart_record_source(
        &real,
        &format!("release\n\nChart-lane: {{sha}} {} {AT}\n", declared - 1),
    );
    chart_refused(&off_by_one, "", "chart-lane scenarios;");
}

const NAMESPACE_STEP: &str = "Require a recorded namespace-lane run for this commit";
const NAMESPACE_LANE_SOURCE: &str = "xtask/src/namespace_lane.rs";

/// A namespace-lane source declaring `count` observations on one line, as the lane does today.
fn namespace_observations(count: usize) -> String {
    namespace_observations_declaring(count, count)
}

fn namespace_observations_declaring(length: usize, entries: usize) -> String {
    let names: Vec<String> = (1..=entries).map(|entry| format!("\"O{entry}\"")).collect();
    format!(
        "const TRAILER: &str = \"Namespace-lane\";\n\n/// The observations.\npub const \
         OBSERVATIONS: [&str; {length}] = [{}];\n\nconst OTHER: [&str; 1] = [\"O99\"];\n",
        names.join(", ")
    )
}

fn namespace_source(observations: &str, message: &str) -> TaggedSource {
    TaggedSource::with_files(&[(NAMESPACE_LANE_SOURCE, observations)], message)
}

fn namespace_refused(source: &TaggedSource, input: &str, reason: &str) {
    refused_by(NAMESPACE_STEP, source, input, reason);
}

#[test]
fn a_release_refuses_without_a_recorded_namespace_lane_run() {
    let require = position(&format!(
        "- name: {NAMESPACE_STEP}\n        id: namespace\n"
    ));
    let release_job = position("\n  release:\n");
    let announce = position("gh release create \"${VERSION}\"");
    assert!(require < release_job && release_job < announce);
    position("namespace_evidence:");
    position("Namespace-lane:");
    position("refused: no recorded namespace lane run for");
    position("## Namespace lane");
    position("cargo xtask namespace-lane observe");
    assert!(WORKFLOW.contains("namespace-evidence: ${{ steps.namespace.outputs.evidence }}"));
    assert!(WORKFLOW.contains("namespace-lane: ${{ steps.namespace.outputs.lane }}"));
    assert!(WORKFLOW.contains("NAMESPACE_EVIDENCE: ${{ steps.namespace.outputs.evidence }}"));
    assert!(
        WORKFLOW.contains("NAMESPACE_EVIDENCE: ${{ needs.preflight.outputs.namespace-evidence }}")
    );
    assert_eq!(occurrences("| namespace lane |"), 2);
    // The release notes say what the record is, or why a recovered older tag carries none.
    let notes = &WORKFLOW[position("- name: Compose release notes\n")..announce];
    assert!(notes.contains("${NAMESPACE_EVIDENCE}"), "{notes}");
    assert!(notes.contains("predates the namespace lane"), "{notes}");
}

#[test]
fn the_namespace_lane_record_naming_the_tagged_commit_and_its_observation_count_is_accepted() {
    let source = namespace_source(
        &namespace_observations(8),
        &format!("release\n\nNamespace-lane: {{sha}} 8 {AT}\n"),
    );
    let (success, stderr, output) = source.run_step(NAMESPACE_STEP, "");
    assert!(success, "{stderr}");
    assert_eq!(
        output.trim(),
        format!("lane=present\nevidence={} 8 {AT}", source.sha)
    );

    // A recovery dispatch carries the same record as an input, for a tag annotated without it.
    let untrailed = namespace_source(&namespace_observations(8), "release\n");
    let (success, stderr, output) = untrailed.run_step_on(
        NAMESPACE_STEP,
        &format!("{} 8 {AT}", untrailed.sha),
        "workflow_dispatch",
    );
    assert!(success, "{stderr}");
    assert!(output.contains("evidence="), "{output}");
}

#[test]
fn the_namespace_lane_record_is_refused_when_absent_or_only_another_lanes_record() {
    let absent = namespace_source(&namespace_observations(8), "release\n");
    namespace_refused(&absent, "", "refused: no recorded namespace lane run for");
    for other in ["Confinement-lane", "Quota-lane", "Chart-lane"] {
        let source = namespace_source(
            &namespace_observations(8),
            &format!("release\n\n{other}: {{sha}} 8 {AT}\n"),
        );
        namespace_refused(&source, "", "refused: no recorded namespace lane run for");
    }
}

#[test]
fn the_namespace_lane_record_for_another_commit_is_refused() {
    let source = namespace_source(
        &namespace_observations(8),
        &format!("release\n\nNamespace-lane: 0123456789abcdef0123456789abcdef01234567 8 {AT}\n"),
    );
    namespace_refused(
        &source,
        "",
        "records commit '0123456789abcdef0123456789abcdef01234567'",
    );
}

#[test]
fn the_namespace_lane_record_with_another_observation_count_is_refused() {
    let fewer = namespace_source(
        &namespace_observations(8),
        &format!("release\n\nNamespace-lane: {{sha}} 7 {AT}\n"),
    );
    namespace_refused(&fewer, "", "records 7 namespace-lane observations;");
    let word = namespace_source(
        &namespace_observations(8),
        &format!("release\n\nNamespace-lane: {{sha}} eight {AT}\n"),
    );
    namespace_refused(
        &word,
        "",
        "records 'eight' where a namespace-lane observation count belongs",
    );
    for (name, observations) in [
        (
            "no array",
            "const TRAILER: &str = \"Namespace-lane\";\n".to_owned(),
        ),
        (
            "a disagreeing length",
            namespace_observations_declaring(9, 8),
        ),
        ("an empty array", namespace_observations(0)),
        (
            "two arrays",
            namespace_observations(8) + &namespace_observations(8),
        ),
        (
            "an entry that is not a quoted name",
            namespace_observations(8).replace("\"O8\"", "O8"),
        ),
    ] {
        let source = namespace_source(
            &observations,
            &format!("release\n\nNamespace-lane: {{sha}} 8 {AT}\n"),
        );
        let (success, stderr, output) = source.run_step(NAMESPACE_STEP, "");
        assert!(!success, "{name}: accepted {output}");
        assert!(
            stderr.contains("declares no OBSERVATIONS array"),
            "{name}: {stderr}"
        );
        assert!(!output.contains("evidence="), "{name}: {output}");
    }
}

#[test]
fn the_namespace_step_counts_an_observations_array_rustfmt_wrapped() {
    // rustfmt packs short names several to a line once the declaration outgrows one line.
    let wrapped = "pub const OBSERVATIONS: [&str; 12] = [\n    \"O1\", \"O2\", \"O3\", \"O4\", \
                   \"O5\", \"O6\", \"O7\", \"O8\", \"O9\", \"O10\",\n    \"O11\", \"O12\",\n];\n";
    let source = namespace_source(
        wrapped,
        &format!("release\n\nNamespace-lane: {{sha}} 12 {AT}\n"),
    );
    let (success, stderr, _) = source.run_step(NAMESPACE_STEP, "");
    assert!(success, "{stderr}");
    let off = namespace_source(
        wrapped,
        &format!("release\n\nNamespace-lane: {{sha}} 11 {AT}\n"),
    );
    namespace_refused(&off, "", "records 11 namespace-lane observations;");
}

#[test]
fn the_namespace_lane_record_with_a_timestamp_that_is_not_rfc_3339_utc_is_refused() {
    for instant in ["2026-10-23", "2026-10-23T10:00:00+02:00", "yesterday"] {
        let source = namespace_source(
            &namespace_observations(8),
            &format!("release\n\nNamespace-lane: {{sha}} 8 {instant}\n"),
        );
        namespace_refused(&source, "", "where an RFC 3339 UTC instant belongs");
    }
}

#[test]
fn only_a_recovered_tag_that_predates_the_namespace_lane_needs_no_namespace_record() {
    let older = TaggedSource::with_files(&[("README.md", "before the lane\n")], "release\n");
    let (success, stderr, output) = older.run_step_on(NAMESPACE_STEP, "", "workflow_dispatch");
    assert!(success, "{stderr}");
    assert_eq!(output.trim(), "lane=absent");
    // A tag push runs this workflow from the tag itself, so the lane is always there; its absence
    // there is a broken source, not an older one, even with a record that names the commit.
    let forged = TaggedSource::with_files(
        &[("README.md", "before the lane\n")],
        &format!("release\n\nNamespace-lane: {{sha}} 8 {AT}\n"),
    );
    namespace_refused(&forged, "", "has no xtask/src/namespace_lane.rs");
}

#[test]
fn the_namespace_step_counts_the_repositorys_own_observations() {
    // The step counts the entries of one literal array in the *tagged* source, the array
    // `cargo xtask namespace-lane observe` records the length of; assert that count against the
    // file it parses, so a reformatting cannot make every release refuse with a message about the
    // check rather than about the release.
    let real = fs::read_to_string(repository_root().join(NAMESPACE_LANE_SOURCE))
        .expect("namespace lane source");
    assert!(
        real.contains("Decision::Record(OBSERVATIONS.len())"),
        "the lane no longer records the length of OBSERVATIONS"
    );
    let declaration = real
        .lines()
        .find(|line| line.starts_with("pub const OBSERVATIONS: [&str; "))
        .expect("the OBSERVATIONS array");
    let declared = declaration
        .trim_start_matches("pub const OBSERVATIONS: [&str; ")
        .split(']')
        .next()
        .and_then(|length| length.parse::<usize>().ok())
        .expect("a declared length");
    assert!(declared >= 8, "{NAMESPACE_LANE_SOURCE} declares {declared}");
    let source = namespace_source(
        &real,
        &format!("release\n\nNamespace-lane: {{sha}} {declared} {AT}\n"),
    );
    let (success, stderr, output) = source.run_step(NAMESPACE_STEP, "");
    assert!(success, "{stderr}");
    assert!(output.contains(&format!(" {declared} ")), "{output}");
    let off_by_one = namespace_source(
        &real,
        &format!("release\n\nNamespace-lane: {{sha}} {} {AT}\n", declared - 1),
    );
    namespace_refused(&off_by_one, "", "namespace-lane observations;");
}

#[test]
fn the_confinement_lane_record_naming_the_tagged_commit_and_its_case_count_is_accepted() {
    let source = confinement_source(&format!("release\n\nConfinement-lane: {{sha}} 167 {AT}\n"));
    let (success, stderr, output) = source.run_step(CONFINEMENT_STEP, "");
    assert!(success, "{stderr}");
    assert_eq!(output.trim(), format!("evidence={} 167 {AT}", source.sha));
}

#[test]
fn a_tag_with_two_lane_records_is_refused_by_both_steps() {
    for (step, trailer, count, make) in lane_steps() {
        // Two identical records are refused as well: one record per tag, never a choice.
        for second in [
            format!("{trailer}: {{sha}} {count} {AT}"),
            format!("{trailer}: 0123456789abcdef0123456789abcdef01234567 3 {AT}"),
        ] {
            let source = make(&format!(
                "release\n\n{trailer}: {{sha}} {count} {AT}\n{second}\n"
            ));
            refused_by(step, &source, "", "carries 2 ");
        }
    }
}

#[test]
fn a_multi_line_or_wrongly_shaped_dispatch_record_is_refused_by_both_steps() {
    for (step, _, count, make) in lane_steps() {
        let source = make("release\n");
        let sha = source.sha.clone();
        refused_by(
            step,
            &source,
            &format!("{sha} {count} {AT}\nevidence=forged 0 never"),
            "spans more than one line",
        );
        refused_by(
            step,
            &source,
            &format!("{sha} {count} {AT}\r"),
            "spans more than one line",
        );
        refused_by(
            step,
            &source,
            &format!("{sha} {count} {AT} trailing"),
            "has 4 fields",
        );
        refused_by(step, &source, &format!("{sha} {count}"), "has 2 fields");
    }
}

#[test]
fn a_tag_record_with_an_extra_field_is_refused_by_both_steps() {
    for (step, trailer, count, make) in lane_steps() {
        let source = make(&format!(
            "release\n\n{trailer}: {{sha}} {count} {AT} extra\n"
        ));
        refused_by(step, &source, "", "has 4 fields");
    }
}

#[test]
fn a_calendar_impossible_instant_is_refused_by_both_steps() {
    for (step, trailer, count, make) in lane_steps() {
        for instant in [
            "2026-13-45T25:61:61Z",
            "2026-02-30T10:00:00Z",
            "2026-10-23T24:00:00Z",
        ] {
            let source = make(&format!(
                "release\n\n{trailer}: {{sha}} {count} {instant}\n"
            ));
            refused_by(step, &source, "", "where an RFC 3339 UTC instant belongs");
        }
        // A real leap-year day is an instant.
        let leap = make(&format!(
            "release\n\n{trailer}: {{sha}} {count} 2028-02-29T23:59:59Z\n"
        ));
        let (success, stderr, _) = leap.run_step(step, "");
        assert!(success, "{step}: {stderr}");
    }
}

const BINARY_ARCHIVES: [&str; 2] = [
    "substrate-daemon-${VERSION}-${BINARY_TARGET}.tar.gz",
    "b10x-substrate-${VERSION}-${BINARY_TARGET}.tar.gz",
];
const GLIBC_STEP: &str = "Refuse release binaries a glibc 2.35 host cannot load";

#[test]
fn release_binaries_come_from_the_pinned_builder_stage() {
    // One pinned builder compiles every shipped binary once, the CLI included, and a scratch
    // stage exports exactly the two archived binaries; the workflow selects that stage.
    let build = DOCKERFILE
        .split_once("cargo build --locked --release")
        .expect("the builder's one release build")
        .1
        .split_once("install -D")
        .expect("the build's package list ends before the first install")
        .0;
    assert!(build.contains("-p b10x-substrate-cli --bin b10x-substrate"));
    assert!(
        DOCKERFILE.contains("install -D /src/target/release/b10x-substrate /out/b10x-substrate")
    );
    let stage = DOCKERFILE
        .split_once("FROM scratch AS release-binaries\n")
        .expect("a scratch stage holding only the release binaries")
        .1
        .split("\nFROM ")
        .next()
        .expect("release-binaries stage body");
    assert!(stage.contains("COPY --from=builder /out/substrate-daemon /substrate-daemon"));
    assert!(stage.contains("COPY --from=builder /out/b10x-substrate /b10x-substrate"));
    assert!(
        DOCKERFILE.trim_end().ends_with("FROM daemon AS release"),
        "`docker build .` must still select the daemon image"
    );

    position("BINARY_TARGET: x86_64-unknown-linux-gnu");
    position("GLIBC_CEILING: \"2.35\"");
    let export = position("--target release-binaries");
    position("--output \"type=local,dest=${BINARY_DIST}/bin\"");
    let image_under_test = position(&format!("- name: {DAEMON_IMAGE_STEP}\n"));
    let identity = position(
        "cmp \"${BINARY_DIST}/bin/substrate-daemon\" target/image-under-test/substrate-daemon",
    );
    let ceiling = position(&format!("- name: {GLIBC_STEP}\n"));
    let floor_smoke = position("\"${GLIBC_FLOOR_IMAGE}\"");
    let package = position("Package the release binaries");
    let daemon_push = position("docker push \"${IMAGE}:${VERSION}\"");
    let mcp_push = position("docker push \"${MCP_IMAGE}:${VERSION}\"");
    // The daemon that is checked and archived is the image's, and every binary refusal happens
    // before any image tag is mutated.
    assert!(export < image_under_test && image_under_test < identity);
    assert!(identity < ceiling && ceiling < floor_smoke && floor_smoke < package);
    assert!(package < daemon_push && package < mcp_push);
    assert!(
        !step_block(DAEMON_IMAGE_STEP).contains("docker push"),
        "the daemon image step must not push before the archived daemon is proven"
    );
    // The oldest supported glibc is an exact image, not a moving tag.
    position("GLIBC_FLOOR_IMAGE: ubuntu:22.04@sha256:");
}

const DAEMON_IMAGE_STEP: &str = "Build and smoke-test the daemon image";
const PROVE_STEP: &str = "Prove the archived daemon is the image's daemon";
const SOURCE_STEP: &str = "Find out whether the tagged source builds release binaries";
const PUBLISH_STEP: &str = "Publish the GitHub release";
const BINARY_STEPS: [&str; 7] = [
    "Export the release binaries from the pinned builder stage",
    PROVE_STEP,
    GLIBC_STEP,
    "Smoke-test the release binaries on the oldest supported glibc",
    "Package the release binaries",
    "Sign the release-binary checksums keylessly",
    "Verify the release-binary checksum signature with the workflow identity",
];

/// Runs the named release-job step offline in `root`, with `tools` (if any) first on `PATH`.
fn run_release_step(
    step: &str,
    root: &Path,
    env: &[(&str, &str)],
    tools: Option<&Path>,
) -> (bool, String, String) {
    let output_file = root.join("github-output");
    fs::write(&output_file, "").expect("GITHUB_OUTPUT");
    let mut command = std::process::Command::new("bash");
    command
        .current_dir(root)
        .env("GITHUB_OUTPUT", &output_file)
        .args(["-e", "-o", "pipefail", "-c", &step_script(step)]);
    if let Some(tools) = tools {
        command.env(
            "PATH",
            format!(
                "{}:{}",
                tools.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        );
    }
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().expect("bash runs");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(&output_file).expect("GITHUB_OUTPUT readable"),
    )
}

/// A dist directory holding a freshly built daemon and an image-under-test daemon.
fn prove_fixture(fresh: &str, image: &str) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temporary checkout");
    let root = directory.path();
    fs::create_dir_all(root.join("dist/bin")).expect("bin");
    fs::create_dir_all(root.join("target/image-under-test")).expect("image under test");
    fs::write(root.join("dist/bin/substrate-daemon"), fresh).expect("fresh daemon");
    fs::write(root.join("target/image-under-test/substrate-daemon"), image).expect("image daemon");
    directory
}

fn prove(directory: &tempfile::TempDir, exists: &str) -> (bool, String) {
    let (success, output, _) = run_release_step(
        PROVE_STEP,
        directory.path(),
        &[
            ("EXISTS", exists),
            ("BINARY_DIST", "dist"),
            ("IMAGE", "ghcr.io/example/daemon"),
            ("VERSION", TAG),
        ],
        None,
    );
    (success, output)
}

#[test]
fn an_initial_release_refuses_an_archived_daemon_that_is_not_the_images() {
    let same = prove_fixture("daemon bytes", "daemon bytes");
    let (success, output) = prove(&same, "false");
    assert!(success, "{output}");
    let differs = prove_fixture("fresh bytes", "image bytes");
    let (success, output) = prove(&differs, "false");
    assert!(
        !success,
        "accepted a daemon that is not the image's: {output}"
    );
    assert!(
        output.contains("refused: the archived substrate-daemon"),
        "{output}"
    );
    // Anything but an explicit answer about the image tag is refused, not read as `false`.
    for exists in ["", "yes"] {
        let (success, output) = prove(&same, exists);
        assert!(!success, "accepted EXISTS={exists:?}: {output}");
    }
}

#[test]
fn a_recovery_run_archives_the_reused_images_daemon() {
    let recovery = prove_fixture("this run's fresh build", "the reused image's daemon");
    let (success, output) = prove(&recovery, "true");
    assert!(success, "{output}");
    assert_eq!(
        fs::read_to_string(recovery.path().join("dist/bin/substrate-daemon")).expect("archived"),
        "the reused image's daemon",
        "a recovery run must archive the daemon of the image it reuses"
    );
}

fn binary_source(dockerfile: Option<&str>, event: &str) -> (bool, String, String) {
    let directory = tempfile::tempdir().expect("temporary checkout");
    if let Some(dockerfile) = dockerfile {
        fs::write(directory.path().join("Dockerfile"), dockerfile).expect("Dockerfile");
    }
    run_release_step(
        SOURCE_STEP,
        directory.path(),
        &[("EVENT_NAME", event)],
        None,
    )
}

#[test]
fn only_a_recovered_tag_that_predates_the_binaries_stage_publishes_without_binaries() {
    let old = "FROM rust:1 AS builder\nFROM builder AS daemon\nFROM daemon AS release\n";
    let (success, output, outputs) = binary_source(Some(old), "workflow_dispatch");
    assert!(success, "{output}");
    assert_eq!(outputs.trim(), "present=false");
    assert!(output.contains("no release binaries"), "{output}");
    // A tag push runs this workflow from the tag itself, so the stage is always there; its
    // absence there is a broken source, not an older one.
    let (success, output, outputs) = binary_source(Some(old), "push");
    assert!(!success, "{output}");
    assert!(output.contains("refused:"), "{output}");
    assert!(!outputs.contains("present="), "{outputs}");
    let (success, output, _) = binary_source(None, "workflow_dispatch");
    assert!(
        !success,
        "a missing Dockerfile must not read as an older one: {output}"
    );
    let (success, output, outputs) = binary_source(Some(DOCKERFILE), "push");
    assert!(success, "{output}");
    assert_eq!(outputs.trim(), "present=true");
    let (success, output, outputs) = binary_source(Some(DOCKERFILE), "workflow_dispatch");
    assert!(success, "{output}");
    assert_eq!(outputs.trim(), "present=true");

    // Every binary step is skipped, and only skipped, on that one answer.
    position(&format!(
        "- name: {SOURCE_STEP}\n        id: binary-source\n"
    ));
    for step in BINARY_STEPS {
        assert!(
            step_block(step).contains("if: steps.binary-source.outputs.present == 'true'\n"),
            "step {step:?} is not conditional on the tagged source building binaries"
        );
    }
    assert!(position(&format!("- name: {SOURCE_STEP}\n")) < position(BINARY_STEPS[0]));
}

const EXECUTOR_SOURCE_STEP: &str = "Find out whether the tagged source builds the executor image";
const EXECUTOR_BUILD_STEP: &str = "Build, smoke-test and push the executor image once";
const EXECUTOR_STEPS: [&str; 4] = [
    EXECUTOR_BUILD_STEP,
    "Sign the executor image keylessly",
    "Verify the executor signature with the workflow identity",
    "Prove the executor image is anonymously retrievable",
];

#[test]
fn the_executor_image_is_pushed_once_signed_verified_and_read_back_before_announcement() {
    position("EXECUTOR_IMAGE: ghcr.io/beyond10x/b10x-substrate-executor\n");
    assert!(DOCKERFILE.contains("\nFROM scratch AS executor\n"));
    let write_once = position("- name: Resolve write-once image version tags\n");
    let source = position(&format!(
        "- name: {EXECUTOR_SOURCE_STEP}\n        id: executor-source\n"
    ));
    let build = position(&format!(
        "- name: {EXECUTOR_BUILD_STEP}\n        id: executor-build\n"
    ));
    let push = position("docker push \"${EXECUTOR_IMAGE}:${VERSION}\"");
    let sign = position("cosign sign --yes \"${EXECUTOR_IMAGE}@${EXECUTOR_DIGEST}\"");
    let verify = position("\"${EXECUTOR_IMAGE}@${EXECUTOR_DIGEST}\" > executor-cosign-verify.json");
    let anonymous = position(
        "DOCKER_CONFIG=\"$anonymous_config\" docker pull \"${EXECUTOR_IMAGE}@${EXECUTOR_DIGEST}\"",
    );
    let notes = position("- name: Compose release notes\n");
    let announce = position("gh release create \"${VERSION}\"");
    assert!(write_once < source && source < build && build < push);
    assert!(push < sign && sign < verify && verify < anonymous);
    assert!(anonymous < notes && notes < announce);
    assert_eq!(
        occurrences("docker push \"${EXECUTOR_IMAGE}:${VERSION}\""),
        1
    );

    let state = step_script("Resolve write-once image version tags");
    assert!(
        state.contains("if docker manifest inspect \"${EXECUTOR_IMAGE}:${VERSION}\""),
        "{state}"
    );
    assert!(state.contains("executor-exists=true"), "{state}");
    assert!(
        state.contains(
            "refused: ${EXECUTOR_IMAGE}:${VERSION} already exists; initial release runs never reuse image tags."
        ),
        "{state}"
    );

    let build_script = step_script(EXECUTOR_BUILD_STEP);
    for argument in [
        "--target executor",
        "--platform linux/amd64",
        "--build-arg \"SOURCE_SHA=${SOURCE_SHA}\"",
        "--provenance=false",
        "--sbom=false",
    ] {
        assert!(build_script.contains(argument), "{build_script}");
    }
    for step in EXECUTOR_STEPS {
        assert!(
            step_block(step).contains("if: steps.executor-source.outputs.present == 'true'\n"),
            "step {step:?} is not conditional on the tagged source building the executor"
        );
    }
    let verify_script = step_script("Verify the executor signature with the workflow identity");
    assert!(
        verify_script.contains(
            "identity=\"${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/.github/workflows/release.yml@${GITHUB_REF}\""
        ),
        "{verify_script}"
    );
    assert!(
        verify_script
            .contains("--certificate-oidc-issuer 'https://token.actions.githubusercontent.com'"),
        "{verify_script}"
    );

    // Recorded beside the other digests: the release notes, the changelog stanza and the summary.
    let composed = &WORKFLOW[notes..announce];
    assert!(composed.contains("## Executor image"), "{composed}");
    assert!(composed.contains("| digest | \\`${EXECUTOR_DIGEST}\\` |"));
    assert!(composed.contains("--certificate-identity ${EXECUTOR_IDENTITY} \\\\"));
    assert!(composed.contains("predates the Dockerfile's \\`executor\\` stage"));
    position(
        "Executor image: \\`${EXECUTOR_IMAGE}:${VERSION}\\` at \\`${EXECUTOR_DIGEST}\\`, keyless-signed",
    );
    position("| executor digest | \\`${EXECUTOR_DIGEST:-<not built>}\\` |");
    position("| executor signature step | ${{ steps.executor-verify.outcome }} |");
    position("| executor anonymous pull step | ${{ steps.executor-anonymous.outcome }} |");
}

fn executor_source(dockerfile: &str, event: &str) -> (bool, String, String) {
    let directory = tempfile::tempdir().expect("temporary checkout");
    fs::write(directory.path().join("Dockerfile"), dockerfile).expect("Dockerfile");
    run_release_step(
        EXECUTOR_SOURCE_STEP,
        directory.path(),
        &[("EVENT_NAME", event)],
        None,
    )
}

#[test]
fn only_a_recovered_tag_that_predates_the_executor_stage_publishes_no_executor_image() {
    let old = "FROM rust:1 AS builder\nFROM builder AS daemon\nFROM daemon AS release\n";
    let (success, output, outputs) = executor_source(old, "workflow_dispatch");
    assert!(success, "{output}");
    assert_eq!(outputs.trim(), "present=false");
    assert!(output.contains("no executor image"), "{output}");
    let (success, output, outputs) = executor_source(old, "push");
    assert!(!success, "{output}");
    assert!(output.contains("refused:"), "{output}");
    assert!(!outputs.contains("present="), "{outputs}");
    for event in ["push", "workflow_dispatch"] {
        let (success, output, outputs) = executor_source(DOCKERFILE, event);
        assert!(success, "{output}");
        assert_eq!(outputs.trim(), "present=true");
    }
}

/// Runs the executor build step against stub `docker`, `readelf` and `git`: the image answers
/// `handshake` to `version`, carries `revision` as its source label, and `readelf` reports an
/// interpreter when `dynamic`. Returns the step's success, log, output and every push it made.
fn executor_build(
    exists: &str,
    handshake: &str,
    revision: &str,
    dynamic: bool,
) -> (bool, String, String, String) {
    let root = tempfile::tempdir().expect("runner directory");
    let tools = tempfile::tempdir().expect("stub tools");
    stub(
        tools.path(),
        "docker",
        &format!(
            "case \"$1\" in\n  buildx) [ \"$2\" = imagetools ] && echo '\"sha256:{}\"'; exit 0 ;;\n  run) printf '%s\\n' \"$HANDSHAKE\" ;;\n  create) echo container ;;\n  cp) printf 'executor' > \"$3\" ;;\n  rm) ;;\n  pull) ;;\n  image) case \"$*\" in *Entrypoint*) printf '%s\\n' '[\"/substrate-executor\"]' ;; *) printf '%s\\n' \"$REVISION\" ;; esac ;;\n  manifest) exit 1 ;;\n  push) echo \"$2\" >> \"$PUSH_LOG\" ;;\n  *) exit 2 ;;\nesac",
            "e".repeat(64)
        ),
    );
    stub(
        tools.path(),
        "readelf",
        "printf 'Program Headers:\\n  LOAD 0x0\\n'\n[ -n \"$DYNAMIC\" ] && printf '  INTERP 0x0\\n'\nexit 0",
    );
    stub(
        tools.path(),
        "git",
        "case \"$1\" in\n  fetch) ;;\n  rev-parse) echo \"$SOURCE_SHA\" ;;\n  *) exit 2 ;;\nesac",
    );
    let push_log = root.path().join("push.log");
    fs::write(&push_log, "").expect("push log");
    let push_log_path = push_log.display().to_string();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let (success, log, output) = run_release_step(
        EXECUTOR_BUILD_STEP,
        root.path(),
        &[
            ("EXISTS", exists),
            ("EXECUTOR_IMAGE", "ghcr.io/example/executor"),
            ("VERSION", TAG),
            ("SOURCE_SHA", sha),
            ("HANDSHAKE", &handshake.replace("{sha}", sha)),
            ("REVISION", &revision.replace("{sha}", sha)),
            ("DYNAMIC", if dynamic { "1" } else { "" }),
            ("PUSH_LOG", &push_log_path),
        ],
        Some(tools.path()),
    );
    (
        success,
        log,
        output,
        fs::read_to_string(&push_log).expect("push log"),
    )
}

const HANDSHAKE: &str =
    "{\"protocol\":\"substrate-executor/1\",\"version\":\"9.9.9\",\"source\":\"{sha}\"}";

#[test]
fn the_executor_image_is_pushed_only_when_its_handshake_names_this_release_and_it_is_static() {
    let (success, log, output, pushes) = executor_build("false", HANDSHAKE, "{sha}", false);
    assert!(success, "{log}");
    assert_eq!(pushes.trim(), format!("ghcr.io/example/executor:{TAG}"));
    assert_eq!(output.trim(), format!("digest=sha256:{}", "e".repeat(64)));

    for (name, handshake, dynamic, reason) in [
        (
            "another release",
            HANDSHAKE.replace("9.9.9", "9.9.8"),
            false,
            "answers the version handshake",
        ),
        (
            "an unstamped source",
            HANDSHAKE.replace("{sha}", "unknown"),
            false,
            "answers the version handshake",
        ),
        (
            "another protocol",
            HANDSHAKE.replace("substrate-executor/1", "substrate-executor/2"),
            false,
            "answers the version handshake",
        ),
        (
            "a second line",
            format!("{HANDSHAKE}\n{HANDSHAKE}"),
            false,
            "answers the version handshake",
        ),
        (
            "a dynamic executable",
            HANDSHAKE.to_owned(),
            true,
            "is not a static executable",
        ),
    ] {
        let (success, log, output, pushes) = executor_build("false", &handshake, "{sha}", dynamic);
        assert!(!success, "{name}: accepted {log}");
        assert!(log.contains(reason), "{name}: {log}");
        assert_eq!(pushes, "", "{name}: pushed before refusing");
        assert!(!output.contains("digest="), "{name}: {output}");
    }

    // Anything but an explicit answer about the image tag is refused, not read as `false`.
    for exists in ["", "yes"] {
        let (success, log, _, pushes) = executor_build(exists, HANDSHAKE, "{sha}", false);
        assert!(!success, "accepted EXISTS={exists:?}: {log}");
        assert_eq!(pushes, "");
    }
}

#[test]
fn a_recovery_reuses_an_executor_image_only_from_the_same_source_and_release() {
    let (success, log, output, pushes) = executor_build("true", HANDSHAKE, "{sha}", false);
    assert!(success, "{log}");
    assert!(log.contains("reusing ghcr.io/example/executor:9.9.9 from the same tagged source"));
    assert_eq!(pushes, "", "a reused image is not pushed again");
    assert!(output.contains("digest=sha256:"), "{output}");
    let (success, log, _, pushes) = executor_build(
        "true",
        HANDSHAKE,
        "fedcba9876543210fedcba9876543210fedcba98",
        false,
    );
    assert!(!success, "{log}");
    assert!(log.contains("names source fedcba98"), "{log}");
    assert_eq!(pushes, "");
}

/// Runs the publication step with a fake `gh` and returns its success, output and the argument
/// lines `gh` received.
fn publish(binaries: &str, assets_present: bool) -> (bool, String, String) {
    let directory = tempfile::tempdir().expect("temporary checkout");
    let root = directory.path();
    let tools = root.join("tools");
    fs::create_dir_all(&tools).expect("tools");
    let gh = tools.join("gh");
    fs::write(&gh, "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$GH_LOG\"\n").expect("fake gh");
    let mut permissions = fs::metadata(&gh).expect("metadata").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    fs::set_permissions(&gh, permissions).expect("executable fake gh");
    fs::create_dir_all(root.join("dist/assets")).expect("assets");
    if assets_present {
        for asset in [
            format!("substrate-daemon-{TAG}-x86_64-unknown-linux-gnu.tar.gz"),
            format!("b10x-substrate-{TAG}-x86_64-unknown-linux-gnu.tar.gz"),
            "SHA256SUMS".to_owned(),
            "SHA256SUMS.sigstore.json".to_owned(),
        ] {
            fs::write(root.join("dist/assets").join(asset), "asset").expect("asset");
        }
    }
    let log = root.join("gh.log");
    let log_text = log.display().to_string();
    let (success, output, _) = run_release_step(
        PUBLISH_STEP,
        root,
        &[
            ("BINARIES", binaries),
            ("VERSION", TAG),
            ("GITHUB_REPOSITORY", "example/substrate"),
            ("BINARY_DIST", "dist"),
            ("BINARY_TARGET", "x86_64-unknown-linux-gnu"),
            ("GH_LOG", &log_text),
        ],
        Some(&tools),
    );
    (
        success,
        output,
        fs::read_to_string(&log).unwrap_or_default(),
    )
}

#[test]
fn the_release_attaches_all_four_binary_assets_or_none() {
    let (success, output, calls) = publish("true", true);
    assert!(success, "{output}");
    let create = calls
        .lines()
        .find(|line| line.starts_with("release create "))
        .expect("gh release create ran");
    for asset in [
        format!("dist/assets/substrate-daemon-{TAG}-x86_64-unknown-linux-gnu.tar.gz"),
        format!("dist/assets/b10x-substrate-{TAG}-x86_64-unknown-linux-gnu.tar.gz"),
        "dist/assets/SHA256SUMS ".to_owned(),
        "dist/assets/SHA256SUMS.sigstore.json".to_owned(),
    ] {
        assert!(
            format!("{create} ").contains(&asset),
            "missing {asset}: {create}"
        );
    }
    assert!(create.contains("--draft"), "{create}");

    // A missing asset is refused before anything is created.
    let (success, output, calls) = publish("true", false);
    assert!(!success, "{output}");
    assert!(output.contains("refused:"), "{output}");
    assert!(calls.is_empty(), "gh ran: {calls}");

    // The one source without the stage publishes the release with no asset at all.
    let (success, output, calls) = publish("false", false);
    assert!(success, "{output}");
    let create = calls
        .lines()
        .find(|line| line.starts_with("release create "))
        .expect("gh release create ran");
    assert!(!create.contains("dist/assets"), "{create}");

    // An unset or unexpected answer is refused rather than read as either.
    for binaries in ["", "maybe"] {
        let (success, output, calls) = publish(binaries, true);
        assert!(!success, "accepted BINARIES={binaries:?}: {output}");
        assert!(calls.is_empty(), "gh ran: {calls}");
    }

    // Notes and summaries say when a release carries no binaries.
    position("BINARIES: ${{ steps.binary-source.outputs.present }}");
    position("This release carries no binaries:");
    position("not published: the tagged source has no release-binaries stage");
}

#[test]
fn readme_names_both_release_signing_identities() {
    let readme = fs::read_to_string(repository_root().join("README.md")).expect("README");
    for identity in [
        "https://github.com/beyond10x/substrate/.github/workflows/release.yml@refs/tags/<version>",
        "https://github.com/beyond10x/substrate/.github/workflows/release.yml@refs/heads/main",
    ] {
        assert!(
            readme.contains(identity),
            "README.md does not name {identity}"
        );
    }
    // The release notes print the identity of the run, whichever event started it.
    assert!(occurrences(".github/workflows/release.yml@${GITHUB_REF}") >= 4);
}

#[test]
fn release_binaries_are_archived_summed_signed_and_verified_before_announcement() {
    for archive in BINARY_ARCHIVES {
        assert!(
            occurrences(archive) >= 2,
            "{archive} must be both packaged and attached"
        );
    }
    let sums = position("sha256sum -- \"${archives[@]}\" > SHA256SUMS");
    let sign = position(
        "cosign sign-blob --yes \\\n            --bundle \"${BINARY_DIST}/assets/SHA256SUMS.sigstore.json\"",
    );
    let verify = position("cosign verify-blob \\");
    let announce = position("gh release create \"${VERSION}\"");
    assert!(sums < sign && sign < verify && verify < announce);
    // The verification reads the bundle the release attaches, for this workflow at this ref.
    let verify_step =
        &step_script("Verify the release-binary checksum signature with the workflow identity");
    assert!(verify_step.contains("cosign verify-blob \\"));
    assert!(verify_step.contains("--bundle \"${BINARY_DIST}/assets/SHA256SUMS.sigstore.json\""));
    assert!(verify_step.contains("--certificate-identity \"${identity}\""));
    assert!(verify_step.contains(".github/workflows/release.yml@${GITHUB_REF}"));
    assert!(verify_step.contains("sha256sum --check --strict SHA256SUMS"));

    // All four files go onto the release the workflow creates, while it is still a draft.
    let create = step_block(PUBLISH_STEP);
    for asset in BINARY_ARCHIVES
        .iter()
        .copied()
        .chain(["SHA256SUMS\"", "SHA256SUMS.sigstore.json\""])
    {
        assert!(
            create.contains(&format!("\"${{BINARY_DIST}}/assets/{asset}")),
            "gh release create does not attach {asset}"
        );
    }
    position("## Release binaries");
    position("cosign verify-blob \\\\");
    position("| binaries signature step | ${{ steps.binaries-verify.outcome }} |");
}

/// Runs a release-job step offline with `objdump` and `readelf` replaced by fakes that report
/// `glibc` symbol versions and `needed` libraries for every binary.
fn run_glibc_step(glibc: &[&str], needed: &[&str]) -> (bool, String) {
    let directory = tempfile::tempdir().expect("temporary dist");
    let root = directory.path();
    let bin = root.join("dist/bin");
    fs::create_dir_all(&bin).expect("bin");
    for binary in ["substrate-daemon", "b10x-substrate"] {
        fs::write(bin.join(binary), "not an ELF; the fakes answer").expect("binary");
    }
    let tools = root.join("tools");
    fs::create_dir_all(&tools).expect("tools");
    let mut objdump = String::from("#!/bin/sh\n");
    for version in glibc {
        writeln!(
            objdump,
            "echo '0000000000000000      DF *UND*  0000000000000000 ({version}) symbol'"
        )
        .expect("writing to a String cannot fail");
    }
    let mut readelf = String::from("#!/bin/sh\n");
    for library in needed {
        writeln!(
            readelf,
            "echo ' 0x0000000000000001 (NEEDED)             Shared library: [{library}]'"
        )
        .expect("writing to a String cannot fail");
    }
    for (name, script) in [("objdump", objdump), ("readelf", readelf)] {
        let path = tools.join(name);
        fs::write(&path, script).expect("fake tool");
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        fs::set_permissions(&path, permissions).expect("executable fake tool");
    }
    let path = format!(
        "{}:{}",
        tools.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = std::process::Command::new("bash")
        .current_dir(root)
        .env("PATH", path)
        .env("BINARY_DIST", "dist")
        .env("GLIBC_CEILING", "2.35")
        .args(["-e", "-o", "pipefail", "-c", &step_script(GLIBC_STEP)])
        .output()
        .expect("bash runs");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned()
            + &String::from_utf8_lossy(&output.stderr),
    )
}

const SHIPPED_LIBRARIES: [&str; 7] = [
    "libssl.so.3",
    "libcrypto.so.3",
    "libz.so.1",
    "libgcc_s.so.1",
    "libm.so.6",
    "libc.so.6",
    "ld-linux-x86-64.so.2",
];

#[test]
fn release_binaries_within_the_glibc_ceiling_and_documented_libraries_are_accepted() {
    // 0.12.0's daemon, measured: GLIBC_2.34 at most, and exactly these seven libraries.
    let (success, output) = run_glibc_step(
        &["GLIBC_2.2.5", "GLIBC_2.4", "GLIBC_2.34", "GLIBC_2.33"],
        &SHIPPED_LIBRARIES,
    );
    assert!(success, "{output}");
    assert!(output.contains("needs GLIBC_2.34"), "{output}");
    let (success, output) = run_glibc_step(&["GLIBC_2.35"], &["libc.so.6"]);
    assert!(success, "{output}");
}

#[test]
fn a_release_binary_needing_a_newer_glibc_is_refused() {
    // Version order, not string order: 2.36 is newer than 2.35, and 2.4 is older.
    for newest in ["GLIBC_2.36", "GLIBC_2.39", "GLIBC_3.0"] {
        let (success, output) =
            run_glibc_step(&["GLIBC_2.4", newest, "GLIBC_2.34"], &SHIPPED_LIBRARIES);
        assert!(!success, "accepted {newest}: {output}");
        assert!(
            output.contains(&format!("refused: substrate-daemon needs {newest}")),
            "{output}"
        );
    }
    let (success, output) = run_glibc_step(&[], &SHIPPED_LIBRARIES);
    assert!(!success, "{output}");
    assert!(output.contains("names no GLIBC symbol version"), "{output}");
}

#[test]
fn a_release_binary_needing_an_undocumented_library_is_refused() {
    let mut needed = SHIPPED_LIBRARIES.to_vec();
    needed.push("libcurl.so.4");
    let (success, output) = run_glibc_step(&["GLIBC_2.34"], &needed);
    assert!(!success, "{output}");
    assert!(
        output.contains("refused: substrate-daemon needs libcurl.so.4"),
        "{output}"
    );
}

#[test]
fn readme_records_what_the_release_binaries_need_at_run_time() {
    let readme = fs::read_to_string(repository_root().join("README.md")).expect("README");
    for needle in [
        "substrate-daemon-<version>-x86_64-unknown-linux-gnu.tar.gz",
        "b10x-substrate-<version>-x86_64-unknown-linux-gnu.tar.gz",
        "SHA256SUMS.sigstore.json",
        "libssl3",
        "zlib1g",
        "glibc 2.35",
    ] {
        assert!(
            readme.contains(needle),
            "README.md does not state {needle:?}"
        );
    }
}

#[test]
fn development_status_is_verified_and_never_described_as_stable() {
    position("dev.b10x.contract.status");
    position("== \"development\"");
    position("remains a **development** bundle");
    assert!(!WORKFLOW.contains("stable contract bundle"));
}

#[test]
fn protected_main_gets_a_pull_request_stanza_not_a_direct_push() {
    assert!(!WORKFLOW.contains("git push"));
    position("CHANGELOG.md is protected by the Full gate and is not pushed directly.");
    position("Contract bundle: \\`${BUNDLE_IMAGE}:${BUNDLE_VERSION}\\` at \\`${BUNDLE_DIGEST}\\`");
}

#[test]
fn every_third_party_action_is_pinned_to_a_commit() {
    for line in WORKFLOW.lines().chain(GATE_WORKFLOW.lines()).map(str::trim) {
        let Some(action) = line.strip_prefix("uses: ") else {
            continue;
        };
        let reference = action
            .split_whitespace()
            .next()
            .expect("action reference before comment");
        let (_, revision) = reference
            .rsplit_once('@')
            .unwrap_or_else(|| panic!("action is unpinned: {line}"));
        assert_eq!(revision.len(), 40, "action is not commit-pinned: {line}");
        assert!(
            revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "action is not commit-pinned: {line}"
        );
    }
}

/// The whole text of the named release-job step, from its `- name:` line to the next step.
fn step_block(name: &str) -> &'static str {
    let rest = &WORKFLOW[position(&format!("- name: {name}\n"))..];
    let end = rest[1..]
        .find("\n      - name: ")
        .map_or(rest.len(), |end| end + 2);
    &rest[..end]
}

#[test]
fn the_archived_daemon_is_compared_with_the_image_before_the_daemon_tag_is_pushed() {
    // A refusal after `docker push` leaves a write-once daemon tag behind, and the only way on is
    // a recovery dispatch, whose branch of the comparison step skips `cmp`. So the byte-identity
    // refusal holds only if it runs before the tag moves, like every other binary refusal.
    let compare = position(
        "cmp \"${BINARY_DIST}/bin/substrate-daemon\" target/image-under-test/substrate-daemon",
    );
    let daemon_push = position("docker push \"${IMAGE}:${VERSION}\"");
    assert!(
        compare < daemon_push,
        "the archived daemon is compared with the image's at byte {compare}, after the daemon tag \
         is pushed at byte {daemon_push}"
    );
}

#[test]
fn the_checksum_signature_is_verified_against_one_exact_identity_and_issuer() {
    let verify =
        step_script("Verify the release-binary checksum signature with the workflow identity");
    assert!(!verify.contains("regexp"), "{verify}");
    assert!(
        verify.contains("--certificate-oidc-issuer 'https://token.actions.githubusercontent.com'"),
        "{verify}"
    );
    assert!(
        verify.contains(
            "identity=\"${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/.github/workflows/release.yml@${GITHUB_REF}\""
        ),
        "{verify}"
    );
    // The command an operator is told to run is exact as well.
    let notes = &WORKFLOW[position("## Release binaries")..position("## Confinement lane")];
    assert!(!notes.contains("regexp"), "{notes}");
    assert!(notes.contains("--certificate-identity ${BINARIES_IDENTITY}"));
    assert!(
        notes.contains("--certificate-oidc-issuer https://token.actions.githubusercontent.com")
    );
    let readme = fs::read_to_string(repository_root().join("README.md")).expect("README");
    let section = readme
        .split_once("### Release binaries")
        .expect("README release-binaries section")
        .1
        .split("\n### ")
        .next()
        .expect("section body");
    assert!(!section.contains("regexp"), "{section}");
}

#[test]
fn release_binary_steps_receive_no_token_or_secret() {
    for step in [
        "Export the release binaries from the pinned builder stage",
        GLIBC_STEP,
        "Smoke-test the release binaries on the oldest supported glibc",
        "Package the release binaries",
        "Prove the archived daemon is the image's daemon",
        "Sign the release-binary checksums keylessly",
        "Verify the release-binary checksum signature with the workflow identity",
    ] {
        let block = step_block(step);
        for forbidden in ["secrets.", "TOKEN", "--secret", "--build-arg"] {
            assert!(
                !block.contains(forbidden),
                "step {step:?} carries {forbidden:?}:\n{block}"
            );
        }
    }
}

#[test]
fn only_the_release_job_holds_packages_and_id_token_write() {
    let release_job = position("\n  release:\n");
    for grant in ["packages: write", "id-token: write", "contents: write"] {
        let lines: Vec<usize> = WORKFLOW
            .match_indices(grant)
            .map(|(at, _)| at)
            .filter(|&at| {
                let line_start = WORKFLOW[..at].rfind('\n').map_or(0, |n| n + 1);
                WORKFLOW[line_start..at].trim().is_empty()
            })
            .collect();
        assert_eq!(lines.len(), 1, "{grant} is granted {} times", lines.len());
        assert!(
            lines[0] > release_job,
            "{grant} is granted outside the release job"
        );
    }
    let top = &WORKFLOW[position("\npermissions:\n") + 1..];
    let top = &top[..top
        .find("\n\n")
        .expect("workflow-level permissions block ends")];
    assert_eq!(top, "permissions:\n  contents: read");
}

/// Runs the packaging step offline in a tagged throwaway repository and returns the bytes of
/// every file it wrote to the assets directory, by name.
fn package_release_binaries(source: &TaggedSource) -> Vec<(String, Vec<u8>)> {
    let root = source.directory.path();
    let _ = fs::remove_dir_all(root.join("dist"));
    let bin = root.join("dist/bin");
    fs::create_dir_all(&bin).expect("bin");
    for binary in ["substrate-daemon", "b10x-substrate"] {
        fs::write(bin.join(binary), format!("{binary} bytes")).expect("binary");
    }
    let output_file = root.join("github-output");
    fs::write(&output_file, "").expect("GITHUB_OUTPUT");
    let output = std::process::Command::new("bash")
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("SOURCE_SHA", &source.sha)
        .env("VERSION", TAG)
        .env("BINARY_TARGET", "x86_64-unknown-linux-gnu")
        .env("BINARY_DIST", "dist")
        .env("GITHUB_OUTPUT", &output_file)
        .args([
            "-e",
            "-o",
            "pipefail",
            "-c",
            &step_script("Package the release binaries"),
        ])
        .output()
        .expect("bash runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut assets: Vec<(String, Vec<u8>)> = fs::read_dir(root.join("dist/assets"))
        .expect("assets")
        .map(|entry| {
            let entry = entry.expect("asset entry");
            (
                entry.file_name().to_string_lossy().into_owned(),
                fs::read(entry.path()).expect("asset bytes"),
            )
        })
        .collect();
    assets.sort();
    assets
}

#[test]
fn release_archives_are_deterministic_named_as_documented_and_summed_exactly() {
    let source = TaggedSource::with_files(
        &[
            ("LICENSE", "licence text\n"),
            ("THIRD_PARTY_LICENSES.html", "<p>notices</p>\n"),
            ("README.md", "# readme\n"),
        ],
        "release",
    );
    let first = package_release_binaries(&source);
    // Input modification times and the wall clock must not reach the archive bytes.
    for file in ["LICENSE", "THIRD_PARTY_LICENSES.html", "README.md"] {
        let status = std::process::Command::new("touch")
            .args(["-d", "2001-02-03T04:05:06Z"])
            .arg(source.directory.path().join(file))
            .status()
            .expect("touch runs");
        assert!(status.success());
    }
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let second = package_release_binaries(&source);
    assert_eq!(first, second, "packaging the same inputs twice differs");

    let daemon = format!("substrate-daemon-{TAG}-x86_64-unknown-linux-gnu.tar.gz");
    let cli = format!("b10x-substrate-{TAG}-x86_64-unknown-linux-gnu.tar.gz");
    let names: Vec<&str> = first.iter().map(|(name, _)| name.as_str()).collect();
    let mut expected = vec!["SHA256SUMS", cli.as_str(), daemon.as_str()];
    expected.sort_unstable();
    assert_eq!(names, expected);

    let sums = String::from_utf8(
        first
            .iter()
            .find(|(name, _)| name == "SHA256SUMS")
            .expect("SHA256SUMS")
            .1
            .clone(),
    )
    .expect("utf-8 sums");
    let mut summed: Vec<&str> = sums
        .lines()
        .map(|line| line.split_once("  ").expect("sha256sum line").1)
        .collect();
    summed.sort_unstable();
    let mut archives = vec![daemon.as_str(), cli.as_str()];
    archives.sort_unstable();
    assert_eq!(
        summed, archives,
        "SHA256SUMS covers exactly the two archives"
    );

    let assets = source.directory.path().join("dist/assets");
    for (archive, binary) in [(&daemon, "substrate-daemon"), (&cli, "b10x-substrate")] {
        let stem = archive.trim_end_matches(".tar.gz");
        let listing = std::process::Command::new("tar")
            .args(["--numeric-owner", "-tvzf"])
            .arg(assets.join(archive))
            .output()
            .expect("tar runs");
        assert!(listing.status.success());
        let listing = String::from_utf8(listing.stdout).expect("utf-8 listing");
        let mut entries: Vec<&str> = listing
            .lines()
            .map(|line| {
                assert!(line.contains(" 0/0 "), "entry not owned by 0/0: {line}");
                line.rsplit(' ').next().expect("entry name")
            })
            .collect();
        entries.sort_unstable();
        let mut want = vec![
            format!("{stem}/"),
            format!("{stem}/{binary}"),
            format!("{stem}/LICENSE"),
            format!("{stem}/README.md"),
            format!("{stem}/THIRD_PARTY_LICENSES.html"),
        ];
        want.sort_unstable();
        assert_eq!(
            entries, want,
            "{archive} holds exactly the documented files"
        );
    }
}

const CHART_LANE_STEP: &str = "Require a green chart-lane run for this commit";
const CHART_SOURCE: &str = "deploy/helm/substrate/Chart.yaml";

/// A routing `gh` stub: `gh api <path> --jq F` applies `F` with `jq -r` to `$STUB_JOBS` for a
/// jobs path, `$STUB_ANNOTATIONS` for an annotations path and `$STUB_JSON` otherwise, and logs
/// each path to `$GH_LOG`. `sleep` fails, so a step that would wait ends at once.
fn routing_stub_tools() -> tempfile::TempDir {
    let tools = api_stub_tools();
    let gh = tools.path().join("gh");
    fs::write(
        &gh,
        "#!/bin/bash\nfilter=.\npath=\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --jq) filter=$2; shift 2 ;;\n    api|--paginate) shift ;;\n    *) path=$1; shift ;;\n  esac\ndone\nprintf '%s\\n' \"$path\" >> \"$GH_LOG\"\ncase \"$path\" in\n  */jobs*) json=$STUB_JOBS ;;\n  */annotations*) json=$STUB_ANNOTATIONS ;;\n  *) json=$STUB_JSON ;;\nesac\nprintf '%s' \"$json\" | jq -r \"$filter\"\n",
    )
    .expect("routing gh stub");
    tools
}

/// Chart-lane runs of the tagged commit, each a `push` to `main` of this repository, as
/// `(status, conclusion)`.
fn main_runs(runs: &[(&str, &str)]) -> String {
    let runs: Vec<String> = runs
        .iter()
        .enumerate()
        .map(|(index, (status, conclusion))| {
            format!(
                "{{\"id\":{index},\"status\":\"{status}\",\"conclusion\":\"{conclusion}\",\"event\":\"push\",\"head_branch\":\"main\",\"head_repository\":{{\"full_name\":\"example/substrate\"}},\"html_url\":\"https://example.invalid/runs/{index}\"}}"
            )
        })
        .collect();
    format!("{{\"workflow_runs\":[{}]}}", runs.join(","))
}

/// Runs the chart-lane preflight step offline against the routing `gh` stub; the tagged source
/// either carries the chart or predates it. Returns the step's success, log, output and the API
/// paths it read.
fn chart_lane_reading(
    with_chart: bool,
    event: &str,
    runs: &str,
    jobs: &str,
    annotations: &str,
) -> (bool, String, String, String) {
    let files: &[(&str, &str)] = if with_chart {
        &[(CHART_SOURCE, "apiVersion: v2\nname: b10x-substrate\n")]
    } else {
        &[("README.md", "a source before the chart\n")]
    };
    let source = TaggedSource::with_files(files, "release {sha}");
    let tools = routing_stub_tools();
    let gh_log = tools.path().join("gh.log");
    fs::write(&gh_log, "").expect("gh log");
    let gh_log_path = gh_log.display().to_string();
    let (success, log, output) = run_release_step(
        CHART_LANE_STEP,
        source.directory.path(),
        &[
            ("SOURCE_SHA", &source.sha),
            ("EVENT_NAME", event),
            ("STUB_JSON", runs),
            ("STUB_JOBS", jobs),
            ("STUB_ANNOTATIONS", annotations),
            ("GH_LOG", &gh_log_path),
            ("CHART_WORKFLOW", "chart-lane.yml"),
            ("GITHUB_REPOSITORY", "example/substrate"),
            ("GITHUB_API_URL", "https://api.github.com"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
        ],
        Some(tools.path()),
    );
    (
        success,
        log,
        output,
        fs::read_to_string(&gh_log).expect("gh log"),
    )
}

fn chart_lane(with_chart: bool, event: &str, runs: &str) -> (bool, String, String) {
    let (success, log, output, _) =
        chart_lane_reading(with_chart, event, runs, "{\"jobs\":[]}", "[]");
    (success, log, output)
}

#[test]
fn a_tag_with_a_green_chart_lane_run_is_accepted_and_its_run_recorded() {
    let (success, log, output) = chart_lane(
        true,
        "push",
        &main_runs(&[("completed", "failure"), ("completed", "success")]),
    );
    assert!(success, "{log}");
    assert!(output.contains("chart=present\n"), "{output}");
    assert!(
        output.contains("run-url=https://example.invalid/runs/1\n"),
        "{output}"
    );
}

#[test]
fn the_chart_lanes_absent_scenarios_are_read_from_its_run_and_carried_to_the_release() {
    let (success, log, output, paths) = chart_lane_reading(
        true,
        "push",
        &main_runs(&[("completed", "success")]),
        "{\"jobs\":[{\"id\":41,\"check_run_url\":\"https://api.github.com/repos/example/substrate/check-runs/41\"}]}",
        "[{\"annotation_level\":\"notice\",\"title\":\"chart-lane absent\",\"message\":\"execution bootstrap: not run on kind\"},{\"annotation_level\":\"warning\",\"title\":\"chart-lane absent\",\"message\":\"a warning is not a record\"},{\"annotation_level\":\"notice\",\"title\":\"something else\",\"message\":\"another notice\"},{\"annotation_level\":\"notice\",\"title\":\"chart-lane absent\",\"message\":\"egress: never dialled\"}]",
    );
    assert!(success, "{log}");
    assert!(
        output.contains("absent=execution bootstrap: not run on kind; egress: never dialled\n"),
        "{output}"
    );
    assert!(
        paths.contains("repos/example/substrate/actions/runs/0/jobs?per_page=100\n"),
        "{paths}"
    );
    assert!(
        paths.contains("repos/example/substrate/check-runs/41/annotations?per_page=100\n"),
        "{paths}"
    );
    for (summary, line) in [
        ("Preflight summary", "chart lane absent, not passed"),
        (
            "Compose release notes",
            "| chart lane absent, not passed | ${CHART_LANE_ABSENT} |",
        ),
        ("Release summary", "chart lane absent, not passed"),
    ] {
        assert!(
            step_block(summary).contains(line),
            "{summary} omits the absent scenarios"
        );
    }
    assert!(WORKFLOW.contains("      chart-lane-absent: ${{ steps.chart-lane.outputs.absent }}\n"));
    assert!(WORKFLOW.contains("      actions: read\n      checks: read\n"));
}

#[test]
fn a_chart_lane_run_with_no_absent_annotation_says_none_recorded() {
    let (success, log, output) = chart_lane(true, "push", &main_runs(&[("completed", "success")]));
    assert!(success, "{log}");
    assert!(output.contains("absent=none recorded\n"), "{output}");
}

#[test]
fn a_tag_whose_chart_lane_runs_all_failed_is_refused() {
    let (success, log, output) = chart_lane(
        true,
        "push",
        &main_runs(&[("completed", "failure"), ("completed", "cancelled")]),
    );
    assert!(!success, "{log}");
    assert!(log.contains("none concluded success"), "{log}");
    assert!(!output.contains("chart="), "{output}");
}

#[test]
fn a_tag_push_of_a_source_without_the_chart_is_refused() {
    let (success, log, output) = chart_lane(false, "push", &main_runs(&[("completed", "success")]));
    assert!(!success, "{log}");
    assert!(log.contains("has no deploy/helm/substrate chart"), "{log}");
    assert!(!output.contains("chart="), "{output}");
}

#[test]
fn a_recovery_of_a_tag_that_predates_the_chart_publishes_no_chart() {
    let (success, log, output) = chart_lane(false, "workflow_dispatch", "");
    assert!(success, "{log}");
    assert_eq!(output, "chart=absent\n");
}

#[test]
fn a_dispatch_run_of_this_repository_is_admitted_and_a_fork_dispatch_is_not() {
    let own = json_runs(&[("workflow_dispatch", "story/x", "example/substrate")]);
    let (success, log, output) = chart_lane(true, "push", &own);
    assert!(
        success && output.contains("chart=present\n"),
        "{log}{output}"
    );
    let fork = json_runs(&[("workflow_dispatch", "main", "someone/substrate")]);
    let (success, log, output) = chart_lane(true, "push", &fork);
    assert!(
        !success && !output.contains("chart=present"),
        "{log}{output}"
    );
    let branch = json_runs(&[("push", "story/x", "example/substrate")]);
    let (success, log, output) = chart_lane(true, "push", &branch);
    assert!(
        !success && !output.contains("chart=present"),
        "{log}{output}"
    );
}

#[test]
fn the_chart_is_published_after_the_daemon_and_signed_attested_and_read_back_before_announcement() {
    let preflight_gate = position("- name: Require a green gate run for this commit\n");
    let preflight_chart = position(&format!("- name: {CHART_LANE_STEP}\n"));
    let release_job = position("\n  release:\n");
    let daemon_push = position("docker push \"${IMAGE}:${VERSION}\"");
    let package = position("-- package-chart \\");
    let digest_argument = position("--image-digest \"${DIGEST}\"");
    let push = position("helm push \"$archive\" \"oci://${CHART_REPOSITORY}\"");
    let sign = position("cosign sign --yes \"${CHART_IMAGE}@${CHART_DIGEST}\"");
    let verify = position("\"${CHART_IMAGE}@${CHART_DIGEST}\" > chart-cosign-verify.json");
    let attest = position("uses: actions/attest-build-provenance@");
    let provenance = position("gh attestation verify \"oci://${CHART_IMAGE}@${CHART_DIGEST}\"");
    let anonymous =
        position("DOCKER_CONFIG=\"$anonymous_config\" oras resolve \"${CHART_IMAGE}:${VERSION}\"");
    let announce = position("gh release create \"${VERSION}\"");
    assert!(preflight_gate < preflight_chart && preflight_chart < release_job);
    assert!(daemon_push < package && package < digest_argument && digest_argument < push);
    assert!(push < sign && sign < verify && verify < attest && attest < provenance);
    assert!(provenance < anonymous && anonymous < announce);
    assert_eq!(occurrences("helm push "), 1);
    assert!(occurrences(".github/workflows/release.yml@${GITHUB_REF}") >= 5);
    let block = step_block("Attest the chart's build provenance");
    assert!(
        block.contains("subject-digest: ${{ steps.chart.outputs.digest }}"),
        "{block}"
    );
    assert!(block.contains("push-to-registry: true"), "{block}");
}

#[test]
fn the_chart_tag_is_write_once_and_a_recovery_reuses_only_the_same_chart() {
    let block = step_block("Package and push the Kubernetes chart once");
    let exists = block
        .find("remote_digest=$(resolve_chart)")
        .expect("the chart tag is looked up before anything is pushed");
    let refused = block
        .find("initial release runs never reuse chart tags")
        .expect("a tag push refuses an existing chart tag");
    let compared = block
        .find("if [ \"$remote_layer\" != \"$local_layer\" ]; then")
        .expect("a recovery compares the existing chart with the local package");
    let raced = block
        .find("appeared before push; it will not be overwritten")
        .expect("the tag is checked again right before the push");
    let push = block.find("helm push ").expect("the push");
    assert!(exists < refused && refused < compared && compared < raced && raced < push);
    let package = position("- name: Package the Kubernetes chart\n");
    let login = position("- name: Log in to the chart registry\n");
    let publish = position("- name: Package and push the Kubernetes chart once\n");
    assert!(
        package < login && login < publish,
        "the repository's packager runs before any chart-registry credential exists"
    );
    assert!(
        !block.contains("cargo "),
        "the step holding the registry login runs no repository code"
    );
    for gated in [
        "Install the pinned Helm release",
        "Package the Kubernetes chart",
        "Log in to the chart registry",
        "Package and push the Kubernetes chart once",
        "Sign the Kubernetes chart keylessly",
        "Verify the chart signature with the workflow identity",
        "Attest the chart's build provenance",
        "Verify the chart provenance with the workflow identity",
        "Prove the chart is anonymously retrievable",
    ] {
        assert!(
            step_block(gated).contains("if: env.CHART == 'present'"),
            "{gated} runs for a source without a chart"
        );
    }
    assert!(
        !step_block("Package the Kubernetes chart").contains("secrets."),
        "the packaging step, which runs repository code, receives no token"
    );
}

/// Runs the chart publication step against a local package `genuine` and a registry stub:
/// `existing` names a chart already at the tag whose layer is `remote_layer`; `helm push` makes
/// the tag resolve to a chart whose layer is `remote_layer` too.
fn chart_publication_against(
    event: &str,
    existing: bool,
    remote_layer: &str,
) -> (bool, String, String, String) {
    let root = tempfile::tempdir().expect("runner directory");
    let tools = tempfile::tempdir().expect("stub tools");
    fs::create_dir_all(root.path().join("target/release-chart")).expect("chart directory");
    fs::write(
        root.path()
            .join(format!("target/release-chart/b10x-substrate-{TAG}.tgz")),
        "genuine",
    )
    .expect("local package");
    stub(
        tools.path(),
        "oras",
        &format!(
            "case \"$1\" in\n  resolve) if [ -n \"$EXISTING\" ] || [ -f \"$STATE/pushed\" ]; then echo sha256:{}; else echo \"Error: $2: not found\" >&2; exit 1; fi ;;\n  manifest) printf '{{\"layers\":[{{\"mediaType\":\"application/vnd.cncf.helm.chart.content.v1.tar+gzip\",\"digest\":\"%s\"}}]}}\\n' \"$REMOTE_LAYER\" ;;\n  *) exit 2 ;;\nesac",
            "c".repeat(64)
        ),
    );
    stub(
        tools.path(),
        "helm",
        "echo \"$*\" >> \"$HELM_LOG\"\n[ \"$1\" = push ] && : > \"$STATE/pushed\"\nexit 0",
    );
    let helm_log = root.path().join("helm.log");
    fs::write(&helm_log, "").expect("helm log");
    let state = root.path().display().to_string();
    let helm_log_path = helm_log.display().to_string();
    let (success, log, output) = run_release_step(
        CHART_PUBLISH_STEP,
        root.path(),
        &[
            ("CHART_IMAGE", "ghcr.io/example/charts/b10x-substrate"),
            ("CHART_REPOSITORY", "ghcr.io/example/charts"),
            ("VERSION", TAG),
            ("EVENT_NAME", event),
            ("EXISTING", if existing { "1" } else { "" }),
            ("REMOTE_LAYER", remote_layer),
            ("STATE", &state),
            ("HELM_LOG", &helm_log_path),
        ],
        Some(tools.path()),
    );
    (
        success,
        log,
        output,
        fs::read_to_string(&helm_log).expect("helm log"),
    )
}

fn genuine_layer() -> String {
    use sha2::Digest as _;
    format!("sha256:{}", hex::encode(sha2::Sha256::digest(b"genuine")))
}

#[test]
fn a_recovery_reuses_an_existing_chart_whose_layer_is_the_local_package() {
    let (success, log, output, helm) =
        chart_publication_against("workflow_dispatch", true, &genuine_layer());
    assert!(success, "{log}");
    assert_eq!(helm, "", "a reused chart is not pushed again");
    assert!(
        output.contains(&format!("digest=sha256:{}\n", "c".repeat(64))),
        "{output}"
    );
    assert!(log.contains("byte for byte"), "{log}");
}

#[test]
fn a_recovery_refuses_an_existing_chart_whose_layer_differs_from_the_local_package() {
    let other = format!("sha256:{}", "d".repeat(64));
    let (success, log, output, helm) = chart_publication_against("workflow_dispatch", true, &other);
    assert!(!success, "{log}");
    assert!(log.contains("not the deterministic package"), "{log}");
    assert_eq!(helm, "");
    assert!(!output.contains("digest="), "{output}");
}

#[test]
fn a_tag_push_refuses_an_existing_chart_even_when_it_is_byte_identical() {
    let (success, log, _, helm) = chart_publication_against("push", true, &genuine_layer());
    assert!(!success, "{log}");
    assert!(
        log.contains("initial release runs never reuse chart tags"),
        "{log}"
    );
    assert_eq!(helm, "");
}

#[test]
fn a_fresh_chart_is_pushed_once_and_its_pushed_layer_is_verified() {
    let (success, log, output, helm) = chart_publication_against("push", false, &genuine_layer());
    assert!(success, "{log}");
    assert_eq!(
        helm,
        format!(
            "push target/release-chart/b10x-substrate-{TAG}.tgz oci://ghcr.io/example/charts\n"
        )
    );
    assert!(output.contains("digest=sha256:"), "{output}");
    let other = format!("sha256:{}", "d".repeat(64));
    let (success, log, output, _) = chart_publication_against("push", false, &other);
    assert!(!success, "{log}");
    assert!(log.contains("not the pushed"), "{log}");
    assert!(!output.contains("digest="), "{output}");
}

#[test]
fn the_write_once_step_refuses_an_existing_or_unresolvable_chart_tag_on_a_tag_push() {
    let block = step_block("Resolve write-once image version tags");
    let chart = block
        .find("if chart_found=$(oras resolve \"${CHART_IMAGE}:${VERSION}\" 2>&1); then")
        .expect("the write-once step resolves the chart tag");
    let refused = block[chart..]
        .find("initial release runs never reuse chart tags")
        .expect("and refuses it on a tag push");
    let unreliable = block[chart..]
        .find("could not resolve ${CHART_IMAGE}:${VERSION} reliably")
        .expect("and refuses an answer that is neither a digest nor not-found");
    assert!(refused < unreliable);
    assert!(block.contains("if [ \"$CHART\" = present ]; then"));
}

#[test]
fn only_the_release_job_may_write_attestations() {
    let release_job = position("\n  release:\n");
    let grants: Vec<usize> = WORKFLOW
        .match_indices("attestations: write")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(
        grants.len(),
        1,
        "attestations: write is granted {} times",
        grants.len()
    );
    assert!(grants[0] > release_job);
}

#[test]
fn the_chart_lane_workflow_runs_static_checks_before_a_pinned_kind_cluster() {
    let lane = include_str!("../../.github/workflows/chart-lane.yml");
    for line in lane.lines().map(str::trim) {
        assert!(
            !line.starts_with('#'),
            "chart-lane.yml carries a comment: {line}"
        );
        if let Some(action) = line.strip_prefix("uses: ") {
            let reference = action.split_whitespace().next().expect("action reference");
            let (_, revision) = reference.rsplit_once('@').expect("pinned action");
            assert_eq!(revision.len(), 40, "action is not commit-pinned: {line}");
        }
    }
    for pin in ["KIND_VERSION", "KIND_SHA256", "KIND_NODE_IMAGE"] {
        let value = |text: &str| {
            text.lines()
                .find_map(|line| line.trim().strip_prefix(&format!("{pin}: ")))
                .map(str::to_owned)
        };
        assert_eq!(
            value(lane),
            value(GATE_WORKFLOW),
            "{pin} differs from gate.yml's"
        );
    }
    let helm_pin = |text: &str| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix("HELM_SHA256: "))
            .map(str::to_owned)
    };
    assert_eq!(
        helm_pin(lane),
        helm_pin(WORKFLOW),
        "the two workflows pin different Helm releases"
    );
    assert!(lane.contains("    needs: static\n"));
    assert!(lane.contains("chart-lane static --helm"));
    assert!(lane.contains("chart-lane cluster \\"));
    assert!(lane.contains("permissions:\n  contents: read\n"));
    for write in ["contents: write", "packages: write", "id-token: write"] {
        assert!(!lane.contains(write), "chart-lane.yml grants {write}");
    }
}

// --- adversary: a run that did not test the tagged tree is not evidence for it -------------------

/// Stub tools for a preflight step that reads the Actions API: `gh api --jq F` applies `F` to
/// `$STUB_JSON` with `jq -r` (as `gh` prints strings raw), and `sleep` fails, so a step that would
/// wait for another run ends unsuccessfully at once instead of hanging the test.
fn api_stub_tools() -> tempfile::TempDir {
    let tools = tempfile::tempdir().expect("stub tools");
    let executable = |name: &str, body: &str| {
        let path = tools.path().join(name);
        fs::write(&path, body).expect("stub tool");
        fs::set_permissions(
            &path,
            <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )
        .expect("stub tool is executable");
    };
    executable(
        "gh",
        "#!/bin/bash\nfilter=.\nwhile [ $# -gt 0 ]; do\n  case \"$1\" in\n    --jq) filter=$2; shift 2 ;;\n    *) shift ;;\n  esac\ndone\nprintf '%s' \"$STUB_JSON\" | jq -r \"$filter\"\n",
    );
    executable(
        "sleep",
        "#!/bin/sh\necho 'stub sleep: the step chose to wait' >&2\nexit 97\n",
    );
    tools
}

/// A `pull_request` run's `head_sha` is the pull request's head commit, but the run checked out
/// and tested `refs/pull/N/merge`: that head merged into whatever base the pull request targets,
/// with the base's workflow file. This repository merges by merge commit, so every pull-request
/// head is an ancestor of `main` and passes the tag step's ancestry check; a chart-lane `push`
/// run exists only for commits pushed to `main`, so for a tagged pull-request head the
/// pull-request run is the only one there is, and it never ran on the tagged tree.
fn pull_request_runs(fork: bool) -> String {
    json_runs(&[(
        "pull_request",
        "story/node-bound-kubernetes-serving-profile",
        if fork {
            "someone/substrate"
        } else {
            "example/substrate"
        },
    )])
}

fn json_runs(runs: &[(&str, &str, &str)]) -> String {
    let runs: Vec<String> = runs
        .iter()
        .enumerate()
        .map(|(index, (event, branch, repository))| {
            format!(
                "{{\"id\":{index},\"status\":\"completed\",\"conclusion\":\"success\",\"event\":\"{event}\",\"head_branch\":\"{branch}\",\"head_repository\":{{\"full_name\":\"{repository}\"}},\"html_url\":\"https://example.invalid/runs/{index}\"}}"
            )
        })
        .collect();
    format!(
        "{{\"total_count\":{},\"workflow_runs\":[{}]}}",
        runs.len(),
        runs.join(",")
    )
}

fn chart_lane_api(event: &str, json: &str) -> (bool, String, String) {
    let source = TaggedSource::with_files(
        &[(CHART_SOURCE, "apiVersion: v2\nname: b10x-substrate\n")],
        "release {sha}",
    );
    let tools = api_stub_tools();
    run_release_step(
        CHART_LANE_STEP,
        source.directory.path(),
        &[
            ("SOURCE_SHA", &source.sha),
            ("EVENT_NAME", event),
            ("STUB_JSON", json),
            ("CHART_WORKFLOW", "chart-lane.yml"),
            ("GITHUB_REPOSITORY", "example/substrate"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
        ],
        Some(tools.path()),
    )
}

#[test]
fn adversary_the_stub_api_admits_a_push_run_on_main() {
    let (success, log, output) =
        chart_lane_api("push", &json_runs(&[("push", "main", "example/substrate")]));
    assert!(success, "{log}");
    assert!(output.contains("chart=present\n"), "{output}");
}

#[test]
fn adversary_a_chart_lane_pull_request_run_is_not_evidence_for_the_tagged_commit() {
    for fork in [false, true] {
        let (success, log, output) = chart_lane_api("push", &pull_request_runs(fork));
        assert!(
            !success && !output.contains("chart=present"),
            "a pull_request run (fork: {fork}) was taken as the chart lane of the tagged commit: {log}{output}"
        );
    }
}

#[test]
fn adversary_a_gate_pull_request_run_is_not_evidence_for_the_tagged_commit() {
    let directory = tempfile::tempdir().expect("step directory");
    let tools = api_stub_tools();
    let runs = pull_request_runs(true);
    let (success, log, output) = run_release_step(
        "Require a green gate run for this commit",
        directory.path(),
        &[
            ("SOURCE_SHA", "1111111111111111111111111111111111111111"),
            ("STUB_JSON", &runs),
            ("GATE_WORKFLOW", "gate.yml"),
            ("GITHUB_REPOSITORY", "example/substrate"),
        ],
        Some(tools.path()),
    );
    assert!(
        !success && !output.contains("run-url="),
        "a fork's pull_request gate run was taken as the gate of the tagged commit: {log}{output}"
    );
}

// Security review (story:node-bound-kubernetes-serving-profile): the chart publication's
// write-once and refuse-before-publish claims, run offline against stub tools.

const CHART_PUBLISH_STEP: &str = "Package and push the Kubernetes chart once";

fn stub(directory: &Path, name: &str, body: &str) {
    let path = directory.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("stub tool");
    fs::set_permissions(
        &path,
        <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .expect("stub tool is executable");
}

/// Runs the chart publication step with `oras` answering `oras_body`; returns the step's
/// success, its log and every `helm` invocation it made.
fn chart_publication(event: &str, oras_body: &str) -> (bool, String, String) {
    let root = tempfile::tempdir().expect("runner directory");
    let tools = tempfile::tempdir().expect("stub tools");
    let digest = format!("sha256:{}", "b".repeat(64));
    stub(tools.path(), "oras", oras_body);
    stub(
        tools.path(),
        "helm",
        "echo \"$*\" >> \"$HELM_LOG\"\ncase \"$1\" in\n  show) printf '  digest: \"%s\"\\n' \"$DIGEST\" ;;\n  pull) d=\"\"; while [ $# -gt 0 ]; do [ \"$1\" = --destination ] && d=\"$2\"; shift; done; printf 'tampered' > \"$d/b10x-substrate-$VERSION.tgz\" ;;\nesac\nexit 0",
    );
    stub(
        tools.path(),
        "cargo",
        "mkdir -p target/release-chart && printf 'genuine' > \"target/release-chart/b10x-substrate-$VERSION.tgz\"",
    );
    let helm_log = root.path().join("helm.log");
    fs::write(&helm_log, "").expect("helm log");
    let runner_temp = root.path().display().to_string();
    let helm_log_path = helm_log.display().to_string();
    let (success, log, _) = run_release_step(
        CHART_PUBLISH_STEP,
        root.path(),
        &[
            ("CHART_IMAGE", "ghcr.io/example/charts/b10x-substrate"),
            ("CHART_REPOSITORY", "ghcr.io/example/charts"),
            ("VERSION", TAG),
            ("DIGEST", &digest),
            ("EVENT_NAME", event),
            ("RUNNER_TEMP", &runner_temp),
            ("HELM_LOG", &helm_log_path),
        ],
        Some(tools.path()),
    );
    (
        success,
        log,
        fs::read_to_string(&helm_log).expect("helm log"),
    )
}

#[test]
fn security_review_an_unanswered_chart_tag_lookup_never_reaches_helm_push() {
    let (_, log, helm) = chart_publication(
        "push",
        "echo 'Error: failed to resolve: GET https://ghcr.io/v2/example/charts/b10x-substrate/manifests/9.9.9: 503 Service Unavailable' >&2\nexit 1",
    );
    assert!(
        !helm.lines().any(|line| line.starts_with("push ")),
        "a registry error was read as an absent tag and the chart was pushed over whatever the tag names:\nhelm: {helm}\nlog: {log}"
    );
}

#[test]
fn security_review_a_recovery_reuses_a_chart_only_after_comparing_its_whole_content() {
    let (success, log, helm) = chart_publication(
        "workflow_dispatch",
        &format!("echo sha256:{}", "c".repeat(64)),
    );
    assert!(
        !success,
        "a recovery signed and attested an existing chart whose content it compared only by one values line:\nhelm: {helm}\nlog: {log}"
    );
}

#[test]
fn security_review_an_existing_chart_tag_is_refused_before_anything_is_published() {
    let refusal = position("initial release runs never reuse chart tags");
    let first_publication = position("oras cp --from-oci-layout --no-tty")
        .min(position("docker push \"${IMAGE}:${VERSION}\""));
    assert!(
        refusal < first_publication,
        "a tag push whose chart tag already exists publishes the bundle and daemon image before it is refused"
    );
}

#[test]
fn security_review_the_release_requires_a_green_namespace_lane_workflow_for_the_tagged_commit() {
    // Design 34 § 10.4: "`release.yml` refuses a tag whose commit has no successful
    // `namespace-lane.yml`, as it does for `gate.yml`". D182 moved the kind cluster part to a
    // workstation record; the `static` job stays in `namespace-lane.yml`, `scripts/gate.sh` does
    // not run `cargo xtask namespace-lane check`, and no ruleset requires that workflow on `main`.
    let release_job = position("\n  release:\n");
    let preflight = &WORKFLOW[..release_job];
    let via_env = WORKFLOW.lines().find_map(|line| {
        line.trim()
            .strip_suffix(": namespace-lane.yml")
            .map(str::to_owned)
    });
    let queried = preflight.contains("actions/workflows/namespace-lane.yml/runs")
        || via_env
            .as_ref()
            .is_some_and(|key| preflight.contains(&format!("actions/workflows/${{{key}}}/runs")));
    assert!(
        queried,
        "the preflight never reads namespace-lane.yml's conclusion for the tagged commit, so a tag \
         whose namespace-lane static check is red is released"
    );
}

/// The executor build step against stubs where the image's entrypoint answers `entrypoint_answer`
/// and `/substrate-executor`, the path the driver runs (`EXECUTOR_PATH`), answers `binary_answer`.
/// Returns the step's success, its log and every push it made.
fn executor_build_with_entrypoint(
    entrypoint_answer: &str,
    binary_answer: &str,
) -> (bool, String, String) {
    let root = tempfile::tempdir().expect("runner directory");
    let tools = tempfile::tempdir().expect("stub tools");
    stub(
        tools.path(),
        "docker",
        &format!(
            "case \"$1\" in\n  buildx) [ \"$2\" = imagetools ] && echo '\"sha256:{}\"'; exit 0 ;;\n  run) explicit=\"\"; prev=\"\"; for a in \"$@\"; do [ \"$prev\" = --entrypoint ] && [ \"$a\" = /substrate-executor ] && explicit=1; prev=\"$a\"; done\n       if [ -n \"$explicit\" ]; then printf '%s\\n' \"$BINARY_ANSWER\"; else printf '%s\\n' \"$ENTRYPOINT_ANSWER\"; fi ;;\n  create) echo container ;;\n  cp) printf 'executor' > \"$3\" ;;\n  rm) ;;\n  pull) ;;\n  image) case \"$*\" in *Entrypoint*) printf '%s\\n' '[\"/wrapper\"]' ;; *) printf '%s\\n' \"$SOURCE_SHA\" ;; esac ;;\n  manifest) exit 1 ;;\n  push) echo \"$2\" >> \"$PUSH_LOG\" ;;\n  *) exit 2 ;;\nesac",
            "e".repeat(64)
        ),
    );
    stub(
        tools.path(),
        "readelf",
        "printf 'Program Headers:\\n  LOAD 0x0\\n'\nexit 0",
    );
    stub(
        tools.path(),
        "git",
        "case \"$1\" in\n  fetch) ;;\n  rev-parse) echo \"$SOURCE_SHA\" ;;\n  *) exit 2 ;;\nesac",
    );
    let push_log = root.path().join("push.log");
    fs::write(&push_log, "").expect("push log");
    let push_log_path = push_log.display().to_string();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let (success, log, _) = run_release_step(
        EXECUTOR_BUILD_STEP,
        root.path(),
        &[
            ("EXISTS", "false"),
            ("EXECUTOR_IMAGE", "ghcr.io/example/executor"),
            ("VERSION", TAG),
            ("SOURCE_SHA", sha),
            (
                "ENTRYPOINT_ANSWER",
                &entrypoint_answer.replace("{sha}", sha),
            ),
            ("BINARY_ANSWER", &binary_answer.replace("{sha}", sha)),
            ("PUSH_LOG", &push_log_path),
        ],
        Some(tools.path()),
    );
    (
        success,
        log,
        fs::read_to_string(&push_log).expect("push log"),
    )
}

#[test]
fn security_review_the_executor_handshake_is_answered_by_the_binary_the_driver_runs() {
    // The driver runs `/substrate-executor` by path (`EXECUTOR_PATH`, the keeper's command and
    // every `pods/exec` call), never the image's entrypoint. The step's handshake runs the
    // entrypoint and its static check reads `/substrate-executor`, so an image whose entrypoint
    // is another program answers for a binary nobody checked the version of.
    let (success, log, pushes) =
        executor_build_with_entrypoint(HANDSHAKE, &HANDSHAKE.replace("9.9.9", "9.9.8"));
    assert!(
        !success && pushes.is_empty(),
        "an executor image whose entrypoint answers the handshake while /substrate-executor is \
         another release was pushed:\npushes: {pushes}\nlog: {log}"
    );
}

#[test]
fn security_review_a_tag_with_a_second_empty_namespace_lane_line_is_refused() {
    // "a release records exactly one": the count of lines runs after command substitution has
    // stripped trailing newlines, so a second, empty `Namespace-lane:` line after the record is
    // not counted.
    let source = namespace_source(
        &namespace_observations(8),
        &format!("release\n\nNamespace-lane: {{sha}} 8 {AT}\nNamespace-lane:\n"),
    );
    namespace_refused(&source, "", "Namespace-lane lines");
}

#[test]
fn security_review_the_executor_binary_refusals_run_before_any_image_tag_moves() {
    // "Every binary refusal runs before any image tag is mutated." The executor's handshake and
    // static-executable refusals run after the daemon and MCP tags are pushed, so a refused
    // executor leaves two unsigned write-once tags that only a recovery dispatch can go on from.
    let executor_check = position("          check_executor\n");
    let daemon_push = position("docker push \"${IMAGE}:${VERSION}\"");
    let mcp_push = position("docker push \"${MCP_IMAGE}:${VERSION}\"");
    assert!(
        executor_check < daemon_push && executor_check < mcp_push,
        "the executor's binary refusals run at byte {executor_check}, after the daemon push at \
         {daemon_push} and the MCP push at {mcp_push}"
    );
}

#[test]
fn security_review_the_namespace_lane_record_resists_carrier_and_shape_substitutions() {
    let observations = namespace_observations(8);

    // A record in the tagged commit's message, not the tag's, is not read.
    let directory = tempfile::tempdir().expect("temporary repository");
    let root = directory.path();
    git(root, &["init", "--quiet"]);
    let file = root.join(NAMESPACE_LANE_SOURCE);
    fs::create_dir_all(file.parent().expect("parent")).expect("source tree");
    fs::write(&file, &observations).expect("source");
    git(root, &["add", "--all"]);
    git(root, &["commit", "--quiet", "-m", "first"]);
    let first = git(root, &["rev-parse", "HEAD"]);
    git(
        root,
        &[
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            &format!("second\n\nNamespace-lane: {first} 8 {AT}\n"),
        ],
    );
    let sha = git(root, &["rev-parse", "HEAD"]);
    git(root, &["tag", "--annotate", TAG, "-m", "release\n"]);
    let commit_carrier = TaggedSource { directory, sha };
    namespace_refused(
        &commit_carrier,
        "",
        "refused: no recorded namespace lane run for",
    );

    // A record inside another line, or indented, is not a record.
    for message in [
        format!("release\n\nNote: Namespace-lane: {{sha}} 8 {AT}\n"),
        format!("release\n\n  Namespace-lane: {{sha}} 8 {AT}\n"),
        format!("release\n\nnamespace-lane: {{sha}} 8 {AT}\n"),
    ] {
        let source = namespace_source(&observations, &message);
        namespace_refused(&source, "", "refused: no recorded namespace lane run for");
    }

    // A sha prefix or another spelling of the commit is not the commit.
    let source = namespace_source(&observations, "release\n");
    for claimed in [source.sha[..12].to_owned(), source.sha.to_uppercase()] {
        let (success, stderr, output) = source.run_step_on(
            NAMESPACE_STEP,
            &format!("{claimed} 8 {AT}"),
            "workflow_dispatch",
        );
        assert!(!success, "accepted {claimed}: {output}");
        assert!(stderr.contains("records commit"), "{stderr}");
    }
    let (success, stderr, output) = source.run_step_on(
        NAMESPACE_STEP,
        &format!("{} 08 {AT}", source.sha),
        "workflow_dispatch",
    );
    assert!(!success, "accepted a zero-padded count: {output}");
    assert!(stderr.contains("namespace-lane observations;"), "{stderr}");

    // Trailing whitespace is trimmed and never published.
    let padded = namespace_source(
        &observations,
        &format!("release\n\nNamespace-lane: {{sha}} 8 {AT} \t \n"),
    );
    let (success, stderr, output) = padded.run_step(NAMESPACE_STEP, "");
    assert!(success, "{stderr}");
    assert_eq!(
        output.trim(),
        format!("lane=present\nevidence={} 8 {AT}", padded.sha)
    );

    // A commented-out entry or a commented-out second declaration does not move the count.
    let commented_entry = observations.replace("\"O8\"];", "\"O8\",\n    // \"O9\",\n];");
    let source = namespace_source(
        &commented_entry,
        &format!("release\n\nNamespace-lane: {{sha}} 9 {AT}\n"),
    );
    let (success, _, output) = source.run_step(NAMESPACE_STEP, "");
    assert!(!success, "counted a commented-out entry: {output}");
    let commented_declaration = format!(
        "// pub const OBSERVATIONS: [&str; 9] = [\"O1\", \"O2\", \"O3\", \"O4\", \"O5\", \"O6\", \
         \"O7\", \"O8\", \"O9\"];\n{observations}"
    );
    for (claimed, accepted) in [(9, false), (8, true)] {
        let source = namespace_source(
            &commented_declaration,
            &format!("release\n\nNamespace-lane: {{sha}} {claimed} {AT}\n"),
        );
        let (success, stderr, _) = source.run_step(NAMESPACE_STEP, "");
        assert_eq!(success, accepted, "claimed {claimed}: {stderr}");
    }
}

const NAMESPACE_WORKFLOW_STEP: &str = "Require a green namespace-lane run for this commit";
const NAMESPACE_WORKFLOW_SOURCE: &str = ".github/workflows/namespace-lane.yml";

/// Runs the namespace-lane workflow preflight step offline against the routing `gh` stub; the
/// tagged source either carries `namespace-lane.yml` or predates it. Returns the step's success,
/// log, output and the API paths it read.
fn namespace_workflow(
    with_workflow: bool,
    event: &str,
    runs: &str,
) -> (bool, String, String, String) {
    let files: &[(&str, &str)] = if with_workflow {
        &[(NAMESPACE_WORKFLOW_SOURCE, "name: Namespace lane\n")]
    } else {
        &[("README.md", "a source before the namespace lane workflow\n")]
    };
    let source = TaggedSource::with_files(files, "release {sha}");
    let tools = routing_stub_tools();
    let gh_log = tools.path().join("gh.log");
    fs::write(&gh_log, "").expect("gh log");
    let gh_log_path = gh_log.display().to_string();
    let (success, log, output) = run_release_step(
        NAMESPACE_WORKFLOW_STEP,
        source.directory.path(),
        &[
            ("SOURCE_SHA", &source.sha),
            ("EVENT_NAME", event),
            ("STUB_JSON", runs),
            ("STUB_JOBS", "{\"jobs\":[]}"),
            ("STUB_ANNOTATIONS", "[]"),
            ("GH_LOG", &gh_log_path),
            ("NAMESPACE_WORKFLOW", "namespace-lane.yml"),
            ("GITHUB_REPOSITORY", "example/substrate"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
        ],
        Some(tools.path()),
    );
    (
        success,
        log,
        output,
        fs::read_to_string(&gh_log).expect("gh log"),
    )
}

#[test]
fn a_tag_with_a_green_namespace_lane_workflow_run_is_accepted_and_its_run_recorded() {
    let (success, log, output, paths) = namespace_workflow(
        true,
        "push",
        &main_runs(&[("completed", "failure"), ("completed", "success")]),
    );
    assert!(success, "{log}");
    assert_eq!(
        output,
        "workflow=present\nrun-url=https://example.invalid/runs/1\n"
    );
    assert!(
        paths.starts_with(
            "repos/example/substrate/actions/workflows/namespace-lane.yml/runs?head_sha="
        ),
        "{paths}"
    );
    assert!(position(&format!("- name: {NAMESPACE_WORKFLOW_STEP}\n")) < position("\n  release:\n"));
    assert!(WORKFLOW.contains("  NAMESPACE_WORKFLOW: namespace-lane.yml\n"));
}

#[test]
fn a_tag_whose_namespace_lane_workflow_runs_all_failed_or_are_absent_is_refused() {
    for runs in [
        main_runs(&[("completed", "failure"), ("completed", "cancelled")]),
        json_runs(&[("workflow_dispatch", "main", "someone/substrate")]),
        json_runs(&[("push", "story/x", "example/substrate")]),
        main_runs(&[]),
    ] {
        let (success, log, output, _) = namespace_workflow(true, "push", &runs);
        assert!(!success, "accepted {runs}: {log}");
        assert!(
            log.contains("refused:") || log.contains("stub sleep"),
            "{log}"
        );
        assert!(!output.contains("run-url="), "{output}");
    }
}

#[test]
fn a_tag_push_of_a_source_without_the_namespace_lane_workflow_is_refused() {
    let (success, log, output, _) =
        namespace_workflow(false, "push", &main_runs(&[("completed", "success")]));
    assert!(!success, "{log}");
    assert!(
        log.contains("has no .github/workflows/namespace-lane.yml"),
        "{log}"
    );
    assert!(!output.contains("workflow="), "{output}");
}

#[test]
fn a_recovery_of_a_tag_that_predates_the_namespace_lane_workflow_needs_no_run_and_says_so() {
    let (success, log, output, paths) = namespace_workflow(false, "workflow_dispatch", "");
    assert!(success, "{log}");
    assert_eq!(output, "workflow=absent\n");
    assert_eq!(
        paths, "",
        "a source without the workflow has no run to read"
    );
    assert!(WORKFLOW.contains(
        "      namespace-lane-workflow: ${{ steps.namespace-workflow.outputs.workflow }}\n"
    ));
    assert!(WORKFLOW.contains(
        "      namespace-lane-run-url: ${{ steps.namespace-workflow.outputs.run-url }}\n"
    ));
    let notes = step_block("Compose release notes");
    assert!(
        notes.contains("\\`.github/workflows/namespace-lane.yml\\` concluded \\`success\\` for it: ${NAMESPACE_LANE_RUN_URL}"),
        "{notes}"
    );
    assert!(
        notes.contains("predates \\`.github/workflows/namespace-lane.yml\\`"),
        "{notes}"
    );
    for summary in ["Preflight summary", "Release summary"] {
        assert!(
            step_block(summary).contains("| namespace lane run |"),
            "{summary} omits the namespace-lane workflow run"
        );
    }
}

#[test]
fn a_tag_with_a_second_empty_lane_line_is_refused_by_every_lane_step() {
    // Command substitution strips trailing newlines, so a count taken from the extracted record
    // misses an empty trailer after it. Every lane step counts the trailers in the tag itself.
    for (step, trailer, count, make) in lane_steps() {
        for message in [
            format!("release\n\n{trailer}: {{sha}} {count} {AT}\n{trailer}:\n"),
            format!("release\n\n{trailer}: {{sha}} {count} {AT}\n{trailer}:   \n"),
            format!("release\n\n{trailer}:\n{trailer}: {{sha}} {count} {AT}\n"),
        ] {
            let source = make(&message);
            refused_by(step, &source, "", &format!("carries 2 {trailer} lines"));
        }
    }
}

#[test]
fn an_executor_image_whose_entrypoint_is_not_the_executor_is_refused() {
    // The binary at /substrate-executor answers the handshake correctly; the image's entrypoint
    // is another program, which a caller that runs the image rather than the path would reach.
    let (success, log, pushes) = executor_build_with_entrypoint(HANDSHAKE, HANDSHAKE);
    assert!(!success, "{log}");
    assert!(log.contains("entrypoint"), "{log}");
    assert_eq!(pushes, "", "pushed before refusing");
    let script = step_script(EXECUTOR_BUILD_STEP);
    assert!(
        script
            .contains("--entrypoint /substrate-executor \"${EXECUTOR_IMAGE}:${VERSION}\" version"),
        "{script}"
    );
    assert!(script.contains("{{json .Config.Entrypoint}}"), "{script}");
    // The refusal must accept the image the Dockerfile builds.
    let executor_stage = &DOCKERFILE[DOCKERFILE
        .find("\nFROM scratch AS executor\n")
        .expect("executor stage")..];
    assert!(
        executor_stage.contains("\nENTRYPOINT [\"/substrate-executor\"]\n"),
        "{executor_stage}"
    );
}

#[test]
fn the_header_comment_enumerates_no_images_or_refusals_that_can_go_stale() {
    let header = &WORKFLOW[..position("\nname: Release\n")];
    assert!(!header.contains("ghcr.io/"), "{header}");
    assert!(!header.contains("What this workflow refuses"), "{header}");
}
