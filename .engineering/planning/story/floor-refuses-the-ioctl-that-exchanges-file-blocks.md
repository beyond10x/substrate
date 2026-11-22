---
format: aep.planning-md/3
id: story:floor-refuses-the-ioctl-that-exchanges-file-blocks
kind: story
status: active
title: The confinement floor refuses the ioctl that exchanges file blocks
summary: EXT4_IOC_MOVE_EXT succeeds inside a linux run and changes content with every stat fact unchanged; the floor refuses it
tags:
- defect
- security
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-22T01:11:05Z", actor: "human:timo", revision: 2, executor: "agent:substrate"}
- {from: "proposed", to: "active", at: "2026-11-22T01:11:06Z", actor: "human:timo", revision: 3, executor: "agent:substrate"}
---
# Story: the confinement floor refuses the ioctl that exchanges file blocks

## Outcome

A confined run or session that calls `EXT4_IOC_MOVE_EXT` gets `EACCES`, and nothing on the file
system changes. Today the call succeeds inside a run: it exchanges the blocks of two files the
run may write and leaves every stat fact of both unchanged.

## Where the requirement comes from

Found on 2026-11-22 while drafting the ADR of `story:exec-file-changes-are-recorded`. The
operator decided the same night, relayed by session `host-operator` and by session `runbook`
(`approval-record:operator-orders-the-floor-to-refuse-move-ext-2026-11-22`): the floor refuses it
for every confined run, as a story of its own ahead of the change record, with a design note
first and a case that shows the call succeeding before and refused after. Stakeholder: the
operator.

## Observed

One run on the real `linux` adapter (bubblewrap, the driver's seccomp program, a delegated
cgroup scope, a writable workspace on `ext4`), with a static program:

```
run exited Some(0); program said: MOVER ioctl(EXT4_IOC_MOVE_EXT) rc=0 errno=0 (ok) moved_len=16
donor before facts [66306, 30472565, 65536, 1927699156903466634, 1927699156903466634] digest ffe4bc29…295b4c
donor after  facts [66306, 30472565, 65536, 1927699156903466634, 1927699156903466634] digest de2f2560…a9cc31
donor after cold content distinct bytes {0}; stat_facts_unchanged=true content_changed=true
```

The five facts are device, inode, size, mtime and ctime. The content was read on the host after
`fdatasync` and `POSIX_FADV_DONTNEED`: until the pages are dropped, a reader still gets the old
content from the page cache. A read-only workspace stops it (`errno=30`). A first attempt
returned `ENODATA` because the files' blocks were not yet allocated; with both files synced the
call succeeds.

## Why it matters

- A run can leave a file whose content on disk differs from what every reader saw after the
  run: a review, a commit or a digest made from the cached content does not describe what the
  disk holds after the cache is dropped or the host restarts.
- No stat fact shows it, so a change record built on stat facts cannot list it, and a full
  digest walk reads the cache and misses it too (ADR for the change record, decision 1).
- The run's seccomp program refuses one ioctl today, `TIOCSETD`
  (`crates/substrate-host/src/seccomp.rs`).
- It is in every release that has the `linux` adapter, 0.23.0 included.

## Not known

- A session-style run was not staged. Read from the source: a session is spawned through the
  same `command()` in `crates/substrate-host/src/process.rs`, which adds the one seccomp program.
- A read-only root was not run.
- Which other calls exchange or move file blocks without a capability and without moving a stat
  fact: the XFS exchange and swap ioctls, `EXT4_IOC_SWAP_BOOT`, `FICLONE`, `FIDEDUPERANGE`,
  direct I/O, `copy_file_range`, `splice` and `io_uring` writes were considered and not tried.
  The served file systems of the executable lists are `ext4`, `overlay` and `tmpfs`; the
  workspace root can lie on others.
- Whether the `oci` and `pod` adapters, whose runtimes bring their own seccomp profile, let it
  through.

## What changes

1. **A design note first** (invariant 8): an ADR that states the finding, the rule (which
   requests the floor refuses and with which errno), why a refusal by request number and not an
   allow-list of ioctls, what the survey of the calls above found (tried where this host can,
   read otherwise, each marked), what the other adapters do, and what stays possible (an
   unconfined host process, a file system the survey did not reach).
2. **A failing case first**: the run above as a lane case; red today (the call returns 0 and the
   content changes), green when the call returns `EACCES` and the content and facts are
   unchanged. It joins `scripts/delegated-lane.sh`. A portable unit case holds the seccomp
   program's bytes for the request.
3. The seccomp program refuses the request for every confined run and session.
4. `AGENTS.md` § *Safety envelope* names it in the floor; `CHANGELOG.md` gets a *Security*
   entry that says every earlier release lets the call through.
5. An adversary pass and a security review before the pull request goes to the operator.

## Acceptance

In the delegated lane a confined run that calls `EXT4_IOC_MOVE_EXT` on two files of its writable
workspace gets `EACCES`, both files keep their content and their five stat facts, read cold on
the host; before the change the same case shows the content changed. The ADR is accepted and
indexed before the code.
