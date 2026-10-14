---
format: aep.planning-md/3
id: review-result:wave-2026-10-14-u1-security-pass-2
kind: review-result
status: active
title: Security review pass 2, unit u1 (session containment)
relations:
- reviews: story:session-containment-retries-an-unproven-kill
- reviews: story:the-daemons-detached-containment-task-is-tracked
revision: 1
---
fixed

Security review pass 2 of the session-containment unit, read from b004ed17 (no build). No blocker;
all four pass-1 fixes hold.

Findings:
- **Warning, fixed.** The comments and the warning on dropping an `unknown` exec said lease
  cleanup kills it again. It does not: the host driver treats `unknown` as terminal and answers a
  further kill with its recorded observation, so only the next daemon start reconciles the
  cgroup. The text now says nothing retries that kill before the daemon restarts (50e183a). The
  missing re-kill itself predates this unit and is recorded as the draft
  `story:an-unknown-exec-is-killed-again-before-restart`.
- **Note, fixed.** The detached-task scan ended a test module only at a line exactly equal to its
  closing brace, so `} // mod tests` hid every line after it. A trailing comment now still closes
  the module, and a module never seen closed is reported by its own line. The reviewer's case went
  red (`found [], expected [7]`) before the fix.
- **Note, infeasible.** Never-attempted entries sort ahead of older unproven ones, so a sustained
  inflow of 64 or more new failed upgrades per round would starve the older ones; no source of
  such an inflow was found.
- **Note.** On a retired write, `terminate_pipe_session` returns the driver's state without
  checking it is terminal; the host never answers a kill with a non-terminal state.
