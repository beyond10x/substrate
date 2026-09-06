---
format: aep.planning-md/1
id: review-result:container-release-preparation-review
kind: review-result
status: active
title: Container runtime release preparation review
relations:
- reviews: story:container-private-exec-bootstrap
revision: 1
---
approve

Independent read-only review of the bounded Substrate 0.7.6 release preparation against `1ceb11bbaa38221ecf3e48a62d41271d9ae57d0f`. No actionable findings. Earlier runtime, profile, Tini and evidence reviews remain immutable. The currently inspected runtime, Dockerfile, profile, checker and design files match their latest approved SHA-256 identities; this follow-up adds no runtime behavior or authority change.

`Cargo.toml`, the six changed internal package manifests and Cargo.lock advance only 0.7.5 to 0.7.6. Reversing that string in memory reproduces each base file exactly. The lock advances eight local workspace packages; external dependency versions, checksums, feature declarations and dependency edges are unchanged. Exact internal path dependency version constraints remain aligned with the workspace version.

`THIRD_PARTY_LICENSES.html:6015` contains exactly eight local-package version substitutions. Reversing those substitutions reproduces the base HTML exactly, with no removed license text, source links or third-party attribution. The retained full gate reports that the generated notice matches the locked graph. The inspected `xtask/src/licenses.rs` performs that check with exact cargo-about 0.9.1, locked workspace generation, failure on unresolved notices and canonical LF comparison. This is an inspected generated-artifact consistency check; no legal compliance opinion is inferred from it.

The README container section is byte-identical to the previously reviewed documentation. It states the opt-in bootstrap prerequisites, permanent non-root transition, separate node-profile installer, confined worker expectations and the distinction between portable CI and actual ordinary/quota container execution. No deployment-specific identifier, credential or unsupported publication claim was added.

`xtask/tests/release_workflow.rs:173` now isolates the MCP stage at the next FROM instruction, avoiding accidental inclusion of the new local checker stage. It still requires the distroless MCP zlib copy, fixed stdio entrypoint, absence of exposed ports or volumes, and release-workflow testing of the exact extracted binary plus runtime image. The daemon check instead verifies that its actual inherited execution-prerequisites stage includes the zlib1g package. This correctly replaces the obsolete assertion that both runtime targets must copy the same builder library. The existing image smoke path is retained; the textual Docker assertions supplement rather than replace executable startup evidence. Release signing, verification, write-once tags and announcement ordering are unchanged by this test adjustment.

Inspected retained validation evidence:

- `substrate-full-gate-release.log` ends with `gate: passed`. Its 48 test-result blocks total 588 passed, zero failed and eight ignored. The adjusted MCP packaging test passes. Release-mode workspace tests and Clippy, formatting, repository checks, advisories, license/package checks and all contract-bundle checks complete successfully.
- `substrate-delegated-lane.log` records the real delegated lane, including the clean-room runtime/stdio journeys, one-use channel-bound hosted attachment checks and adversarial aperture-ceiling cases, with successful test results. This supplies separately exercised delegated behavior rather than treating the portable gate as confinement proof.
- `substrate-image-startup.log` reports four passed and zero failed: protected byte-identical default/quota executables with the expected file capabilities; ordinary startup with an empty bounding set; ordinary startup with only a SYS_ADMIN bounding allowance but no active capabilities; and explicit quota startup with only SYS_ADMIN active. The observed daemon threads run as UID/GID 65532 with empty inheritable/ambient capabilities.

These image-startup checks exercise the existing ordinary/quota entrypoints. The earlier retained Tini PTY evidence separately covers the hosted bootstrap and descendant cleanup. The startup log selects a local image tag; it is not an immutable published source/image binding. This review did not rebuild, rerun gates, mutate live services or access credentials. Protected source CI and immutable release publication remain subsequent steps; deployed terminal acceptance and model credentials are outside this release-preparation verdict.

Reviewed release-preparation file SHA-256 identities:

```text
da1047246db8cd3c68db46e80f7f22cb2157bbddd6f590a6324e8da847ce4bc6  Cargo.toml
3ca33edc10628dde45d4a129d1ccbddcc2a38d37935a009fb9ad8c24a535b506  Cargo.lock
909f9e2a0677b2d87a5410cd0dcefa6c99cc0dc680fb4e181fbfa27f3131f813  crates/b10x-substrate-sdk/Cargo.toml
34c25fd12b7c37eb974a6f5a6835fff6633dd538074dee3c5db0d4a671694785  crates/substrate-daemon/Cargo.toml
ee902fc31000f359445a6c4b493a1ed2aba6b6df446f111dd3c8d07e7d5d06f7  crates/substrate-host/Cargo.toml
99b5cdeee81dec8744ce33db023b94cf446f561ddb10f6c3ac1186b7ad43b086  crates/substrate-mcp/Cargo.toml
b25be0b5db4162baba18651eecb08be323b5221186856725b5c4424f0bd0657d  crates/substrate-store/Cargo.toml
716ab16afb2b22c240b67bb07684bf056e8488b57e876f1c43d2e6ae208742d7  xtask/Cargo.toml
d77dcacf1b801e69a3735768952e55d330e6b26aafd809c9146c8e0fdb1b7445  THIRD_PARTY_LICENSES.html
5a2a939efc81c5290af535b06bd5bcf00baa36119115769aea62a386df9b7676  README.md
e1959a5e945c20f500de9af28b4fd4a02b153a901ec2846bc6424a469cef91f0  xtask/tests/release_workflow.rs
```

The full-index binary Git diff for those eleven paths against the stated base hashes to `5259952b4f2fa5f41a6edfc0181768c359d0a3f5695d80c6d30482d3be325124`. It intentionally excludes previously reviewed runtime additions and planning records.

Retained evidence SHA-256 identities:

```text
a1110df7739fb0bb2015a39938332a6b81d64673403b896f06fa019d85bac13c  substrate-full-gate-release.log
e6b9c9bd0bd73afc88484fa2ea5d5ce3b921033fa2944db03cee6f681f0c37c6  substrate-delegated-lane.log
5f2d91478e7bcb490b2312d604978ba8896ea671b2e45f99b39089f121a3723a  substrate-image-startup.log
```

```findings
[]
```
