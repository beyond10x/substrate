---
format: aep.planning-md/3
id: review-result:aperture-and-empty-list-acceptance-round-1
kind: review-result
status: active
title: 'Plan critic (acceptance), round 1: aperture dial outcome, resolver aperture, explicitly empty allow-list'
tags:
- plan-critic
relations:
- reviews: story:aperture-reports-dial-outcome
- reviews: story:resolver-aperture-serves-dns
- reviews: story:explicitly-empty-exec-subpaths-means-the-program-only
revision: 1
---
needs-revision

story:resolver-aperture-serves-dns — the acceptance states its before-state as fact ("before this story no run can resolve a name") while the Why says it is not established whether an ordinary TCP aperture to port 53 already resolves for a child whose resolver configuration names it, so the acceptance may already hold; it should name the observable difference the work creates, such as a run that selects the resolver aperture resolving where the same declaration made today as an ordinary aperture does not — .engineering/planning/story/resolver-aperture-serves-dns.md:92 (against :68)

Read: 3 of 3 ids (`aep plan artifact show` on each, plus `kinds` and `lifecycle` not needed). I also checked every path:line the acceptances name against the tree. These are `profile.rs:72` and `:216`, `macos.rs:3-5`, `confinement-public-items.txt:1-3`, `wire lib.rs:1874-1890`, the `macos-15` lane in `.github/workflows/macos.yml`, and `confinement.fact-unserved` in `confinement/mod.rs`. I read the other stories' acceptances only to follow the relations, and I did not read design 35, which is not in the checkout.

Could not establish:
- Out of my lane (scope or design, not acceptance): `story:aperture-reports-dial-outcome` cites `egress.rs:785` as "the forwarder dials the pinned address". That line is `probe_from_inside`'s connect to the loopback listener. The forwarder's dial is `relay_body` at `:1216`. The citation should be corrected.
- The delegated lane in `story:aperture-reports-dial-outcome` and in the resolver story is `runtime_vectors.rs`. In `story:explicitly-empty-exec-subpaths-means-the-program-only` it is presumably `ess_confinement_linux.rs`, via `scripts/delegated-lane.sh`, but that story names neither. I judged this checkable and did not make it a finding.
- I did not count the "Checks" bullets as extra acceptance statements, because each story has one headline acceptance sentence.

```findings
- file: .engineering/planning/story/resolver-aperture-serves-dns.md
  line: 92
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance asserts 'before this story no run can resolve a name' while the Why (line 68) says it is not established whether a TCP aperture to port 53 already resolves for a child configured to use it, so the before-state is unproven and the acceptance may read the same before the work as after it"
```
