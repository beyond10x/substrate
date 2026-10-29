---
format: aep.planning-md/3
id: story:test-socket-paths-fit-sun-len
kind: story
status: draft
title: Test socket paths fit SUN_LEN under any TMPDIR
revision: 1
---
# Story: test socket paths fit SUN_LEN under any TMPDIR

## Observation

The local full gate on `wave/adapter-io` at `2e8337fb` failed on 2026-10-29 in
`crates/substrate-daemon/tests/http.rs:679`
(`special_entries_are_omitted_from_both_listing_routes_rather_than_refused`):
"bind socket: Error { kind: InvalidInput, message: \"path must be shorter than SUN_LEN\" }". TMPDIR
was a `mktemp -d` directory under a user cache directory, longer than `/tmp`. The same test passed in
an earlier run with a different random TMPDIR name, so the margin is a few bytes.

The coordinating session reports the same defect class in another repository of the collection.

## Acceptance

- Every test that binds a Unix socket places it on a path whose length is checked against the
  platform limit (108 bytes on Linux, 104 on macOS) before binding, and falls back to a short
  directory it owns when TMPDIR is too long, or fails with a message naming the path length and
  the limit.
- One case runs a socket-binding helper under a TMPDIR of 100 bytes and passes.
- No production path changes; the daemon's own `--socket` handling is out of scope.
