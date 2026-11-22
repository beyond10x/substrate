---
format: aep.planning-md/3
id: review-result:floor-block-exchange-security
kind: review-result
status: active
title: 'Security review: the floor refuses the block-exchange ioctls'
relations:
- reviews: story:floor-refuses-the-ioctl-that-exchanges-file-blocks
revision: 1
---
# Security review: the floor refuses the block-exchange ioctls (pull request 254, `c5c4b98ea`)

Verdict: nothing found that holds the change; three notes. Read, not run.

- One command builder adds the seccomp program for every exec, pipe session and pty session, with
  or without an executable list; the `linux` and `systemd` adapters and `confine run` arrive there.
- The architecture check is first, the x32 bit is refused next, and nothing earlier allows
  `ioctl`; the comparison reads the low word.
- No configuration field, feature gate or profile member removes the program or a row.
- The driver's probe sandboxes other than the floor probe run without the program: fixed argv,
  cleared environment, the driver's own temporary directories; no caller reaches them.
- Notes 1 and 2 (wording) are fixed in `6279df923`; note 3 is left as it is.
- Not verified: a session or an `int 0x80` call under the installed program (the adversary ran
  both), `aarch64`, what XFS does with the request.

```findings
[
{"file":"website/docs/concepts/confinement.md","line":32,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The page states what XFS_IOC_SWAPEXT does as fact without the read-not-run qualifier the changelog, AGENTS.md and ADR 0050 carry."},
{"file":"CHANGELOG.md","line":25,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The pipe- and pty-session claim rests on reading process.rs:3069; no case runs a session under either request and the argv case pins --seccomp for the quiet channel only."},
{"file":"crates/substrate-host/tests/floor_refuses_block_exchange.rs","line":245,"category":"boundary","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"EXT4_SUPER_MAGIC also matches ext2/ext3, so the case can print executed where the request could never exchange, though its EACCES assertion still needs the filter."}
]
```
