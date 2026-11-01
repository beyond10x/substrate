---
format: aep.planning-md/3
id: approval-record:d355-u1-clarification-inside-adr-0047
kind: approval-record
status: approved
title: 'D355: U1 rests on a clarification inside ADR 0047 decisions 3 and 11'
relations:
- decides: story:namespace-no-egress-observed-on-every-node
- informed_by: approval-record:d354-substrate-wave-2026-11-01
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-01T15:18:48Z", actor: "human:timo", revision: 2}
---
# D355: U1 rests on a clarification inside ADR 0047, decided by the orchestrator

## Decision

b10x-chef-orchestrator, 2026-11-01, D355: "A. U1 is a clarification inside ADR 0047 decisions 3
and 11, with no design 34 edit (f6d02148 on main, D339). Record it in your store; U1 may merge
after its review."

## The fork it settled

`story:namespace-no-egress-observed-on-every-node` (wave D354 unit U1) implements design 34's
amendment of 2026-11-01 (`docs/design/34-kubernetes-namespace-driver.md` § 6.4 and *Amendments of
2026-11-01*, landed in `f6d02148` under D339): the probe's keeper node is bound to the snapshot the
driver publishes, and an exec start whose workspace keeper is on another node is refused
`exec.sandbox-unavailable` (501) before the Pod create.

Options put to the orchestrator: A) a clarification inside ADR 0047 decisions 3 and 11, the
orchestrator decides; B) host-monitor approves on the operator's behalf, U1 held unmerged; C) U1
leaves the wave. Recommended and taken: A.

Grounds (cited): ADR 0047 decision 11 makes facts observations of disposable resources
(`adr/0047-*.md`, decision 11) and lists "exec while the probe proved no confinement" as
`exec.sandbox-unavailable` (501), an existing refusal; decision 3 grants the driver no `nodes`
access, which is why probing every node was not chosen; exec Pods were already pinned to the
keeper's node. No wire field, no new refusal code and no Role verb is added, and U1 edits no design.
