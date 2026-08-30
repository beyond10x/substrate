# Design 15: a container driver is admitted by what it proves, not by a socket it can reach

**Status:** proposed · **Date:** 2026-08-30

This document precedes the ADR that `story:docker-driver-entry-gate` names as its evidence. It is
**not** a Docker driver and it writes no Docker code: it fixes what a container driver must prove at
startup before one `exec` is served through it, which container options are refused outright and by
what rule rather than by a list, which clauses of the host confinement floor a container driver can
meet and what the named refusal is where it cannot, and where a driver's name may and may not be
read. It claims **no ADR number**: `adr/` admits `accepted` and `superseded` only
(`xtask/src/adrs.rs:12`), so the number is assigned by the operator at acceptance, exactly as
[design 12](12-aperture-byte-ceiling.md) waited for one.

## Context

[Plan 03](../plan/03-container-driver.md) § *Entry criteria* names four conditions. The first — the
minimum host slice passes public-wire and driver conformance — is met (`STATUS.md` § *Current
state*, phase 3 complete). The second is `story:driver-port-carries-no-host-types`, **implemented**
on 2026-08-29: its test fixes the checkable form of invariant 4 —

> Outside the composition root, `substrate-daemon` names nothing from `substrate_host` except the
> port trait and the types the port trait's own signature forces on a caller.

(`crates/substrate-daemon/tests/driver_port.rs:12-13`.) The third and fourth are this document.

The consequence is already written down three times and acted on nowhere.
[Design 04](04-security-and-isolation.md) § 1 says *"Do not assume Docker socket access is a security
boundary: a Docker-backed deployment is root-equivalent to its host unless separately isolated by its
environment"* (`docs/design/04-security-and-isolation.md:18-20`).
[Design 01](01-contract.md) § 9 says a docker-driver daemon **is** root-equivalent on its machine and
calls that a fact of the substrate rather than a solvable defect
(`docs/design/01-contract.md:343-346`).
[ADR 0004](../../adr/0004-the-host-driver-refuses-without-linux-confinement.md) defers it in one line
(`:30`). None of them says what the daemon *does* about it — no flag, no probe, no fact, no refusal.

Two things at HEAD constrain every answer below. `HostDriverKind` has exactly one variant
(`crates/substrate-wire/src/lib.rs:1380-1383`) and the **released** `0.8.0` capability schema pins
`"driver": { "const": "host" }` (`contracts/substrate-wire/0.8.0/schemas/capability.json:11-13`),
with `facts` closed by `additionalProperties: false` (`:19`) and `CapabilityFacts` closed by
`deny_unknown_fields` (`crates/substrate-wire/src/lib.rs:1385-1387`). A container driver therefore
cannot publish a capability document that any released bundle validates.

**Where the story's own body drifts.** Its acceptance maps plan 03's third criterion — *"capability
predicates and applied-enforcement observations are stable"* — onto a driver-parameterised
conformance harness. A harness proves criterion 1 for a second driver; criterion 3 is a different
question about whether the predicate and observation *vocabulary* survives one, and § 5 answers it.
The body also says design 04 "gains the section". Design 04 is **accepted v1 design** dated
2026-08-13 (`:3`), and this repository's convention is that a decision arrives as its own proposed
design and edits the accepted document at acceptance — the path designs 12, 13 and 14 took. This is
that proposal; § *Consequences* says exactly which edit design 04 takes.

## Decision

### 1. The entry conditions are a probe, in the shape `probe.rs` already has

The host driver's whole exec capability is one conjunction of observations:

```rust
let exec = namespaces && cgroup && unprivileged && close_range && backend.is_some();
```

(`crates/substrate-host/src/probe.rs:53`.) Every clause ran something. `probe_bubblewrap` spawns a
throwaway sandbox and requires exit 0 (`:198`); `backend_binding` canonicalises the backend path,
hashes its bytes and records its device and inode (`:144`); `unprivileged` reads the effective uid
(`:51`). Nothing there reads configuration and calls the result a fact — design 02 § 3 verbatim:
*"Configured intent is not a fact. A present-but-unusable Docker socket … produces an absent
capability plus a diagnostic"* (`docs/design/02-driver-and-capability-model.md:50-52`).

