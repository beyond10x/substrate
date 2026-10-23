---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-b9-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit b9 (project-quota lane runner)
relations:
- reviews: story:project-quota-lane-has-a-provisioned-runner
revision: 1
---
# Adversary pass 1, unit b9 (project-quota lane runner)

Agent type `aep:adversary`. Branch `unit/2026-10-23-b9-quota-runner`, commit `62736d2`. Cases in `xtask/tests/quota_fixture_adversary_pass1.rs`; the fixture cases drive a fake `docker` on `PATH`. Verdict: NEEDS-CHANGE, 6 red of 8.

```findings
- file: xtask/src/quota_fixture.rs
  line: 150
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: HEAD and cleanliness are sampled once before a minutes-long run on a live checkout, so a tree edited during the run still gets a Quota-lane record for HEAD.
- file: xtask/src/quota_fixture.rs
  line: 41
  category: concurrency
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: All checkouts share one target volume at the same /src path, so cargo's mtime freshness can run another checkout's binaries under this checkout's record.
- file: .github/workflows/release.yml
  line: 438
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: A multi-line quota_evidence dispatch input validates only its first line and appends a second unvalidated evidence= line to GITHUB_OUTPUT.
- file: .github/workflows/release.yml
  line: 385
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: A tag with two disagreeing Quota-lane records is accepted on the first one.
- file: .github/workflows/release.yml
  line: 431
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: The RFC 3339 check is a shape regex that accepts month 13 and 25:61:61.
- file: .github/workflows/release.yml
  line: 420
  category: contract-drift
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: The release counts fn real_quota_* lines while the fixture counts ignored functions, so the two can disagree on one source.
```

Red output: adv1 two disagreeing records accepted; adv2 `2026-13-45T25:61:61Z` accepted; adv3 two `evidence=` lines written to GITHUB_OUTPUT; adv4 `records 7 real-quota cases; … declares 8.`; adv7 a record printed for a checkout dirty at the end of the run; adv8 both checkouts mount `b10x-substrate-quota-lab-target`. File alone: 2 passed, 6 failed.

Held: abbreviated or upper-case sha, trailing CR, `+00:00` offset and a padded count refused; lightweight tag refused at release.yml:181; dispatch input reaches the shell only through an environment variable; the count is read from the tagged commit; the step cannot be skipped; `verdict` refuses a missing summary, a failed case, a short run and a non-zero exit.

Findings 3 to 5 also exist, pre-existing, in the Confinement-lane step (release.yml:313, :358, :365).

Coordinator decision: fix all six; run the fixture from a snapshot of HEAD with a per-source volume; apply the newline, duplicate-record and real-instant checks to the Confinement-lane step as well.
