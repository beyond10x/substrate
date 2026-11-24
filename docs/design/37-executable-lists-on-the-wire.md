# Design 37: the executable lists on the wire

Status: draft of 2026-11-25, not accepted. Written against `main` at `c5f0b8716`. Serves
`story:exec-start-carries-executable-lists` of
`epic:runs-are-governed-and-limit-hits-are-events`. The stakeholder of that story is the session
`sdc`; § 15 lists the six questions that have more than one defensible answer, each with the
choice this document is written with.

It adds a successor contract bundle, `substrate-wire/0.29.0`, with no new route: two optional
members on the `sandbox` of an exec start, the same two on the applied record, one new
confinement property, one optional member on the capability snapshot, and seven refusals a start
that names a list can earn. § 14 lists the decisions, § 16 every claim that is inferred and not
read in this tree, § 17 what is not decided here, § 18 what was not read.

The requirement, relayed 2026-11-21: "every process goes through Substrate so runs can move
remote; hard limits per run and per action …, an executable allow-list and deny-list". The
in-process `linux` adapter applies both lists since design 35. A run that moves from that adapter
to a daemon loses them today, because no start field carries a list.

Design 35 is the rule this document carries, and it is not restated here: what an entry is, where
it may lie, what the helper does, what a list does not stop. This document decides only what the
wire says.

## 1. Evidence: what the tree does at `c5f0b8716`

