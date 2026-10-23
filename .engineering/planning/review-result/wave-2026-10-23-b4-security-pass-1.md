---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-b4-security-pass-1
kind: review-result
status: active
title: Security review pass 1, unit b4 (confined sockets argument allowlist)
relations:
- reviews: story:a-confined-process-cannot-mutate-host-global-kernel-state
revision: 1
---
# Security review pass 1, unit b4 (confined sockets argument allowlist)

Agent type `aep:security-reviewer`. Branch `unit/2026-10-23-b4-kmod`, commit `84c92cb`. Cases in `crates/substrate-host/tests/seccomp_review_pass1.rs`, run inside a real confined exec in a delegated scope. Verdict: NEEDS-CHANGE, 2 red.

```findings
- file: crates/substrate-host/src/seccomp.rs
  line: 157
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: The x32 compat ioctl (514|X32 bit) reaches the kernel past the TIOCSETD refusal on this x32-enabled kernel, measured as EBADF instead of EACCES inside an admitted exec.
- file: crates/substrate-host/src/seccomp.rs
  line: 213
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: undecided
  message: socketpair(AF_UNIX, SOCK_RAW) is admitted and the kernel creates it as SOCK_DGRAM, so the datagram-pair refusal is bypassed.
```

Red output: x32 case `REVIEW-CONTROL ERRNO 13` / `REVIEW-SUBJECT ERRNO 9`; socketpair case `REVIEW-CONTROL ERRNO 13` / `REVIEW-SUBJECT OK SO_TYPE 2`.

Full delegated lane on `84c92cb` without the review file: EXIT=0 (host 267 passed, 9 ignored; kernel_module_triggers 4; processes 59, 1 ignored; runtime_vectors 11; SDK 2/2/3; MCP stdio 3; tls_listener 1).

Held: label assembler offsets and bounds, 32-bit argument reads, SOCK_NONBLOCK/SOCK_CLOEXEC admission, i386/arm32 audit-arch refusal, allowlist equal to the ADR table, no autoload alias reachable through the unchecked socket type on this host, FAMILY_POLICY answers all 13 rows, one-sentence residual.

Coordinator decision: refuse every x32 system call (EACCES); socketpair admits SOCK_STREAM and SOCK_SEQPACKET only.
