---
format: aep.planning-md/3
id: approval-record:d332-no-egress-fix-wave
kind: approval-record
status: active
title: 'D332: a fix wave for confinement.no-egress published on a cluster whose egress was open'
relations:
- decides: story:namespace-probe-never-proves-no-egress-from-a-timeout
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-31T18:33:18Z", actor: "human:timo", revision: 2}
---
# D332: a fix wave for confinement.no-egress published on a cluster whose egress was open

## Approval

b10x-chef-orchestrator, 2026-10-31, D332, under the grant in the collection's session
instructions, after the 0.18.0 namespace lane at `3b3998c8` gave no record (S3: "the snapshot
publishes confinement.no-egress, which no probe here observes"). Ordered steps: one traced
namespace lane run that logs the daemon probe's three attempt outcomes, so the cause is settled by
a runtime observation; a fix with a failing test first that withholds the fact on any ambiguity;
an adversary pass; a pull request through the bot, merge on green, the four lanes at the new main
commit, then the tag. Stop rule: the same failure twice on one cause.

## Runtime observations (traced lane runs at `3b3998c8`, kind, 2026-10-31)

- Trace 1, lane step S3 daemon: Kubernetes Service `10.96.0.1:443` reached in 0 ms, nameserver
  `10.96.0.10:53` reached in 21 ms, egress probe address `172.24.0.1:9` refused (errno 111) in
  0 ms; `no_egress` false; S3 passed.
- Trace 2, lane step S3 daemon: all three attempts timed out (3001 ms, 3058 ms with errno 11,
  3001 ms); `no_egress` true; S3 failed.
- In both runs the lane's own O3 Pod, under the same default-deny policy, reached all three
  targets, so egress was observed open.
- Trace 1's S5 daemon (runtime vectors, same cluster) also published `confinement.no-egress`.

Verified: three timed-out attempts make the probe state `confinement.no-egress` on a cluster
whose egress was observed open, and they occur in some runs and not others. Inferred, not
verified: kind's network plugin enforces NetworkPolicy only after a Pod's first moments.
