---
format: aep.planning-md/3
id: review-result:floor-block-exchange-adversary
kind: review-result
status: active
title: 'Adversary pass: the floor refuses the block-exchange ioctls'
relations:
- reviews: story:floor-refuses-the-ioctl-that-exchanges-file-blocks
revision: 1
---
# Adversary pass: the floor refuses the block-exchange ioctls (pull request 254, `c5c4b98ea`)

Verdict: no blocker; one change asked for.

- Ran, in real runs under the filter: `EXT4_IOC_MOVE_EXT` answers `EACCES` for an exec, a pipe
  session and a pty session, natively, with the high word set, through the x32 entry and through
  `int 0x80`; `XFS_IOC_SWAPEXT` and `io_uring_setup` answer `EACCES`; both files unchanged cold.
- Red before round 2: `ioctl(-1, 0x4018F514)` answered `ERRNO 9`. Fixed in `6279df923`, which
  refuses `F2FS_IOC_SEC_TRIM_FILE`; the case is in the suite.
- Read: the requests dispatched in ext4, xfs, btrfs, f2fs, ocfs2, nilfs2, fat, exfat, ntfs3,
  gfs2, jfs, ceph, cifs and `fs/ioctl.c` at kernel v6.18; only the two f2fs requests turned up.
- Not checked: `aarch64` at run time, any run on XFS or f2fs, a host with SELinux, `oci`, `pod`.
- No mutant was run.

```findings
[
{"file":"adr/0050-the-floor-refuses-the-ioctl-requests-that-exchange-file-blocks.md","line":113,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"F2FS_IOC_SEC_TRIM_FILE (0x4018F514) meets decision 1's rule (write descriptor only, erases mapped blocks, no stat fact moves; read at v6.18, not run), is not named by the survey and passes the filter (red case adversary_floor_block_exchange.rs:688)"},
{"file":"adr/0050-the-floor-refuses-the-ioctl-requests-that-exchange-file-blocks.md","line":113,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"F2FS_IOC_START_ATOMIC_WRITE with abort or close reverts written content without setting a time; live-writer class, not named in the survey (read, not run)"},
{"file":"scripts/delegated-lane.sh","line":330,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"the not-ext4 absence is accepted on every host, the hosted release lane included, and the Confinement-lane record does not show it"},
{"file":"crates/substrate-host/src/probe.rs","line":647,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"probe sandboxes in pty.rs, read_only_files.rs, writable_roots.rs, workdir.rs, egress.rs and probe.rs start bubblewrap without the seccomp program; fixed driver-written argv over driver-made 0700 directories, no caller reach (read)"}
]
```
