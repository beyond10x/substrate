---
format: aep.planning-md/3
id: review-result:wave-2026-10-31-design-30-security-pass-1
kind: review-result
status: active
title: 'Wave 2026-10-31 (design 30): security review pass 1'
relations:
- reviews: story:namespace-driver-admits-an-exec-on-observed-confinement
revision: 1
---
# Security review, wave 2026-10-31 (design 30), pass 1

Reviewer: aep:security-reviewer, tree `substrate-w1031b-sec`, range `11f002ed...11042902`; cases
committed as `e05c9d72` on `review/w1031b-sec`.

```
unit: substrate wave 2026-10-31 at 11042902, review commit e05c9d72 on review/w1031b-sec (tree substrate-w1031b-sec)
verdict: red
cases: executed 117 (113 existing + 4 new), red 3, green 1
origin: introduced 6 / pre-existing 0 / undecided 1
wrote-outside-worktree: scratch logs under the wave's scratch directory
needs-coordinator: S1/S2 need a design decision (when must no-egress be withheld?); S3 overlaps the adversary's claim 2; J1 needs a ruling on design 30 D5 vs design 34 § 4
```

Suite (`cargo test -p b10x-substrate-kubernetes`, exit 101):
`test result: FAILED. 114 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.43s`

```findings
[
{"file":"crates/substrate-kubernetes/src/namespace/probe.rs","line":810,"category":"property","severity":"blocker","verdict":"CONFIRMED","origin":"introduced","message":"S1: no_egress() publishes confinement.no-egress from three attempts made by the probe Pod, which lacks the b10x-substrate/exec label. An additive NetworkPolicy that matches that label (allowed under design 34 § 14 Q1 B) opens egress for every exec Pod. The binding generation moves, the re-probe still sees blocked, and the fact is published again (probe.rs:370). Exec Pods are then admitted on the neutral set while they can reach the network. Invariant 3: an optimistic fact. Test: crates/substrate-kubernetes/src/namespace/probe_tests.rs:1373 a_policy_selecting_exec_pods_but_not_the_probe_pod_withholds_no_egress"},
{"file":"crates/substrate-kubernetes/src/namespace/probe.rs","line":810,"category":"property","severity":"blocker","verdict":"CONFIRMED","origin":"introduced","message":"S2: Same mechanism through a bound policy that selects every Pod and allows egress to a range the probe never dials (203.0.113.0/24). The driver holds a policy that allows egress and still publishes no-egress. Test: crates/substrate-kubernetes/src/namespace/probe_tests.rs:1415 a_bound_policy_allowing_egress_to_a_range_the_probe_does_not_dial_withholds_no_egress"},
{"file":"crates/substrate-kubernetes/src/exec.rs","line":1492,"category":"contract-drift","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"S3: The namespace driver's applied record sets requires: None whatever the start carried, which breaks ADR 0048 D4. U3's neutral admission makes this record reach the client wire. Test: crates/substrate-kubernetes/src/exec_tests.rs:1853 security_the_applied_record_echoes_the_requires_the_start_carried"},
{"file":"crates/substrate-kubernetes/src/exec.rs","line":1494,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"undecided","message":"J1: cgroup is set to the Pod name sbx-ex-<hash>, but the namespace driver opens no cgroup adapter. Design 30 D5 says no value is invented; design 34 § 4 implies the Pod name. Not fixable here without a design ruling."},
{"file":"crates/substrate-executor/src/probe.rs","line":212,"category":"judgement","severity":"warning","verdict":"INFEASIBLE","origin":"introduced","message":"J2: TimedOut and Unreachable count as blocked. A workload that already has egress could saturate conntrack or bandwidth, or a remote ICMP host-unreachable could arrive, and no-egress would read as proved. I could not show anybody reaches this state."},
{"file":"crates/substrate-kubernetes/src/namespace/probe.rs","line":496,"category":"judgement","severity":"warning","verdict":"INFEASIBLE","origin":"introduced","message":"J3: The probe observes NetworkPolicy enforcement on the one node its Pod landed on, but publishes no-egress for exec Pods on every node. I could not show a cluster that enforces policy on some nodes and not others."},
{"file":"crates/substrate-host/src/probe.rs","line":298,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"J4: confinement.family.audit is set to Some(true) unconditionally, even when no confinement floor opened: declared, not proved. Whether audit needs a proof is a design call, not fixable here."}
]
```

Cleared with nothing found: forgery through the probe's writable volume or
`SUBSTRATE_PROBE_TARGETS` (fresh volume per run, a client's environment reaches only its own exec,
no exec Pod holds a service-account token, the heartbeat changes on every beat); the argv check
before the observation; closed targets and report format; facts lapse with the Lease epoch, the
binding generation and `valid_until`; the four Linux facts never published; admission needs all
six facts; `substrate_adapter_info` only on the operational listener; `driver_version` names no
adapter; the snapshot binds adapter and fact set; the portable lane publishes only workspace and
audit; `network-ingress` never published; a client-supplied `requires` only refuses or passes.
