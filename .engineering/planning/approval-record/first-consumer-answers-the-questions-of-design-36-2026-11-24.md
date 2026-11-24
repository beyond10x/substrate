---
format: aep.planning-md/3
id: approval-record:first-consumer-answers-the-questions-of-design-36-2026-11-24
kind: approval-record
status: draft
title: The first consumer answers the five questions of design 36 (2026-11-24)
relations:
- decides: epic:runs-are-governed-and-limit-hits-are-events
revision: 1
---
# The first consumer answers the five questions of design 36 (2026-11-24)

Design 36 (`docs/design/36-limit-hits.md`, § 11) lists five questions that have more than one
defensible answer and change what a consumer of `substrate_host::confinement` writes. The
stakeholder of `epic:runs-are-governed-and-limit-hits-are-events` is the session that relayed
the requirement on 2026-11-21, `sdc`. It was not running on 2026-11-24. This session put the
questions to session `loom`, the first consumer, whose reading of tag `0.21.0` named the gaps
the epic records. The choice of whom to ask is this session's.

## The questions as put, each with default A

- Q1. A run ended by the aperture byte ceiling or by an output stall. A: two variants,
  `RunLimit::EgressBytes` and `RunLimit::OutputStall`. B: `limit` stays absent for both.
- Q2. `#[non_exhaustive]` on `RunObservation` and `AuditRecord`. A: yes, with
  `RunObservation::new`; a consumer's struct expressions and exhaustive matches stop compiling
  once. B: no; every later field breaks them again.
- Q3. Process-count hits while the run runs. A: `observe` of a running run carries the count on
  `linux` and `systemd`. B: only the ended observation and `run_ended` carry it.
- Q4. When the audit sink gets `run_ended`. A: after the end is published, off the runtime;
  the record may arrive after `wait` returned. B: before; a blocking sink holds `wait`.
- Q5. A runtime that shuts down while a run runs. A: `run_ended` with `state: unknown`, written
  from the destructor. B: nothing is written.

## The answer, as received from session `loom` on 2026-11-24

> Q1 A, Q2 A, Q3 A, Q4 A, Q5 A. Loom takes the one-time compile break from Q2 A at its next
> Substrate pin bump.

The same message gave one input: the host driver writes `cpu.max` as the declared CPU time
spread over the declared wall time, so a program with a large CPU budget and a long timeout
runs at a fraction of a core for its whole life; and, where a CPU-time limit and a wall-time
limit are both true at once, the order of D4 (CPU before wall) "is fine for Loom". Design 36
states the rate in its note on `linux` under D7 and lists a bound for the rate under "Not
decided here". `story:a-cpu-budget-is-declared-apart-from-a-cpu-rate` records it as a draft.

## What this record does

The design's status is accepted with the five choices it was written with. The three stories
it serves may start their code once the design is on `main`.
