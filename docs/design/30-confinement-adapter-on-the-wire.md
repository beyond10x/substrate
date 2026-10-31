# Design 30: what the wire says about the confinement adapter

Status: accepted, 2026-10-31, with the recommended answer to each of D1 to D10 of § 9
([ADR 0048](../../adr/0048-the-wire-states-what-a-confinement-adapter-proves-never-its-name.md)).
Proposed 2026-10-28. Owned by `story:confinement-adapter-on-the-wire`. Carried by the successor
bundle `0.26.0`, the next free version under D10: `0.25.0` is design 34 § 11's.

Invariant 8 asks for a design before a contract change. This note settles the question the
confinement epic left open: what the wire states about which adapter served a request, and how a
consumer chooses an adapter when the wire does not name one. § 9 lists the decisions it leaves to
the operator, each with a recommended answer.

## 1. The deferral this closes

`epic:confinement-contract-family`, § *Out of scope*:

> Any wire change: a successor bundle naming adapters and their facts on the wire
> (`HostDriverKind` variants, ADR 0037 decision 5) is the next epic's first story.

The same epic, § *Order*, item 2, on `story:confinement-contract-on-linux-and-local`:

> No wire change: `HostDriverKind` is wire format
> (`crates/substrate-wire/src/lib.rs:3368`), so naming adapters on the wire waits for a successor
> bundle.

[ADR 0044](../../adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md),
§ *Consequences*:

> A wire publication of adapters or facts is a `HostDriverKind` change and needs a successor bundle
> (invariant 6). Atlas ADR 0109 names that as a later step.

Atlas ADR 0109 (`architecture/adr/0109-confinement-is-one-substrate-contract-with-adapters.md` in
the atlas repository), § *Consequences*: S0 "changes no wire type, because naming adapters on the
wire is a `HostDriverKind` change". Its decision 2 is titled **"Facts, not names."**

Each of these assumed that publishing adapters means a `HostDriverKind` change. This design argues
that it does not: the wire publishes what an adapter proves, and `HostDriverKind` stays as it is.

## 2. What the wire and its consumers do today

- **One driver kind.** `CapabilitySnapshot.driver` is `HostDriverKind`, a closed enum with the one
  variant `host` and no `#[non_exhaustive]` (`crates/substrate-wire/src/lib.rs`). The released
  schema pins it: `"driver": {"const": "host"}` in
  `contracts/substrate-wire/0.23.0/schemas/capability.json`. Design 02 § 1 and its V1 decision 1:
  one active driver per daemon, and "ordinary client commands target resource identities and
  requirements rather than a driver name".
- **The SDK already drops the driver.** `Machine` in `crates/b10x-substrate-sdk/src/model.rs` has
  `driver_version` and the facts, and no `driver` field. It derives `exec_cgroup_limits` and
  `exec_cgroup_kill`, which are named for the Linux mechanism.
- **Facts that name a mechanism.** `exec.namespaces`, `exec.cgroup-limits` and `exec.cgroup-kill`
  are Linux facts. On a macOS daemon (ADR 0037 step 2) they are absent, which is correct, but a
  client then has no fact for what macOS does prove: sealed paths, a private `/tmp`, paths that are
  host paths.
- **The applied record carries a Linux member.** `AppliedConfinement.cgroup` is a required string.
  A run confined by Seatbelt has no cgroup, so a macOS daemon could only fill it with an invented
  value, which invariant 3 forbids.
- **The wire already has a declared-unconfined path.** The host-process family
  ([ADR 0036](../../adr/0036-host-processes-are-a-supervised-resource-family.md)) behind the
  `process.host` fact: its record always says `profile: host` and `confinement: none`. That is a
  separate resource family with an explicit statement, not an adapter name on the exec route.
