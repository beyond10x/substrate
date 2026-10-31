---
format: aep.planning-md/3
id: review-result:wave-2026-10-30-u1-security-pass-1
kind: review-result
status: active
title: Security review pass 1, unit u1 (the clean-room conformance runner)
relations:
- reviews: story:remote-clean-room-conformance
revision: 1
---
# Security review pass 1, unit u1 (the clean-room conformance runner)

Agent type `aep:security-reviewer`. Tree on integration head `81b9e99b` (unit commits `3af50843`,
`855d7730`). Four cases committed in `crates/b10x-substrate-conformance/tests/hostile_endpoint.rs`
(`f05c8642`), run against a misbehaving in-process TLS 1.3 endpoint, all red: a usize::MAX chunk
size panics the runner; a dripping first answer outlasts `--deadline`; an echoed credential lands
in the report as the snapshot and as `driver_version` (the latter schema-valid). The xtask tests and
`cargo xtask check-packages` were not run: the scratch root holding the build-slot script was
emptied at 21:04. Verdict: red.

```findings
- file: crates/b10x-substrate-conformance/src/probe.rs
  line: 529
  category: robustness
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: A chunk size of ffffffffffffffff panics the runner with an add overflow (exit 101, no report) instead of the named harness failure exit 3.
- file: crates/b10x-substrate-conformance/src/lib.rs
  line: 300
  category: robustness
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: The first and last machine reads run outside the run deadline, so an endpoint that drips its answer holds the runner past --deadline.
- file: crates/b10x-substrate-conformance/src/lib.rs
  line: 442
  category: security
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: word() and the schema's word pattern admit a whole bearer credential, so an endpoint-echoed bearer lands in a schema-valid report as driver_version, against design 31 section 5 point 5.
- file: xtask/src/namespace_lane_run.rs
  line: 1166
  category: security
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: The restart hook's kill target is the first daemon's pid, but the lane reaps and restarts a daemon that exits, so a later hook can SIGKILL a reused pid that is not the lane daemon.
- file: .github/workflows/chart-lane.yml
  line: 185
  category: security
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: The clean-room report is uploaded under if always(), so a report the judge refused for holding the run's credential is still published as an artifact.
- file: xtask/src/chart_cluster.rs
  line: 3234
  category: security
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: carries() looks for the full credential only, not its body or the prefix that design 31 section 5 point 4 checks.
- file: crates/b10x-substrate-conformance/src/cases.rs
  line: 1026
  category: robustness
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: The events slice panics when the endpoint returns a shorter page on a later poll.
- file: crates/b10x-substrate-conformance/src/token.rs
  line: 150
  category: robustness
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: After a timeout or an over-long answer the token-fd exchange is not resynchronised, so the next fetch reads a stale answer.
- file: crates/b10x-substrate-conformance/src/cases_exec.rs
  line: 374
  category: robustness
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: Session output keeps growing until the case deadline for an endpoint that streams frames without stopping.
```

Held: TLS 1.3 only, the given roots only, host name checked, no dangerous config, every negative
case through the same verified config; the token command argv-only with env_clear, a 130-byte read
and a 10 s timeout; `--token-fd` close-on-exec; the runner undumpable with core limit zero; stdout
and stderr carry only case ids and closed codes; the workflow has contents read, no
pull_request_target, no secrets, a SHA-pinned upload action; the runner image is FROM scratch.

Coordinator decision: all nine go to the u1 fix unit (the merged security cases must turn green);
the snapshot echo overlaps adversary finding lib.rs:281.
