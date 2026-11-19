---
format: aep.planning-md/3
id: story:release-lanes-run-on-hosted-runners
kind: story
status: draft
title: The four release lanes run on GitHub-hosted runners, and a release reads their records
summary: release-lanes.yml runs the confinement, quota, chart node-profiles and namespace lanes on hosted runners; release.yml takes each record from its successful run when the tag carries none
relations:
- informed_by: decision-blocker:namespace-lane-runs-on-a-workstation
- informed_by: story:project-quota-lane-has-a-provisioned-runner
- informed_by: story:chart-lane-local-execution-record
- informed_by: story:ci-runs-the-full-gate
revision: 2
---
# Story: The four release lanes run on GitHub-hosted runners, and a release reads their records

## Outcome

Whoever releases Substrate no longer needs a workstation to run the four release lanes and hand-type
their lines into the tag: `release-lanes.yml` runs them on GitHub-hosted runners for every commit on
`main`, and `release.yml` takes each record from that workflow's recorded `success` for the tagged
commit, checked exactly as a trailer is.

## Context

Every release so far carried `Confinement-lane:`, `Quota-lane:`, `Chart-lane:` and
`Namespace-lane:` trailers that an operator produced on a workstation (AGENTS.md § *Releases*).
The docs gave two reasons: a hosted runner was said to lack bubblewrap, a delegated cgroup subtree,
project quotas and Localhost AppArmor profiles, and the namespace kind lane did not fit the CI
budget (`decision-blocker:namespace-lane-runs-on-a-workstation`, D182). A GitHub-hosted
`ubuntu-24.04` runner is a full VM with passwordless sudo, systemd, cgroup v2 and an AppArmor
kernel, so root setup steps can give it what a workstation has. A self-hosted runner is ruled out
(Timo, 2026-11-19: "just fix it, no self hosted runner though").

What each job sets up as root, and why:

- confinement: `bubblewrap` and `socat`; an AppArmor profile granting `userns` to `/usr/bin/bwrap`
  only (Ubuntu 24.04 keeps `kernel.apparmor_restrict_unprivileged_userns=1` for everything else);
  linger for the runner user, so a user manager exists. The runner process sits in
  `system.slice/hosted-compute-agent.service`, which the user manager cannot move it out of, so
  `scripts/delegated-lane.sh` runs in a transient user service and asks for its delegated scope
  from there; that service gets `LimitNOFILE=65536`, because under systemd's default soft limit of
  1024 the 513th process event stream of
  `security_pass1_a_subject_holds_at_most_512_process_event_streams` is reset on connect.
- quota: `modprobe quota_v2` (present in the runner's kernel modules; without it an ext4
  `prjquota` mount fails with `ESRCH`).
- chart: nothing; the runner kernel reports AppArmor enabled (`Y`) and the lane mounts its
  securityfs into its own kind worker.
- namespace: nothing beyond kind; Docker Hub pulls worked anonymously from the runner.

## Acceptance

- `release-lanes.yml` runs on push to `main`, on dispatch and on pull requests that change what the
  lanes build; each of its four jobs runs the lane's own command unchanged and records the lane's
  line as a notice annotation titled with its trailer.
- The preflight step *Read the hosted lane records for this commit* waits for that workflow's
  `success` for the exact tagged commit, counting only a `push` to `main` or a dispatch of this
  repository, and refuses a run set with no success or two annotations of one lane.
- Each record step accepts the hosted value only when neither the dispatch input nor the tag carries
  one, and refuses it for another commit, another count than the tagged source declares, a
  non-RFC-3339 instant or the wrong shape, exactly as a trailer. The release notes name the carrier.
- `xtask/tests/release_workflow.rs` executes those paths offline; the four lanes pass on hosted
  runners on the pull request that adds the workflow.

## Out of Scope

- Making `release-lanes.yml` a required check on `main`: a ruleset change, Timo's call.
- Retiring the hand-recorded carriers: they stay for recovery and for tags without the workflow.
- Changing what any lane verb checks.

## Open Questions

- Whether D182's budget reason still stands now that the namespace lane takes about 15 minutes on a
  hosted runner (Timo).
