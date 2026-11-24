---
format: aep.planning-md/3
id: review-result:limit-observed-security
kind: review-result
status: active
title: 'Security review: an ended run''s observation names the limit that ended it'
relations:
- reviews: story:run-observation-names-the-limit-that-ended-it
revision: 1
---
# Security review: an ended run's observation names the limit that ended it

2026-11-24, on the first commit of the branch `feat/run-observation-names-its-limit`
(`c50488cf3`): design 36, story 1. The reviewer added four cases, three in a test module of
`crates/substrate-host/src/confinement/systemd.rs` and one in
`crates/substrate-host/tests/ess_confinement_local.rs`, and changed no production line.
Verdict: needs change. Host library selection executed 288 to 291, red 1;
`ess_confinement_local` and `consumer_surface` 47 to 48, red 1. S1: 1. S2: 1. S3: 1.

## Held

- `linux` maps the driver's code only for a run that is not running. `oci`, `systemd` and `pod`
  store the limit once, before the end is published. `local` and `macos` read the stall after
  their drains finished.
- `LIMIT_ORDER` is design 36 D4's: output stall, egress bytes, memory, CPU time, wall time.
- A `linux` cgroup name and a `systemd` run cgroup name are fresh per run, the sandbox mounts
  no `/sys`, and a count that cannot be read is absent.
- The end read comes after the kill and before the cgroup is removed, does not return early,
  and reads a kernel file the run cannot write.
- The rows per adapter are D7's table.
- The public items are those of D26 and D9.
- The change of `process.rs` adds one read and gives the same refusal with and without
  measurements, readable or not. The argv, the environment, the cgroup writes and the kill path
  are unchanged.
- Every line the delegated lane newly requires prints after its assertions.
- Two cases, green: a tick past both the CPU budget and the deadline records CPU time before
  the kill, with a real child; and the run cgroup's OOM count is its own `oom_kill`, absent
  where the key, the value, the file or the cgroup is missing.

Not established by the review: no delegated `linux` or `systemd` lane, no `oci`, `pod` or macOS
lane ran. The `systemd` case runs the real watch and end read on a stand-in cgroup directory.

## Findings and what was done

1. S1. Rule asked (D1): on `systemd`, a deadline that passes while an ended run's tree drains
   records no limit. The adversary pass of the same commit has the finding; done there. The
   case kept is this review's:
   `a_run_whose_leader_exited_before_its_deadline_names_no_limit_while_its_tree_drains`.
2. S2. Rule asked (D1, D5): a stall names a limit only where it ended the run. Done, decided
   by the session that works this repository and put to the story's stakeholder with the pull
   request: `OutputStall` is named only where the stall came
   before the leader's exit. Case:
   `a_local_run_that_exits_0_before_its_unread_output_stalls_names_no_limit`; the `linux`
   adapter holds the same through `EndOfRun::output_stall_after_exit`.
3. S3. On `oci` and `pod`, `Memory` is named from the runtime's OOM flag beside an exit status
   above 128. The flag is raised for any process of the container, and the status does not say
   who ended the main process. D7 accepts the row and the adapters' inputs cannot tell the
   cases apart. Done: the documentation of `oci_limit` and `pod_limit` and D7's notes say what
   is named.
