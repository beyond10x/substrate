---
format: aep.planning-md/3
id: approval-record:repository-owner-accepts-static-musl-releases-2026-11-26
kind: approval-record
status: approved
title: The repository owner accepts the static musl release story
relations:
- decides: story:release-publishes-static-musl-archives
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-26T07:55:49Z", actor: "human:operator", revision: 2}
---
# The repository owner accepts the static musl release story

The repository owner decided on 2026-11-25, option A: static musl Linux releases for every
beyond10x Rust tool, mimalloc where a tool is allocation-heavy, and consumers pin the musl
archives. The order that reached this repository on 2026-11-26:

> Into your next wave (not a separate PR): an `x86_64-unknown-linux-musl` static archive in your
> release workflow beside the gnu one, CI check static + `--version` in a plain Alpine container;
> mimalloc where allocation-heavy (measure, keep only if not slower).

and, after the previous wave merged: "Next wave as planned (musl story included)."

This record accepts `story:release-publishes-static-musl-archives` into the wave of 2026-11-27.
