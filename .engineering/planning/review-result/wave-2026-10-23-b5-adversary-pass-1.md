---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-b5-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit b5 (directory listings name omitted entries)
relations:
- reviews: story:directory-listing-names-omitted-entries
revision: 1
---
# Adversary pass 1, unit b5 (directory listings name omitted entries)

Agent type `aep:adversary`. Unit tree on base `32a012c`, uncommitted. Five wire cases written in `crates/substrate-daemon/tests/omitted_entries_adversary_pass1.rs` (not run in the pass: disk was under the 25G floor). Verdict: NEEDS-CHANGE, one finding from reading the code.

```findings
- file: crates/b10x-substrate-sdk/src/lib.rs
  line: 966
  category: contract-drift
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: Workspace::tree and read_directory drop x-b10x-omitted-entries, so once truncated no longer reports an omission an SDK caller (workspace/crates/workspace-service/src/main.rs:2203) sees a tree with a FIFO in it as complete, with no signal it can read.
```

Held: the bundle SHA pin equals sha256 of 0.23.0 bundle.json; frozen bundles 0.1.0 to 0.22.0 unchanged; the count saturates and each scan is capped by MAX_DIRECTORY_SCAN_ITEMS; no header is sent when the count is 0; adopted workspaces and declared roots list through the same path. Device nodes are infeasible to create without privilege.

Coordinator decision: breaking a consumer is out of scope. truncated keeps its 0.10.0 meaning (true on an omission as well as the limit); the header is the distinct signal; ADR 0043 amended first. The SDK adds tree_with_omissions and read_directory_with_omissions and keeps the existing signatures. The adversary's five cases are kept; three expectations changed to follow that decision, and one case had its own expected tree item missing "size": null.