A container driver's `exec` fact is the same conjunction and gains six clauses. Each is an
observation made through the runtime the driver will actually use, on a throwaway container, at
every capability snapshot:

| clause | what it must observe | host analogue |
|---|---|---|
| endpoint binding | the canonicalised endpoint, its owner and mode, and the runtime's self-reported version and API version, folded into the snapshot material | `BackendBinding` (`probe.rs:144`) |
| a container ran | a throwaway container was created, ran, exited 0 and was removed | `probe_bubblewrap` (`probe.rs:198`) |
| no route | a connect attempt **from inside** that container failed — not "network mode was set to none" | `exec.no-egress` (`probe.rs:102`) |
| read-only root, one writable path | a write outside the workspace mount failed, observed from inside | `--ro-bind` never `--bind` (`process.rs:1141`) |
| bounds read back and kill observed | the pids/memory/cpu bounds re-read from the runtime after creation, and a kill that leaves the container empty | `exec.cgroup-limits`, `exec.cgroup-kill` (`probe.rs:103-108`) |
| the container's identity is not host root | a user-namespace mapping observed from inside, not a configured `--user` | `effective_uid() != 0` (`probe.rs:51`) |

Any clause unobserved leaves `exec.*` **absent**, and the daemon then answers every start
`unserved` / `exec.sandbox-unavailable`, 501 (`crates/substrate-daemon/src/app/operations.rs:379-393`).
That refusal needs no new code and no new name, and § 5 says why that is the whole point.

**Nothing is probed at all when no container endpoint is declared**, the rule the secret-slot and
aperture probes already follow (`crates/substrate-host/src/probe.rs:60-73`), so a host-only daemon
pays nothing.

### 2. The socket is declared, never discovered

State the consequence plainly, because it is the reason for everything in this section: **anyone who
can write to the container daemon's socket can start a container that bind-mounts `/` read-write and
walk out as root on the host.** Access to the socket is not *like* root, it is root, and a driver
that acquires it acquires the machine.

That is a claim about a container runtime and not about this repository; nothing here proves it, and
§ 1's endpoint clause exists so the driver observes what its endpoint can actually do rather than
assuming a posture from a path.

**No discovery.** The driver does not probe a default socket path, does not read `DOCKER_HOST`, and
does not read a client context file. It exists only where an operator passed an explicit endpoint
flag; absent that flag the driver is not constructed and the daemon is host-only. A driver that
*finds* a socket is a driver that hands out host root by accident of file mode — a permission change
somewhere else on the machine would silently become a capability here, which is the inverse of
invariant 3.

**The authority is a published fact, not a driver name.** The snapshot gains
`exec.host-equivalent-authority`, `true` when the endpoint reaches a runtime whose containers the
§ 1.6 clause could not show unmapped from host root. This is how plan 03's exit criterion *"Docker
socket and daemon privileges are visible deployment facts"* (`docs/plan/03-container-driver.md:27`)
becomes a fact in `/v1/machine` rather than a sentence in a README — and it is the only invariant-4
respecting way to expose it, because a client may require `{fact: "exec.host-equivalent-authority",
op: "eq", value: false}` (design 02 V1 decision 2, `:85-88`) and be answered `unserved`, whereas a
client that reads `driver == "docker"` has made the driver part of the contract (`AGENTS.md:38-40`).

The posture is **not** refused outright. Design 01 § 9 already accepted it for a single trust domain
on dedicated machines (`:343-346`), and refusing it here would delete phase 5 rather than gate it.
What changes is that a deployment which cannot accept it now has a predicate to say so.

### 3. `--privileged`, and the rule that generates its whole class

Refused outright, whether it arrives as operator configuration or as request data: `--privileged`;
any capability addition; any `--security-opt` that unconfines seccomp, AppArmor or SELinux; host
`pid`, `network`, `ipc`, `uts`, `userns` or `cgroupns` sharing; device pass-through; any bind mount
substrate did not itself construct, and the container daemon's own socket most of all; and running as
uid 0 without a user-namespace mapping.

