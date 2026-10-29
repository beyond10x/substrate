---
format: aep.planning-md/3
id: review-result:design-34-u4-security-pass-1
kind: review-result
status: active
title: Security review of design 34 U4, the in-cluster executor, pass 1 (2 blockers, 1 warning)
relations:
- reviews: story:kubernetes-namespace-workspace-and-exec-slice
revision: 1
---
# Security review of design 34 U4 (in-cluster executor), pass 1

Reviewed `unit/executor` at `576a17c2` (merged into `wave/k8s-namespace-1` as `30af0070`),
2026-10-29. Verdict: needs change, two blockers and one warning, all introduced by the unit; three
red cases committed as `bdd6685b` on `fix/u4-security`.

## Findings

| # | file:line | severity | finding | red case |
|---|---|---|---|---|
| 1 | `crates/substrate-executor/src/run.rs:149-158` | blocker | the child runs as the executor's uid, neither holds capabilities, and the executor stays dumpable, so the child can reopen `/proc/1/fd/1` and append a forged frame (a second `exit`, an `applied`, output past the limit); design 34 § 7.3 promises a child cannot forge a frame | `a_child_cannot_append_a_frame_to_the_executors_own_output` (log holds `[Code(0), Code(3)]`, expected `[Code(3)]`) |
| 2 | `run.rs:222-229` | blocker | every pipe read becomes one frame line; one-byte writes take one line per byte, breaking the `ceil(L/6144)` lines per stream that the `exec.output-limit-bytes` budget assumes (about 13 MiB of frames for 1 MiB of output), so the kubelet can rotate the `applied` record out of the log the driver reads | `a_stream_takes_at_most_one_line_per_full_chunk_of_its_output` ("40 bytes of output took 40 lines") |
| 3 | `xtask/src/packages.rs:76` | warning | the executor's `cargo tree` rule reads the host target only; a musl-only edge to a fenced crate passes, and the executor ships as a musl build | `a_musl_only_edge_to_a_fenced_crate_is_refused_by_the_executor_tree_rule` |

Notes without a case: a TERM before `CHILD_GROUP` is stored (`run.rs:166`) or before the handler
is installed is dropped (deletion still ends at the 5 s grace); as PID 1, `run` does not reap
orphaned grandchildren; the output queue is unbounded if stdout blocks (only the Pod memory limit
caps it); design 34 § 7.2 still says 64 KiB for the header where the code uses 16 MiB; no build or
release step checks that the binary is static.

## The implementor's calls, checked

TERM forwarding now has a case (`term_to_the_executor_reaches_the_childs_whole_process_group`,
green, the background grandchild dies too). The 16 MiB header is justified by inline
`FileReplaceInput`/`FileEditInput`, and reads allocate only as bytes arrive. `open_single` keeps
`openat2` beneath, no symlinks, no magic links, no mount crossing; create and destroy are refused.
The stat, check, `omitted_entries`, spawn-failed 127 and spec-invalid 2 shapes agree with the
frames. Wire-maximum sizes are a backstop beneath the driver's lower limits.

Also sound: environment cleared, program lookup only through the spec's `PATH`, frames over the
line limit refused before decode, exit code always at most 255, a musl release build is
`static-pie` with no `INTERP`.

## Counts and limits

Executor package 29 to 31 cases plus the TERM case run on its own, xtask 1; 3 red. Not verified:
the Dockerfile build, a real Pod, the full xtask suite. The fixes are fix unit E of this wave.
