---
format: aep.planning-md/3
id: approval-record:wave-2026-11-05-substrate
kind: approval-record
status: approved
title: 'Substrate wave 2026-11-05: host features, egress reaps, ESS pin, release 0.20.0'
relations:
- decides: story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes
- decides: story:every-egress-reap-retries-through-eintr
- decides: story:systemd-netns-floor-tolerates-down-fallback-tunnels
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-05T06:18:55Z", actor: "human:timo", revision: 2}
---
# Substrate wave 2026-11-05: host features, egress reaps, ESS pin, release 0.20.0

## Approval

The orchestrator approved this wave on 2026-11-05, under its grant (class C).

## Units

| unit | artifact | note |
|---|---|---|
| U1 | `story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes` | draft to active; additive packaging, the defaults keep today's set, not ADR-level |
| U2 | `story:every-egress-reap-retries-through-eintr` | draft to active |
| U3 | `story:systemd-netns-floor-tolerates-down-fallback-tunnels` | active to implemented, store only |
| U4 | the ESS pin moves from 0.81.0 to 0.86.0 | `ESS_VERSION` and `ESS_SHA256` move together. If 0.86.0 refuses scenarios or validation, the findings become a draft and the pin stays 0.81.0; this does not hold the wave |

## Terms

- **U1 budget:** tell at 8G or 25 cargo minutes, stop at 10G or 35, because it builds two feature
  graphs of the host crate.
- **U2 budget:** the default.
- **Gate:** stages A to D with `BUILD_SLOTS=2`. Each stage starts only with the disk floor at its
  expected peak plus 40G, and never under 45G. The trees budget is checked before each unit and
  each stage.
- **Release:** version 0.20.0. On green, the lead opens the pull request, merges it, runs the
  release lanes at the merge commit with at least 60G free on `/`, tags, releases and verifies,
  without asking again.
