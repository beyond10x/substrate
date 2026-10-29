---
format: aep.planning-md/3
id: review-result:execution-profile-v2-security-pass-1
kind: review-result
status: active
title: Security review of execution profile v2, pass 1 (nothing found)
relations:
- reviews: story:execution-profile-v2-admits-atime-remounts
revision: 1
---
# Security review of execution profile v2, pass 1

Reviewed `unit/execution-profile-v2` at `18112f07` (diff `be6e0c46..18112f07`), 2026-10-29.
Verdict: nothing found. Findings: none.

## Checked and cleared

- v1 and v2 differ in three lines: the profile declaration and the two self-peers renamed, and
  `noatime,nodiratime` appended to `remount options in (...) /**`
  (`deploy/apparmor/substrate-host-execution-v2:33`). v1 is byte-identical to base; its SHA-256 is
  still `e3ff9b10…843a`.
- Both files compile with `apparmor_parser --skip-kernel-load`, exit 0 each.
- The diff test models `options=` as an exact set, `options in` as any subset, and no options as any
  flags, as `apparmor.d(5)` describes. An unknown option or path panics.
- Its text half (`container_profile_remount.rs:293`) compares every line and admits one changed line.
- 15 mutant v2 files in a scratch copy each failed the combined test. The text half caught all 15;
  the semantic half alone missed `audit remount,`, `allow remount,` and a bare `mount,`.
- Profile names: v1 and v2 have disjoint prefixes (`container_profile_name.rs:84`); a suffix is
  `[a-z0-9-]` and cannot inject into the profile text. `verify_file` compares against the chosen
  version's own rendering.
- Upgrade: the installer has no remove path and never overwrites; v2 is its own file, so a loaded v1
  stays loaded. A pinned v1 name renders the v1 bytes it always did.

## Tests run

`substrate-container-profiles` 18 and `substrate-container-exec` 13, all pass; xtask filter
`profile` 16 pass.

## Not verified

The kernel's own remount decision under v2 here; the chart lane at `deb154c8` showed it in a
kind node (4 of 4 scenarios). A default upgrade can start the daemon pod before the DaemonSet has
loaded v2: an availability question, not a safety one.
