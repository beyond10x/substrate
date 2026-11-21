---
format: aep.planning-md/3
id: review-result:aperture-and-empty-list-acceptance-round-2
kind: review-result
status: active
title: 'Plan critic (acceptance), round 2: aperture dial outcome, resolver aperture, explicitly empty allow-list'
tags:
- plan-critic
relations:
- reviews: story:aperture-reports-dial-outcome
- reviews: story:resolver-aperture-serves-dns
- reviews: story:explicitly-empty-exec-subpaths-means-the-program-only
revision: 1
---
approve

I read the three stories whole and found nothing that would change what was drafted. Each acceptance is observable, and each names a before state and an after state.

- **`story:aperture-reports-dial-outcome`**: the acceptance is a delegated-lane exec with one refused dial. It names the before state ("on the predecessor bundle the record has no such member"). I checked the cited code. `AppliedAperture` has no dial member (`crates/substrate-wire/src/lib.rs:1874-1890`), and `relay_body` calls a blocking `connect` with no deadline (`crates/substrate-host/src/egress.rs:1204-1216`).
- **`story:resolver-aperture-serves-dns`**: the acceptance is `getent hosts <name>` succeeding in a run that selects the aperture, where it fails today. `git grep` finds no `resolv.conf` generation in `crates/substrate-host/src`, and `README.md:436` records that the sandbox gets no resolver. The before state therefore holds. The ADR's open decisions on UDP, a name list and a new aperture kind are left to the ADR, as the brief allows.
- **`story:explicitly-empty-exec-subpaths-means-the-program-only`**: the acceptance is an explicitly empty list on `linux` running its literal program, with an `execve` of any other path failing, where the same profile runs unrestricted today. `profile.rs:72` and `:216` confirm that `Vec<String>` cannot tell absent from empty. The `macos.rs:3-5` citation also checks out.

What I read: 3 of 3 given ids, with `aep plan artifact show` on each. I also checked the cited lines in `crates/substrate-wire/src/lib.rs`, `egress.rs`, `profile.rs` and `macos.rs`, and ran `aep plan artifact list` to confirm the neighbouring stories' status.

What I could not establish:
- `docs/design/35-executable-lists.md` is not in this checkout, so I could not check how the third story's "design 35 as decided" compares with the story's before state. The `profile.rs` and `macos.rs` citations are consistent with the story.
- Out of my lane, for `plan-critic-design`: the third story's acceptance needs `linux` to enforce an executable allow-list. That comes from `story:linux-executable-allow-list` (active), which is not in the story's `depends_on`. The path may run through `story:executable-deny-list`.
- Out of my lane, for `plan-critic-design`: the second story's checks assume a capability fact and a named refusal, which are wire-visible. Its step 2 makes the bundle conditional ("if the ADR adds a wire member"), and its checks omit the bundle checks that the first story lists.

```findings
[]
```
