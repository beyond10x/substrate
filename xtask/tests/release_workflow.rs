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
        current, "0.24.0",
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
    assert_eq!(occurrences("from the same tagged source ${SOURCE_SHA}"), 2);
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
    assert_eq!(
        DOCKERFILE.matches("cargo build --locked --release").count(),
        1
    );
    assert!(DOCKERFILE.contains("-p b10x-substrate-daemon --bin substrate-daemon"));
    assert!(DOCKERFILE.contains("-p b10x-substrate-mcp --bin substrate-mcp"));
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
        let output_file = self.directory.path().join("github-output");
        fs::write(&output_file, "").expect("GITHUB_OUTPUT");
        let output = std::process::Command::new("bash")
            .current_dir(self.directory.path())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
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

/// Both lane steps, each with a source it accepts and the trailer name it reads.
fn lane_steps() -> [LaneStep; 2] {
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
    ]
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