- **The macOS daemon waits for this.** `HostDriver::open` refuses `host.platform-unserved` on
  macOS (`crates/substrate-host/src/lib.rs`, `PLATFORM_UNSERVED`), and
  `story:macos-adapter-on-seatbelt` keeps that "until the successor-bundle story outside this
  epic".
- **A stale snapshot is already refused.** A start names `sandbox.capability_snapshot`; a
  mismatch is `exec.capability-stale` (422) before dispatch
  (`crates/substrate-daemon/src/app/operations.rs`).
- **Operators already select by facts.** [ADR 0045](../../adr/0045-operational-health-and-metrics-use-a-separate-listener.md)
  makes readiness depend on `--ops-require-fact`, and its metric catalogue is closed.
  [ADR 0046](../../adr/0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md)
  runs the host driver in a pod, so that daemon answers `driver: host`.

Consumers, read from sibling checkouts at the time of writing:

| consumer | how it reaches Substrate | what it reads about the driver |
|---|---|---|
| harness | `b10x-substrate-wire` and `b10x-substrate-host` by Git revision (`harness/crates/harness-substrate/Cargo.toml`), and the daemon socket | `Facts.driver` as an optional string it never branches on (`harness/crates/harness-substrate/src/lib.rs`); `confines_execution` reads `exec.argv-only`, `exec.cgroup-limits` and `exec.resource-usage` |
| loom | `b10x-substrate-host` and `-wire` by Git revision (`loom/crates/loom-intake-slice/Cargo.toml`), in process | the host API; no wire driver field |
| takt | `b10x-substrate-host` in process (`takt/Cargo.lock`) | the host API's `confinement::open`, which takes an adapter name by design (ADR 0044 decision 8) |
| mantle | `b10x-substrate-sdk` by Git revision (`mantle/crates/mantle/Cargo.toml`) | the SDK's `Machine`, which has no driver field; test fixtures write `"driver": "host"` |

No consumer branches on `driver`. Keeping it that way is the point of invariant 4.

## 3. Options for what the wire names

| | option | what it costs |
|---|---|---|
| **A** | **facts only.** The wire publishes the families and properties the serving adapter proved. `driver` stays `host`; no adapter, mechanism or profile name appears | a client cannot tell Seatbelt from bubblewrap, and should not need to. An operator who wants to know reads the operational listener (§ 6) |
| B | facts plus an opaque, informational `adapter` member such as `"linux"`, documented as "not for selection" | a readable name is a name a client will branch on. The SDK already removed `driver` from `Machine` for that reason. Diagnostics alone do not justify a member of the client contract |
| C | new `HostDriverKind` variants (`linux`, `macos`, …) | `HostDriverKind` has no `#[non_exhaustive]`, so a consumer that matches it exhaustively stops compiling. Every adapter would need a bundle. A client reading `driver` has "made the driver part of the contract" (invariant 4). ADR 0044 Considered A refused the same shape for drivers |
| D | a deployment profile name, such as ADR 0046's node-bound Kubernetes serving profile | a profile is operator configuration. What a client needs from that profile, such as whether execution is enabled or whether quotas are served, is already a fact. A name adds a second, unverified way to say it |

**Recommendation: A.** It is atlas ADR 0109 decision 2 applied to the wire.

## 4. Design (option A)

### 4.1 `driver` keeps its bytes and gets a fixed meaning

`driver` stays the constant `host`, meaning the daemon's one driver contract: workspaces and
processes on the machine the daemon runs on. The macOS daemon of ADR 0037 step 2 also answers
`host`. `HostDriverKind` gains no variant for a confinement adapter. `driver_version` stays a
version string; a test asserts it never encodes an adapter or mechanism name.

### 4.2 Which adapters serve the wire

The daemon opens one adapter at startup (design 02, V1 decision 1). The operator chooses it,
today by platform:

- `linux` on Linux, the existing host driver;
- `macos` on macOS, once its daemon story lands.

Whether that adapter runs inside a `systemd` scope is operator configuration, and it shows only
as facts. The daemon's exec and session routes never serve these adapters:

