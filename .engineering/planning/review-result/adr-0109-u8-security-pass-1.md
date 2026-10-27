---
format: aep.planning-md/3
id: review-result:adr-0109-u8-security-pass-1
kind: review-result
status: active
title: 'Security review pass 1: macos S1 and S2'
relations:
- reviews: story:macos-adapter-on-seatbelt
- reviews: story:macos-seatbelt-policy-runs-cargo-test
revision: 1
---
# Security review, pass 1: macos S1 and S2

Reviews `story:macos-adapter-on-seatbelt` and `story:macos-seatbelt-policy-runs-cargo-test` at
integration `344dd86`. Red cases: `crates/substrate-host/tests/security_review_macos.rs` on branch
`review/adr-0109-macos-security` (`b0d4f44`). None executed in this pass. Three macos-15 runs
(40449843611, 40450039916, 40450472418) stopped at the "Test the host crate" step, in earlier binaries.

## Findings

| # | file:line | severity | verdict | finding |
|---|---|---|---|---|
| F1 | `crates/substrate-host/src/confinement/macos.rs:407` | blocker | PLAUSIBLE | A relative exec subpath gets a lexical check, then `workspace.join`. `resolve_ancestor` follows a symlink out of the workspace, and the profile grants read, exec and map-executable on the target. |
| F2 | `crates/substrate-host/src/confinement/macos.rs:591` | warning | PLAUSIBLE | `drop(private)` ignores removal errors. A directory the run leaves without write or search permission outlives `wait`. |
| F3 | `crates/substrate-host/src/confinement/macos.rs:136` | warning | PLAUSIBLE | `ipc_signals_contained` is declared without a probe at open. No case shows that one run cannot signal another. |
| F4 | `crates/substrate-host/src/seatbelt.rs:226` | note | CONFIRMED | The unscoped `(allow sysctl-read)` is absent from ADR 0037's gap table. |
| F5 | `.github/workflows/macos.yml:55` | note | CONFIRMED | The host test step has no `--no-fail-fast`, so one failing binary hides every later one, the macOS lanes included. |

Checked, nothing found:
- `sandbox-exec` is called by fixed path and argv only.
- SBPL literals refuse quote, backslash, control and non-UTF-8 characters.
- The policy is `(deny default)` with no network rule.
- A build fact the probe did not show is absent.
- Refused starts are audited.
- The environment is cleared and shaped.
- `macos.yml` pins actions by SHA and is read-only.
- The pinned counts match.

## Decisions taken by the coordinator

- F1: refuse a relative exec subpath with a symlink component, or check its resolved path against the declared locations; the implementor names which.
- F2: make the TMPDIR tree owner-writable before removal. If removal still fails, audit it and warn.
- F3: keep the case. If it is red, the fact is absent unless a probe at open proves it.
- F4: add the rule to ADR 0037's gap table; the policy stays.
- F5: add `--no-fail-fast` to the host test step.
- All five go to the S2 fix unit on `fix/adr-0109-macos-s2`, proven by one macos-15 run.
