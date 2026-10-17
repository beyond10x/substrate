---
format: aep.planning-md/3
id: review-result:adversary-w20-u1-pass-1
kind: review-result
status: active
title: Adversary pass 1, wave 2026-10-17b unit u1 (adopted-workspace runtime vectors)
relations:
- reviews: story:exec-in-an-adopted-directory
revision: 1
---
# Adversary pass 1, wave 2026-10-17b unit u1 (adopted-workspace runtime vectors)

Reviewed: the merge of u1 at 6f57ab57 (unit commits 033bccf0, 5604ed5d). Verdict: needs change.
Judged by reading plus one direct bubblewrap probe; the two new delegated cases were not run (the
delegated lane was refused for disk).

| # | where | finding | origin |
|---|---|---|---|
| F1 | `crates/substrate-host/src/process.rs` default exec mount | exec binds the workspace by path (`--bind <path> /workspace`), which follows a symlink: an adopted entry replaced by a symlink after adopt mounts the link target read-write outside the workspace root; adopt and file I/O refuse links with `O_NOFOLLOW`, and scoped access already pins a descriptor | pre-existing, more reachable for an adopted directory |
| F2 | `runtime_vectors.rs` portable adopted exec | `exec.sandbox-unavailable` is decided before any workspace lookup, so the case observes nothing about adoption | introduced |
| F3 | `runtime_vectors.rs` working-directory case | compares two literals and cannot fail; no case observes the exec's cwd | pre-existing |
| F4 | `runtime_vectors.rs` nested-userns case | accepts any `OSError` rather than `ENOSPC`, so a host-level user-namespace restriction would pass it without `--disable-userns` | introduced |
| F5 | `runtime_vectors.rs` | a `too_many_lines` allow moved onto a short helper | introduced |

Cases added by the adversary (delegated): an adopted directory swapped for a symlink never mounts
the link target; an adopted directory removed after adopt is a named refusal at exec.

Not broken: the adopted isolation run uses the adopted id; `/workspace` is the operator's
directory and writes are visible on the host; destroy leaves the operator's files; the stated case
counts match the constants; out-of-set names and `ws_` collisions are covered in process.