| what | where |
|---|---|
| a confinement profile holds `exec_subpaths: Vec<String>` and `exec_denied_paths: Vec<String>` | [`confinement/profile.rs`](../../crates/substrate-host/src/confinement/profile.rs), `ConfinementProfile` |
| the deny-list holds at most `MAX_EXEC_DENIED_PATHS` (64) entries. No constant bounds the number of allow entries | [`confinement/mod.rs`](../../crates/substrate-host/src/confinement/mod.rs) |
| on `linux` a path in the view is at most 4095 bytes (`MAX_PATH_BYTES`), and an entry of either list has no empty, `.` or `..` segment and no `"`, `\` or control character | [`exec_gate.rs`](../../crates/substrate-host/src/exec_gate.rs), `check_literal`, `check_denied_literal`; [`seatbelt.rs`](../../crates/substrate-host/src/seatbelt.rs), `relative_literal`, `absolute_literal` |
| a relative entry lies beneath `/workspace`. An absolute entry lies beneath `/usr`, `/bin`, `/lib`, `/lib64` (`BASE_MOUNTS`), `/workspace` or a declared mount | `exec_gate.rs`, `eligible` |
| every entry and the program are located at the start, before dispatch, by one function. It returns no gate for a start without a list, whose argv is then what it was before the lists existed | [`process.rs`](../../crates/substrate-host/src/process.rs), `gate_start` |
| the host driver has three starts a wire request reaches: `start`, `start_with_git_roots` and `start_pipe_with_git_roots`. Each passes `ExecLists::wire()`, which is no list. A test counts the three sites | the same file |
| the `linux` adapter passes the admitted profile's lists, `ExecLists::of(admitted)` | [`confinement/linux.rs`](../../crates/substrate-host/src/confinement/linux.rs) |
| a start with a list and a controlling terminal is refused `session.pty-unserved` (501), address `mode`, before dispatch. A declared mount at or beneath `/.substrate-exec-gate` on a start with a list is refused by the code its kind has for a mount the driver owns | `process.rs`, `listed_terminal_unserved`, `listed_mounts_leave_the_helpers` |
| the driver proves two facts at its open, `exec_subpaths` and `exec_denied_paths`, each by a throwaway run under the helper `HostConfig.exec_gate` names. Unset, neither is proved | `confinement/linux.rs`, `ExecListProof` |
| where a path of the open lies on a file-system type outside `ext4`, `overlay` and `tmpfs`, or the kernel reports no mount ID for it, the driver opens, proves neither fact and keeps one `FactWithheld` per fact and path: `fact`, `cause`, `address`, `path`, `file_system` | [`confinement/facts.rs`](../../crates/substrate-host/src/confinement/facts.rs); [`place.rs`](../../crates/substrate-exec-gate/src/place.rs), `LISTED_FILE_SYSTEMS` |
| `publish_confinement` sets the wire fact `confinement.exec-subpaths` to absent although the adapter proves it (design 35 D16). Two test constants name the exception: `PROVED_AND_NOT_PUBLISHED` and `PROVED_WITHOUT_A_WIRE_PROPERTY` | [`probe.rs`](../../crates/substrate-host/src/probe.rs) |
| the daemon never sets `HostConfig.exec_gate`: it has no flag for it | [`runtime.rs`](../../crates/substrate-daemon/src/runtime.rs), `open_host_driver`; [`main.rs`](../../crates/substrate-daemon/src/main.rs) |
| `ConfinementRequest`, the `sandbox` of a start, is closed (`deny_unknown_fields`) and holds `capability_snapshot`, `network`, `aperture`, `apertures`, `profile`, `require` and the optional `requires` | [`substrate-wire`](../../crates/substrate-wire/src/lib.rs), `ConfinementRequest` |
| a session start embeds a whole exec start as its `exec` member | the same file, `PipeSessionStartInput` |
| `AppliedConfinement` echoes `requires`, `workdir`, `read_only_roots`, `read_only_files`, `secret_slots` and `writable_roots`, each skipped when absent or empty | the same file |
| `ConfinementProperty` has seven variants. `ExecSubpaths` is `exec-subpaths`, its fact `confinement.exec-subpaths`. There is no deny-list property | the same file, `ConfinementProperty::ALL` |
| a start whose `requires` names a property the snapshot does not hold is `confinement.fact-unserved` (501), address `sandbox.requires.facts`, message `Confinement fact {name} is not proved by this driver.` | the same file, `CONFINEMENT_UNSERVED_REFUSALS`, `confinement_requires_refusal` |
| a member that is a list and must be present when written is decoded by `present_list`: `null` is refused, an absent member stays `None` | the same file |
| `CapabilitySnapshot` is closed and holds `snapshot`, `driver`, `driver_version`, `config_generation`, `probed_at`, `valid_until` and `facts` | the same file |
| the daemon checks a start in this order: schema, `exec.capability-stale`, `requires`, aperture, secret slots, read-only files, writable roots, working directory, read-only roots, measurements, scratch, the profile's proof, capsule; then the result reserve | [`operations.rs`](../../crates/substrate-daemon/src/app/operations.rs), `validate_exec_input`; [`responses.rs`](../../crates/substrate-daemon/src/app/responses.rs), `within_result_reserve` |
| a mutation whose widest success result exceeds 524,217 bytes of compact JSON is refused `operation.result-limit` (429) before its side effect. An applied list is counted as the request sent it | [ADR 0053](../../adr/0053-a-mutation-is-refused-before-dispatch-when-its-success-result-cannot-be-recorded.md); [`result_widths.rs`](../../crates/substrate-wire/src/result_widths.rs), `widest_applied` |
| the Rust SDK refuses a daemon whose `x-b10x-contract` or bundle digest is not its own (`SdkError::ContractMismatch`) | [`transport.rs`](../../crates/b10x-substrate-sdk/src/transport.rs), `verify_contract_headers` |
| the daemon and the SDK advertise `substrate-wire/0.28.0`. `contracts/substrate-wire/0.28.0` is the newest released directory | `substrate-wire`, `ADVERTISED_CONTRACT_BUNDLE` |
| a bundle addition is one module that holds the released bytes to the wire crate's constants. The module for the read-only roots of `0.23.0` holds two starts, the applied members, the capability the member requires and one vector per refusal | [`xtask/src/bundle/read_only_roots.rs`](../../xtask/src/bundle/read_only_roots.rs) |
| the session start's `exec` object must declare the property set of the exec start | [`xtask/src/bundle/session_exec_fields.rs`](../../xtask/src/bundle/session_exec_fields.rs) |
| the runtime-vector runner asserts 136 portable, 201 delegated and 64 namespace cases | [`runtime_vectors.rs`](../../crates/substrate-daemon/tests/runtime_vectors.rs), `PORTABLE_CASES`, `DELEGATED_CASES`, `NAMESPACE_CASES` |
| the Kubernetes namespace driver publishes neither fact | [`substrate-kubernetes`](../../crates/substrate-kubernetes/src/exec.rs), `unproved` |

## 2. The two members

Both members stand in the `sandbox` of an exec start, beside `requires`:

```json
"sandbox": {
  "capability_snapshot": "…",
  "network": "none",
  "profile": "standard",
  "require": true,
  "exec_subpaths": ["/usr/bin/git", "tools"],
  "exec_denied_paths": ["tools/unsafe"]
}
```

A session start carries them in `exec.sandbox`, because it embeds the exec start. The names are
the profile's field names, so one name means one thing in the adapter API and on the wire.

| member | Rust type | absent | present |
|---|---|---|---|
| `exec_subpaths` | `Option<Vec<String>>`, decoded by `present_list`, skipped when `None` | no allow-list: every program of the view executes, as on every start before this bundle | 1 to `MAX_EXEC_SUBPATHS` entries: the run executes its program and what the entries admit, and nothing else (design 35 § 4). An empty list is refused (§ 3) |
| `exec_denied_paths` | `Vec<String>`, skipped when empty | no deny-list | 1 to `MAX_EXEC_DENIED_PATHS` entries: what they name does not execute, and a denied path wins (design 35 § 8). An empty list reads as an absent one |

They stand in `sandbox` and not at the top of the start, where `read_only_roots` and `workdir`
are, for two reasons. The lists are confinement of the run, as `requires` is, and they declare no
mount. And the applied record is the start's `sandbox` as applied: a member that is requested at
`sandbox.exec_subpaths` and echoed at `sandbox.exec_subpaths` has one address.

**Bounds.** Three constants of the wire crate, each bound into the schemas from
`xtask/src/bundle.rs` (`AGENTS.md` § *The gate*: a new constant cannot be a `$wire` binding):

| constant | value | schema |
|---|---|---|
| `MAX_EXEC_SUBPATHS` | 64 | `maxItems` of `exec_subpaths` |
| `MAX_EXEC_DENIED_PATHS` | 64 | `maxItems` of `exec_denied_paths` |
| `MAX_EXEC_LIST_ENTRY_BYTES` | 4095 | `maxLength` of an entry of either list. The wire crate counts bytes |

The host crate's `MAX_EXEC_DENIED_PATHS` becomes the wire crate's constant, as its other
published bounds are. The allow-list has no count bound in the host crate today; the adapter API
gets the same 64 in the same change, as `confinement.exec-subpaths-exceeded` (422) beside the
deny-list's code, so a profile and a start have one bound. 64 is the deny-list's figure and is
not derived from a measurement: a listed directory admits everything beneath it, so a list names
directories and single tools, not every file.

**What the wire crate checks**, in one function `validate_exec_lists`, which the daemon answers
`request.schema-invalid` (422) at the member's address: more entries than the bound, an entry
that is empty or longer than `MAX_EXEC_LIST_ENTRY_BYTES`, and an entry written twice in one
list. These are the rules a schema states. `null` for either member is refused by the decoder.

**What the driver checks**, at the start and before dispatch, is everything design 35 decides by
place or by the view: § 6 lists the codes. The wire crate holds none of those rules, so the
adapter API and the wire cannot disagree on them.

**The program.** On a start with an allow-list the first argument is an absolute path in the
view; a bare name is refused `exec.sandbox-path-unrepresentable` (design 35 § 12). A start
without a list keeps today's rule for its program.

## 3. An absent allow-list and an empty one

The stakeholder decided on 2026-11-21 that a start whose allow-list member is present and empty
is refused by a named refusal, published with its vector
(`approval-record:sdc-accepts-design-35-2026-11-21`): no accepted byte means "runs as today" now
and "the program only" later.

- The refusal is `exec.sandbox-exec-subpaths-empty`, 422, class `refused`, not retriable,
  address `sandbox.exec_subpaths`, message
  `An executable allow-list names at least one entry.`
- The daemon answers it for every driver, before any fact is read. A daemon whose host proved
  nothing answers it too: the lexical rule comes first, as it does for the read-only roots.
- The wire can tell the two apart because the member is an `Option`. The profile cannot:
  `exec_subpaths` is a `Vec` there, and an empty one runs every program of the view (design 35
  D1). The driver's wire starts therefore map `None` to no list and never pass an empty list.
- `story:explicitly-empty-exec-subpaths-means-the-program-only` can later retype the profile's
  field and give the empty list its meaning. It then cuts a successor bundle that removes this
  refusal's vector. No byte this bundle accepts changes its meaning in that step.

An empty deny-list is not refused. It denies nothing under every reading, so it reads as an
absent list, is not echoed, and a start that carries it hashes as one without the member. This
is the rule `apertures` has.

## 4. The applied record

`AppliedConfinement`, the `sandbox` of an exec's record, gains the same two members with the
same types and the same skipping rule:

- `exec_subpaths`: present exactly when the start named an allow-list. The entries are the
  request's, in the request's order and spelling.
- `exec_denied_paths`: present exactly when the start named at least one deny entry, in the same
  form.

The record echoes what was asked and held, as `requires` does. It does not publish the place an
entry resolved to: a device and an inode of the host are host-private (`ExecListProof`), and a
client that wants to know what ran reads the exec's own observation.

A session's exec has the same record. `0.28.0` and every earlier bundle have neither member on
either start or record, which the bundle check of § 9 holds.

## 5. The facts

Two confinement properties name the two proofs:

| property | fact | published when |
|---|---|---|
| `exec-subpaths` (exists) | `confinement.exec-subpaths` | the driver's open proved `exec_subpaths` |
| `exec-denied-paths` (new) | `confinement.exec-denied-paths` | the driver's open proved `exec_denied_paths`, which it proves only where the first held |

- `ConfinementProperty` gains `ExecDeniedPaths` directly after `ExecSubpaths`. That is the
  position the host's `Fact::ExecDeniedPaths` has in `spec/confinement/`, and `ALL` has eight
  entries.
- Each fact is `const true` and absent when not proved, as every confinement property is.
- `publish_confinement` publishes both from `linux::proof`. The two test constants of
  `probe.rs` that name the exception go, and `the_published_set_is_the_linux_adapters_proof`
  holds the published set equal to the proof.
- A start may name either property in `sandbox.requires.facts`. That answer is unchanged:
  `confinement.fact-unserved` at `sandbox.requires.facts`.

**Naming a list requires its fact.** A start with `exec_subpaths` requires
`confinement.exec-subpaths`, and a start with a non-empty `exec_denied_paths` requires
`confinement.exec-denied-paths`, whether or not `requires` names them. The daemon checks this in
the step that checks `requires`, against the snapshot the start names, for every driver:

- code `confinement.fact-unserved`, 501, class `unserved`, not retriable;
- address `sandbox.exec_subpaths` or `sandbox.exec_denied_paths`;
- message `Confinement fact exec-subpaths is not proved by this driver.`, or the same with
  `exec-denied-paths`: the sentence `confinement_requires_refusal` writes.

One code is used for the adapter API and the wire because the cause is the same: design 35 § 12
refuses a profile with a list on an adapter that did not prove the fact with this code. The
precedent of a code per member (`exec.read-only-files-unserved`) is not followed; Q2 of § 15.

The lists are applied whatever `sandbox.profile` and `sandbox.require` say. A list is not a
profile, and a start that names one gets it or is refused.

## 6. Refusals

Every row is answered before dispatch and recorded as refused. "Daemon" rows hold for every
driver; "driver" rows are the host driver's, at `gate_start`.

| # | code | status | address | when | who |
|---|---|---|---|---|---|
| 1 | `request.schema-invalid` | 422 | the member | `null`, more entries than the bound, an empty or over-long entry, an entry twice | daemon; existing code |
| 2 | `exec.sandbox-exec-subpaths-empty` | 422 | `sandbox.exec_subpaths` | the member is present and empty (§ 3) | daemon; **new** |
| 3 | `confinement.exec-denied-paths-without-subpaths` | 422 | `sandbox.exec_denied_paths` | a deny entry on a start without an allow-list | daemon; the host's code, published here |
| 4 | `confinement.fact-unserved` | 501 | `sandbox.exec_subpaths`, `sandbox.exec_denied_paths` | the snapshot does not hold the fact (§ 5) | daemon; existing code, two new addresses |
| 5 | `session.pty-unserved` | 501 | `mode` | a `pty` session whose start names an allow-list (design 35 D29) | driver; existing code, new use |
| 6 | `exec.sandbox-path-unrepresentable` | 422 | the member, or `argv` | an entry or the program the view cannot carry: not normal, a refused character, a link rule of design 35, an allow entry that does not exist, a program that is not absolute | driver; the host's code, published here |
| 7 | `exec.sandbox-exec-subpath-undeclared` | 422 | the member | an absolute entry outside the eligible locations | driver; the host's code, published here |
| 8 | `exec.sandbox-program-unlisted` | 422 | `argv` | the program lies in a denied path or is a denied file | driver; the host's code, published here |
| 9 | `exec.sandbox-place-unserved` | 501 | the member, `argv`, or the member of the declared mount | a place the start needs cannot be established, among the causes a file-system type outside the list (design 35 D26, D30) | driver; the host's code, published here |

Rows 1 to 4 are checked in that order, in the daemon's step for `requires`: after
`exec.capability-stale`, before every other field's check. Row 4 is checked for the allow-list
first. Rows 5 to 9 keep the order `gate_start` has.

- The mount-reserved codes for a declared mount at the helper's own path are already published.
  A start with a list can now earn them on the wire, with no new vector: a client cannot see the
  helper's path, and the code already names the cause.
- `exec.capability-stale` is unchanged. The helper's bytes are read once at the open, so a
  replaced helper file does not make a snapshot stale (design 35 D20).
- `confinement.exec-denied-paths-exceeded`, and the allow-list's new counterpart, stay codes of
  the adapter API. The wire answers a list over its bound with row 1 and never reaches them.
- `config.exec-gate-invalid` and `config.exec-gate-unserved` are refusals of the daemon's
  startup (§ 7), not of a request. The bundle does not publish them.
- A helper that stops after dispatch ends the run with status 125 (design 35 § 6). That is an
  exit status, not a refusal, and this bundle gives it no code.

Message and address of rows 6 to 9 are the driver's as design 35 writes them. Their messages
name paths of the view, which the request itself sent, and never a host path.

## 7. The daemon

**One flag.** `--exec-gate <path>` names the helper and sets `HostConfig.exec_gate`. It is
optional, has no default and no environment variable, and conflicts with `--driver kubernetes`
in the argument parser, as every host-only flag does
(`every_host_only_flag_conflicts_with_the_kubernetes_driver` in `main.rs`, which gains the flag).

- Unset: the driver proves neither fact, the snapshot holds neither, and every start that names a
  list is row 4.
- Set, and the open refuses the helper (`config.exec-gate-invalid`, `config.exec-gate-unserved`):
  the daemon does not start, and its error is the driver's. A daemon that was told to serve the
  lists and cannot does not start without them (`AGENTS.md` invariant 3).
- Set, and the open withholds the facts (design 35 D27): the daemon starts, the snapshot holds
  neither fact and names why (§ 8), and the daemon writes one line per withheld entry to its
  standard error with the cause, the configuration field, the path and the file-system type.

**The three starts.** `start`, `start_with_git_roots` and `start_pipe_with_git_roots` take the
lists from the start's `sandbox` in place of `ExecLists::wire()`. `ExecLists::wire` goes, and the
test that counts its three sites then holds that each wire start builds its lists from the
input. A start without the members builds no list, so `gate_start` returns no gate and the argv
is what it is today: `a_start_without_a_list_builds_the_argv_it_built_before` keeps holding it.

**Replay.** The members are part of the request, so the request hash covers them and a retried
start with other lists is `operation.request-conflict`, as for every other member.

**The result reserve** (ADR 0053). Both lists are echoed, so both count toward the widest
result of an exec start and of a session start, each entry as the request sent it. At the bounds
(128 entries of 4095 bytes) the two lists alone are wider than the reserve of 524,217 bytes, so
`widest_applied` counts them and a start whose widest result does not fit is
`operation.result-limit` (429) before dispatch. A start whose two lists hold 64 entries in all,
of 255 bytes each, adds under 17 KiB.

**The Kubernetes namespace driver** proves neither fact and enforces nothing (the story's
boundary). Rows 1 to 4 are the daemon's, so a start that names a list is refused there without
a line of driver code. A unit test holds its snapshot to both facts absent.

**The release image and the chart are not changed by this story.** The helper is a static
binary of this workspace; an image that ships it, and a chart value that names it, are
`story:the-daemon-image-ships-the-exec-gate-helper`, drafted with this story's plan (Q5 of
§ 15). Until then an operator who wants the lists on a daemon builds the helper and passes the
flag.

## 8. Why a fact is absent

The stakeholder's input (`approval-record:sdc-accepts-d27-2026-11-21`): the capability snapshot
should be able to name why a fact is absent. A confinement fact is `const true` or absent, and
that stays.

The snapshot gains one optional member, `withheld`: a list, absent when empty, of closed
entries.

```json
"withheld": [
  {"fact": "confinement.exec-subpaths", "cause": "file-system-unlisted", "address": "config.workspace_root"},
  {"fact": "confinement.exec-denied-paths", "cause": "file-system-unlisted", "address": "config.workspace_root"}
]
```

| member | values |
|---|---|
| `fact` | a fact name of the snapshot. In this bundle: `confinement.exec-subpaths`, `confinement.exec-denied-paths` |
| `cause` | `file-system-unlisted`, `mount-id-unreported` |
| `address` | `config.exec_gate`, `config.workspace_root`, `config.writable_roots` |

- Every member is a closed enumeration, so the list is bounded without a count: at most
  `MAX_WITHHELD_FACTS` (12) entries, the product of the three sets. The driver writes each
  distinct entry once, in the order fact, address, cause.
- The entry carries no host path and no file-system type. A path of the deployment is not a
  client's to read, and a mount's type field can hold text its mounter chose (`fuse.<subtype>`).
  Both are in the daemon's own line (§ 7), where the operator who can change them reads them.
- An entry is present only for a fact that is absent from `facts`, and only for the causes of
  design 35 D27. A daemon without `--exec-gate` has no entry: nothing was withheld, the
  capability was not configured. A fact that is absent with no entry is one this deployment does
  not serve.
- The snapshot's identity already covers the proof the entries are read from (`ExecListProof`),
  so a change of the entries is a new snapshot.
- The refusal of row 4 keeps its fixed message. A client that wants the cause reads the
  snapshot it named.

This is the first member of the snapshot that explains an absence. It is written for the two
facts that have a recorded cause today and is named so that another fact can use it; no other
fact gets an entry in this bundle.

## 9. The bundle

`substrate-wire/0.29.0`, predecessor `0.28.0`, additive, no route added or removed. It is
rendered with `cargo xtask render-bundle 0.29.0` from `xtask/bundle-source/0.29.0/`, and its
check joins `scripts/gate.sh`'s `check-bundles` line. A new module,
`xtask/src/bundle/exec_lists.rs`, holds the released bytes to the wire crate:

- on `schemas/exec-start.json` and under `/properties/exec` of `schemas/pipe-session-start.json`,
  the `sandbox` object declares `exec_subpaths` and `exec_denied_paths` with the three bounds
  of § 2, `minLength` 1 and `uniqueItems`; `exec_subpaths` has no `minItems`, because the empty
  list has its own refusal and vector;
- each member names its capability (`x-b10x-requires-capability`) and its refusals
  (`x-b10x-refusals`);
- the applied record declares both members;
- the capability snapshot declares the fact `confinement.exec-denied-paths` and the member
  `withheld`, its three enumerations held to the wire crate's;
- `sandbox.requires.facts` admits the eight names of `ConfinementProperty::ALL`, in its order;
- one refusal vector for each of rows 2, 3 and 6 to 9, two for row 4 (one per address), and one
  for row 5; each is held to a row of a new table, `EXEC_LIST_REFUSALS`, in the wire crate;
- `0.28.0` declares none of this, which the module holds as the earlier modules do for their
  predecessors.

The vectors of rows 2, 3 and 4 are executable on the portable lane, and those of rows 5 to 8 on
the delegated lane. Row 9's vector is published and not executed: no lane has a volume on a
file-system type outside the list (§ 16).

The bundle's README and the reference text of both members state the limits design 35 states,
in its words (design 35 § 7, § 8.2, § 8.5): the allow-list restricts what the run may execute,
not what an admitted interpreter or loader then runs; a deny-list is exact only where the run
cannot write; another hard link to a denied file beneath a listed path runs. Publishing the
property without them would promise more than design 35 proved, so Q3 of § 15 asks the
stakeholder to confirm the three defaults design 35 left revisable until this story.

## 10. Consumer compatibility

- **A start without the members is byte for byte what it was.** Both are skipped when absent, so
  the request serializes and hashes as on `0.28.0`, and the applied record of such a start has
  neither member. A vector of the new bundle holds one unchanged start to its `0.28.0` hash.
- **A client and a daemon move together.** The SDK refuses a daemon that advertises another
  bundle (`SdkError::ContractMismatch`), so no client of `0.28.0` reads a snapshot with
  `withheld` or a record with a list.
- **Rust consumers of the wire crate.** `ConfinementRequest` and `AppliedConfinement` gain two
  public fields each, `CapabilitySnapshot` and `CapabilityFacts` one each, and
  `ConfinementProperty` one variant. None of the five is `#[non_exhaustive]`, so a consumer that
  writes one as a struct literal or matches the enum without a wildcard adds a line when it moves
  to the release that carries this. The changelog's entry names the five.
