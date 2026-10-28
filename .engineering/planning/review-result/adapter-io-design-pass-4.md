---
format: aep.planning-md/3
id: review-result:adapter-io-design-pass-4
kind: review-result
status: active
title: Independent review of design 32, pass 4
relations:
- reviews: story:adapter-stdin-pipe
revision: 1
---
# Independent review of design 32, pass 4

Reviewed at PR 189 head 53fab50f, 2026-10-28. Verdict: needs-revision. H1-H6 answers hold against
process.rs, local.rs, linux.rs, site_drift.rs and the lane pins (49 -> 57 -> 65).

- J1 (blocking, design 32 § 6, D9, § 13 H3; adr/0044:50-56; design 29:159-161): reading ADR 0044
  decision 6 as not binding stories that edit existing adapters is unrecorded; design 29 reads it
  as binding. Record the reading, clarify the ADR, or keep the spec edit with a separate author.
- J2 (note, design 32 § 3): the local queue close on Kill names no mechanism; Receiver::close
  needs the receiver mutably, so it must be a sending-side flag.
