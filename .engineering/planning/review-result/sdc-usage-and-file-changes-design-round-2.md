---
format: aep.planning-md/3
id: review-result:sdc-usage-and-file-changes-design-round-2
kind: review-result
status: active
title: 'Plan critic (design), round 2: usage on the observation and the file-change record'
tags:
- plan-critic
- sdc
relations:
- reviews: story:run-observation-carries-usage
- reviews: story:exec-file-changes-are-recorded
- reviews: story:exec-file-change-record-on-the-wire
revision: 1
---
needs-revision

story:run-observation-carries-usage — item 2's one carrier is the host driver's read in `process.rs` before `reconcile_cgroup`, but the acceptance requires `systemd` around `local`, whose runs never go through that read, and no item names the read it needs — .engineering/planning/story/run-observation-carries-usage.md:85, .engineering/planning/story/run-observation-carries-usage.md:109, crates/substrate-host/src/confinement/systemd.rs:11
story:run-observation-carries-usage — the code cited for `systemd` (`cpu_usage_micros`, `systemd.rs:836-842`) is a private point sample of `cpu.stat` used for the budget poll, and `systemd.rs` reads no `memory.peak`, `pids.peak` or wall time, so the cited code supports one of the four figures — crates/substrate-host/src/confinement/systemd.rs:836, crates/substrate-host/src/confinement/systemd.rs:1167
story:exec-file-changes-are-recorded — the two stories both keep the record's content and neither says which one reads from which. This story's item 3 has the host driver digest the mounts and the adapter expose the record, while the wire story's item 3 has the daemon "store what the ADR says is kept". The daemon reads `ExecObservation` directly and has no adapter API. Item 3 should say the host driver keeps record and content for the ADR's retention and exposes them to the daemon, and the wire story should say the daemon reads from there — .engineering/planning/story/exec-file-changes-are-recorded.md:124, .engineering/planning/story/exec-file-change-record-on-the-wire.md:84, crates/substrate-daemon/src/app/operations.rs:11

What I walked and found fine:
- **Graph:** no cycle. The chain is process-count → usage → file-changes → exec-start-carries-executable-lists → exec-observation-reports-process-count-hits → file-change-wire → confinement-profile-ingress-routes. Every link has its shared files written beside it in § Order, so I do not report it as a serialising chain.
- **Acceptance reads:** the three stories' reads that land on a producer are all recorded. Usage reads the limit and the `run_ended` record, which `process-count-limit-hit-is-observed` carries transitively. The wire story reads the record and the predecessor bundle through its two `depends_on` edges.
- **Cited code:** the citations I checked match `main` for usage and for the file-change and wire stories. These are `mod.rs:696-706` and `:280-324`, `observation.rs:19-34`, `process.rs:3115-3128` and `:3290-3295`, `linux.rs:427`, `lib.rs:1502-1515`, `:2161-2185` and `:2308-2318`, and `operations.rs:854-855`.
- **`systemd` wording:** the file-change story's "`systemd` around `linux`" is right, because `systemd.rs:1-12` has both variants and that one goes through the host driver. The usage story's "around `local`" is the variant the carrier does not reach.

**What I read:** three stories whole, plus `run-observation-names-the-limit-that-ended-it`, `audit-records-run-end-and-limit`, `process-count-limit-hit-is-observed` and `exec-observation-reports-process-count-hits`. I ran `aep plan artifact show` on each, `aep plan artifact relations` and `aep plan artifact list --format json` (about 25 edges, including the epic's other stories and the two later items ordered after the set). I ran `aep plan artifact validate`, which reports only unrelated review-result notices. I read the cited ranges in `crates/substrate-host/src/`, `crates/substrate-wire/src/lib.rs` and `crates/substrate-store/src/operations.rs`, plus `crates/substrate-daemon/src/app/operations.rs` and the Kubernetes driver's `ExecObservation` literals. I traced 9 acceptance reads to producers in or behind the set, and all 9 are recorded as edges, directly or transitively.

**What I could not establish:**
- **Digest-only mechanism (decided, so I did not reopen it):** it holds only digests, so "content for both sides of a modified file" needs the before-side captured at exec start. The ADR's cost bullet prices only the two digests and not that capture. This is the ADR's job, and it is partly a question for the acceptance critic.
- **`ExecObservation` literals (out of my lane, for parallel safety):** the daemon's `observation_from_stored`, daemon tests and `crates/substrate-kubernetes/src/exec.rs:1533` and `:1578` build `ExecObservation` literally. A new member from the file-change story breaks them, and none of those paths is in its scope.
- **Wire design in an in-process ADR:** the file-change story's ADR also decides the wire shape, while the sibling wire story for process-count hits writes its own design section. It is a choice, not a defect, but the in-process acceptance cannot check the wire half.

```findings
- file: .engineering/planning/story/run-observation-carries-usage.md
  line: 85
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "item 2's one carrier is the host driver's read before reconcile_cgroup, but the acceptance requires systemd around local, whose runs own their cgroup and never pass through that read, and no item names the read it needs"
- file: crates/substrate-host/src/confinement/systemd.rs
  line: 836
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the code cited for systemd is a private cpu.stat point sample for the budget poll, and systemd.rs reads no memory.peak, pids.peak or wall time, so the citation supports one of the four figures"
- file: .engineering/planning/story/exec-file-changes-are-recorded.md
  line: 124
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "item 3 has the host driver digest and the adapter expose the record while the wire story has the daemon store the content, and neither body says which keeps it and which reads it, though the daemon reads ExecObservation and has no adapter API"
```