- **Rust consumers of the host crate.** `MAX_EXEC_DENIED_PATHS` is re-exported from where it is
  today. A profile with more than 64 allow entries, admitted today, is refused
  `confinement.exec-subpaths-exceeded`; the changelog names it under *Changed*.
- **No SDK setter and no CLI flag** (the story's boundary). An SDK client sets the two fields
  on `ConfinementRequest` itself.
- **A daemon that publishes a new fact.** A host with a helper configured publishes
  `confinement.exec-subpaths` for the first time. A client that listed it in `requires` was
  refused before and is admitted now, which is what the property was published for.

## 11. The file-system types

The lists are served where every place of the open lies on `ext4`, `overlay` or `tmpfs`
(`LISTED_FILE_SYSTEMS`). Two obligations come with the wire:

- **The xfs probe** (design 35 D27, C; the story's check). Before the story is planned, cases F1
  to F7 of design 35 § 3.9 run on an xfs volume in the quota fixture's container, and the run is
  recorded in design 35. `xfs` joins the list only if every case holds; `btrfs` stays refused by
  name. The fixture's image has neither `mkfs.xfs` nor bubblewrap today, so the probe adds both
  to a probe image of its own and changes no committed file. This document does not depend on
  the result: an added type changes `LISTED_FILE_SYSTEMS` and one sentence, no wire byte.
- **The deployment guide** says, in § *Before you install*: "The executable lists are served only
  on ext4, overlay and tmpfs volumes today.", with the list as it stands when the story merges.

## 12. How it is proved

Acceptance is the story's: in the delegated lane of `runtime_vectors.rs`, an exec start and a
session start that each name both lists run, a denied execution fails inside each, and each
applied record echoes both lists.

| lane | cases |
|---|---|
| portable | both facts absent and no `withheld`; an exec start and a session start naming an allow-list, and one naming both, are row 4 and recorded as refused; an empty allow-list is row 2 on both starts; a deny entry without an allow-list is row 3; `null`, 65 entries, an empty entry and a duplicate are row 1 |
| delegated | both facts published; the acceptance's two starts, each with a listed program that runs, an unlisted one that fails with `EACCES` inside the run, and a denied one beneath a listed directory that fails the same way; both records echo both lists; a start without the members has a record without them; rows 5, 6, 7 and 8 once each; row 2 and row 3 again, to hold that the lexical rules come before the facts |
| namespace | both facts absent in the probed snapshot |

- Each lane's function reports its count, and `PORTABLE_CASES`, `DELEGATED_CASES` and the
  figures `AGENTS.md` states move in the same change. `scripts/delegated-lane.sh` builds the
  static helper already; it passes the path to the daemon the runner starts.
- A daemon started with `--exec-gate` naming a dynamically linked helper does not start: one
  case, with the copy the lane already builds (`SUBSTRATE_EXEC_GATE_DYNAMIC`).
- `withheld` is held by a unit case of the host driver's snapshot on a proof with entries
  written by hand, and by the bundle check. No lane has a volume that withholds (§ 16).
- The wire crate's cases: `validate_exec_lists` at each bound and one past it, the decoder on
  `null`, the two skipping rules, and the widths of § 7 at the bounds of ADR 0053's table.
- `the_refusal_codes_are_the_wire_constants` in the daemon's library tests counts the new code
  constants (`AGENTS.md` § *The gate*).
- Every new test is timed before it is added. The delegated cases start one daemon per lane
  function, as today; none starts a build.

Design 35's cases are not repeated on the wire. What a list stops and does not stop is proved
on the adapter; the wire cases prove that the request reaches that mechanism and that the
record says so.

## 13. Order of work

1. This document is accepted and merged before any code of the story (`AGENTS.md` invariant 8).
2. The xfs probe runs and is recorded in design 35 (§ 11).
3. The story is planned and its critics read it.
4. One change set: the wire crate, the host driver's three starts and `publish_confinement`, the
   daemon's flag and checks, bundle `0.29.0` with its module and gate line, the runtime vectors,
   the deployment guide, the changelog and `AGENTS.md`.

`story:exec-observation-reports-process-count-hits` follows and cuts its bundle after this one.

## 14. Decisions

| # | decision |
|---|---|
| D1 | the members are `sandbox.exec_subpaths` and `sandbox.exec_denied_paths` on the exec start, and the same on a session's `exec`. Q1 |
| D2 | `exec_subpaths` is an optional list that tells absent from empty; absent is no list; a present list has 1 to 64 entries |
| D3 | a present and empty allow-list is `exec.sandbox-exec-subpaths-empty` (422), answered by the daemon for every driver. Decided by the stakeholder on 2026-11-21; this document names the code |
| D4 | an empty deny-list reads as an absent one |
| D5 | the bounds are 64 entries a list and 4095 bytes an entry, three wire constants; the adapter API takes the allow-list's bound in the same change. Q4 |
| D6 | the schema's rules are `request.schema-invalid`; no code is added for a list over its bound |
| D7 | the applied record echoes both lists as sent and publishes no place |
| D8 | `confinement.exec-subpaths` is published from the proof, and the deny-list gets the property `exec-denied-paths` directly after it |
| D9 | naming a list requires its fact; the answer is `confinement.fact-unserved` (501) at the member's address. Q2 |
| D10 | the host's five codes for a start with a list are published under their own names and statuses; the driver stays the one place that decides them |
| D11 | the daemon gets `--exec-gate`; a helper the open refuses stops the daemon's start |
| D12 | the snapshot gains `withheld`: closed entries of fact, cause and address, without a path or a file-system type. Q6 |
| D13 | the bundle is `0.29.0`, additive on `0.28.0`, with one new module and one vector per refusal row |
| D14 | the published reference text carries design 35's stated limits. Q3 |
| D15 | the release image and the chart are a story of their own. Q5 |
| D16 | the lists count toward the result reserve as sent; no bound is lowered to make every admitted list fit |

## 15. Questions with more than one answer

Each is the stakeholder's (`approval-record:stakeholder-decides-for-the-operator-2026-11-21`).
The first option is the one this document is written with.

**Q1. Where do the members stand?**

- **A. In `sandbox`** (D1): one address for the request and the applied record, beside
  `requires`.
- B. At the top of the start, beside `read_only_roots` and `workdir`. It follows the members
  that declare mounts; the applied record then echoes at another address than the request.

**Q2. Which code refuses a list the host did not prove?**

- **A. `confinement.fact-unserved`** at the member's address (D9): the code the adapter API
  gives the same cause.
- B. A code per member, `exec.exec-subpaths-unserved` and `exec.exec-denied-paths-unserved`,
  as the read-only files and the working directory have. Two more codes, and a client moving a
  run from the adapter to a daemon handles a second name for one cause.

**Q3. Do the three defaults of design 35 § 19 stand for the published property?**

- **A. All three stand, and the published text states them** (D14): no `memfd_create` refusal
  (its Q3), the hard-link limit is stated and not refused (its Q4), and its Q2 no longer arises
  on the wire, where an empty list is refused.
- B. One of them is decided again before the bundle is cut. Each such change is a change of the
  mechanism and its own design revision of design 35, ahead of this story.

**Q4. The bound of the allow-list.**

- **A. 64 entries**, the deny-list's figure, on the wire and in the adapter API (D5).
- B. A higher figure, such as 256. The result reserve admits it only for short entries, and the
  helper's rule count grows with it.
- C. 64 on the wire and no bound in the adapter API, as today. Then a profile exists that no
  start can carry.

**Q5. Does this story change the release image and the chart?**

- **A. No** (D15): a story of its own carries the helper into the daemon image and a chart
  value. This story stays one change set, and its acceptance is the delegated lane.
- B. Yes: the `Dockerfile` builds the static helper, `release.yml` checks it has no
  interpreter, and the chart gets the value. The story then waits for a chart-lane run with
  node profiles.

**Q6. What does the snapshot say about an absent fact?**

- **A. `withheld` with fact, cause and address** (D12): closed, bounded, no host text.
- B. The same with the file-system type as a fourth, bounded string. A client can then show
  `xfs`; the daemon must refuse or rewrite a type field its mounter chose.
- C. Nothing on the wire; the daemon's own line is the only place. The stakeholder's input
  asked for the snapshot.

## 16. Inferred, not observed

- **An entry beneath `/runtime`.** Inferred from `exec_gate.rs`, where `eligible` names the
  four base mounts, `/workspace` and the declared mounts and has no line for the capsule mount:
  on a start with a capsule and an allow-list the capsule's program runs as the start's
  program, and an allow entry beneath `/runtime` is row 7. Not run. The story's implementation
  holds it with one delegated case, and § 17 leaves the other answer to a later story.
- **The session start's width.** ADR 0053's width function for an exec start counts the
  applied record; that a session start's width counts the same record for its exec is inferred
  from the ADR's table and not read in `result_widths.rs`.
- **The figure "under 17 KiB"** of § 7 is arithmetic (64 entries of 255 bytes with two quotes
  and a comma each: 16,512 bytes), not a measured request. The same arithmetic gives 524,544
  bytes for 128 entries of 4095 bytes, which is over the reserve.
- **Row 9 on a lane.** That no lane's host has a volume on a type outside the list is read
  from `AGENTS.md`'s lane description, not from a run of this change.
- **The daemon's request hash covers a new member of `sandbox` without further code.** Inferred
  from how `requires` was added (design 30); the hashing function was not read for this
  document.
- **The order of the withheld entries** (§ 8) is this document's choice; the host keeps one
  entry per fact and path today, and the reduction to distinct triples is new code.

## 17. Not decided here

- An SDK builder setter and a CLI flag for the lists: each is its own story once the bundle
  exists (the story's boundary).
- Enforcement on the Kubernetes namespace driver, and on the `oci` and `pod` adapters
  (`story:executable-lists-on-oci-and-pod`).
- An entry beneath the capsule mount (§ 16).
- A named failure for a helper that stops after dispatch, and a record of a denied execution
  (design 35 § 11, § 20).
- An explicitly empty allow-list that means the program only
  (`story:explicitly-empty-exec-subpaths-means-the-program-only`).
- Whether `xfs` enters the list (§ 11): the probe decides it, by the rule design 35 wrote.
- `withheld` for any other fact.

## 18. Not read

- Design 30's text and ADR 0048; only the wire crate's types and documentation comments that
  cite them.
- Design 13 and ADR 0013.
- The rows D29 and D30 of design 35 past their first sentences, and its § 5 and § 6 in full.
- ADR 0053 past its decisions 1, 2, 5, 6, 9 and 10.
- The specification under `spec/`. Whether `spec/confinement/` or the workspace projection
  needs a scenario for the wire members is settled when the story is planned.
- `deploy/helm/substrate` and the `Dockerfile` (Q5).
