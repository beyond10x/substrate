---
format: aep.planning-md/3
id: approval-record:d353-substrate-0181-release-recovery
kind: approval-record
status: approved
title: 'D353: the 0.18.1 release is recovered by the repository''s dispatch'
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-01T14:28:18Z", actor: "human:timo", revision: 2}
---
# D353: the 0.18.1 release is recovered by the repository's dispatch

## Approval

b10x-chef-orchestrator, 2026-11-01, D353 (class C, under the grant): "dispatch (D353, the
repository's own release recovery path, release.yml:976-1005). Record it in your store. 1. Before
the POST, confirm that main's release.yml builds the 0.18.1 tag's commit bf0e0443 and not main's
head. [...] 2. Then POST the dispatch through the App: ref main, inputs {"version":"0.18.1"}."

## What happened

- The tag 0.18.1 (tag object `32eb5704`, commit `bf0e0443`, four lane trailers) started Release
  run 41313523262. Every test step passed; the job that publishes pushed
  `b10x-substrate-executor:0.18.1` (`sha256:45b1b072dfb3993c81c493dcf48cd406cc6fa07591b0436c8bfbed723b610793`)
  and `b10x-substrate-daemon:0.18.1`
  (`sha256:52260497de95ac7e3b363d520e2b241c0f0794f16f984da1ef6fb6162be3b3ab`), then failed at
  14:19:22Z pushing `b10x-substrate-mcp:0.18.1` with `unknown blob` from the registry.
- A re-run keeps the push event, and the step "Resolve write-once image version tags" refuses an
  existing `${IMAGE}:${VERSION}` on anything but `workflow_dispatch`, so a re-run was not used.
- Checked before the dispatch: `origin/main` is `bf0e0443`, its `release.yml` equals the tag's,
  and the workflow checks out the tag's peeled commit (`source-sha`) and requires a dispatch from
  `refs/heads/main`.
- Dispatched through the App 2026-11-01 14:27Z: ref `main`, inputs `{"version":"0.18.1"}`; run
  41316297168. The lane inputs are empty, so the tag's trailers carry the four lane records.
