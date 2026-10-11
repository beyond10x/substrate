---
status: accepted
date: 2026-10-11
---

# ADR 0033: a single host file is admitted read-only

## Context

[ADR 0010](0010-declared-host-roots-are-mounted-read-only.md) admits declared host **directories**
read-only. A root whose host path is not an existing directory is refused, so a caller that needs
one program, such as a single binary in a directory that holds many others, or one configuration
file has two options. It can bind the whole directory around the file, which admits every sibling
the caller never meant to expose. Or it can copy the file into the workspace, which makes it
writable and turns a host input into workspace state.

Neither option is the request. The request is one file, readable, at one path, and nothing beside
it.

## Decision

Add an optional, bounded list of **declared read-only files** to exec and raw-pipe start, beside
`read_only_roots`. Roots keep their exact meaning and validation. The field rides successor bundle
`0.18.0`, which is not yet released, with
[ADR 0032](0032-exec-usage-without-block-io.md).

```json
"read_only_files": [{ "host_path": "/opt/gates/bin/gate", "mount": "/opt/gates/bin/gate" }]
```

Both halves are the caller's. As with a root, substrate knows nothing about what the file is.

### Validation refuses rather than adjusts

Every rule is checked before dispatch. A refused file refuses the start, and no file is silently
dropped, re-pointed or made writable. The lexical rules come first, because they are the caller's
mistake and depend on no host. The daemon checks them before the confinement capability, so both
the portable and the delegated lanes observe them. The driver checks them again independently.

| Rule | Refusal | Status |
|---|---|---|
| More than `MAX_READ_ONLY_FILES` entries | `exec.read-only-files-exceeded` | 422 refused |
| `host_path` or `mount` is not absolute and canonical (no `.`, `..`, `//`, trailing `/` or NUL), is longer than `MAX_READ_ONLY_FILE_PATH_BYTES` (4000) bytes, has a component longer than `MAX_READ_ONLY_FILE_COMPONENT_BYTES` (255) bytes, or is deeper than `MAX_PATH_DEPTH` (64) components | `exec.read-only-file-path-invalid` | 422 refused |
| `mount` equals, lies beneath or is a parent of a mount substrate owns: `/`, `/usr`, `/bin`, `/lib`, `/lib64`, `/proc`, `/dev`, `/tmp`, `/workspace`, the capsule's `/runtime`, the scratch `/scratch`, and the aperture's `/etc/hosts` and CA bundle | `exec.read-only-file-mount-reserved` | 422 refused |
| `mount` equals, lies beneath or is a parent of a declared root's mount, or another file's mount lies beneath it | `exec.read-only-file-mount-overlap` | 422 refused |
| Two files name the same `mount` | `exec.read-only-file-mount-duplicate` | 422 refused |
| A start declares files and the snapshot publishes no `exec.read-only-files` fact | `exec.read-only-files-unserved` | 501 unserved |
| `host_path` is not present | `exec.read-only-file-absent` | 422 refused |
| `host_path` is reached through a symlink, in its last component or any other | `exec.read-only-file-symlink` | 422 refused |
| `host_path` is not a regular file (a directory, a device, a socket or a FIFO) | `exec.read-only-file-not-a-file` | 422 refused |

The last three are filesystem questions only the driver can answer. Substrate does not answer them
with a path check that a concurrent rename could invalidate. The driver opens `host_path` once with
`openat2(O_PATH | O_NOFOLLOW, RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS)`, so every symlink
component is refused by the kernel, not by a string comparison. It then checks that the opened
object is a regular file and keeps that descriptor. Bubblewrap mounts the descriptor itself with
`--ro-bind-fd`, the same descriptor bind [ADR 0023](0023-workspace-write-access-is-explicit.md)
now uses for its workspace mounts. The file that is mounted is therefore the object admission
inspected, and a rename or symlink swap after admission cannot redirect it: bubblewrap also
checks that the mount it made has the descriptor's device and inode, and refuses to start
otherwise. Roots are checked by path today; this decision
does not change them.

A directory on the way that the daemon may not search (`EACCES`) is refused as
`exec.read-only-file-absent`, with a message that says the path is not reachable rather than
missing: the daemon cannot observe the object, so for this host it is not there. No tenth code is
added for it. `ENAMETOOLONG` cannot occur: the lexical bounds keep every path and component below
the kernel's `PATH_MAX` and `NAME_MAX`.

### No pinned descriptor reaches the program

The pinned descriptors must be inherited by bubblewrap, which mounts them, and by nothing after it.
Bubblewrap's descriptor binds close each descriptor once its mount is made, before the program
starts. A bind of the path `/proc/self/fd/<n>` does not: bubblewrap does not close a descriptor it
was handed that way, and the confined program inherits it. A descriptor kept open in the program
defeats the read-only mount. Reopening `/proc/self/fd/<n>` for writing resolves to the object on
the host's writable mount, not to the read-only bind, so the program could rewrite the host file.
The first cut of this decision bound by path and had exactly that defect. The same retention
reached the program from ADR 0023's read-only and scoped workspace roots since that decision
shipped: a read-only workspace could be written through its inherited root descriptor. Both now
use descriptor binds. The pinned workspace descriptors are also moved above `MAX_SECRET_SLOT_FD`.

