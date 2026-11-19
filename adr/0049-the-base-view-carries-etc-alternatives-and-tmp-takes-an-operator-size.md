---
status: accepted
date: 2026-11-18
---

# ADR 0049: the base view carries `/etc/alternatives`, and `/tmp` takes an operator size

Decider: controller, under the operator's standing grant ("everything else can go automatically
and decided by you", 2026-11-17). Decides `story:base-view-carries-etc-alternatives` and
`story:tmp-takes-an-operator-size`, from issues #223 and #224.

## Context

- **The Linux base view binds `/usr`, `/bin`, `/lib` and `/lib64` read-only and no `/etc`**
  (`crates/substrate-host/src/process.rs`, `command`). On a Debian-family host many `/usr/bin`
  names are links into `/etc/alternatives` (`/usr/bin/cc -> /etc/alternatives/cc`), so inside a
  sandbox they dangle: `cc`, `c++`, `awk`, `which` and `vi` are missing, and `cargo`, whose
  default linker is `cc`, links nothing (#223).
- **`/tmp` is `--tmpfs /tmp` at the kernel default, half the RAM** (#224). A build's temporary
  files go there; a full test gate no longer fit. Its pages are charged to the run's memory
  cgroup, so the run's memory bound already caps what it can hold, but nothing lets an operator
  state a smaller or larger size, and the size a sandbox gets is not something the operator
  chose.
- **The sandbox root is bubblewrap's own tmpfs and holds only the mount points it creates.**
  Substrate places nothing a run is meant to write there; a run's home and temporary space belong
  in `/tmp`, the workspace or a writable root, and any tmpfs page a run writes counts against its
  memory bound.

## Decision

1. **The base view binds `/etc/alternatives` read-only, when the host has one**
   (`--ro-bind-try`, as `/bin`, `/lib` and `/lib64`). It holds links into the system paths the
   view already carries; a link into a path the view does not carry still dangles, so it widens
   nothing a run can read beyond the link names themselves. `/etc` otherwise stays absent, and
   `WRITABLE_ROOT_RESERVED_MOUNTS` still refuses a writable root at or beneath `/etc`.
2. **`--tmp-bytes N` (`SUBSTRATE_TMP_BYTES`, `HostConfig::tmp_bytes`) sizes every confined
   run's `/tmp`** with bubblewrap's `--size N` before `--tmpfs /tmp`. Absent keeps today's
   kernel default. The sandbox root gets no size option. Startup refuses, as `config.tmp-bytes-invalid`, a size under one MiB, and,
   as `config.tmp-bytes-unserved`, a bubblewrap whose `--help` does not name `--size`: a size
   the operator asked for is applied or startup stops, never silently dropped (invariant 3).
3. **No wire change.** No route, schema, fact or refusal of the wire contract moves, so no
   bundle is cut. A run sees its size with `df /tmp`. Publishing the size in the capability
   snapshot or the applied record is a wire addition for a successor bundle, and a story of its
   own if a client needs it.
4. **Backing `/tmp` by a host directory is not added.** A writable root at `/tmp` stays refused
   (ADR 0038 decision 4); a run that needs disk-backed temporary space sets `TMPDIR` to a
   writable root or to `/scratch`.

## Consequences

- A Rust build links in a sandbox on a Debian-family host without the client building a folder
  of direct links.
- An operator bounds `/tmp` per daemon. The size is per run, not shared across runs.
- The ESS projections model neither the bubblewrap argv nor daemon flags, so `spec/` does not
  change; `config.tmp-bytes-*` are startup refusals like `config.exec-cpu-cores-invalid`.
