---
format: aep.planning-md/3
id: story:exec-start-carries-executable-lists
kind: story
status: draft
title: An exec start carries the executable lists on the wire
summary: 'A successor bundle at the next free version: start fields for the allow-list and deny-list, the applied record and named refusals, so a run keeps its lists on a remote daemon.'
owner: substrate
tags:
- confinement
- sdc
- wire
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:executable-deny-list
- depends_on: story:process-count-limit-hit-is-observed
- depends_on: story:exec-file-changes-are-recorded
scope:
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: README.md
- confidence: inferred
  path: contracts/substrate-wire
- confidence: inferred
  path: crates/b10x-substrate-sdk/src
- confidence: inferred
  path: crates/substrate-daemon/src/app/operations.rs
- confidence: inferred
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: cited
  path: crates/substrate-host/src/probe.rs
- confidence: inferred
  path: crates/substrate-host/src/process.rs
- confidence: inferred
  path: crates/substrate-kubernetes
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: docs/design/37-executable-lists-on-the-wire.md
- confidence: inferred
  path: scripts/gate.sh
- confidence: inferred
  path: website/docs
- confidence: inferred
  path: website/docs/guides/deployment.md
- confidence: inferred
  path: xtask/bundle-source
- confidence: inferred
  path: xtask/src/bundle
- confidence: inferred
  path: xtask/src/bundle.rs
revision: 20
---
# Story: An exec start carries the executable lists on the wire

## Outcome

A client of the daemon names an executable allow-list and deny-list on an exec start and on a
session start. A daemon whose host proved the facts applies the lists, echoes them in the applied
record and publishes both properties. A daemon whose host did not refuses the start by name before
dispatch.

## Where the requirement comes from

- The relayed requirement of 2026-11-21 (`epic:runs-are-governed-and-limit-hits-are-events`):
  every process goes through Substrate so runs can move remote, with an executable allow-list
  and deny-list. A run that moves from the in-process `linux` adapter to a daemon loses its
  lists without this story.
- The stakeholder is the session `sdc`. It decided the present-and-empty refusal
  (`approval-record:sdc-accepts-design-35-2026-11-21`), the xfs probe and the deployment
  sentence (`approval-record:sdc-accepts-d27-2026-11-21`,
  `approval-record:sdc-accepts-d26-and-withheld-facts-2026-11-21`).
- The design is [design 37](../../../docs/design/37-executable-lists-on-the-wire.md), accepted
  on 2026-11-25 by the operator's session on the default of its six questions
  (`approval-record:operators-session-answers-the-questions-of-design-37-2026-11-25`). `sdc` may
  reopen any of the six; a reopened answer before the bundle is cut revises the design and this
  story.

## Why

- The wire publishes `confinement.exec-subpaths` as a property a start may require
  (`ConfinementProperty::ExecSubpaths`, `crates/substrate-wire/src/lib.rs`). No start field
  carries a list.
- `publish_confinement` withholds `confinement.exec-subpaths` although the `linux` adapter
  proves it, and the deny-list fact has no wire property
  (`PROVED_WITHOUT_A_WIRE_PROPERTY`, `crates/substrate-host/src/probe.rs`).
- The three wire starts of the host driver build no list: `ExecLists::wire()` at three sites of
  `crates/substrate-host/src/process.rs`.

## What changes

Design 37 is the specification; its section is named with each item. One change set on one
branch.

