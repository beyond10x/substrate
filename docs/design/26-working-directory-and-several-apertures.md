# Working directory and several apertures per run

Status: accepted for implementation, 2026-10-22. Owned by `story:exec-working-directory` and
`story:several-apertures-per-run`; carried by successor bundle `0.21.0`. Read-only roots bound by
descriptor (`story:read-only-roots-bound-by-descriptor`) changes no wire byte and is recorded in
§ 3 only so the three changes of the wave are read together.

Invariant 8 asks for a design before a contract or capability change. Neither change needs an ADR:
design 04 says the working directory is "a substrate-owned workspace address" and this keeps it one;
ADR 0013 lets a request "reference an aperture by name" and fixes no count, and design 10 § 9
decision 5 limited the *first slice* to one. A writable mount outside `/workspace` does need one
(ADR 0023, ADR 0010, ADR 0033 each say the workspace is the only writable path); it is proposed as
`architecture-decision-record:writable-host-roots-are-operator-declared` and is not built here.

## Evidence

Loom 0.33.0 (`crates/loom-intake-slice/src/confinement.rs`, C:966, C:974-976) gives a child one
aperture and `/workspace` as its working directory because Substrate offers no more. takt's
bot-route push mints an installation token at `api.github.com` and pushes to `github.com`: two
hosts in one run (takt `dependency-blocker:loom-child-confinement`, "Against Loom 0.33.0").

## 1. The working directory

`ExecStartInput` gains an optional `workdir`. Absent, the child starts in `/workspace`, exactly as
before, and the start hashes as it always did (`skip_serializing_if`). Because `PipeSessionStartInput`
embeds `ExecStartInput`, sessions gain it with the same rule.

Present, it is an absolute, lexically normal path (no `.` or `..` component, no empty component, no
trailing `/`, at most `MAX_WORKDIR_BYTES` = 4096 bytes, no NUL) that lies at or beneath one of the
mounts Substrate itself places for this start:

| mount | writable |
|---|---|
| `/workspace` | as `workspace_access` grants |
| a declared `read_only_roots[].mount` | no |
| `/scratch` (`EXEC_SCRATCH_MOUNT`) when `scratch` is set | yes |
| `/runtime` when a capsule is set | no |

Anything else, `/tmp` and `/usr` included, is refused before dispatch. The directory must exist in
the sandbox once every mount is in place; a missing directory, or one that is a symlink at any
component, is refused by bubblewrap's `--chdir` failing, which the host reports as the named
refusal below rather than a generic launch failure. Lexical checks run in `substrate-wire` so the
daemon refuses before the ledger dispatches (invariant 5).

| condition | code | status |
|---|---|---|
| not absolute, not normal, too long, NUL | `exec.workdir-invalid` | 422 |
| outside every mount of § 1 | `exec.workdir-outside-mounts` | 422 |
| does not exist or is not a directory in the sandbox | `exec.workdir-missing` | 422 |

The fact `exec.workdir` (boolean) is published on every host whose driver applies it; a portable
host without bubblewrap leaves it absent and refuses a start carrying `workdir` as
`exec.workdir-unserved` (501), never ignoring the field (invariant 3). The applied confinement
reports `workdir` when the start set one.

`PWD` handling is unchanged: bubblewrap's injected `PWD` is still removed; a request that sets
`PWD` in `env.set` still gets exactly that value.

## 2. Several apertures per run

`ConfinementRequest` gains `apertures`, a list of aperture names, beside the existing `aperture`.

- `aperture` keeps its exact meaning, and a start that uses it serializes and hashes as before.
- `apertures` requires `network: "aperture"` and at least two names; one name is written as
  `aperture`. At most `MAX_EGRESS_APERTURES` (4) names, each distinct, each passing the same
  per-name rule `validate_aperture_request` applies today (name syntax, no destination, no ceiling).
- Naming both `aperture` and `apertures` is refused.

