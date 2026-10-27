---
format: aep.planning-md/3
id: review-result:adr-0109-fu12-14-adversary-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: confine run reaping and public items over field types'
relations:
- reviews: story:confine-run-reaps-killed-groups
- reviews: story:public-items-cover-field-types
revision: 1
---
# Adversary pass 1: confine run reaping and public items over field types

Verdict: needs change. Read at `d19411b8` (units `0cb24ff5` and `d19411b8`); cases on branch
`review/adr-0109-fu-host-adversary` at `e318f41a`. `confinement_public_items` executed 14 to 18,
`confine` (delegated) 16 to 18, 5 red.

## Findings

- R1 `crates/b10x-substrate-cli/src/confine/cgroup.rs:198` (warning, introduced): a SIGKILL'd
  invocation its parent has not reaped is a zombie whose `/proc/<pid>/stat` still matches, so its
  empty group is left. Red: `adversary_a_sigkilled_runs_group_is_removed_while_its_invocation_is_unreaped`.
- R2 `cgroup.rs:198` (warning, introduced): no test held the owner check; removing it left the
  story's case green. New case `adversary_a_live_owners_empty_group_is_left_by_the_next_run` is red
  on that mutant.
- F1 `crates/substrate-host/tests/confinement_public_items.rs:798` (warning, pre-existing): impl
  blocks of module types were read only inside `src/confinement/`. Red:
  `adversary_a_method_added_to_a_module_type_outside_the_module_changes_the_list`.
- F2 `:1031` (warning, introduced): a wire type sharing a name with a module type
  (`ConfinementProfile`) was never held. Red:
  `adversary_a_wire_type_sharing_a_module_type_name_is_held_when_reached`.
- F3 `:989` (warning, introduced): types published through a renaming `pub use … as` were not
  indexed. Red: `adversary_a_type_reached_through_a_renaming_reexport_is_held`.
- F4 `:449` (note, pre-existing): associated types of trait impls on held types were not recorded.
  Red: `adversary_an_associated_type_of_a_trait_impl_on_a_held_type_changes_the_list`.
- R3 `cgroup.rs:187` (note, infeasible): a SIGKILL before bubblewrap sets its parent-death signal
  leaves the sandbox running; its group outlives the next invocation because the reaper never kills.
- R4 `cgroup.rs:161` (note, infeasible): same as security F3.

## Outcomes

- R1 fixed in `bfdf7873`: stat state `Z` or `X` counts as gone.
- R2: the case is kept and its `executed` line is required by `scripts/delegated-lane.sh`.
- F1 to F4 fixed in `bfdf7873` (the scanner reads impls in every host file, keeps wire paths,
  follows renaming re-exports and records trait-impl associated items); the committed list
  re-blesses with no diff. `bfdf7873`'s message omits these and the CHANGELOG; merge `979e5c8a`
  names them.
- R3 and R4: accepted as notes; the reaper never kills.
- After the fixes: `confine` delegated `ok. 21 passed`, `confinement_public_items` `ok. 18
  passed`. Merged on the follow-up branch as `979e5c8a`.