1. **The wire crate** (§ 2 to § 6, § 8).
   - `ConfinementRequest` and `AppliedConfinement` gain `exec_subpaths` (optional, tells absent
     from empty) and `exec_denied_paths` (skipped when empty).
   - Constants `MAX_EXEC_SUBPATHS` (64), `MAX_EXEC_DENIED_PATHS` (64),
     `MAX_EXEC_LIST_ENTRY_BYTES` (4095) and `MAX_WITHHELD_FACTS` (12). The host crate's
     `MAX_EXEC_DENIED_PATHS` becomes a re-export of the wire constant from where it is today.
   - `validate_exec_lists`.
   - `ConfinementProperty::ExecDeniedPaths` directly after `ExecSubpaths`, with its fact
     `confinement.exec-denied-paths`; `ALL` has eight entries.
   - Code constants for the new `exec.sandbox-exec-subpaths-empty` and for the five codes the
     host has today and the wire does not: `confinement.exec-denied-paths-without-subpaths`,
     `exec.sandbox-path-unrepresentable`, `exec.sandbox-exec-subpath-undeclared`,
     `exec.sandbox-program-unlisted` and `exec.sandbox-place-unserved`. The host crate's
     constants are bound to them where they are today: three in
     `crates/substrate-host/src/seatbelt.rs`, `PLACE_UNSERVED` in `exec_gate.rs`, one in
     `confinement/mod.rs`. `PATH_UNREPRESENTABLE` of `confinement/oci.rs` is another code and
     stays. The six constants stand in the wire crate's source in the order of their entries
     in the daemon's `REFUSAL_CODES`. The table `EXEC_LIST_REFUSALS` has one row for each of
     rows 2 to 9 of § 6: code, status and address. Rows 2, 3 and 5 also hold their message, a
     fixed text; the driver takes row 5's from the table. Row 4's message is computed by a function of the wire crate beside the
     table, as `confinement_requires_refusal` computes the one of `requires`. The messages of
     rows 6 to 9 are the driver's and hold a path the request sent, so the table holds none.
   - The snapshot's optional `withheld` with its three closed enumerations.
   - Both lists in the widest applied record (`widest_applied`,
     `crates/substrate-wire/src/result_widths.rs`, ADR 0053).
2. **The host driver** (§ 5, § 7, § 8).
   - The three wire starts take the lists from the start's `sandbox`. `ExecLists::wire` goes.
   - A wire start addresses a refused list entry at the wire's member: `sandbox.exec_subpaths`
     and `sandbox.exec_denied_paths`, where the adapter API has `exec_subpaths` and
     `exec_denied_paths` (`ENTRY` and `DENIED`, `crates/substrate-host/src/exec_gate.rs`).
     `ExecLists` carries the two addresses: the adapter's constructor `of` the adapter's, the
     wire starts' constructor the wire's. `argv` and a declared mount's member are the same
     on both (design 37 § 6, amended with this plan).
   - `publish_confinement` publishes both facts from the adapter's proof and fills `withheld`
     from `ExecListProof`, each distinct entry once, in the order fact, address, cause.
     `PROVED_WITHOUT_A_WIRE_PROPERTY` and its exception go.
   - The adapter API bounds the allow-list at 64 entries:
     `confinement.exec-subpaths-exceeded` (422), a new variant of the public
     `ConfinementError`. The two exhaustive matches over it gain an arm:
     `crates/b10x-substrate-cli/src/confine.rs` and
     `crates/substrate-host/tests/support/mod.rs`.
   - On the wire, the message of row 9 names the path of the view and the cause word, and
     neither a file-system type nor the list of served types (design 37 § 6, amended with this
     plan by the rule of its § 8). The adapter API's message keeps both.
3. **The file-system types** (§ 11). `xfs` joins `LISTED_FILE_SYSTEMS`
   (`crates/substrate-exec-gate/src/place.rs`), by the probe design 35 § 3.14 records. Every
   test that uses `xfs` as the type outside the list uses `btrfs`, and every sentence that names
   the served types names four. `btrfs` stays refused by name.
4. **The daemon** (§ 6, § 7).
   - The flag `--exec-gate <path>`, which conflicts with `--driver kubernetes`.
   - A helper the open refuses stops the daemon's start. An open that withholds the facts lets
     the daemon start. The line per withheld entry is the host driver's own, which exists:
     `Opening::announced` (`crates/substrate-host/src/exec_gate.rs`) writes it at `warn` with
     fact, cause, address, path and file-system type, and the daemon's log carries it. The
     daemon writes no second line and reads no host item for it (design 37 § 7, amended with
     this plan).
   - The checks before dispatch, in the `requires` step after `exec.capability-stale`: the
     lexical rules, the empty allow-list, the deny-list without an allow-list, then the fact,
     the allow-list's first. The driver's rows keep the order of `gate_start`.
