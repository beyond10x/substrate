---
format: aep.planning-md/3
id: task:release-0-7-10
kind: task
status: implemented
title: Release Substrate 0.7.10 with bounded live output delivery
relations:
- delivers: story:live-output-stall
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-02T22:51:13Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-02T22:51:13Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-02T23:45:32Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"test_result":3}}}
---
## Outcome
Publish the reviewed live-output-stall correction as Substrate0.7.10, including daemon/MCP OCI artifacts and the unchanged0.17 development contract bundle. Existing source outcome: story:live-output-stall implemented, PR116 merged7d21dbb4. The operator asked to continue shipping the usable integration after the prior release stop; this follows the existing release intent and correction work.

## Scope and verification

Cargo.toml, Cargo.lock, internal exact package-version edges in crates/*/Cargo.toml and xtask/Cargo.toml, CHANGELOG.md, and the canonical THIRD_PARTY_LICENSES.html generated from the release lockfile. The notice correction changes exactly eight workspace package version labels, with no third-party license or dependency change. No runtime behavior or immutable bundle changes. Run the full repository gate, exact-tagged-commit delegated lane, required CI, annotated bare version tag, release workflow, signatures and required artifact verification. Capture absence of quota and resource-counter fixtures honestly. Docs publish asynchronously under the repository completion boundary.

## Delivery
Root owns AEP, bot publication, release and artifact verification. Implementor prepares version/changelog only in the retained managed worktree. Preserve private untracked evidence. No existing worker restart or workspace destruction is authorized by this source release task.

## Release progress

PR117 merged through app/b10x-bot as91278986 after exact65304edf candidate CI passed. Candidate is an ancestor of main and has the same tree. Annotated bot tag0.7.10 points to65304edf and records the successful97-case delegated lane at2026-10-02T23:10:42Z. Release workflow37076712470 is queued; release/artifact publication is not yet claimed. Root independently reviewed version edges, lock, changelog and eight generated notice labels. The plan scope correction is retained after tagging without changing the qualified source identity.

## Publication recovery

Initial release run37076712470 failed at signing the unchanged development bundle: the GitHub Actions OIDC token endpoint timed out, followed by expired_token. No daemon/MCP image build or publication occurred in that run. This is observed in the failed step log, not a source-gate failure. Root dispatched the supported protected-main release recovery for the same immutable0.7.10 tag through the bot API. Publication remains pending; no unsigned artifact is adopted.

## Published result

Substrate0.7.10 was published at2026-10-02T23:39:47Z by successful recovery run37077554387. Source remains65304edf6ebdf4a95f9c2c6138b0c20ea47d157e; required candidate CI37075826048 and exact97-case delegated lane passed. Root independently verified all three release-note digests with cosign3.0.6 and the exact release.yml@refs/heads/main certificate identity plus GitHub Actions issuer. Anonymous Docker configuration avoids unrelated ambient registry credentials. Daemon manifest90469e101c828c7e88fbf1e98c63ec82b9b010e022cbe8432bfc0619c38d1511 links normal daemon layer4812b85baaf0bdc80f08673e21f5daf1e141eaf0646b20f040b1965de99bf281 and source-labelled config65304. Extracted normal daemon binary SHA256 iscdfed912f75676fc2c99370c36699ae88d097b47f92fcb29f5d70fa2379aa707. Independent reviewer Kuhn verified the manifest/config/history/layer/binary chain and distinguished the separate quota daemon layer. Changelog records all published artifact digests. Documentation publication remains asynchronous and is not claimed here.
