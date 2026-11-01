---
format: aep.planning-md/3
id: approval-record:child-process-entry-point-ignore-reason
kind: approval-record
status: approved
title: 'A test helper run in its own process carries the ignore reason ''child process entry point: run by <case>'''
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-01T23:08:45Z", actor: "human:timo", revision: 2}
---
# A test helper run in its own process carries one ignore reason

## Decision

A test function that is not a check of its own, but the entry point a case re-executes the test
binary into (`--exact --ignored`, gated by an environment variable), carries exactly

```rust
#[ignore = "child process entry point: run by <case>"]
```

where `<case>` is the name of the test that runs it. The release verification exempts only that
phrase from its rule that a release diff may not add `#[ignore`; any other wording is flagged.

## Source

b10x-chef-orchestrator wave rules, 2026-11-01: "A test helper run in its own process carries
`#[ignore = \"child process entry point: run by <case>\"]`; verify-release exempts only that phrase
(2026-11-01, substrate 0.19.0 egress.rs:3644/:4029 used other wording). Substrate next wave: record
this convention in its store as a store-only commit." Item D364, 2026-11-02: "A store-only commit
rides along. It records the ignore-reason convention."

## Where it applies today

Read from `git grep -n '#\[ignore' -- '*.rs'` at `95ee993d`: five helpers predate the phrase and
use other wording, `crates/substrate-host/src/egress.rs:3505`, `:3644`, `:4029`,
`crates/substrate-daemon/tests/processes.rs:2253` and `crates/substrate-host/src/host_process/tests.rs:887`.
They are not renamed by this record; a unit that edits one of them, or adds a helper, uses the
phrase. Fixture-gated cases (`real_quota_*`, the Connectors TLS proxy fixture, the Gitleaks
download) are not helpers and keep their own reasons.
