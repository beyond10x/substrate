---
format: aep.planning-md/3
id: approval-record:wave-2026-11-04-substrate-netns-floor-before-release
kind: approval-record
status: approved
title: The netns floor fix goes alone, before release 0.19.2
relations:
- decides: story:systemd-netns-floor-tolerates-down-fallback-tunnels
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-05T02:30:38Z", actor: "human:timo", revision: 2}
---
# The netns floor fix goes alone, before release 0.19.2

Decided by the orchestrator under its grant on 2026-11-05, after pull request 212 merged as
`55072372`. It amends `approval-record:wave-2026-11-04-substrate-merge-and-release`.

## Decision

- `story:systemd-netns-floor-tolerates-down-fallback-tunnels` is accepted and goes to active.
- The release of 0.19.2 is blocked. `release.yml` needs a `Confinement-lane:` record, which
  `scripts/delegated-lane.sh` produces, and its line 56 runs every `b10x-substrate-host` test.
  That includes the netns floor test, and the test fails on a host that has `ipip` loaded.
- Fix: the floor tolerates a kernel fallback tunnel only while its operstate is down and it is not
  up. Any other interface, or such a device up, still fails.
- The fix goes as one pull request that names its blocked consumer: the 0.19.2 release, through
  the delegated lane at `scripts/delegated-lane.sh:56`. The pull request also carries the wave's
  close store moves and a changelog "Fixed" line under 0.19.2.
- Unloading the module is not taken, because it needs root.
- On all-green CI, the pull request is merged through the bot. The four lanes then run at that merge
  commit, with at least 60G free on `/`. The tag `0.19.2` goes there, followed by the release
  verification and the announcement. A red check stops the work.