- **`local`.** The safety envelope's isolation set is a floor, so a declared-insecure run on the
  exec route would be a quiet downgrade. The wire's declared-unconfined path stays the
  host-process family, which says `confinement: none` explicitly.
- **`oci` and `pod`.** ADR 0044 decision 8 puts them behind substrate-host's API only.

None of these four reaches the exec route, so their facts are not published: `declared_insecure`,
`local_permitted`, `host_network` and `image_digest_pinned`.

### 4.3 The capability document publishes families and properties

The successor bundle adds optional facts to `CapabilityFacts`. Each is `true` when the opened
adapter proved it, and absent otherwise; an absent fact is never written `false`.

- **One fact per family**, in ADR 0044 decision 1's order: `confinement.family.process-run`,
  `.process-watch`, `.output-read`, `.wait`, `.workspace`, `.filesystem-view`,
  `.network-egress`, `.network-ingress`, `.limits` and `.audit`. `network-ingress` is never
  published until ADR 0044 decision 5's reserved fields are filled. `network-egress` is published
  only where the operator declared an aperture, matching `exec.egress-apertures`.
- **One fact per mechanism-neutral property** from the `Fact` type of `spec/confinement/`:
  `confinement.whole-tree-kill`, `confinement.ipc-socket-allowlist`,
  `confinement.ipc-signals-contained`, `confinement.sealed-paths`, `confinement.exec-subpaths`,
  `confinement.private-tmpdir` and `confinement.paths-are-host-paths`.

Expected sets, taken from the declared sets in `spec/confinement/confinement.yaml` above
`OpenAdapter`:

- `linux`: the families above except `network-ingress`, and `network-egress` only with an
  aperture; the properties `whole-tree-kill`, `ipc-socket-allowlist` and `ipc-signals-contained`.
- `macos`: no `limits` and no `network-egress`; the properties `ipc-signals-contained`,
  `sealed-paths`, `exec-subpaths`, `private-tmpdir` and `paths-are-host-paths`.

The last property is one a client needs on the wire: under it, a read-only root's `mount` must equal
its `host_path`, which a client can know only from a fact.

The existing `exec.namespaces`, `exec.cgroup-limits`, `exec.cgroup-kill` and `exec.no-egress` keep
their bytes and meanings. Nothing is renamed or removed. The new facts state, independent of
mechanism, what those facts state for Linux.

### 4.4 A start can require families and facts, and the daemon refuses by name

`ConfinementRequest` (the `sandbox` member of an exec start, and so of a session start, which
embeds `ExecStartInput`; design 27) gains an optional closed object:

```json
"requires": {"families": ["process-run", "limits"], "facts": ["whole-tree-kill"]}
```

- Both lists are closed enums of the names in § 4.3, without the `confinement.family.` or
  `confinement.` prefix. Each is unique and bounded by its enum's size. An unknown name, a
  duplicate or an empty object is `request.schema-invalid` (422).
- **Checked after `exec.capability-stale` and before every field-specific check.** The first
  family the snapshot does not serve, in ADR 0044's order, is `confinement.family-unserved` (501).
  The first property it does not prove, in the `Fact` order, is `confinement.fact-unserved` (501).
  These are ADR 0044's codes and statuses, so a client meets the same refusal through the wire as
  through the host API. The refusal is recorded like every refusal before dispatch.
- Absent `requires` behaves exactly as today. The checks a field implies keep their codes, for
  example `exec.egress-apertures-unserved` for an aperture list.
- The applied record echoes `requires` when the start carried it: these were checked and held.

### 4.5 The applied record stops requiring a Linux member

`AppliedConfinement.cgroup` becomes optional in the successor schema. It is present whenever
`linux` applied a cgroup, as on every run today, and absent when the adapter applied none. No
value is invented.

### 4.6 The snapshot binds the adapter

