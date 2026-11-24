---
format: aep.planning-md/3
id: review-result:git-create-limits-adversary-6
kind: review-result
status: active
title: 'Confirming adversary pass 6: the admitted keys of a cargo configuration'
relations:
- reviews: story:git-create-work-and-memory-follow-its-limits
revision: 1
---
# Confirming adversary pass 6: the admitted keys of a cargo configuration

2026-11-24, on the commit `00762067a` of the branch `fix/check-packages-closed-patch-rule`. The
adversary added three cases in the test module of `xtask/src/packages.rs` and did not edit the
implementation. Each input was confirmed against cargo 1.97.1 on offline probe workspaces
(`cargo metadata --offline`). Verdict: three cases red, all in the one exception of the rule,
the fixture workspace's configuration; none in the list of admitted keys. `packages` cases
executed 53 to 56, red 3. S1: 0. S2: 4. S3: 0.

The three red inputs are hand-built manifests. No file of the repository has one.

## Held

- `-C` on a command line and in an alias, string and array form: cargo 1.97.1 refuses the flag
  as unstable, and in an alias reads no other configuration.
- `-Z` in an alias: refused on the stable channel.
- An alias with `--manifest-path` onto the fixture's manifest builds the fixture workspace,
  which is no build of the root.
- `profile.dev.rustflags`: refused, the feature is not stable in cargo 1.97.1.

Not established by the pass: a byte-order mark, or a file cargo and the `toml` crate parse
differently (the check fails closed on a parse error); a FIFO or device named `config`; a
`.cargo` reached through a symbolic link to a directory outside the walk; quoted or dotted
spellings of `alias` and `profile`; other spellings of the root manifest's patch table.

## Findings and what was done

1. S2. Rule asked: a root member beneath the fixture directory voids the exception under every
   spelling cargo reads as that directory. Two cases, one with a `.` component and one with a
   doubled separator.
2. S2. Rule asked: a package beneath the fixture directory that a member takes by path is a
   member of the root workspace, and voids the exception. One case.
3. S2, judgement, no case. Rule asked: a build of the root started beneath the fixture
   directory reads no source replacement from a file of the repository.

Done in the branch's next commit, on a decision of the session that works this repository,
reported to the operator's session: the exception is removed. The fixture's configuration is
no file of the repository; the two macOS test files that run the fixture write it into the
copy each run gets. Every cargo configuration beneath the root holds only `alias` and
`profile`. The reason: the exception's condition was incomplete in passes 5 and 6, and finding 3
holds for any condition on the manifests.

Runs of the pass, release profile, in the build pool: the `packages` cases 56 run, 53 passed,
3 failed (the three cases).