**The rule, so that list cannot grow by accident, is that the list is not the mechanism.** The driver
emits a container specification it constructs whole. The option set is fixed in its source; every
member either narrows confinement or names an input substrate already owns (the image, the argv, the
workspace and read-only mounts, the capsule). There is no pass-through field, no `extra_args`, no
merge with an operator-supplied specification. An option is admissible only if § 1's probe still
passes **with that option applied** — so a configuration that weakens the floor takes the capability
away instead of shipping a quieter exec, and the enforcement is the probe rather than a maintained
denylist.

**A request cannot name a container option because the wire has no field for one.**
`ConfinementRequest` is `deny_unknown_fields` (`crates/substrate-wire/src/lib.rs:670-671`), so a
conforming client's `privileged: true` is schema-invalid first. The typed refusal
`exec.container-option-in-request` exists so a rejected escalation reads as one rather than as a
schema typo — the shape ADR 0014 gave `exec.aperture-ceiling-in-request`
(`docs/design/12-aperture-byte-ceiling.md:94-102`).

### 4. The floor, clause by clause, with the refusal where it cannot be met

The floor is `AGENTS.md:78-84` and design 04 § 7 (`:85-100`). Nothing below weakens it; the column
that matters is the last one.

| floor clause | host mechanism | container driver | if unproven |
|---|---|---|---|
| no egress | `--unshare-net` (`process.rs:1111`), fact at `probe.rs:102` | **meetable**, but only by § 1's connect attempt from inside; a network-less container still has loopback, exactly as the sandbox netns does | fact absent → `exec.sandbox-unavailable`, 501 |
| system read-only, workspace the only writable mount | `--ro-bind` never `--bind` (`process.rs:1141`, `:1147`), workspace at `:1160`; `AppliedFilesystem::WorkspaceReadWriteSystemReadOnly` (`lib.rs:714-716`) | **meetable** as read-only mounts plus one writable bind | as above |
| declared host roots read-only (ADR 0010) | read-only binds, recorded applied (`lib.rs:701-704`) | **meetable**; the applied record already states the narrow guarantee it can make | as above |
| verified capsule at `/runtime` (ADR 0009) | digest-verified then mounted read-only (`lib.rs:70`) | **meetable**: verification happens on substrate's side before the container exists | as above |
| pids/memory/cpu bounds, whole-cgroup kill | delegated cgroup v2 subtree; facts at `probe.rs:103-108` | **conditional.** Substrate does not own the cgroup — the runtime does. The bounds are requested of the runtime and must be **read back**, and the kill must be observed to empty rather than assumed | facts absent → `exec.sandbox-unavailable`, 501 |
| sealed secret slots (ADR 0012) | memfd, `F_SEAL_WRITE\|SHRINK\|GROW\|SEAL`, `dup2` in `pre_exec`, descriptor pass-through probed every snapshot | **not meetable.** Substrate does not fork the child; the runtime does, so there is no `pre_exec` to place a descriptor in, and handing the value to the runtime's API puts it in the runtime's own state — the leak ADR 0012 exists to close | `secrets.slots` absent → `unserved` / `exec.secret-slots-unserved`, 501 (`operations.rs:506-516`), already asserted by the portable lane (`crates/substrate-daemon/tests/runtime_vectors.rs:11`) |
| declared egress apertures (ADR 0013) | a forked forwarder that `setns`es into the sandbox's user and net namespaces and joins the run's cgroup (`egress.rs:490-496`) | **unproven.** Whether the same handback works against a container the runtime created is a kernel-permission question nobody here has observed | `exec.egress-apertures` absent → `unserved` / `exec.egress-apertures-unserved`, 501 (`operations.rs:453-463`) |
| cleared environment, no ambient daemon credential | `--clearenv` (`process.rs:1115`) | **meetable** for the environment; the endpoint credential not being reachable from inside is a § 1 clause, never an assertion | fact absent → 501 |

Two rows are the honest answers this document exists to give. **Sealed slots are absent on a
container driver**, so a confined vendor harness holding a credential stays a host-driver capability
until a separate ADR designs a container mechanism; there is no weaker delivery, per ADR 0012's own
consequence. **Apertures are absent until a spike observes them**, exactly as design 10 refused to
accept the host mechanism on reasoning alone before ADR 0013 (`:112-116`).

