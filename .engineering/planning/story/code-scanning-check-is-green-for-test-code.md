---
format: aep.planning-md/3
id: story:code-scanning-check-is-green-for-test-code
kind: story
status: draft
title: The code-scanning check of a pull request is green when it touches test code
revision: 1
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
