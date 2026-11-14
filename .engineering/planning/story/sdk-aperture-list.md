---
format: aep.planning-md/3
id: story:sdk-aperture-list
kind: story
status: active
title: The Rust SDK's session and exec builders select several apertures
relations:
- decomposes: epic:rust-sdk
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-14T20:14:07Z", actor: "human:agent", revision: 3, executor: "human:timo"}
- {from: "proposed", to: "active", at: "2026-11-14T20:14:07Z", actor: "human:agent", revision: 4, executor: "human:timo"}
---
# Story: The Rust SDK's session and exec builders select several apertures

## Outcome

A client of the Rust SDK selects several operator-declared egress apertures on a pipe session, a
PTY session or an exec, as the wire has carried since bundle 0.21.0 (design 26 § 2). Until now the
SDK builders kept one name and always sent `apertures: []`
(crates/b10x-substrate-sdk/src/lib.rs, in both builders).

## Why

conductor runs its runtime in a box that must reach two destinations: the model host, and
conductord's dial-in listener on host loopback (the runtime's instance API cannot be a Unix socket
in the box, ADR 0039). The daemon already admits a list of up to `MAX_EGRESS_APERTURES`; only the
SDK cannot send one.

## Acceptance

- `PipeSessionBuilder` (and so `pty_session`) and `CommandBuilder` each gain
  `apertures(impl IntoIterator<Item = impl Into<String>>) -> Self`; the names reach the start
  request's `sandbox.apertures` in the order given, with `network: "aperture"`.
- One name, whether given by `aperture` or as a one-name list, is sent as `sandbox.aperture` with
  no `apertures` member, so an existing start serializes and hashes as before. The later of
  `aperture` and `apertures` replaces the earlier.
- The SDK sends the list as given; an empty, over-long or repeated list is the daemon's existing
  named refusal (design 26 § 2), unchanged.
- No wire, bundle or daemon change.

## Boundaries

- No contract change: design 26 is the governing document; no ADR is owed.