The descriptors the confined program legitimately holds are its standard input, output and error,
and the secret slots it named ([ADR 0012](0012-secret-slots-are-sealed-memfds.md)) at
their declared descriptors. Bubblewrap consumes and closes its seccomp, launch-barrier and info
descriptors before the program starts. Those three, like the pinned descriptors, are moved above
`MAX_SECRET_SLOT_FD` and are close-on-exec in the daemon; only the bubblewrap child clears the flag,
so no other spawn inherits them and no slot placement can replace one. A start in which any
descriptor bubblewrap inherits lies in the slot range is refused before the spawn. Every other
descriptor is closed by `close_range` before bubblewrap runs.

### The path bounds

A declared path is at most `MAX_READ_ONLY_FILE_PATH_BYTES`, **4000** bytes, with no component over
`MAX_READ_ONLY_FILE_COMPONENT_BYTES`, **255** bytes, and no more than `MAX_PATH_DEPTH`, **64**,
components. The kernel's `PATH_MAX` is 4096 bytes including the terminator, and bubblewrap prefixes
a mount with `/newroot` while it builds the sandbox root. Measured with bubblewrap 0.12.0, a
4087-byte mount binds and a 4088-byte mount fails. 4000 bytes keeps the admitted path clear of both
limits, so it can never fail as a machinery error. The bundle's `host-absolute-path` definition
states the same numbers: `maxLength` 4000, and a pattern with at most 64 components of at most 255
characters each. JSON Schema counts characters, so for a non-ASCII path the wire crate's byte bounds
are the stricter check. The `0.18.0` additions check pins the schema's numbers to the constants.

Pinned descriptors are moved above `MAX_SECRET_SLOT_FD`, as are bubblewrap's own seccomp,
launch-barrier and info descriptors, so placing a secret slot at its declared descriptor can never
replace one, and every slot recorded as placed reaches the program at its declared number. A
pinned descriptor that cannot be moved is the machinery failure
`exec.read-only-file-pin-failed` (500, retriable), not a refusal, because the request was
admissible.

### The bound

`MAX_READ_ONLY_FILES` is **16**. A gate run needs a handful of named programs and their
configuration files. Sixteen leaves room for that without letting a caller rebuild a directory
file by file: a closure that needs more is a root, or a capsule. The bound also limits what one
start adds to the confinement command: 16 pinned descriptors and 48 bubblewrap arguments. The
capability snapshot publishes it as `exec.read-only-files: { "max_files": 16 }`.

### The mount and the observation

The mount is `--ro-bind-fd` of the single file, after the declared roots and before the workspace.
Bubblewrap creates the mount point and its missing parent directories in the sandbox's own root, so
nothing else from the host directory becomes visible: siblings stay absent. The file keeps its host
mode bits, so a host program with its executable bits set runs when its mount's directory is on the
`PATH` the caller sets. The bind is read-only, so a write through the mount fails, and the
workspace remains the only writable path.

The applied confinement observation lists every file that was bound, with its host path and mount,
just as it lists roots. What substrate cannot verify must at least be visible. Like a root, a
declared file is trusted by the caller and its bytes are not verified by substrate.

The fact is published only after a startup probe pinned and bound a regular file exactly as the
driver does, in a throwaway sandbox, executed it, failed to write it, found no sibling, and found no
descriptor in the program that resolves to it. The probe also writes through every descriptor the
program holds and requires the host file unchanged. The scoped workspace probe of ADR 0023 now pins
and binds through the same driver code and makes the same two checks for the workspace root and
its subtree. It is never published from a constant. Without it, every declared file is `exec.read-only-files-unserved`
(invariant 3).

## Affected surface

- **Capabilities.** New optional fact `exec.read-only-files` with one member, `max_files`. The
  snapshot digest moves wherever the fact is published.
- **Inputs.** Optional `read_only_files` on exec start and, through its embedded exec input, on
  session start. Absent means none. An empty list is skipped on serialization, so the canonical
  request hash and ledger bytes of every existing start are unchanged.
- **Refusals.** The nine codes in the table above, all before dispatch. No existing code changes.
- **Observations.** `applied.read_only_files` on the exec and session resource. It is absent when
  none was declared.
- **Events and routes.** No event kind, route, ledger format or persisted row shape changes.
  `0.18.0` keeps all 36 of `0.17.0`'s routes.

## Compatibility

The change is additive and optional. Every request that was valid under `0.17.0` is valid and
hashes identically under `0.18.0`, and every resource without declared files is byte-identical. A
consumer that validates resources against `0.18.0` accepts the new observation member. Harness,
the one in-collection client, pins the wire crate by Git revision and moves at its next pin. It is
not changed here. Every released bundle directory stays byte-for-byte frozen.

## Verification

Wire unit tests cover each lexical refusal. Host tests cover a directory, a symlinked path, a
symlinked parent component and an absent path. They also cover the delegated journey: a host
program bound at a mount on `PATH` runs, writing it fails, and its host sibling is absent.
Adversarial host cases write through every descriptor the program holds, for a declared file, a
read-only workspace and a scoped workspace, and require the host bytes unchanged and no descriptor
resolving to them. A host unit case shows the probe's descriptor check refusing a path bind and
passing a descriptor bind. Wire cases cover NUL and both byte bounds and the depth bound. The
runtime vectors drive the lexical refusals over the daemon's socket in both lanes, the unserved
refusal in the portable lane, and the filesystem refusals and the journey in the delegated lane.
The rendered `0.18.0` bundle is checked by `cargo xtask check-bundles`. Its additions check pins
the input field, the fact, the observation and the published refusal set to the wire crate.
Filesystem confinement stays outside the ESS accepted-operation projection (`spec/coverage.md`), so
no specification model changes.
