---
format: aep.planning-md/3
id: review-result:wave-d342-u2-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1 of wave D342, unit U2
relations:
- reviews: story:chart-lane-stand-ins-never-fail-with-etxtbsy
revision: 1
---
# Adversary pass 1 of wave D342, unit U2

Agent type `aep:adversary`, against the story's Acceptance only. Tree `substrate-d342-u2` at
`2f0b9a46`. Module `adversary_d342` in `xtask/src/chart_cluster.rs`, four cases; `cargo test -p
xtask --bin xtask`: 459 passed, 1 failed, 9 ignored. A probe writing the script in-process with
`fs::write` in the race test's loop failed 92, 82 and 82 of 1000 starts, so the race test catches
the old bug.

```findings
- file: xtask/src/chart_cluster.rs
  line: 4197
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: stand_in passed the body as one argv string, so a body over 128 KiB (MAX_ARG_STRLEN) or containing a NUL panicked with E2BIG or InvalidInput where fs::write succeeded; no current caller passes such a body.
```

Held: byte identity for `%`, a leading `-`, backslashes, quotes, `$` expansions, a trailing newline
or none, CRLF and UTF-8; mode set by path, never reopened for writing; no other in-process write of
an executed file in the `xtask` test binary; the race test green 10 times alone and
`chart_cluster::tests` green 5 times in parallel; a writer failure names the path and status; the
existing stand-in tests keep their assertions.

Out of scope: none.

Coordinator decision (D342, 2026-11-01):

- `chart_cluster.rs:4197`: fixed in the wave (`fix(xtask): a stand-in body goes to its writer on
  a pipe`, `a2efee43`): the body goes to `cat > "$1"` on stdin; the pipe is not the script, so it
  cannot cause ETXTBSY. NUL is no longer an argv problem. Green: 461 passed, 9 ignored; clippy
  clean.
