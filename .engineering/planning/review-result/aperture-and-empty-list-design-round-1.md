---
format: aep.planning-md/3
id: review-result:aperture-and-empty-list-design-round-1
kind: review-result
status: active
title: 'Plan critic (design), round 1: aperture dial outcome, resolver aperture, explicitly empty allow-list'
tags:
- plan-critic
relations:
- reviews: story:aperture-reports-dial-outcome
- reviews: story:resolver-aperture-serves-dns
- reviews: story:explicitly-empty-exec-subpaths-means-the-program-only
revision: 1
---
needs-revision

story:aperture-reports-dial-outcome — the claim "the forwarder dials the pinned address itself" cites `egress.rs:785`, which is `probe_from_inside`, the install probe's connect to loopback inside the namespace, and only `:1216` (`relay_body`) is the forwarder's dial; the cite is wrong and lets a reader think the probe is a counting site — .engineering/planning/story/aperture-reports-dial-outcome.md:66, crates/substrate-host/src/egress.rs:758-785
story:aperture-reports-dial-outcome — the timed-out check and the "No change to which destinations a run may reach" boundary conflict, because `relay_body`'s `connect` at `:1216` is a blocking connect with no deadline, so a timeout outcome needs a new dial deadline that can refuse a slow destination that connects today, and the body never says that deadline is new behaviour — .engineering/planning/story/aperture-reports-dial-outcome.md:98, crates/substrate-host/src/egress.rs:1216
story:resolver-aperture-serves-dns — the scope cites `egress.rs:785` as relevant to serving the relay, but that line is the install probe's loopback connect, not the relay or forwarder path (`:1216` is), so the second cite supports nothing the body says — .engineering/planning/story/resolver-aperture-serves-dns.md:121, crates/substrate-host/src/egress.rs:785
story:explicitly-empty-exec-subpaths-means-the-program-only — it splits one three-state abstraction with `story:exec-start-carries-executable-lists`: its Boundaries hand the wire's absent-versus-empty decision to that story, but that story's body never mentions empty or absent beyond the portable-lane lines, and no edge links the two, so an exec-start-carries that lands first maps the wire list onto the `Vec<String>` this story then retypes (and daemon `process.rs`, the SDK and `probe.rs` all change again); add `depends_on story:exec-start-carries-executable-lists` and have that story's body state the absent-versus-empty decision, or fold the field-type change into it — .engineering/planning/story/explicitly-empty-exec-subpaths-means-the-program-only.md:94, `aep plan artifact graph`

What I read: 3 drafts plus 4 neighbours (`story:executable-deny-list`, `story:linux-executable-allow-list`, `story:exec-file-change-record-on-the-wire`, `story:exec-start-carries-executable-lists`), the approval record that decided design 35 Q2, and `aep plan artifact relations`, `graph`, `waves` and `validate` (`valid`). I walked about 12 declared edges, many outside the set, and found no cycle. I traced 5 acceptance reads to a producer in the set. Two of them, `story:aperture-reports-dial-outcome` after `story:exec-file-change-record-on-the-wire` and `story:resolver-aperture-serves-dns` after `story:aperture-reports-dial-outcome`, are shared-surface order edges. The reason is written beside each (bundle files, `egress.rs`, `scripts/gate.sh`), so I did not count them as a serialising chain. The third story's read of linux's `exec_subpaths` reaches `story:linux-executable-allow-list` only through `story:executable-deny-list`, which is recorded.

What I could not establish:
- `docs/design/35-executable-lists.md` is absent from this checkout, so I could not check the claim that design 35 as decided keeps an empty list running as today on linux.
- Whether `story:process-count-limit-hit-is-observed` has an edge to `story:exec-start-carries-executable-lists` for bundle order is not mine, and the bundle numbers of the stories before `story:aperture-reports-dial-outcome` look unrecorded. That belongs to parallel safety.
- Whether the dial-outcome acceptance can be checked with no listener, or with a timeout case, belongs to the acceptance critic.
- Line numbers in `crates/substrate-wire/src/lib.rs` for `AppliedAperture` are off by about 10 in this older checkout. The claim holds.

```findings
- file: .engineering/planning/story/aperture-reports-dial-outcome.md
  line: 66
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the claim that the forwarder dials the pinned address cites egress.rs:785, which is probe_from_inside (the install probe's loopback connect inside the namespace); only :1216 (relay_body) is the forwarder's dial, so the cite is corrected"
- file: .engineering/planning/story/aperture-reports-dial-outcome.md
  line: 98
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the timed-out check presumes a dial deadline, but relay_body's connect at egress.rs:1216 has none, so a timeout outcome adds a new deadline that can refuse a slow destination that connects today and conflicts with the 'no change to which destinations a run may reach' boundary; the body states that the deadline is new and reconciles the boundary"
- file: .engineering/planning/story/resolver-aperture-serves-dns.md
  line: 121
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the scope cites egress.rs:785 as the relay path, but that line is the install probe's loopback connect; only :1216 is the relay's dial, so the cite is dropped or corrected"
- file: .engineering/planning/story/explicitly-empty-exec-subpaths-means-the-program-only.md
  line: 94
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the absent-versus-empty abstraction is split with story:exec-start-carries-executable-lists: this body leaves the wire half to that story, whose body states no absent-versus-empty decision, and no depends_on edge links them, so a wire field mapped onto Vec<String> is retyped later; add depends_on story:exec-start-carries-executable-lists and have that story state the decision, or fold the field-type change into it"
```
