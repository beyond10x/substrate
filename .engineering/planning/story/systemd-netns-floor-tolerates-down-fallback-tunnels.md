---
format: aep.planning-md/3
id: story:systemd-netns-floor-tolerates-down-fallback-tunnels
kind: story
status: active
title: The systemd run's netns floor holds on a host with a tunnel module loaded
tags:
- from-finding
relations:
- informed_by: approval-record:wave-2026-11-04-substrate-gate-host-gap
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T02:30:25Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-05T02:30:25Z", actor: "human:timo", revision: 3}
---
# Story: the systemd run's netns floor holds on a host with a tunnel module loaded

## Outcome

`security_review_systemd_linux_run_reaches_no_user_manager_and_keeps_the_floor` passes on a host
that has a tunnel module loaded, and it still fails when a confined run's network namespace holds
any interface the run could use.

## Observation

Gate stage B of the 2026-11-04 wave, run on the operator's workstation, failed this one test
(`crates/substrate-host/tests/security_review_systemd.rs:329`). The probe's line was
`NET lo tunl0`, and the test wants exactly `NET lo`.

- The `ipip` module is loaded on that host. A fresh `unshare -rn ip -o link` lists `lo` and
  `tunl0@NONE: <NOARP> ... state DOWN`.
- The kernel log shows the ipip driver loading at 08:47:04Z on 2026-11-04, 61 s after a local kind
  cluster with Calico was created. The attribution to that cluster rests on the timing.
- The kernel creates the ipip fallback device `tunl0` in every new network namespace. It is down
  and has no address, so the run cannot reach anything through it.
- The wave changed neither this test nor the systemd adapter. CI's runner has no ipip loaded.

## Design

Have the probe report each interface with its state (`/sys/class/net/<if>/operstate`, or
`ip -o link`). The floor then holds when every interface other than `lo` is a kernel-created
fallback tunnel device (`tunl0`, `gre0`, `gretap0`, `erspan0`, `ip_vti0`, `ip6tnl0`, `sit0`,
`ip6gre0`, `ip6_vti0`) that is down. Any other interface, or any of those devices up, still fails.

## Acceptance

- With `ipip` loaded, the case passes; with a fallback device brought up inside the run, it fails.
- An extra veth or any other interface inside the run still fails the case.
