---
format: aep.planning-md/3
id: story:sandbox-repository-parity
kind: story
status: draft
title: The sandbox repository's settings are covered by Substrate
owner: substrate
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:oci-adapter
- depends_on: story:confinement-contract-on-linux-and-local
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/b10x-substrate-cli/src
- confidence: inferred
  path: crates/substrate-host/tests/sandbox_parity.rs
- confidence: inferred
  path: spec/client/cli.yaml
- confidence: inferred
  path: spec/client/client.yaml
- confidence: inferred
  path: spec/generated/cli
revision: 2
---
# The sandbox repository's settings are covered by Substrate

ADR 0109 decision 5: `sandbox` is retired into Substrate; its bubblewrap settings are proved covered
by moving its mirror test (`sandbox` `tests/substrate_mirror.rs` at `1b0377d`) into Substrate, and
Atlas's O6 loop then calls Substrate's command line. This story does the Substrate half. Atlas's O6
loop keeps calling the `b10x-sandbox` binary (`atlas` `scripts/o6-loop.sh:67-83`) until its own hop;
archiving `sandbox` is the operator's act.

## Acceptance

- `crates/substrate-host/tests/sandbox_parity.rs` holds the `sandbox` bubblewrap argument list
  (`src/confinement.rs:179-258`: `--unshare-user --disable-userns`, `--unshare-ipc`, `--unshare-pid`,
  `--unshare-uts`, `--unshare-net` unless `--net`, `--die-with-parent`, the read-only `/usr`, `/bin`,
  `/lib`, `/lib64`, `--proc`, `--dev`, `--tmpfs /tmp`, read-only roots, the `/workspace` tmpfs with
  binds and `--remount-ro`, `--chdir`, `--clearenv` and the five variables) as a table, and asserts
  each is present in the `linux` adapter's argv for the equivalent profile, so Substrate's set is a
  superset (it adds `--new-session` and `--seccomp`).
- The same file holds the `sandbox` Docker flags (`src/docker.rs:42-130`) and asserts each is
  present in the `oci` adapter's argv for the equivalent profile.
- `b10x-substrate confine run` takes `sandbox`'s flags (`--dir`, `--ro`, `--net`,
  `--backend linux|oci`, `--image`, `--dry-run`) and runs the command through `confinement::open`;
  `--dry-run` prints the program and argv one per line, and the O6 loop's own check (the recorded
  argv contains `--unshare-net`) holds for it. The CLI's ESS binding (`spec/client/`) and its
  generated contract (`spec/generated/cli`) carry the verb, and `cargo xtask check-ess` passes.
- `--net` under `linux` is admitted only as `network_egress` with no aperture refused, or `local`
  with a reason: never a silent host network (the verb states which one it chose).

## Scope

`crates/substrate-host/tests/sandbox_parity.rs` (new), `crates/b10x-substrate-cli/src/` (the verb),
`spec/client/client.yaml`, `spec/client/cli.yaml`, `spec/generated/cli/`, `CHANGELOG.md`.

## Depends on

`story:oci-adapter` (the Docker half) and `story:confinement-contract-on-linux-and-local`.
