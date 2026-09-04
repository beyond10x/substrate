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
  path: crates/substrate-daemon/src/app/events.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/metrics.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/sessions.rs
- confidence: cited
  path: crates/substrate-daemon/src/runtime.rs
revision: 9
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

First step is the observation, not the fix. **The observation was made on 2026-09-04 and the finding
holds: the permit is released at upgrade.** Recorded as `test_result` evidence against this story.

A test drove the production unix loop — `accept_authorized` + `http1_builder` + `.with_upgrades()`
+ `enforce_connection_lifetime` — over a real `UnixListener` at per-uid budget 1 and global 4. It
read `HTTP/1.1 101`, then read a frame off the upgraded socket, proving that socket still serving,
and the daemon then admitted a second connection from the same uid:

```
assertion `left == right` failed: a second connection from the same uid is refused at accept while
the upgraded socket lives, but the daemon served: HTTP/1.1 400 Bad Request
  left: 128
 right: 0
```

**Three `on_upgrade` sites, not two**, and the release happens upstream of all of them in
`crates/substrate-daemon/src/runtime.rs` at the three `.with_upgrades()` call sites — unix, TCP and
TLS alike:

| seam | own bounded permit inside `on_upgrade`? |
|---|---|
| `crates/substrate-daemon/src/app/events.rs` | yes |
| `crates/substrate-daemon/src/app/sessions.rs` | yes |
| `crates/substrate-daemon/src/app/metrics.rs` | yes, since `story:metrics-streams-are-bounded` |

**A fix therefore needs four files at once:** `runtime.rs` — where `ConnectionPermit` is private to
the module, so any hand-off is a visibility change there — plus all three `app/` upgrade sites. That
is why this story left the 2026-09-04 wave rather than being worked beside two of them, and why its
`scope` now records all four.

**The observing test is not in the tree.** It lived on a branch that never merged, was red by
design, and the branch was retired on 2026-09-04 to clear `cargo xtask check-secrets`, which scans
`--all` refs and matched the `Sec-WebSocket-Key` header the test sends. Rebuild it rather than hunt
for it: a `#[cfg(test)]` case in `runtime.rs` named
`an_upgraded_websocket_keeps_its_per_uid_connection_permit`, holding the per-uid budget with
upgraded WebSockets and asserting the next plain HTTP connection is refused at accept. Spell the
handshake key as a `HANDSHAKE_KEY` constant, the way every other hand-written WebSocket client in
this repository does, or the scan will match it again.

Labelled as a code read and not a measurement: `enforce_connection_lifetime` carries the 5-minute
`connection_lifetime` as well as the permit, so that bound is dropped at upgrade too.
