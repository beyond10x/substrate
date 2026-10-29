---
format: aep.planning-md/3
id: review-result:adapter-io-design-pass-2
kind: review-result
status: active
title: Independent review of design 32, pass 2
relations:
- reviews: story:adapter-io-spec
revision: 1
---
# Independent review of design 32, pass 2

Reviewed `docs/design/32-adapter-stdin-and-live-output.md` at PR 189 head be25c7ee, 2026-10-28.
Verdict: needs-revision. Pass-1 answers F4-F7 and F9-F13 hold; F1, F2, F3, F8, F12 reopened.
Process.rs is `crates/substrate-host/src/process.rs`; D32 is the design.

- G1 (blocking, D32:244-246): on `linux` a signal, timeout, CPU, aperture or bound end closes the
  receiver (process.rs:1884, 3361-3394, 3419); `forward_frame` then returns false
  (process.rs:3762-3767) while `drain_capped` keeps filling the record, with no event and
  `output_stalled` false. State that delivery stops on any such end, the record is the complete
  account and the observation names the cause; hold T4/T6 equality to natural ends.
- G2 (blocking, D32:236-237): there are no per-stream bound flags; `run_child` shares one local
  `AtomicBool` (process.rs:2959-2964). Emit `Bounded` from the adapter's delivered offset and the
  final observation's `stdout_truncated`.
- G3 (note, D32:108-115): `tokio::spawn` in `Drop` panics without a runtime; use
  `Handle::try_current` and state the fallback. Concurrent `read_output` needs an adapter-side
  reader lock (process.rs:1625).
- G4 (note, D32:96-97 vs 424): write-after-end is undefined; the host never drops stdin at run
  end (process.rs:1391); T10 races.
- G5 (blocking, D32:354-371, 481-493): lane runners compare the declared set exactly, so with both
  facts in one spec story no adapter unit can show a green lane; the spec story must also edit
  pinned per-lane counts (`ess_confinement_macos.rs:90`). Split the spec per fact or fold units.
- G6 (blocking, D32:130-138, 415, 481-493): story 2 reads `OutputDelivery` from story 3;
  `check_streams`, T1 and the linux `Pipes` channel are shared; no `depends_on` edge records the
  order. Add live → stdin and move `confinement.live-output-unread` and T7 to the live story.
- G7 (blocking, D32:422-424): T8/T9 never cancel (a 64 KiB write into an empty pipe completes);
  write one full chunk, then cancel a second. T10's `setsid` descendant must redirect output or
  `local` never ends the run (local.rs:296-298).
- G8 (note, D32:130-138): `run_with_stdin` with `Null` stdin and input, input over `limit_bytes`,
  or its own cancellation can leave a started run running.
- G9 (note, D32:98-101, 320): on `oci` "the pipe took every byte" means the runtime client's pipe;
  T15 must prove EOF; `systemd` around `linux` must forward all four new trait methods. ADR 0016
  is superseded but cited as current (D32:157, 225, 396).