Mechanism. Each aperture keeps its own forwarder, byte counters and declared ceiling (ADR 0014
applies per aperture, per run). The forwarders listen on distinct loopback addresses in the run's
namespace: the first listed name on `127.0.0.1` (`APERTURE_LOOPBACK_ADDRESS`), the n-th on
`127.0.0.n`. The generated `/etc/hosts` maps each declared host name to its own address. Two
apertures may share a port because they do not share an address. An aperture whose declared host
is a literal address has no name to map; the child reaches it at its listen address, which the
applied record states.

Two listed apertures whose declared hosts are the same name cannot both be reached: the generated
`/etc/hosts` would map that name twice and the child's resolver answers with the first line, so the
second aperture would be installed and unreachable by its name. The host refuses such a list as
`exec.aperture-host-listed-twice` before anything is installed. Host names compare ASCII
case-insensitively and without a trailing `.`, as a resolver compares them. The generated file opens
with `127.0.0.1 localhost`, the first listen address, so `localhost` is already one of its names: an
aperture declaring `localhost` may be listed first, where its line and its listen address agree,
and anywhere else is refused by the same code, because it would be answered by the first
aperture's forwarder and its bytes would reach the first aperture's destination. The generated
mapping holds the rule again where it is written and refuses such a set as
`exec.aperture-install-failed`. A listed set that names nothing is refused wherever it reaches the
host, not only by the wire's `exec.apertures-too-few`, so it can never run as a start with no
network. The published
`exec.egress-apertures` fact carries the pinned destination and not the declared host, so this is
the driver's refusal at admission, not the daemon's. Two apertures declared with the same literal
address are not refused: neither has a name to map, and each is reached at its own listen address.

Installation stays all-or-nothing: if any one aperture cannot be installed exactly as declared, all
forwarders already started for the run are killed and the dispatch refuses with
`exec.aperture-install-failed` (ADR 0013). Exceeding any one aperture's ceiling ends the run as a
single-aperture run ends today.

Applied record. `AppliedNetwork` gains a third serialized form: an array of applied apertures, used
only when the start named `apertures`. Each element is today's applied-aperture object plus
`listen` (`address:port`). A start that named `aperture` keeps reporting the single object, without
`listen`, so no `0.20.0` reader's record changes.

| condition | code | status |
|---|---|---|
| `aperture` and `apertures` both present | `exec.aperture-mode-mismatch` | 422 |
| `apertures` with fewer than two names | `exec.apertures-too-few` | 422 |
| more than `MAX_EGRESS_APERTURES` names | `exec.apertures-too-many` | 422 |
| a name listed twice | `exec.aperture-listed-twice` | 422 |
| two listed names whose declared hosts are one host name, or `localhost` declared by any but the first | `exec.aperture-host-listed-twice` | 422 |
| a name that is invalid, a destination or a ceiling | the existing per-name codes | 422 |
| a name the operator did not declare | the existing undeclared-aperture refusal, naming it | 501 |

## 3. Read-only roots bound by descriptor

`read_only_roots` are mounted today with `--ro-bind <host_path> <mount>`: bubblewrap resolves the
host path again when it mounts, after admission checked it. A rename or symlink swap between the two
mounts a different directory. The workspace (ADR 0023) and single files (ADR 0033) are already
bound through descriptors the daemon opened. Roots now follow them: the host opens each root
`O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` through `openat2` with `RESOLVE_NO_SYMLINKS`,
passes the descriptor to bubblewrap and mounts it with `--ro-bind-fd`. The existing codes keep their meaning: a root that is not there is `exec.read-only-root-absent`; a
root that is not a directory, or has a symlink at any component, is
`exec.read-only-root-not-a-directory`. A symlink above the last component was followed before; it
is now refused, as ADR 0033 already refuses one for a single file. The applied record is
unchanged. No wire byte changes.

## Bundle

Successor `0.21.0` carries: the `workdir` start field, the `apertures` request field, the array
form of the applied network, the `exec.workdir` fact, the eight new codes above, and the bound
constants `MAX_WORKDIR_BYTES`. It is additive over `0.20.0`: route counts unchanged, no member
removed, no frozen directory rewritten (invariant 6).
