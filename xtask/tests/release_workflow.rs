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
        current, "0.23.0",
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
        let directory = tempfile::tempdir().expect("temporary repository");
        let root = directory.path();
        git(root, &["init", "--quiet"]);
        let file = root.join(QUOTA_SOURCE);
        fs::create_dir_all(file.parent().expect("quota source parent")).expect("source tree");
        fs::write(&file, source).expect("quota source");
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
            .args(["-e", "-o", "pipefail", "-c", &step_script(QUOTA_STEP)])
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
    let (success, stderr, output) = source.run(input);
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
    for instant in [
        "2026-10-23",
        "2026-10-23 10:00:00",
        "2026-10-23T10:00:00+02:00",
        "yesterday",
        "",
    ] {
        let source = TaggedSource::new(
            &quota_source(7),
            &format!("release\n\nQuota-lane: {{sha}} 7 {instant}\n"),
        );
        refused(&source, "", "where an RFC 3339 UTC instant belongs");
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