### 5. Invariant 4: what a second driver may add, and what it may not change

Plan 03's third criterion cannot mean *unchanged*. `HostDriverKind` gains a variant, the capability
schema's `const` becomes an enum, and `CapabilityFacts` gains members — all three are wire
(`crates/substrate-wire/src/lib.rs:1380-1383`, `:1385-1387`;
`contracts/substrate-wire/0.8.0/schemas/capability.json:11-13`, `:19`). Stable means: **the second
driver adds facts and changes the meaning of none.** `exec.no-egress: true` states the same sentence
on both drivers or it is not the same fact, and § 1's clauses are written as observations precisely
so the sentence is checkable rather than conventional.

The observation vocabulary is mechanism-shaped in exactly one place, and it is survivable.
`AppliedConfinement.cgroup` is a required `String` (`crates/substrate-wire/src/lib.rs:687-689`). A
container driver fills it with the cgroup path the runtime reports; it is never empty and never
invented, because the same probe clause that would leave it unfillable also removes
`exec.cgroup-limits`, and exec is then `unserved` before dispatch. Everything else already names a
guarantee rather than a mechanism: `SandboxProfile` has one variant, `workspace` (`:664-667`), and it
**stays one** — the profile is the guarantee shape, and a `docker` profile variant would be the
contract bending around the driver. `AppliedNetwork` is `none` or an applied aperture (`:739-742`),
which is a statement about reach and not about how reach was withheld.

The daemon's admission path is already driver-blind and this is the load-bearing evidence: the exec
precondition reads `facts.exec_namespaces`, `facts.exec_cgroup_limits`, `facts.exec_cgroup_kill`,
`facts.exec_no_egress` and never a driver kind (`crates/substrate-daemon/src/app/operations.rs:379-393`).
A container driver that proves nothing is refused by code written for the host driver, unchanged.

The driver name stays **provenance**. `/v1/machine` publishes it and every resource records it
(design 02 § 1, `:11-13`), because an auditor needs to know what ran a thing. A client that changes
its request because the string reads `docker` has made the driver part of the contract; the predicate
it should have written names a fact.

*One stale citation, in files this document does not touch.* `crates/substrate-daemon/tests/driver_port.rs:7`
and the implemented story both cite `crates/substrate-host/src/lib.rs:171` for `pub trait Driver`; at
`d65db79` it is `:201`. The test scans text and never reads the number, so nothing is broken.

### 6. The conformance journey takes the driver as a parameter

The journey is design 07 § 2's twelve operations
(`docs/design/07-specification-and-conformance.md:44-57`), driven over the wire. The harness takes the
driver as a parameter and **the host driver passes unchanged** — that is the story's acceptance, and
an unchanged pass is the only evidence that parameterising it did not quietly rewrite what it
asserts.

What it must not become is a fixture with a driver switch in it. Where a driver legitimately differs,
the fixture reads a capability fact before it asserts, and a case whose fact is absent is **absent,
never reported as passed** — the shape the delegated lane already has (`AGENTS.md:188-194`). The
clean-room runner is the precedent and the enforcement:
`crates/substrate-daemon/tests/runtime_vectors.rs` spawns the shipped binary and asserts only on the
wire, so a response that needed a driver-specific type to read would not compile there. That is where
plan 03's *"a client does not branch on Docker-specific response types"* (`:21`) stops being a hope.

### 7. Named refusals

| condition | class | code | address | status |
|---|---|---|---|---|
| a start names a container option, at any depth | `refused` | `exec.container-option-in-request` | `sandbox` | 422 |
| exec asked for while the driver proved no floor | `unserved` | `exec.sandbox-unavailable` *(exists)* | `exec.namespaces` | 501 |
| a secret slot on a container driver | `unserved` | `exec.secret-slots-unserved` *(exists)* | `secret_slots` | 501 |
| an aperture before the container spike | `unserved` | `exec.egress-apertures-unserved` *(exists)* | `exec.network-aperture` | 501 |
| the runtime's identity moved since admission | `refused` | `exec.capability-stale` *(exists)* | `capability_snapshot` | 422 |
| an endpoint flag whose probe fails a § 1 clause with a configured option applied | **startup**: the daemon does not start | — | — | — |
| no endpoint flag | **startup**: the driver is not constructed; the daemon is host-only | — | — | — |

