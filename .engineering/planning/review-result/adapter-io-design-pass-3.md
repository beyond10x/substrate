---
format: aep.planning-md/3
id: review-result:adapter-io-design-pass-3
kind: review-result
status: active
title: Independent review of design 32, pass 3
relations:
- reviews: story:adapter-stdin-pipe
revision: 1
---
# Independent review of design 32, pass 3

Reviewed `docs/design/32-adapter-stdin-and-live-output.md` at PR 189 head dc106e35, 2026-10-28.
Verdict: needs-revision. G1-G9 answers hold on the facts they cite; pinned lane counts match the
tree (portable 18, 17 on macOS; `LINUX_DELEGATED` 9; systemd 5; macos 8; oci 5; pod 3).

- H1 (blocking, D32:287-291, 377): the host's `signal` closes the receiver under the same lock
  `read_pipe` holds for its whole `timeout(recv)` (process.rs:1625, 1884, 3419), so a long
  `read_output` on a silent `linux` child stalls `signal(Kill)`. Read in slices of at most 100 ms
  or make the close lock-free; add a case: a 30 s read in one task, `signal(Kill)` from another
  returns within 3 s.
- H2 (blocking, D32:263-275, 375, D16; local.rs:370-387): `Int` and `Term` can be trapped on
  `local`, `macos`, `oci` and `systemd` around `local`; closing the queue on any signal stops live
  output silently. Close on `Kill` only there; state that `linux` is end-equivalent (`grace_ms: 0`);
  add a TERM-trapping case.
- H3 (blocking, story bodies; D32:441-447, 576-579): a spec story whose acceptance needs its
  adapter story cannot merge alone under the wave protocol (one story per unit, merged when
  green). Fold each spec and adapter pair into one story, or move the green clause to the adapter
  story and state the spec story has no merge of its own.
- H4 (note, D32:427-430): `check-docs` also holds
  `website/docs/reference/contracts.md#scenarios-and-lanes` (site_drift.rs:284-320) and
  `spec/coverage.md:183`; each spec step moves those counts.
- H5 (note, D32:281-284): the `linux` bound is `min(io_bounds.output_bytes,
  config.output_limit_bytes)` (process.rs:1413-1415); compare the delivered offset to the final
  record length.
- H6 (note, D32:523, T8): T19 can stall on a pause over 1 s; write a fixed amount instead of
  `yes`; BSD `wc` pads with spaces; `read_pipe` can add up to 5 s after `None`
  (process.rs:1637).
