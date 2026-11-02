---
format: aep.planning-md/3
id: review-result:wave-2026-11-02-d364-u3-adversary-pass-1
kind: review-result
status: active
title: 'D364 U3 adversary pass 1: the aperture forwarder pidfd'
relations:
- reviews: story:aperture-install-sees-forwarder-death-at-once
revision: 1
---
# Adversary pass 1 — U3 of wave D364

Unit `story:aperture-install-sees-forwarder-death-at-once`, tree `substrate-d364-u3` at `cd572120`
(merged into `wave/2026-11-02-d364` at `9b4b0729`). Report as returned by the adversary
(`aep:adversary`, Opus) over two runs (the first was stopped by the pre-check); machine paths
shortened to the unit's scratch.

```
unit: story:aperture-install-sees-forwarder-death-at-once, tree substrate-d364-u3, findings cover cd572120; cases committed as 48b82224
verdict: nothing found
cases: executed 37→41 (egress lane, --skip d364_adv for the before count), red 0
origin: introduced 0 / pre-existing 0 / undecided 0
wrote-outside-worktree: 8 log files in the unit's adversary scratch
needs-coordinator: none
```

Cases in `crates/substrate-host/src/egress.rs`, `mod d364_adv` inside
`tests::observed_inside::d354_adv`, commit `48b82224` (test code only), all green:

- `a_record_sent_before_the_forwarder_exits_is_read_while_its_end_is_held`: a hook keeps a second
  copy of the report socket's other end and waits until the forwarder is a zombie; the install
  refuses with stage 6 (the forwarder's own report) within 500 ms and the forwarder is reaped. A
  mutant that read the exit before the socket would give stage 11.
- `an_installed_forwarder_leaves_the_installer_no_descriptor_on_it`: after a successful install the
  installer holds no pidfd on the forwarder (`/proc/self/fdinfo`, the scan first proved on a pidfd
  the case opened).
- `a_report_wait_interrupted_by_a_handled_signal_still_reads_the_report` (own process): `SIGUSR1`
  with `SA_RESTART` every 20 ms to the installing thread; the install still refuses with stage 6.
- `a_forwarder_without_a_descriptor_is_refused_killed_and_reaped` (own process): a full descriptor
  table makes `pidfd_open` fail with EMFILE; the install refuses with `aperture forwarder
  descriptor`, leaves no child and the same descriptors open.

Not absent: the own-process helpers run directly with `--ignored --nocapture` reached their
markers (`D364-ADV interrupted-and-read`, `D364-ADV descriptor-refused`, the latter
`(aperture forwarder descriptor: Too many open files (os error 24))`), 2 passed.

Runs: egress lane 37 passed 4 ignored before, 41 passed 6 ignored after; `-p b10x-substrate-host
--lib --locked` 420 passed, 15 ignored; fmt `--check` exit 0 after a formatting-only change in the
module; clippy `--all-targets -D warnings` exit 0.

Attacked and not broken: report then exit (read, not lost; `SOCK_SEQPACKET` cannot split a record);
EINTR; the pidfd refusal path (killed, reaped, no leak); no pidfd leak on success; no pid-reuse
window; the hook exists only under `cfg(test)` and fires once; mutants dropping the pidfd from the
poll set or ignoring its readiness would fail the unit's `after_kill < 500ms` (reasoned, not run).

```findings
[]
```