5. **Bundle `substrate-wire/0.29.0`** (§ 9), predecessor `0.28.0`, additive, no route.
   - Rendered with `cargo xtask render-bundle 0.29.0` from `xtask/bundle-source/0.29.0/`.
   - The new module `xtask/src/bundle/exec_lists.rs`; each new constant bound from
     `xtask/src/bundle.rs`, never from `xtask/src/render.rs`. It binds `0.29.0` only: the
     vector of row 2 is that bundle's, so a later bundle can give the empty list a meaning.
   - `confinement_facts::check` and `check_introduction`
     (`xtask/src/bundle/confinement_facts.rs`) run for every bundle from `0.26.0` on and hold
     its facts to `ConfinementProperty::ALL`. They hold `0.26.0` to `0.28.0` to the seven
     names those bundles published and `0.29.0` to eight. No byte under `contracts/` moves
     for it.
   - No vector message of `0.29.0` names a file-system type, so the bundle does not depend on
     `LISTED_FILE_SYSTEMS`.
   - `0.29.0` joins the `check-bundles` line of `scripts/gate.sh`.
   - The daemon and the Rust SDK advertise it: `ADVERTISED_CONTRACT_BUNDLE` and
     `ADVERTISED_CONTRACT_BUNDLE_SHA256`. Every pin of `0.28.0` as the current bundle moves
     with them (*Scope*).
6. **The runtime vectors** (§ 12): the cases of the portable, delegated and namespace lanes,
   with `PORTABLE_CASES`, `DELEGATED_CASES`, `NAMESPACE_CASES` and each lane function's count.
   `scripts/delegated-lane.sh` passes the static helper it already builds to the daemon the
   runner starts.
7. **The words.**
   - `website/docs/guides/deployment.md` § *Before you install*.
   - The reference text of both members and the bundle's README, with the three limits of
     design 35 (§ 9 of design 37).
   - `CHANGELOG.md`: *Added*; under *Changed* the allow-list bound and the six public types
     that gain a field or a variant: the five of § 10 and the host crate's `ConfinementError`.
     One line more, for `story:git-create-work-and-memory-follow-its-limits`: the request
     for the budget in the published `gix-pack` is
     <https://github.com/GitoxideLabs/gitoxide/issues/3244>, which
     `vendor/gix-pack-0.74.2/README.substrate.md` links too.
   - `AGENTS.md`: invariant 6, the three case counts, the bundle range of the gate, the flag,
     the sentence that names the served types, and the sentence that lists the properties the
     delegated lane's machine publishes.
   - `README.md`, `STATUS.md` and the site pages `cargo xtask check-docs` holds to the
     advertised bundle.

**The specification under `spec/` gets no scenario** (design 37 § 18 left it to this plan).
`spec/confinement/confinement.yaml` already declares the fact `exec_denied_paths` and the
deny-list's refusal, and the confinement suite runs the adapter, not the daemon. The wire
members are held by the bundle check and the runtime vectors. `cargo xtask check-ess` passes
unchanged; if it prints drift because a generated table reads `ConfinementProperty`, the
regenerated file is part of this change.

## Steps

The items above are not independent. One branch, five steps; only step 2 has parts that are
worked at the same time, by at most two agents, each in a worktree of its own.

1. **Item 1 and its compile fan-out, one agent.** `ConfinementRequest`, `AppliedConfinement`
   and `CapabilitySnapshot` derive no `Default`, so every struct literal of them gains a line:
   48 files hold a `ConfinementRequest` literal, `crates/substrate-host/src/process.rs` among
   them. This step also carries the exhaustive matches over `ConfinementProperty`
   (`crates/substrate-wire/src/confinement_facts_tests.rs`, and
   `xtask/src/bundle/confinement_facts.rs` with its rule by version of item 5) and the six
   entries of `REFUSAL_CODES` (`crates/substrate-daemon/src/app/health.rs`), which no later
   step writes. The host crate's constants are not rebound here: that is part A's. The list
   `crates/substrate-host/tests/confinement-public-items.txt` holds the wire crate's items
   too, so this step regenerates it (`SUBSTRATE_BLESS_PUBLIC_ITEMS=1 cargo test
   -p b10x-substrate-host --test confinement_public_items`) and runs that target. Proof:
   `cargo check --workspace --all-targets --release --locked`, the wire crate's tests, that
   target, and `cargo xtask check-bundles 0.26.0 0.27.0 0.28.0`, which also holds that a
   start without the lists has the widest result bundle `0.28.0` states. The host crate's
   library tests are red after this step, in `the_published_set_is_the_linux_adapters_proof`,
   until part A.
