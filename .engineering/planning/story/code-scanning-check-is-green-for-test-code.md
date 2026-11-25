---
format: aep.planning-md/3
id: story:code-scanning-check-is-green-for-test-code
kind: story
status: draft
title: The code-scanning check of a pull request is green when it touches test code
revision: 2
---
# Story: the code-scanning check of a pull request is green when it touches test code

## Outcome

The `CodeQL` check of a pull request does not fail for an alert in test code. On 2026-11-24
`main` held 11 open alerts, all of severity critical and all in test code, and the check of pull
request 295 failed twice for three of them (27, 32, 33) because the pull request changed lines
beside them. The check is not required by a ruleset, so it blocks no merge, but a wait on
"every check green" stops on it and a new alert is not seen among the old ones.

## Where the requirement comes from

The operator's session, 2026-11-24 16:39 CET, about pull request 295: "Alerts 27, 32, 33 are not
dismissed by anyone; file a story to fix or annotate them in the test code so the check goes
green". Stakeholder: the operator.

## What is there

Open on `main` at `db15951fb`, read from the code-scanning API:

| alert | rule | where |
|---|---|---|
| 22 | `rust/hard-coded-cryptographic-value` | `crates/substrate-kubernetes/src/namespace/probe_tests.rs` |
| 23 to 26 | `rust/hard-coded-cryptographic-value` | `crates/substrate-store/src/backend/tests.rs` |
| 27 | `rust/hard-coded-cryptographic-value` | `crates/substrate-daemon/tests/backend_ledger.rs` |
| 29 | `rust/unsafe-deserialization` | `crates/substrate-daemon/tests/processes/adversary_pass1.rs` |
| 30 | `rust/unsafe-deserialization` | `crates/substrate-daemon/tests/processes/events.rs` |
| 31 | `rust/unsafe-deserialization` | `crates/substrate-daemon/tests/processes/security_pass1.rs` |
| 32, 33 | `rust/unsafe-deserialization` | `crates/substrate-daemon/tests/pipe_session.rs` |

Not read yet: each alert's data flow, and whether any of the eleven is a finding about what the
test does and not about a fixed test value.

## What changes

1. Read each of the eleven alerts' flow. One that names a real defect of a test (a value that
   should not be fixed, input that should be checked) is fixed in the test.
2. For the others, the test code is changed or annotated so that the analysis no longer reports
   them: the form the analysis documents for test code, decided after step 1 (a query filter
   for test paths in the workflow's configuration, or a change of the flagged expression). No
   alert is dismissed in the repository's settings, by anyone.
3. A filter, if one is chosen, covers test code only: paths under `tests/`, `src/**/tests.rs`,
   `*_tests.rs`, and nothing a shipped binary compiles.

## Acceptance

`main` holds no open code-scanning alert in test code, and the `CodeQL` check of a pull request
that changes a line beside one of today's eleven places is green. The list of what a filter
excludes is in the repository, and a test or check holds that it names no path of shipped code.

## Not in this story

An alert in shipped code: it is a finding and goes the way `SECURITY.md` says.

## The owner's decision and the wiring (2026-11-25)

The repository owner's pick (2026-11-25): "A: scanner skips test paths". The option's text: a pull request adds a CodeQL configuration that
ignores test files (`crates/*/tests/**`, `**/tests.rs`); test code is no longer scanned, shipped
code is scanned as before; the open test alerts close by themselves. No alert is dismissed.

What the pull request holds:

- `.github/codeql/codeql-config.yml`: `paths-ignore` with those two patterns, and nothing else.
- Three cases in `xtask/tests/planning_only_classification.rs`: the file is those two patterns;
  a `tests` module counts as test code only under `#[cfg(test)]`, both sides; no matched path is
  compiled into a shipped binary.

What is not a pull request. The repository runs CodeQL's default setup (read 2026-11-25 from
`GET /repos/beyond10x/substrate/code-scanning/default-setup`: `configured`; languages actions,
javascript-typescript, python, rust). Default setup reads a configuration file only through an
organization repository property, which an organization owner creates and sets (organization
Settings, Repository, Custom properties). On that day the organization had no custom property.

| | |
|---|---|
| property name | `github-codeql-config-file` |
| type | Text |
| value for `substrate` | `remote=beyond10x/substrate@main:.github/codeql/codeql-config.yml` |

The value names the file on `main` (the repository owner's pick, 2026-11-25), so a pull
request cannot change what its own scan skips, and a branch that does not hold the file is
analysed the same. Order: the pull request is merged first, then the property is set; it applies
at the next analysis. Until the property is set the file changes nothing.

Alerts by pattern, read from the API that day: 27 and 29 to 33 match `crates/*/tests/**`; 23 to
26 match `**/tests.rs`. Alert 32's place was removed by pull request 300. Alert 22
(`crates/substrate-kubernetes/src/namespace/probe_tests.rs`) matches neither pattern and stays
open: the acceptance's first sentence is not met for it, and `*_tests.rs` of step 3 is not part
of the pick.