The snapshot digest (design 02, V1 decision 5) adds the opened adapter's name and its proved
families and properties to its input. The name is bound internally and never shown. A daemon
restarted on a different adapter therefore publishes a new snapshot, and a start that names the
old one is `exec.capability-stale`, as today.

## 5. How a consumer selects an adapter by facts alone

1. **The operator chooses where.** Each daemon opens one adapter. A deployment that needs, for
   example, limits names `confinement.family.limits` through ADR 0045's `--ops-require-fact`, so a
   daemon without it never becomes ready. ADR 0046's per-pod daemons are selected the same way.
2. **The client chooses among daemons.** It reads `/v1/machine` and evaluates its own required
   families and properties, as harness already evaluates predicates (`Facts::admits`). Where it
   holds several endpoints, such as ADR 0046 ordinals or a Linux and a macOS daemon, it picks one
   whose facts satisfy it. It never chooses by name.
3. **The daemon holds the client to it.** The start carries `requires`, and the daemon refuses by
   family or fact name against the snapshot the start names. A client never has to trust that the
   facts it read are still the facts that run it.

In-process embedders (loom, takt) keep `confinement::open(AdapterName, …)`. ADR 0044 decision 8
makes choosing an adapter by name a host-API act, outside the wire.

## 6. Observations and events

- **Capability document:** the facts of § 4.3. `driver` stays `host`.
- **Applied record:** `requires` echoed, and `cgroup` present exactly when applied (§ 4.4 and
  § 4.5).
- **Operational listener (operator only, not the client wire):** ADR 0045's closed catalogue gains
  `substrate_adapter_info{adapter}`, a gauge that is always 1. Its label takes values from the
  generated `AdapterName`, restricted to the adapters the daemon can open. This is the one place an
  adapter name appears, and only for the operator (decision D7).
- **Events:** no new event kind. The two refusals arrive as `operation.refused`, like every refusal
  before dispatch. `snapshot.created` is unchanged; a new snapshot after an adapter change is
  already an event. No event names an adapter.

## 7. Refusals

| code | status | when | new on the wire? |
|---|---|---|---|
| `confinement.family-unserved` | 501 | `requires.families` names a family the admitted snapshot does not serve | yes, published with a vector |
| `confinement.fact-unserved` | 501 | `requires.facts` names a property the snapshot does not prove | yes, published with a vector |
| `request.schema-invalid` | 422 | an unknown name, a duplicate, an empty `requires`, or a member outside the closed object | existing |
| `exec.capability-stale` | 422 | the start names a snapshot the daemon no longer holds, including after an adapter change | existing |
| `exec.sandbox-unavailable` | 501 | no confinement at all, as today | existing |

These refusals never reach the wire:

- `confinement.adapter-unserved`: a client never names an adapter. An adapter that cannot open
  leaves the daemon unready (design 02, V1 decision 4).
- `confinement.local-reason-missing` and `confinement.local-refused`: `local` is not served
  (§ 4.2).
- `confinement.image-unpinned`: no image on the exec route.
- `confinement.reserved-field-unserved`: no wire field reaches the reserved ingress-route and
  rate-bound fields.

## 8. Consumer compatibility and the successor bundle

**Bundle.** `contracts/substrate-wire/0.25.0` (or the next free version) is cut with
`cargo xtask render-bundle` from `xtask/bundle-source/<version>/`. It adds no route; its
compatibility block states the predecessor and pins the unchanged route counts. It contains:

- the optional facts of § 4.3;
- the optional `sandbox.requires` on both starts;
- the optional `requires` and optional `cgroup` on the applied record;
- the two refusals, each with a vector and held to its `substrate_wire` constant, as earlier
  bundles do.

The schema's `"driver": {"const": "host"}` stays. `ADVERTISED_CONTRACT_BUNDLE` and its digest move
to the new bundle. `scripts/gate.sh`'s `check-bundles` list and `check-json` gain the version, and
AGENTS.md invariant 6 records it. Every released bundle stays byte for byte.

