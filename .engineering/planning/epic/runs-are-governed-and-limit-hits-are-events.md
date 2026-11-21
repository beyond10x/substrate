---
format: aep.planning-md/3
id: epic:runs-are-governed-and-limit-hits-are-events
kind: epic
status: active
title: A run's executables, limits and reach are enforced, and every limit hit is an event
summary: 'Executable allow-list and deny-list, the limit that ended a run, process-count hits, ingress and SDK requires: the gaps session loom found for SDC protocol runs, verified at adfde4020.'
owner: substrate
tags:
- confinement
- sdc
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T02:33:41Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"review_outcome":2}}, correlation: "run-governance-first-slice-accepted"}
- {from: "proposed", to: "active", at: "2026-11-21T02:33:41Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"review_outcome":2}}, correlation: "run-governance-first-slice-accepted"}
---
# Epic: A run's executables, limits and reach are enforced, and every limit hit is an event

## Outcome

A consumer that runs every process through Substrate can hold one run to an executable allow-list
and deny-list, to memory, CPU, wall-time and process-count limits, and to declared egress and
ingress. It reads which limit ended a run on the run's observation, and it receives a record for
every run that ended and every process-count hit. This holds on the in-process path
(`substrate_host::confinement::open`) and on the wire, so a run can move from an embedded adapter
to a remote daemon without losing a guarantee. Serves O1 (governed reach, `AGENTS.md` § Serves).

## Where the requirement comes from

Relayed by session `sdc` on 2026-11-21 as Timo's requirement of the same day for Loom, the runtime
that runs SDC protocols: "every process goes through Substrate so runs can move remote; hard limits
per run and per action (memory, CPU, wall time, process count), an executable allow-list and
deny-list, the confinement level, egress and ingress allow-lists, and every limit hit visible as an
event". Session `loom` read tag `0.21.0` and listed gaps 1, 2, 3, 5 and 8. This epic records each
gap as verified at `main` `adfde4020`.

Session `sdc` answered three questions on 2026-11-21. It marked the answers as its own and
reversible; Timo has not ruled on them:

- Loom runs on the in-process `linux` adapter first and moves to the daemon in a later epic.
- A limit summed over several starts of one protocol run is Loom's.
- Loom always sends an allow-list when it sends a deny-list.

## Gaps verified at `adfde4020`

Paths: C is `crates/substrate-host/src/confinement`, H is `crates/substrate-host/src`.

- **Gap 2, executables.** Confirmed. Only `macos` holds a run to its program and listed subpaths
  (`C/macos.rs:3-5`, `:28-35`, `:574`). `linux` does not prove `exec_subpaths` (`C/linux.rs:29-31`),
  and the specification leaves it unmapped there (`spec/confinement/confinement.yaml:199-201`).
  `systemd`, `oci` and `pod` do not list the fact (`C/systemd.rs:1450-1456`, `C/oci.rs:419-421`,
  `C/pod.rs:205`). No profile field, fact or rule names a denied executable
  (`C/profile.rs:43-97`). The wire publishes `confinement.exec-subpaths` as a property a start may
  require (`crates/substrate-wire/src/lib.rs:3612-3613`, `:3696`), and the start has no field that
  carries the list.
- **Gap 1, the limit that ended a run.** Confirmed, with a correction. `RunObservation` is at
  `C/mod.rs:696-706` (the gap report says `:681-706`) and has no field for a limit.
  `memory_oom_kills` is set only by `pod` (`C/pod.rs:1261-1273`); `linux`, `local`, `oci`, `macos`
  and `systemd` write `None` (`C/linux.rs:524`, `C/local.rs:200`, `C/oci.rs:817`, `C/macos.rs:520`,
  `C/systemd.rs:1071`). `AuditRecord` has four variants, none for a run's end or a limit
  (`C/mod.rs:280-324`). The host driver already names the limit on the wire observation:
  `exec.memory-limit`, `exec.cpu-limit` and `exec.timeout` (`H/process.rs:3280-3322`). The `linux`
  adapter drops that field when it builds a `RunObservation` (`C/linux.rs:507-526`). `oci` reads
  `OOMKilled` (`C/oci.rs:741-769`) and still reports `memory_oom_kills: None` (`C/oci.rs:817`).
  `exec.output-limit` (`H/process.rs:1878`) refuses an output read above the probed limit; it does
  not end a run. Output over a run's bound is truncated and stated on `RunOutput`
  (`C/mod.rs:709-719`).
- **Gap 5, process count.** Confirmed. The host reads `pids.events` `max`
  (`H/process.rs:4181`) into `ResourceUsage.process_limit_hits`
  (`crates/substrate-wire/src/lib.rs:2174`), which exists only for a start that asked for
  measurements. `record_resource_bound` has no branch for it (`H/process.rs:3280-3322`), the wire
  event union has no limit transition (`crates/substrate-wire/src/lib.rs:2308-2318`), and
  `RunObservation` has no field for it.
- **Gap 3, ingress.** Confirmed. `ingress_routes` and `rate_bounds` are reserved
  (`C/profile.rs:93-96`) and refused `confinement.reserved-field-unserved` (`C/mod.rs:549-557`);
  `network_ingress` is served by no adapter (`C/linux.rs:29`).
  `story:inbound-endpoint-for-a-confined-exec` is draft and covers the wire and the host driver.
- **Gap 8, `sandbox.requires` in the SDK.** Confirmed. The SDK sends `requires: None` at
  `crates/b10x-substrate-sdk/src/lib.rs:1443` and `:1921`. `story:sdk-confinement-requires` is
  draft; it cites those two sites at their earlier lines `:1404` and `:1848`.

## Order

The requester asked for the gaps in the order 2, 1, 5, 3, 8. Within gap 2 the in-process `linux`
stories come first and the `oci`, `pod` and wire stories after the first slice, as session `sdc`
answered.

First slice, the in-process `linux` adapter:

1. `story:linux-executable-allow-list` (gap 2).
2. `story:executable-deny-list` (gap 2).
3. `story:run-observation-names-the-limit-that-ended-it` (gap 1).
4. `story:audit-records-run-end-and-limit` (gap 1).
5. `story:process-count-limit-hit-is-observed` (gap 5).

After it:

6. `story:executable-lists-on-oci-and-pod` (gap 2).
7. `story:exec-start-carries-executable-lists` (gap 2, the wire).
8. `story:exec-observation-reports-process-count-hits` (gap 5, the wire).
9. `story:inbound-endpoint-for-a-confined-exec` (gap 3, existing draft, unchanged), then
   `story:confinement-profile-ingress-routes` (gap 3, the in-process field).
10. `story:sdk-confinement-requires` (gap 8, existing draft, unchanged). It shares no file with
    the others and can be worked at any time.

Stories 1 to 5 are worked one at a time in this order, and so are 7, 8 and 9. Each carries a
`depends_on` edge to the story it follows; its § Order says which edges exist only because of a
shared file. Stories 6 and 7 share no file and have no edge between them: both follow story 5 and
may be worked at the same time. Story 9's second story follows both 6 and 8.

Stories 7 and 8 and `story:inbound-endpoint-for-a-confined-exec` each cut a successor contract
bundle. Each takes the next free version when it starts; no story reserves a number.

## Out of scope

- A limit summed over several starts ("per run" where a run is many actions). Substrate bounds one
  start (`ProcessBounds`, `C/profile.rs:159-166`); summing across starts is the consumer's.
- A record of a denied execution or a denied connection. The run sees the failure. Whether an
  adapter can observe a denied execution at all is a question
  `story:linux-executable-allow-list` puts to its design document; no story here promises the
  record.
- A record written while a run is still running. `run_ended` is written when the run ends.
- `rate_bounds`, block-IO and bandwidth limits (`story:block-io-and-bandwidth-limits`).
- Loom, loom-sdk and SDC changes. They are other repositories' work.
- A release. It follows `AGENTS.md` § Releases after the stories merge.

## Done when

Every story in § Order is implemented, and a consumer-written test in this repository runs one
profile that names an executable allow-list, a deny-list and process bounds on the `linux` adapter
in the delegated lane, sees a denied execution fail inside the run, and reads the limit that ended
the run and the process-count hits on the run's observation and on its `run_ended` record.
