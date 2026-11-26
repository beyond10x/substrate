---
format: aep.planning-md/3
id: approval-record:consumer-answers-adr-0051-questions-2026-11-26
kind: approval-record
status: approved
title: The consumer answers the five new questions of ADR 0051
relations:
- decides: story:exec-file-changes-are-recorded
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-26T08:16:56Z", actor: "human:operator", revision: 2}
---
# The consumer answers the five new questions of ADR 0051

On 2026-11-26 the consumer of `story:exec-file-changes-are-recorded` answered the five questions
ADR 0051 added to the eighteen it answered on 2026-11-22:

> all defaults (D105 A, D106 A, D107 A, D108 A, D109 A). Reasons from [the consumer's] side: A
> for D105 lets the run record say why a change list is missing; D106 A keeps a read-only run
> admitted ([the consumer] must not fail a delegation for asking); D108 A matches [the
> consumer's] journal (RFC 3339 UTC); D109 A keeps run records small, and the coming
> decision-branches epic reads content per file on demand.

| ADR 0051 | decision | answer |
|---|---|---|
| Q1 (D105) | the audit record of an unavailable record | A: two variants, `file_changes_recorded` and `file_changes_unavailable` with the reason |
| Q2 (D106) | a run that asks but has no covered mount | A: admitted; an available record with `mounts: []` and no change |
| Q3 (D107) | the reason's spelling in the record | A: the closed-list word; the full code only where surfaced as an error |
| Q4 (D108) | the form of `fs_mtime`, `started_at`, `ended_at` | A: RFC 3339 UTC with nanoseconds |
| Q5 (D109) | how content is read | A: one call per side; the record carries no bytes |
