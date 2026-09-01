//! Offline release-workflow assertions for the contract-bundle publication boundary.
//!
//! The live registry and Sigstore transparency log are necessarily release-time evidence. These
//! tests keep the repository-controlled half fail-closed: the current bundle is explicit, its
//! deterministic OCI layout is the thing copied, canonical tags are checked before publication,
//! both artifacts are signed and verified by digest before the GitHub release is announced, and
//! protected `main` is never pushed around. A byte-identical write-once bundle may be reused by a
//! later daemon release, while a digest mismatch is always refused.

use std::fs;
use std::path::{Path, PathBuf};

const WORKFLOW: &str = include_str!("../../.github/workflows/release.yml");

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
        current, "0.12.0",
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
}

#[test]
fn canonical_tags_are_never_overwritten() {
    let refusal = position("Refuse an existing release or daemon-image version tag");
    let daemon_push = position("docker push \"${IMAGE}:${VERSION}\"");
    let bundle_push = position("oras cp --from-oci-layout");
    assert!(refusal < daemon_push);
    assert!(refusal < bundle_push);
    assert!(occurrences("oras resolve \"${BUNDLE_IMAGE}:${BUNDLE_VERSION}\"") >= 3);
    position("reusing byte-identical ${BUNDLE_IMAGE}:${BUNDLE_VERSION}");
    position("not deterministic package ${PACKAGE_DIGEST}; it will not be overwritten");
    position("appeared at ${remote_digest}, not ${PACKAGE_DIGEST}; it will not be overwritten");
}

#[test]
fn both_artifacts_are_signed_and_verified_before_announcement() {
    let daemon_sign = position("cosign sign --yes \"${IMAGE}@${DIGEST}\"");
    let daemon_verify = position("\"${IMAGE}@${DIGEST}\" > cosign-verify.json");
    let bundle_sign = position("cosign sign --yes \"${BUNDLE_IMAGE}@${BUNDLE_DIGEST}\"");
    let bundle_verify =
        position("\"${BUNDLE_IMAGE}@${BUNDLE_DIGEST}\" > bundle-cosign-verify.json");
    let announce = position("gh release create \"${VERSION}\"");
    assert!(daemon_sign < daemon_verify && daemon_verify < announce);
    assert!(bundle_sign < bundle_verify && bundle_verify < announce);
    assert!(occurrences("https://token.actions.githubusercontent.com") >= 2);
    assert!(occurrences(".github/workflows/release.yml@${GITHUB_REF}") >= 2);
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
    for line in WORKFLOW.lines().map(str::trim) {
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
