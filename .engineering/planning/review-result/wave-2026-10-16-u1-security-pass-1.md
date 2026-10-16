---
format: aep.planning-md/3
id: review-result:wave-2026-10-16-u1-security-pass-1
kind: review-result
status: active
title: Security review pass 1, unit u1 (client CLI)
relations:
- reviews: story:a-client-cli-drives-the-daemon
revision: 1
---
no-op

Security review pass 1 on the client CLI unit, against 0ff355a4; cases in `tests/security.rs` (7, green on both lanes). Verdict: no blocker.

Checked, sound: requests carry no identity header or body member, and the exec environment is sent only when set; no argument names identity or a remote transport, and none reads the environment; argv reaches the daemon and the confined command byte for byte, a non-UTF-8 word is refused before any request, an empty word keeps the daemon's refusal; a network-shaped socket value never connects over TCP; no socket selected connects nowhere; capability facts absent from the daemon stay absent; `publish = false` with inherited SPDX metadata.

- **S1, note, accepted.** Human `exec run` writes the workload's raw stderr before the CLI's own `b10x-substrate:` status line, so a workload can print a line that looks like one. Raw pass-through is the documented behaviour; the exit status comes from the daemon's observation, and `--output json` escapes workload output. Scripts and agents use `--output json`.

Both lanes 43 passed, 0 failed.