Class-to-status is the existing map (`crates/substrate-daemon/src/app/operations.rs:1250-1257`). Four
of the five wire refusals **already exist and already fire on facts**, so the entry gate costs one new
code. The two startup rows are `bail!`s at construction, the shape `serve_tcp` uses for a posture it
will not serve (`crates/substrate-daemon/src/runtime.rs:765-776`) — a daemon that will not start is
louder than a capability that is quietly absent, and a misconfigured *endpoint* is the one thing here
whose blast radius is the whole host.

### 8. Successor bundle `0.9.0`, provisionally

`contracts/substrate-wire/0.8.0` is the frontier, and records `predecessor: 0.7.0`, `adds_routes: 0`,
`preserves_routes: 26` (`contracts/substrate-wire/0.8.0/bundle.json:5-10`).
[Design 13](13-pty-sessions.md) (`:166-168`) and
[design 14](14-network-session-authority.md) (`:175-178`) **both already name `0.9.0`
provisionally**; this makes three. The number belongs to whichever is accepted first and the others
move to its successor. This one adds no route: it widens `capability.json`'s `driver` from a `const`
to an enum, adds `exec.host-equivalent-authority` and the container facts to a closed fact map, and
adds one refusal code. Its `cargo xtask check-bundle 0.9.0` goes into `scripts/gate.sh` in the same
change — a bundle whose check is not in the gate is unverified from the next commit onward — under
`xtask/bundle-source/0.9.0/`. Any constant it binds comes from `xtask/src/bundle.rs` and **never**
from `xtask/src/render.rs`, whose sha256 every rendered `bundle.json` carries (`AGENTS.md:203-209`).
Earlier directories keep their bytes (invariant 6, `AGENTS.md:43-48`).

**That frozen `const` is itself the gate.** Because `0.8.0` pins `"driver": { "const": "host" }`, a
container driver cannot publish a capability document any released bundle validates. The entry gate
is therefore enforced by invariant 6 and by `cargo xtask check-bundle`, not only by this document: no
Docker code can ship conformant before the successor exists, and the successor cannot be cut before
the decisions above are accepted.

## Consequences

A container driver becomes writable only after it can prove, on the machine it runs on, the same
floor the host driver proves — and where it cannot, the capability is absent and the request is a
named refusal that already exists. A deployment that cannot accept a root-equivalent daemon socket
gets a predicate instead of a warning in prose.

**At acceptance, design 04 gains a section and design 01 § 9 gains a sentence.** Design 04's new
section is § 4's table plus § 2's socket consequence — the container counterpart of § 7's *minimum
host guarantee*, in the same voice — and design 01 § 9's third bullet gains the pointer to the fact
that makes root-equivalence machine-readable. Neither is edited here: this document is proposed, and
an accepted v1 design is amended when the decision it depends on is accepted.

**Nothing about phase order changes here.** Plan 03 stays *deferred* and `ROADMAP.md` phase 5 stays
*pending* while phase 4 is in progress (`ROADMAP.md:5`, `:13`, `:14`); those two move together, in the
change that exits phase 4, and the story's own acceptance says so.

The costs are stated rather than hidden. The second driver is strictly weaker than the first on two
axes — no sealed secret slots, and no egress apertures until a spike — so a container-backed exec is
not a drop-in replacement for a host-backed one, and any client that needs those facts will be
answered `unserved` on it. `--privileged` and its class are refused by construction, which also means
a workload that genuinely needs a device is not served by this driver and owes its own decision.

The gate proves the negative half on a hosted runner: the request-side refusal, the absent facts, the
class-to-status mapping and the successor schema shape. The positive half — a container that has no
route, a read-only root observed from inside, a bound read back, a kill that empties — needs a machine
with a container runtime, and is reported **absent rather than passed** (invariant 3,
`AGENTS.md:36-37`).
