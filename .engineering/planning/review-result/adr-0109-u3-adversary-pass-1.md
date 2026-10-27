---
format: aep.planning-md/3
id: review-result:adr-0109-u3-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1 on story:confinement-contract-on-linux-and-local
relations:
- reviews: story:confinement-contract-on-linux-and-local
revision: 1
---
# Adversary pass 1: story:confinement-contract-on-linux-and-local

Verdict: needs change. Read at the unit head 15f43d6; 10 cases added in three test binaries,
8 red.

## Findings

1. `crates/substrate-host/src/confinement/local.rs:368` (warning): once the leader has exited,
   `signal` returns Ok without signalling the process group; a background child survives KILL and
   `wait` never returns.
2. `confinement/mod.rs:399`, `:682` (warning): refusals at start (`adapter-unserved`,
   `family-unserved process_run`, `workspace-required`) write no `profile_refused` audit record.
3. `confinement/linux.rs:149` (warning): `filesystem_view` is served without
   `exec.writable-roots.host`; a profile with writable roots, or secret slots, is admitted and
   refused only at start.
4. `confinement/mod.rs:483` (warning): `decide` admits a profile without `inner` on an adapter
   opened with one, where the model refuses `adapter-unserved`.
5. `confinement/profile.rs:161` (note): a reason of only zero-width or BOM characters is admitted.
6. `confinement/profile.rs:169` (note): `image_is_pinned` accepts two `@sha256:` digests and does
   not check the name part.
7. `confinement/local.rs:200` (note): `local` follows a symlinked workdir out of the workspace
   where `linux` refuses `exec.workdir-missing`.
8. `tests/ess_confinement_local.rs:68`, `ess_confinement_linux.rs:79` (warning): the scenario count
   is compared with the selection that produced it, so neither lane pins its count.

## Checked and not broken

Local policy values other than unset or exactly `permit` fail closed; refusal order matches the
model apart from finding 4; workdir `..`, `.`, absolute and empty are refused; linux workspace
escapes are refused; on an undelegated host `limits` is refused; `render` and `start` produce the
same bwrap argv with descriptors masked (delegated case); reserved fields are refused first.