2. **After step 1, side by side, file sets disjoint:**
   - A, one agent, item 3 then item 2: `crates/substrate-exec-gate`,
     `crates/substrate-host` and `crates/b10x-substrate-cli/src/confine.rs`, whose match over
     `ConfinementError` the host crate's test build compiles. The two items write the same
     files (`probe.rs`, `exec_gate.rs`, `lib.rs`, `confinement/mod.rs`,
     `confinement/linux.rs`). A also binds the host's code constants and
     `MAX_EXEC_DENIED_PATHS` to the wire crate's (item 1), and regenerates the public-items
     list last. Of item 3, A writes the source and the crate README; the sentences in
     `AGENTS.md`, `docs/design/35-executable-lists.md` and
     `website/docs/guides/confinement-profiles.md` are step 5's. Proof: the library tests of
     both packages and `--test confinement_public_items`.
   - B, item 5 without the pins: `xtask/src/bundle.rs`, `xtask/src/bundle/exec_lists.rs`,
     `xtask/src/bundle/session_exec_fields.rs` if the session start's schema check names the
     members (inferred), `xtask/bundle-source/0.29.0`, `contracts/substrate-wire/0.29.0`,
     `scripts/gate.sh`. Proof: `cargo xtask check-bundle 0.29.0` and the module's cases.
   - C, item 4: `crates/substrate-daemon/src` (`main.rs`, `runtime.rs`, `app/operations.rs`,
     `app/sessions.rs`). `HostConfig::exec_gate` exists, so C changes no file of the host
     crate. Proof: the daemon's library tests, and `driver_port` if a file is added.
3. **The pins of the advertised bundle**, which need B's rendered `bundle.json`:
   `ADVERTISED_CONTRACT_BUNDLE` and its digest, and every pin of `0.28.0` as current (*Scope*).
4. **Item 6**, which reads what steps 1 to 3 produce:
   `crates/substrate-daemon/tests/runtime_vectors.rs` and `scripts/delegated-lane.sh`, and
   `crates/substrate-daemon/tests/contract_vectors.rs` if a vector of `0.29.0` is executed
   there (inferred; its `0.28.0` cases stay).
5. **Item 7, one agent, last**: the counts and the bundle range come from step 4, and
   `AGENTS.md`, `CHANGELOG.md` and `STATUS.md` are written once.

The branch is pushed once, after step 5 and the checks under *Before it is pushed*.

## Acceptance

`bash scripts/delegated-lane.sh` on a workstation, and `release-lanes.yml` on the hosted runner,
run these cases of `crates/substrate-daemon/tests/runtime_vectors.rs` against a daemon started
with `--exec-gate`. An exec start and a session start each name an allow-list and a deny-list.
Inside each, a listed program runs, an unlisted one fails with `EACCES`, and a denied one beneath
a listed directory fails the same way. The applied record of each start's exec echoes both lists
as sent. The record is compared with what the run did: the denied program failed in the run
whose record names it.

Where the host's open withholds a fact (a path of the open on a type outside the list), each of
these cases prints `ABSENT (<cause>)`, is not counted as passed, and
`scripts/delegated-lane.sh` refuses the run, as it does for every case of its inventory.
`gate.yml` has no delegated root and reports the lane absent.

Checks:

- Design 37 is merged before any code of this story (invariant 8).
- **Portable lane.** Both facts are absent and the snapshot has no `withheld`. An exec start
  and a session start naming an allow-list, and one naming both lists, are
  `confinement.fact-unserved` (501) at the member's address. A present and empty allow-list is
  `exec.sandbox-exec-subpaths-empty` (422) on both starts. A deny entry without an allow-list
  is `confinement.exec-denied-paths-without-subpaths` (422). `null`, 65 entries, an empty entry
  and a duplicate are `request.schema-invalid` (422). A retried start with the same operation
  id and other lists is `operation.request-conflict`. Each refusal is recorded as refused.
