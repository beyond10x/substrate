---
format: aep.planning-md/3
id: story:confine-run-reaps-killed-groups
kind: story
status: active
title: A SIGKILL'd confine run leaves no confine group behind
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:sandbox-repository-parity
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T07:24:38Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-27T07:24:38Z", actor: "human:timo", revision: 3}
---
## Outcome

A `b10x-substrate confine run` that is killed with SIGKILL leaves no `confine-*` group behind
for longer than the next invocation: the next `confine run` on the same
`SUBSTRATE_CONFINE_CGROUP_ROOT` removes every `confine-<pid>-<ns>` group whose process is gone
and that holds no process, and never touches a group whose owner is alive.

## Origin

`review-result:adr-0109-u10-security-pass-1` and the sandbox-parity fixes (`a2211af`): SIGINT and
SIGTERM remove the private group; SIGKILL cannot be caught, so its group stays (PR 160, "Not in
this change").

## Acceptance

A delegated-lane case starts `confine run` running `sleep`, kills it with SIGKILL, starts a
second `confine run`, and observes the first group gone, while a third, live invocation's group is
still present. The case is required by `scripts/delegated-lane.sh`.