**Who has to move:**

- **The Rust SDK** refuses a daemon advertising another bundle (`SdkError::ContractMismatch`), as
  for every successor. `Machine` gains mechanism-neutral helpers (served families, proved
  properties) beside the existing fields, which stay.
- **harness** reads facts as an open map, so a newer daemon stays readable to it. Its
  `confines_execution` keeps working on Linux and correctly withholds `run` on macOS, because
  `exec.cgroup-limits` is absent there. Moving it to `confinement.family.limits` is harness's own
  change after its pin moves. Not done here.
- **loom and takt** use the host API in process and may keep naming adapters there. The Rust type
  `AppliedConfinement` gains `requires` and its `cgroup` becomes `Option<String>`, so a crate
  that constructs or exhaustively destructures it (loom does, in
  `loom-intake-slice/src/confinement.rs`) must adapt when its substrate pin moves.
- **mantle** is unaffected until its SDK pin moves. Its fixtures' `"driver": "host"` stays valid.

## 9. Decisions left open, each with a recommendation

| id | question | options | recommendation |
|---|---|---|---|
| D1 | What does the wire say about the adapter? | A facts only; B an opaque informational id; C `HostDriverKind` variants; D a deployment profile name | **A** (§ 3) |
| D2 | Shape of the family facts | one boolean fact per family, present only when served; one array fact; one closed object with every family as a key | **one boolean per family.** Absent stays absent, and the predicate client harness already has evaluates `eq true` without a membership operator |
| D3 | Mechanism-neutral properties: new names or reuse? | new `confinement.*` facts beside the frozen `exec.*` Linux facts; redefine `exec.cgroup-kill` and the others as mechanism-neutral | **new names beside them.** Redefining a released fact's meaning would contradict invariant 6 |
| D4 | Does a start carry `requires` that the daemon checks? | yes, in this bundle; no, clients evaluate facts themselves | **yes.** Only the daemon can refuse against the snapshot it runs with, and only a daemon refusal names the family (invariant 3) |
| D5 | Make `AppliedConfinement.cgroup` optional now, or in the macOS daemon story's own bundle? | now; later | **now.** The macOS daemon story then needs no wire change: its facts and its applied record already fit |
| D6 | Does the daemon ever serve `local` on the exec or session route? | no; yes behind an operator flag with a reason | **no.** The host-process family is the declared-unconfined path, and the isolation floor stays a floor |
| D7 | Is the adapter visible to the operator? | `substrate_adapter_info{adapter}` on ADR 0045's operational listener; nowhere | **on the operational listener only.** It changes ADR 0045's catalogue, not the client contract. If that listener has not shipped, add the gauge when it does |
| D8 | Does acceptance need an ADR? | accept this design alone; record an ADR | **an ADR (0047 or the next free number) at acceptance.** It narrows ADR 0044's consequence that wire publication "is a `HostDriverKind` change". Atlas ADR 0109 states the same expectation, so atlas is told in that ADR's text. No atlas file changes here |
| D9 | Later wire drivers (plan 03's Docker, the Kubernetes namespace driver, Firecracker): do they get `driver` values? | decide here; decide in each driver's entry-gate story | **in each entry-gate story.** This design fixes only that a confinement adapter is never a `driver` value |
| D10 | Bundle number | `0.25.0`; the next free version when the story starts | **the next free version when the story starts.** `0.24.0` is taken, and another change may land first |

## 10. Out of scope

- The macOS daemon itself (ADR 0037 step 2): its own story. Under D5 it needs no further wire
  change.
- Filling the reserved ingress-route and rate-bound fields (ADR 0044 decision 5).
- Any change in a consumer repository. § 8 says what each would do.
- A `b10x-substrate` CLI flag for `requires`. It can follow as its own change.