- **Delegated lane.** Both facts are published. A start without the members has an applied
  record without them. Rows 5, 6, 7 and 8 of § 6 are each answered once. An allow entry beneath
  `/runtime` on a start with a capsule is row 7 (§ 16). Rows 2 and 3 are answered again, before
  the facts are read. One start with a Git writable root and an allow-list: an unlisted program
  fails with `EACCES` in it. Design 37 § 7 names three wire starts of the host driver, and
  this case is the run that holds the third.
- **Every refusal answer equals its row.** Each case compares code, status and address with
  its `EXEC_LIST_REFUSALS` row: rows 2 to 4 and 6 to 9 on an exec start and on a session
  start, row 5 on a `pty` session start, the only start that has it. Rows 2 to 5 compare the
  whole message with the table's text or the function's result. Rows 6 to 9 compare that the
  message holds the entry or the program the request sent; row 9 has no executed case
  (*Boundaries*). A session start has the exec start's addresses (`sandbox.exec_subpaths`,
  with no `exec.` before it): the daemon checks a session's exec with the exec start's own
  function (`validate_exec_input`, `crates/substrate-daemon/src/app/operations.rs`), as it
  does for `sandbox.requires.facts` today, and the driver's rows are addressed by the wire
  starts' `ExecLists` (item 2; design 37 § 6, amended with this plan).
- **Namespace lane.** `NAMESPACE_CASES` gains the case that both facts are absent in the probed
  snapshot. It runs only in `cargo xtask namespace-lane observe` on a workstation; unset, the
  lane prints that it is absent. The unit case that holds it on every host exists:
  `assert_never_published` in `crates/substrate-kubernetes/src/namespace/probe_tests.rs` loops
  over `ConfinementProperty::ALL`, so it covers the new property unchanged.
- **The three wire starts.** `the_wire_constructor_is_named_at_exactly_the_three_wire_sites` in
  `crates/substrate-host/src/process.rs` is replaced by a case that holds: `fn wire` is not in
  the production source, and the constructor that reads the start's `sandbox` is written at
  three production sites. `a_start_without_a_list_builds_the_argv_it_built_before` passes
  unchanged.
- `the_published_set_is_the_linux_adapters_proof` holds the published set equal to the
  adapter's proof with no exception.
- **The flag.** `every_host_only_flag_conflicts_with_the_kubernetes_driver`
  (`crates/substrate-daemon/src/main.rs`) lists `--exec-gate`. One case of the delegated
  lane, `a_dynamically_linked_helper_stops_the_daemon_before_it_listens`, a test function of
  `crates/substrate-daemon/tests/runtime_vectors.rs`, not a runtime vector and not in
  `DELEGATED_CASES`: a daemon started with `--exec-gate`
  naming the dynamically linked helper (`SUBSTRATE_EXEC_GATE_DYNAMIC`) exits non-zero with
  `config.exec-gate-invalid` before it listens. It prints `ABSENT (` where the variable is
  unset, and the lane requires its `executed` line.
- **`withheld`.** A unit case of the host driver's snapshot on a proof written by hand: one
  entry per distinct fact, address and cause, in the order fact, address, cause; no host path
  and no file-system type; no entry for a fact that is published; no entry when no helper is
  configured. A unit case of the host driver holds that an open that withholds returns the
  entries `Opening::announced` writes, each with its path and file-system type. A case of the
  daemon holds that a daemon whose driver withholds the facts starts. Residual, as design 37 § 12 accepts: no lane has a volume
  that withholds, so no run holds that a real open fills `withheld`.
- **Row 9's message on the wire.** A unit case of the host driver holds, for the cause
  `file-system-unlisted` on a mount table written as text: the wire start's refusal names the
  path of the view and the cause word, and neither the row's type field nor an entry of
  `LISTED_FILE_SYSTEMS`. A case of the bundle module refuses a vector message of `0.29.0` that
  holds `ext4`, `overlay`, `tmpfs`, `xfs` or `btrfs`.
- **The earlier bundles.** `cargo xtask check-bundles 0.26.0 0.27.0 0.28.0` passes with the
  eighth property in the wire crate, and a case of `confinement_facts.rs` holds that a bundle
  before `0.29.0` declaring `confinement.exec-denied-paths` is refused.
