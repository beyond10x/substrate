---
format: aep.planning-md/3
id: approval-record:d331-merge-202-codeql-preexisting
kind: approval-record
status: active
title: 'D331: merge pull request 202 with CodeQL red on alerts main already holds'
relations:
- decides: story:confinement-adapter-on-the-wire
- decides: story:namespace-driver-admits-an-exec-on-observed-confinement
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-31T16:30:06Z", actor: "human:timo", revision: 2}
---
# D331: merge pull request 202 with a CodeQL check red on alerts main already holds

## Approval

b10x-chef-orchestrator, 2026-10-31, D331, under the grant in the collection's session
instructions: "merge PR 202 through the bot (PUT /pulls/202/merge). Basis: gh pr checks 202
--required shows Full gate and common / Security and privacy pass; rules/branches/main has no
code_scanning rule; your read that the 15 CodeQL alerts are main's existing ones."

## Evidence

- Head `f0c1f0c6`: every check passed except CodeQL, which reported 15 alerts in 3 s.
- 14 alerts are `rust/cleartext-logging` in host test files, each at a path and line where main
  `fa3423d7` holds the same open alert.
- The 15th, `rust/hard-coded-cryptographic-value` at
  `crates/substrate-kubernetes/src/namespace/probe_tests.rs:677`, is the test nonce
  `"leftoverleftover"` from `8980ad44` (2026-10-29), open on main at line 460; the wave's review
  cases moved the line.
- The rulesets on `main` require `common / Security and privacy` and `Full gate`; they hold no code
  scanning rule.

## Outcome

Pull request 202 merged through the App as `1c5985c3`. The alerts stay open on main as before;
neither this wave nor this record changes them.
