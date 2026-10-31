---
format: aep.planning-md/3
id: approval-record:d334-only-a-local-denial-proves-blocked
kind: approval-record
status: active
title: 'D334: only a local denial proves an egress attempt blocked'
relations:
- decides: story:namespace-probe-never-proves-no-egress-from-a-timeout
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-31T18:52:24Z", actor: "human:timo", revision: 2}
---
# D334: only a local denial proves an egress attempt blocked

## Approval

b10x-chef-orchestrator, 2026-10-31, D334, on finding 5 of
`review-result:wave-2026-10-31-d333-adversary-pass-1`: "narrow "blocked" to EPERM/EACCES only in
this wave … Basis: D333 says fail closed on any ambiguity, and your adversary shows
ENETUNREACH/EHOSTUNREACH do not prove a stop inside the Pod (remote ICMP, wrong family). Reading
errno with elapsed time to tell local from remote goes into story B." Failing test first
(ENETUNREACH and EHOSTUNREACH are not blocked); the docs and the CHANGELOG name EPERM/EACCES.

## Consequence

`EgressOutcome::blocked` is `Denied` only (`EPERM`, `EACCES`). `Unreachable` (`ENETUNREACH`,
`EHOSTUNREACH`) can come from a remote router's ICMP or an address of the wrong family, so it
proves nothing, as a timeout proves nothing. A namespace driver admits an exec only where the
network plugin denies each of the probe's three attempts with `EPERM` or `EACCES`. Telling a local
no-route from a remote one by errno and elapsed time belongs to
`story:namespace-probe-repeats-and-sticks-on-egress`.