- **The allow-list bound.** A profile with 65 allow entries is
  `confinement.exec-subpaths-exceeded` (422) on the adapter API, one unit case.
- **The wire crate.** `validate_exec_lists` at each bound and one past it; the decoder on
  `null`; the two skipping rules. `the_refusal_codes_are_the_wire_constants`
  (`crates/substrate-daemon/src/app/health.rs`) counts the six new code constants.
- **The result reserve.** One case per start kind with 64 entries of 4095 bytes in each list:
  an exec start and a session start are each `operation.result-limit` (429) before dispatch.
  A session's own result carries no list (`widest_session` is one figure for every session);
  that the daemon's check of a session start counts its exec's record is inferred (design 37
  § 16). If it does not, the implementor reports it before writing the case.
- **Compatibility.** One vector of the new bundle holds a start without the members to its
  `0.28.0` request hash.
- **The bundle module** `xtask/src/bundle/exec_lists.rs` holds, each as a case that fails on a
  changed byte: the three bounds, `minLength` 1 and `uniqueItems` on both members of both
  starts; no `minItems` on `exec_subpaths`; `x-b10x-requires-capability` and
  `x-b10x-refusals` of each member; both members on the applied record; the fact
  `confinement.exec-denied-paths` and the member `withheld` with its three enumerations equal
  to the wire crate's; the eight names of `sandbox.requires.facts` in the order of
  `ConfinementProperty::ALL`; one vector per row of `EXEC_LIST_REFUSALS`, two for row 4; and
  that `0.28.0` declares none of it.
- `cargo xtask check-bundles 0.5.0 ... 0.29.0` and `cargo xtask check-json` pass in
  `scripts/gate.sh`. `git diff --stat origin/main -- contracts/substrate-wire` names only
  paths under `0.29.0`.
- **The advertised bundle.** `ADVERTISED_CONTRACT_BUNDLE` is `substrate-wire/0.29.0` and
  `ADVERTISED_CONTRACT_BUNDLE_SHA256` is the digest of `0.29.0`'s inner `bundle.json`, held by
  the wire crate's existing case for that pair. `git grep -n '0\.28\.0'` outside
  `contracts/`, `xtask/bundle-source/`, `CHANGELOG.md` and history sentences names no line
  that calls `0.28.0` current.
- **xfs.** A row of type `xfs` passes the place check, one unit case of
  `crates/substrate-exec-gate/src/place.rs`, and `LISTED_FILE_SYSTEMS` has four entries.
  Each test that had `xfs` as the type outside the list has `btrfs`, and the library tests of
  `b10x-substrate-exec-gate` and `b10x-substrate-host` pass. Every line that
  ``git grep -nE 'ext4[`, ]+(and )?[`]?overlay' -- crates website AGENTS.md docs/design/35-executable-lists.md``
  prints names `xfs` in its sentence, read line by line because a sentence can wrap. Design 37
  keeps its sentences on the three types of the tree it was written against. The files that change for it:
  `crates/substrate-exec-gate/src/place.rs`, `crates/substrate-exec-gate/README.md`,
  `crates/substrate-host/src/exec_gate.rs`, `exec_gate_tests.rs`, `lib.rs`, `probe.rs`,
  `confinement/mod.rs` (the refusal message), `confinement/facts.rs`, `confinement/linux.rs`,
  `confinement/systemd.rs`, `website/docs/guides/confinement-profiles.md`,
  `docs/design/35-executable-lists.md` and `AGENTS.md`.
- `website/docs/guides/deployment.md`, § *Before you install*, says: "The executable lists are
  served only on ext4, overlay, tmpfs and xfs volumes today."
- `PORTABLE_CASES`, `DELEGATED_CASES`, `NAMESPACE_CASES` and the counts `AGENTS.md` states
  move in the same change.
- **Before it is pushed**, each through the pool: the library tests of the crates that read
  the wire crate (`AGENTS.md` § *The gate*); `driver_port` if a file under
  `crates/substrate-daemon/src/` is added; the `xtask` targets but three with
  `SLOT_TIMEOUT=30m`; and `bash scripts/delegated-lane.sh`, started without the `slot` prefix
  because it mixes builds and waiting, its `cargo` calls each taking a slot through the shim.
  No job runs over five minutes without naming its time; none over 30 minutes.
