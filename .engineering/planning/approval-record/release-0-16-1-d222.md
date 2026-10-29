---
format: aep.planning-md/3
id: approval-record:release-0-16-1-d222
kind: approval-record
status: active
title: Finish release 0.16.1 and archive 0.16.0 (D222)
relations:
- decides: task:release-0-16-1
- decides: task:release-0-16-0
revision: 3
transitions:
- {from: "draft", to: "active", at: "2026-10-29T21:07:29Z", actor: "human:timo", revision: 3}
---
# Decision: finish release 0.16.1 and archive 0.16.0

The coordinating session took this decision on 2026-10-29 as its D222, under the operator's standing grant, and relayed it to this repository's session. Release 0.16.1 is the one the operator asked for. In his words: "0.16.0 tagged and published, with the Kubernetes namespace driver; a Namespace-lane record at the tagged commit; the release notes state that exec on a cluster is unverified and clean-room conformance follows. all docs reconsiled and updated according to the new features". 0.16.0 could not publish because of RUSTSEC-2026-0418, so the same scope ships as 0.16.1 (SM-D204).

## What it decides

- `task:release-0-16-1` moves to implemented once its release run succeeds and its artifacts are verified. Its GitHub release notes are edited through the bot so they say that exec on a cluster is unverified and that clean-room conformance follows.
- `task:release-0-16-0` is superseded by `task:release-0-16-1` and archived.
- One pull request records the published digests and reconciles the documentation with 0.16.1 and the namespace driver. It also drops the extra `Cargo.toml` argument at `release.yml:222`, the inferred cause of attempt 1's exit 141.
