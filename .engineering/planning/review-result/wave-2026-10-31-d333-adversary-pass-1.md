---
format: aep.planning-md/3
id: review-result:wave-2026-10-31-d333-adversary-pass-1
kind: review-result
status: active
title: 'Wave 2026-10-31 D333 (no-egress fail closed): adversary pass 1'
relations:
- reviews: story:namespace-probe-never-proves-no-egress-from-a-timeout
revision: 1
---
# Adversary review, wave 2026-10-31 D333 (no-egress fail closed), pass 1

Reviewer: aep:adversary, tree `substrate-d333-adv`, range `3b3998c8...46d56a38`; its one case is
carried onto the wave branch uncommitted by the reviewer and committed with the fixes.

```
unit: substrate wave 2026-10-31 D333, range 3b3998c8...46d56a38 at 46d56a38
verdict: CONFIRMED (one red case); no path found where a timeout, reach, refusal, read failure or unread target yields confinement.no-egress
cases: executed 131 to 132 (kubernetes 122 to 123, executor 9), red 1
origin: introduced 4 / pre-existing 2 / undecided 0
needs-coordinator: broadcast/multicast refusal in this wave or as a story; the counted set (finding 5) is the operator's call
```

Suite: `cargo test -p b10x-substrate-kubernetes --lib`: `test result: FAILED. 122 passed; 1 failed`
(`driver_tests::adversary_d333_a_broadcast_or_multicast_egress_probe_address_is_refused`), exit 101.
`cargo test -p b10x-substrate-executor --lib`: `ok. 9 passed`.

Attacked and not broken: `blocked()` has one caller; both attempt kinds report a timeout as
`TimedOut`; DNS on a connected UDP socket ends `TimedOut` on a non-fatal ICMP; fewer, more,
reordered, duplicate or misaddressed attempts, `Unread`, `Failed` and a join panic all withhold
the fact; the bound-policy step only clears it.

```findings
[
{"file":"crates/substrate-kubernetes/src/driver_tests.rs","line":1348,"category":"test","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"The extension admits a broadcast or multicast egress_probe_address (config.rs:194 refuses only loopback, unspecified and port 0), and the kernel answers a TCP connect there ENETUNREACH, so the third attempt counts as Unreachable without any policy; red case adversary_d333_a_broadcast_or_multicast_egress_probe_address_is_refused."},
{"file":"crates/substrate-kubernetes/src/namespace/probe.rs","line":830,"category":"judgement","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The doc comment of no_egress() still says each attempt 'failed with nothing answering', the old rule that counted timeouts."},
{"file":"docs/design/34-kubernetes-namespace-driver.md","line":257,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The § 6.2 probe table still says each attempt 'must fail within 3 s', contradicting the 2026-10-31 amendment."},
{"file":"crates/substrate-kubernetes/src/namespace/probe_tests.rs","line":1330,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"Existing test docs still describe the blocked fixture as attempts that 'failed with nothing answering', though it is now Denied/Denied/Unreachable."},
{"file":"crates/substrate-executor/src/probe.rs","line":443,"category":"judgement","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"classify() has no test producing a raw ETIMEDOUT, ENETUNREACH, EHOSTUNREACH, EPERM or EACCES, so a mutant moving ETIMEDOUT into the Unreachable or Denied arm stays green and lets a timeout publish no-egress again."},
{"file":"crates/substrate-executor/src/probe.rs","line":216,"category":"judgement","severity":"warning","verdict":"INFEASIBLE","origin":"pre-existing","message":"Unreachable also results from remote ICMP (host/net unreachable, code 13 admin-prohibited), a neighbour failure at about 3 s and a wrong-family address, and no_egress() ignores errno and elapsed_ms, so 'stopped inside the Pod' overstates what the operator-chosen set proves; inferred from kernel code, not observed."},
{"file":"crates/substrate-kubernetes/src/namespace/probe_tests.rs","line":1484,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"traced() sets errno and elapsed_ms that no_egress() never reads, so the D332 replay tests would pass identically without the traced values."}
]
```