- Every new test is timed before it is added. None runs over 5 s without being named with its
  time, none over 60 s, and the test step stays within 120 s.

## Boundaries

- No Rust SDK builder setter and no CLI flag for the lists; each is its own story once the
  bundle exists.
- No enforcement on the Kubernetes namespace driver, and none on `oci` and `pod`
  (`story:executable-lists-on-oci-and-pod`).
- The release image and the chart do not change:
  `story:the-daemon-image-ships-the-exec-gate-helper`. `BUNDLE_VERSION` in
  `.github/workflows/release.yml` moves, because a successor bundle moves that pin.
- An explicitly empty allow-list that means the program only is
  `story:explicitly-empty-exec-subpaths-means-the-program-only`. This story refuses the empty
  list by name, so no accepted byte changes its meaning later.
- Design 35's cases are not repeated on the wire. The wire cases prove that the request reaches
  the adapter's mechanism and that the record says so.
- Row 9 of design 37 § 6, `exec.sandbox-place-unserved`, is published with a vector and not
  executed: no lane has a volume of a type outside the list.

## Order

After `story:executable-deny-list`: it carries that story's field and fact. The store's edges
to `story:process-count-limit-hit-is-observed` and `story:exec-file-changes-are-recorded` were
set for a shared file, `crates/substrate-host/src/process.rs`. The process count's change to
that file is on `main` since release 0.27.0.
`story:exec-file-changes-are-recorded` is active and its own *Order* puts this story after
it, for `spec/confinement/confinement.yaml`, `crates/substrate-host/src/process.rs` and
`crates/substrate-host/src/probe.rs`; it also names `confinement/facts.rs`,
`confinement/mod.rs`, `confinement/linux.rs` and the public-items list. This story does not
wait for it: its design (ADR 0051, branch `design/exec-file-change-record`) is not on `main`
and waits for the operator's answer on a change of architecture, and session `runbook`, which
set that order on 2026-11-21, named this story its next gap on 2026-11-24. The file-change
story takes this story's `main` when it starts, and so does
`story:exec-file-change-record-on-the-wire`, which cuts its bundle after `0.29.0` and writes
`crates/substrate-wire/src/lib.rs`, `xtask/src/bundle.rs` and `scripts/gate.sh` too.
`story:executable-lists-on-oci-and-pod` shares no source file with this story. Both can touch
`docs/design/35-executable-lists.md` (this story's sentences on the served types) and
`spec/confinement-suite.json` (only if `check-ess` prints drift here), so the second of the two
to merge takes `main` first.
`story:exec-observation-reports-process-count-hits` follows this story and cuts the bundle after
this one's. `story:the-daemon-image-ships-the-exec-gate-helper` follows it too.
`story:explicitly-empty-exec-subpaths-means-the-program-only` changes
`crates/substrate-exec-gate` and is not worked beside this story.

## Scope

Cited means the symbol or line was read at `main` `7641b34c1`.

- `vendor/gix-pack-0.74.2/README.substrate.md`: one link (item 7). No source file of the copy
  changes.

- `crates/substrate-wire/src/lib.rs`. Cited: `ConfinementRequest`, `AppliedConfinement`,
  `ConfinementProperty`, `CapabilityFacts`, `CapabilitySnapshot`, `ADVERTISED_CONTRACT_BUNDLE`.
- `crates/substrate-wire/src/result_widths.rs` (`widest_applied`),
  `crates/substrate-wire/src/result_width_tests.rs`,
  `crates/substrate-wire/src/confinement_facts_tests.rs` (the property table and a match over
  every variant). Cited.
- `crates/substrate-host/src/process.rs`. Cited: `ExecLists::wire` and its three sites,
  `the_wire_constructor_is_named_at_exactly_the_three_wire_sites`.
- `crates/substrate-host/src/probe.rs`. Cited: `publish_confinement`,
  `PROVED_WITHOUT_A_WIRE_PROPERTY`, `the_published_set_is_the_linux_adapters_proof`.
- `crates/substrate-host/src/exec_gate.rs`, `crates/substrate-host/src/exec_gate_tests.rs`,
  `crates/substrate-host/src/lib.rs`, `crates/substrate-host/src/confinement/mod.rs`,
  `confinement/linux.rs`, `confinement/systemd.rs`, `confinement/facts.rs`. Cited: each names
  `xfs` as a type outside the list or names the three served types; the host's code constants
  are in `exec_gate.rs` and `confinement/mod.rs`.
- `crates/substrate-exec-gate/src/place.rs` (`LISTED_FILE_SYSTEMS`),
  `crates/substrate-exec-gate/README.md`. Cited.
- `crates/substrate-daemon/src/main.rs`. Cited:
  `every_host_only_flag_conflicts_with_the_kubernetes_driver`.
- `crates/substrate-daemon/src/app/operations.rs` (the `requires` step and
  `validate_exec_input`), `crates/substrate-daemon/src/app/sessions.rs` (the session start's
  result reserve), `crates/substrate-daemon/src/app/health.rs`
  (`the_refusal_codes_are_the_wire_constants`), `crates/substrate-daemon/src/runtime.rs` (the
  host driver's configuration). Cited.
- Every file with a struct literal of `ConfinementRequest`, `AppliedConfinement` or
  `CapabilitySnapshot` (step 1): in the wire, host, store, kubernetes, daemon, SDK and
  conformance crates.
- `crates/substrate-host/src/confinement/profile.rs`, `confinement/macos.rs`,
  `crates/substrate-host/src/seatbelt.rs`: they hold the host's code literals.
  `crates/substrate-host/tests/confinement-public-items.txt` and
  `crates/b10x-substrate-cli/src/confine.rs` name `MAX_EXEC_DENIED_PATHS`. The item list is
  regenerated: `ConfinementProperty::ALL` has eight entries there, and `ConfinementError` one
  variant more. `crates/substrate-host/tests/support/mod.rs` matches `ConfinementError`.
- `crates/substrate-daemon/tests/runtime_vectors.rs`. Cited: `PORTABLE_CASES` (136),
  `DELEGATED_CASES` (201), `NAMESPACE_CASES` (64).
- Pins of `0.28.0` as the current bundle. Cited: `crates/substrate-daemon/tests/tls_listener.rs`,
  `crates/substrate-daemon/tests/read_only_roots_adversary_2.rs`,
  `crates/b10x-substrate-conformance/src/registry.rs` (`PINNED_CONTRACT` and two
  `include_str!` paths), `crates/b10x-substrate-conformance/tests/local_endpoint.rs`,
  `settling_endpoint.rs`, `hostile_endpoint.rs` and `adversary_report_and_input.rs` of the
  same directory (each a `substrate-wire/0.28.0` constant),
  `crates/b10x-substrate-sdk/src/lib.rs` (vector paths), `xtask/src/site_release.rs`,
  `xtask/tests/release_workflow.rs`, `.github/workflows/release.yml` (`BUNDLE_VERSION`),
  `ROADMAP.md` ("The current development bundle is `substrate-wire/0.28.0`").
- `xtask/bundle-source/0.29.0`, `contracts/substrate-wire/0.29.0` (both new),
  `xtask/src/bundle.rs` (constants and the version gates),
  `xtask/src/bundle/exec_lists.rs` (new), `xtask/src/bundle/confinement_facts.rs` (a match
  over every `ConfinementProperty`), `xtask/src/bundle/session_exec_fields.rs`,
  `scripts/gate.sh`, `scripts/delegated-lane.sh`.
- `crates/substrate-kubernetes`. No change expected: `assert_never_published` covers the new
  property.
- `spec/`. No scenario; a regenerated file only if `cargo xtask check-ess` prints drift.
- `website/docs` (`guides/deployment.md`, `guides/confinement-profiles.md`, `guides/rust-sdk.md`,
  `concepts/model.md`, `reference/contract.md`, `status.md`, `releases.md`), `AGENTS.md`,
  `README.md`, `STATUS.md`, `docs/design/35-executable-lists.md` (the served types),
  `CHANGELOG.md`.
