---
format: aep.planning-md/3
id: approval-record:wave-2026-11-04-substrate-gate-host-gap
kind: approval-record
status: approved
title: The 2026-11-04 wave's gate counts stage B green except one host gap
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-04T23:31:54Z", actor: "human:timo", revision: 2}
---
# The 2026-11-04 wave's gate counts stage B green except one host gap

Decided by the orchestrator under its grant on 2026-11-04, during gate stage B of
`approval-record:wave-2026-11-04-substrate`.

## Decision

- `security_review_systemd_linux_run_reaches_no_user_manager_and_keeps_the_floor` is recorded as a
  host gap in the gate's end line. The test failed only because the workstation has the `ipip`
  module loaded:
  - a fresh `unshare -rn` lists `tunl0`;
  - the kernel log shows the ipip load at 08:47:04Z.
- CI's run on the pull request decides this test.
- The module is not unloaded, because unloading needs root and a live local cluster may use it.
- If the rerun with `--no-fail-fast` shows every other test green, stage B counts as green except
  the host gap, and stages C and D follow on their disk floors.
- The fix is filed as the draft story `story:systemd-netns-floor-tolerates-down-fallback-tunnels`
  for a later wave.
