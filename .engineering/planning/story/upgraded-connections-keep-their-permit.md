---
format: aep.planning-md/1
id: story:upgraded-connections-keep-their-permit
kind: story
status: active
title: A WebSocket upgrade stays inside the transport connection budget
summary: The connection permit lives in enforce_connection_lifetime, which hyper resolves at upgrade; upgraded sockets are uncounted (runtime.rs:596,919,1084).
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: crates/substrate-daemon/src/runtime.rs
revision: 5
---
# Story: Upgraded connections keep their permit

## Context

Every listener wraps the hyper connection future in `enforce_connection_lifetime`, which owns the
per-uid and global `ConnectionPermit` (`crates/substrate-daemon/src/runtime.rs:596`, `:919`,
`:1084`; limits at `:53-56`). hyper 1.11.1 (`Cargo.lock:1141`) resolves an upgradeable connection
future when it hands the socket to the upgrade, so the permit is released the moment a WebSocket
upgrade succeeds and the upgraded socket runs in a task the transport budget no longer counts.
Inferred from the library's semantics and consistent with the separate 1 h attachment lifetime the
session code needs (`app/sessions.rs:70`); not observed at runtime.

## Acceptance

A test holds the per-uid connection budget with upgraded WebSockets and shows the next plain HTTP
connection from that uid is refused at accept, or the story records the observation that the
permit already survives the upgrade and closes with no code change.

## Notes

First step is the observation, not the fix. If the permit is released, either move it into the upgrade task or give every upgrade its own bounded permit as event streams and pipe attachments already have. story:unattached-claimed-session-is-contained lands on the same `on_upgrade` seam for the session attach route and is `informed_by` this story; whichever remedy is chosen here decides where that story puts its claim.
