# Design 10: destination-bound egress apertures

**Status:** accepted as [ADR 0013](../../adr/0013-egress-apertures-are-declared-by-the-operator.md) · **Date:** 2026-08-29

This document precedes the ADR that `story:destination-bound-egress` names as its first evidence. It
fixes the aperture's shape, declaration surface, refusals and observation, and reports what the
mechanism at HEAD can and cannot be made to do. Its successor bundle number assumes it is the only
0.5.0 change; if a sibling design lands first, this one moves to that bundle's successor.

## 1. Problem

A sealed secret slot without egress unlocks nothing. A confined `codex` or `claude` process holding a
model credential on a declared descriptor still sits in a network namespace whose only interface is
loopback, so the credential is unspendable and the run is not confined — it is dead.
[Plan 04](../plan/04-direct-byte-plane.md) says exactly this: "A live vendor harness is refused until
the required secret and egress capabilities exist."

The consumer is concrete. `atlas/ROADMAP.md` records *autodev → the fleet* as dispatching `codex` and
`claude` only, and *harness → llmgw* as **intended** — three doc comments name `llmgw` as an example
endpoint (`harness/crates/harness-cli/src/lib.rs:377`, `:425`,
`harness/crates/harness-responses/src/lib.rs:71`), no dependency, no contract. Both are one shape:
**one confined process must reach exactly one model endpoint and nothing else.**

The floor does not move. `AGENTS.md` § *Safety envelope* lists namespace no-egress in the enforced
isolation set and says weakening any member is invariant 3's named refusal, never a quiet downgrade.
An aperture is therefore **not** a relaxation of the default but a separately declared, probed and
observed capability a deployment may add; ordinary execution keeps `--unshare-net` and no route out,
and a daemon that cannot prove an aperture publishes no aperture.

## 2. What exists

| Fact | Evidence |
|---|---|
| Every exec and pipe session runs under `--unshare-net` with `--unshare-user/ipc/pid/uts` | `crates/substrate-host/src/process.rs:901-905` |
| The startup probe runs the same argv before publishing anything | `crates/substrate-host/src/probe.rs:180-184` |
| The sandbox mounts `/usr`, `/bin`, `/lib`, `/lib64`, `/proc`, `/dev`, `/tmp` — **no `/etc`** | `crates/substrate-host/src/process.rs:901-926` |
| `exec.no-egress` is published only when the whole exec predicate holds, and `exec` is withheld when the daemon is root — `unprivileged = effective_uid() != 0` | `crates/substrate-host/src/probe.rs:49-51`, `:78` |
| Admission refuses `network: "aperture"` as `unserved` before dispatch, and the driver refuses it again independently | `crates/substrate-daemon/src/app/operations.rs:290-300`, `crates/substrate-host/src/process.rs:810-815` |
| The request word exists — `NetworkMode::{None, Aperture}` — but the **applied** word does not: `AppliedNetwork` has one variant | `crates/substrate-wire/src/lib.rs:595-599`, `:657-660` |
| 0.4.0 pins applied network to `"network": {"const": "none"}` in both branches | `contracts/substrate-wire/0.4.0/schemas/resource.json`, `$defs/confinement-applied` |
| The refusal is a frozen vector covering `security.no-egress`, with postconditions `/probes/network_was_weakened == false` and `dispatch_count == 0` | `contracts/substrate-wire/0.4.0/vectors/http/egress-unserved.json` |
| The only **executed** no-egress proof is one confined `socket.create_connection(('1.1.1.1',53),1)` asserted to exit non-zero | `crates/substrate-daemon/tests/runtime_vectors.rs:620-640` |
| The portable lane proves the typed refusal without confinement | `crates/substrate-daemon/tests/pipe_session.rs:800-824` |
| A spawn barrier already exists between namespace creation and `exec`: `--block-fd`, released after cgroup attach | `crates/substrate-host/src/process.rs:947-948`, `:398-402` |

**`exec.no-egress` is derived, not observed.** It is `exec.then_some(true)` where `exec` means the
bubblewrap probe succeeded (`probe.rs:51`, `:78`); nothing in the probe attempts a connection, and a
black-box lane is not a capability fact. Design 02 § 3 publishes a capability only after its probe
succeeds — an aperture fact must exercise its mechanism, not infer it.

**A hosted runner cannot prove any of this.** `.github/workflows/gate.yml:110-111` and `:129-131`
report the portable lane as run and the delegated lane as **absent — not executed, not passed**,
because a hosted runner is not inside a delegated cgroup subtree. CI proves the typed refusals and the
schema shape; reachability of a declared destination needs a self-hosted runner.

## 3. The aperture

**Shape.** A destination is one tuple — `host` (a name or a literal address), `port`, `protocol` —
matched exactly. The first slice serves `tcp` and nothing else: no UDP, no ICMP, no raw sockets, no
CIDR, no wildcard, no port range.

**Named, declared by the operator.** An aperture has an operator-chosen name, declared in daemon
configuration at startup alongside the existing clap surface
(`crates/substrate-daemon/src/main.rs:16-65`): one repeated
`--egress-aperture <name>=<host>:<port>/tcp`, `SUBSTRATE_EGRESS_APERTURE` in environment form.
Changing the set changes `config_generation` and invalidates every snapshot (design 02 V1 decision 5).
This is deployment authority, held where the daemon's other authority is held.

**A request carries a name, never a destination.** The successor bundle's sandbox block gains an
optional `aperture` field whose value is a declared name, and **no** destination field at any depth.
`docs/design/04-security-and-isolation.md:62-65` already forbids widening by request fields; a raw
destination in a request body is that widening, spelled out. It is the shape design 04 § 4 fixes for
credentials — select a configured binding, never supply a new destination — and deliberately *not*
ADR 0010's read-only roots, which are caller-supplied because a directory carries no reach.

**DNS is outside the aperture.** Design 05 § 3 keeps DNS out of substrate v1, and the sandbox has no
`/etc` to hold a resolver configuration (§ 2). The daemon resolves the declared host once, at
declaration, and pins the aperture to that address; the sandbox gets no resolver and performs no
lookup. Design 04 § 4 requires matching "after resolution and on connect", and § 8 fixes the same
pattern for Git — pin to a validated address while TLS verifies the configured name. § 9 decision 2
carries the consequence for a vendor harness.

**Observed as applied.** The run's record states which aperture was installed and to what — reported,
not inferred, the idiom ADR 0010 set for mounted roots (`crates/substrate-wire/src/lib.rs:627-635`).

## 4. Enforcement

| | (a) veth + nftables in the sandbox netns | (b) forwarder at a declared descriptor | (c) per-run forwarder inside the sandbox netns |
|---|---|---|---|
| Proves | kernel refuses every other destination, including from a raw socket; the rule set is readable evidence | the child has no address and no name to change: there is nothing in the sandbox to redirect | kernel still refuses everything else — the netns has no interface, so the forwarder is the only reachable peer |
| Needs | **CAP_NET_ADMIN in the host netns** to move the veth peer and to route/NAT — root, or a setuid helper with its own identity and probe | nothing; same unprivileged daemon identity | no host-netns capability; the forwarder must join a namespace the daemon's own child created |
| Bytes at the daemon | none | **all of them**, in the daemon process | in a per-run substrate-owned process, not the daemon; inside the run's cgroup and its whole-tree kill |
| Portable refusal | fact absent → `unserved`; no rule set installed | fact absent → `unserved`; no descriptor passed | fact absent → `unserved`; no forwarder started |
| Consumer fit | good | **poor** — `codex` and `claude` take a base URL, not a descriptor | good — the base URL is `http://127.0.0.1:<port>` |

**The nftables half of (a) is not the hard part; giving an unprivileged network namespace a route out
is.** Under `--unshare-net` there is no interface for a rule set to filter until something privileged
creates one, and two facts at HEAD bear on that: bubblewrap 0.11.2 offers `--userns FD`,
`--userns2 FD` and `--pidns FD` and **no** `--netns` (`bwrap --help`, checked on this host), so
substrate cannot hand bwrap a prepared namespace; and the daemon withholds `exec` entirely when it is
root (`probe.rs:49-51`), so it cannot hold CAP_NET_ADMIN itself. Option (a) therefore means a
privileged helper — a new trust boundary this repository does not have.

**Recommendation: (c)** — since confirmed by running it; see [10a: egress mechanism spike](10a-egress-mechanism-spike.md), which also records two silent-failure traps an implementation must avoid.

**Recommendation: (c).** It is the only option that keeps the kernel floor literally intact — the
sandbox netns still has no interface — while working with a vendor binary that speaks a base URL, and
it needs no privilege the daemon has refused to hold. The forwarder starts at the existing spawn
barrier (`process.rs:398-402`), joins the child's namespaces, listens on loopback, connects out from
the host namespace to the pinned address, and dies with the run's cgroup.

**It is not yet proven and must not be accepted on reasoning alone.** Whether a process can `setns`
into the namespaces bubblewrap created for its sibling, holding only `/proc/<pid>/ns/*`, under this
daemon's unprivileged identity, is a kernel-permission question this document has not observed; the
ADR is owed a spike that does it once on a delegated host. Rootless container tooling solves the
identical problem (`pasta`, `slirp4netns`) — evidence the shape works, not that this daemon can.

## 5. Refusals

| Condition | Class | Code | Address |
|---|---|---|---|
| Request names an aperture the deployment did not declare | `unserved` | `exec.aperture-undeclared` | `exec.network-aperture` |
| Aperture declared but the mechanism does not verify at probe time | fact **absent**; every aperture request is | `unserved` | `exec.network-aperture` |
| Request carries a raw destination where a name belongs | `refused` | `exec.aperture-destination-in-request` | `sandbox.network.aperture` |
| Aperture cannot be installed exactly as declared at dispatch | `failed` | `exec.aperture-install-failed` | — |
| Declared byte ceiling exceeded mid-run | `exhausted` | `exec.aperture-byte-limit` | — |

The first **names the aperture**: an operator debugging a harness needs to know which name was asked
for, and a name is deployment vocabulary, not secret material (design 04 § 6). The second is invariant
3 and design 02 § 3 verbatim — configured intent is not a fact; an unverified mechanism leaves an
absent capability plus a diagnostic and the daemon ready for everything else. The third is belt and
braces: the successor input schema has no destination field, so a conforming client's raw destination
is `schema-invalid` first, and the typed refusal exists so a *name* that parses as `host:port` fails
legibly rather than as "no such aperture" — a rejected escalation, not a configuration typo. Unchanged:
a request omitting the aperture gets `--unshare-net` and `applied.network = "none"`, and a driver
serving no apertures answers exactly as today (`operations.rs:290-300`, `process.rs:810-815`).

## 6. Observation

`applied.network` becomes an object on the aperture branch — `{mode, name, destination, mechanism,
bytes}` — where `destination` is the pinned address and port actually installed, not the configured
host string. Both are stated because they can differ, and design 04 § 4 matches on the resolved one.

The capability document publishes `exec.egress-apertures` — declared names and destinations, present
only when the mechanism verified — so `/v1/machine` answers "what could this daemon ever reach".

No new event kind. The vocabulary is closed (`crates/substrate-wire/src/lib.rs:892-902`) and the
applied block already rides the exec observation, so `exec.accepted`, `exec.running` and `exec.exited`
carry the aperture with no wire addition; session projections carry the same block (`:1101`). A
**refused** aperture is equally visible: pre-dispatch refusals are durably bound to the canonical
request (invariant 5, design 08), so "asked and denied" sits in the ledger beside "asked and granted".

## 7. Conformance vectors

| Vector | Layer | Lane | Proves |
|---|---|---|---|
| `egress-defaults-to-none` | http | portable | no aperture field → `applied.network = "none"`; `/probes/network_was_weakened == false` |
| `aperture-undeclared-is-unserved` | http | portable | 501 `unserved`, the aperture named, `dispatch_count == 0` |
| `aperture-fact-absent-refuses` | http | portable | a snapshot without `exec.egress-apertures` refuses every aperture request |
| `aperture-destination-in-request-refused` | http | portable | a destination-shaped name is `refused`, not silently treated as unknown |
| `declared-aperture-is-reachable` | driver | delegated | the model-free fake app-server on a loopback endpoint inside the aperture; exit 0 and bytes read |
| `undeclared-destination-is-unreachable` | driver | delegated | a second listener outside the aperture, same run: non-zero exit, in the shape of `crates/substrate-daemon/tests/runtime_vectors.rs:620-640` |
| `applied-aperture-is-observed` | http | delegated | `applied.network.name` equals the declared name in both the exec record and `exec.exited` |

The two delegated vectors are one run with two connects, so "reachable" and "unreachable" are proven
against the same installed aperture rather than two configurations. `crates/substrate-daemon/tests/runtime_vectors.rs`
grows a `check_confined_apertures` beside `check_confined_execs`, behind the same `--cgroup-root`
gate. Per § 2, CI proves rows one to four and reports five to seven absent.

## 8. Compatibility

A successor bundle `0.5.0`: predecessor `0.4.0`, `adds_routes: 0`, `preserves_routes: 26`, its own
checker added to the gate — a bundle whose checker is not in the gate is unverified from the next
commit onward (`AGENTS.md` § *The gate*). Earlier directories keep their bytes (invariant 6).

0.5.0 adds an optional `aperture` name in the sandbox block of `exec.start` and `session.start`, a
third branch of `confinement-applied` whose `network` is an object, and `exec.egress-apertures` in the
capability schema; it adds no route, no destination field and no event kind. Cross-version behaviour
is already correct and already frozen: a 0.4.0 daemon answers a 0.5.0 aperture request with
`unserved` (`operations.rs:290-300`), which is exactly the vector
`contracts/substrate-wire/0.4.0/vectors/http/egress-unserved.json`; a 0.4.0 client against a 0.5.0
daemon omits the field and gets `none`. One Rust-level break to plan for: `AppliedNetwork` gains a
variant (`crates/substrate-wire/src/lib.rs:657-660`), breaking exhaustive matches downstream.
`harness` pins substrate by tag (`atlas/ROADMAP.md`, arrow *harness → substrate*), so this lands on a
release boundary and is re-pinned, not hot-swapped.

## 9. Open decisions

| # | Decision | Owner | DEFAULT if nobody answers |
|---|---|---|---|
| 1 | Enforcement mechanism | operator, in the ADR | **Settled: (c).** The § 4 spike ran and passed — see [10a](10a-egress-mechanism-spike.md). Option (a) is refuted on this host by real output: `ip link add veth` and `RTM_SETLINK(IFLA_NET_NS_FD)` both return EPERM at uid 1000. |
| 2 | DNS inside the aperture | operator | Outside. Resolve at declaration, pin the address, give the sandbox no resolver. If a named destination is required for TLS, bind a generated read-only `/etc/hosts` with exactly the declared mapping — the sandbox has no `/etc` today. |
| 3 | Protocol set | this design | `tcp` only. |
| 4 | Re-resolution mid-run | this design | Pinned for the run's lifetime. A re-resolve is a later decision with its own vectors. |
| 5 | Apertures per run | this design | One named aperture per run in the first slice. |
| 6 | What the aperture probe verifies | this design | The **mechanism**, in a throwaway sandbox at startup — never the declared destination's liveness. A reachability check would make readiness depend on someone else's uptime. |

## 10. Proposed ADR

Accepted on 2026-08-29 as [ADR 0013](../../adr/0013-egress-apertures-are-declared-by-the-operator.md),
with its `adr/README.md` row. The block below is what was extracted, kept here as the record of what
this document proposed.

```markdown
---
status: accepted
date: 2026-08-29
---

# ADR 00NN: egress apertures are declared by the operator and referenced by name

## Context

Ordinary execution has no egress: every exec and pipe session runs under bubblewrap's
`--unshare-net`, and the namespace has loopback and nothing else. That floor is enforced and is the
reason a process can be trusted with a workspace. It also makes a sealed secret slot worthless — a
confined vendor harness holding a model credential cannot spend it, so the run is not confined, it
is dead. The vendor case and the b10x case are one shape: one process reaching one model endpoint.

Design 04 already fixes the vocabulary — egress, listening sockets and exposed endpoints are
separate capabilities, an aperture is deployment authority, and a request cannot widen it. Missing
are the mechanism, the declaration surface and the refusal.

## Decision

An **egress aperture** is a named operator declaration in daemon configuration: one destination
tuple of host, port and `tcp`. A request may reference an aperture **by name** and may never carry
a destination, at any depth, in any field. Configuration owns reach; a request selects among what
configuration already permitted.

The default does not move. Without an aperture the sandbox keeps `--unshare-net` and no interface.
An aperture is a separately probed capability fact, `exec.egress-apertures`, published only after
the mechanism verified in a throwaway sandbox — never after reading configuration; an unverified
mechanism leaves the fact absent with a diagnostic. DNS stays outside the aperture: the daemon
resolves the declared host once at declaration, pins the address, and gives the sandbox no resolver.

Refusals are typed and named: an undeclared aperture is `unserved` with the aperture named, an
absent fact is `unserved`, a raw destination where a name belongs is `refused`, and an aperture that
cannot be installed exactly as declared refuses the dispatch with nothing partial installed. The
applied aperture — name, pinned destination, mechanism — is an observation in the run's record and
rides the existing `exec.*` and `session.*` events. A successor bundle carries the request field,
the capability fact and the applied branch; earlier bundle bytes are unchanged.

## Consequences

Substrate gains its first outbound authority, and with it the first deployment-held decision about
where a confined process may reach. That is the cost, bounded by being operator-declared,
name-referenced, exactly matched, probed and observed. A hosted runner cannot prove the positive
half: reachability of a declared destination and unreachability of an undeclared one need a
delegated lane on a self-hosted runner. CI proves the typed refusals and the schema shape and
reports the rest absent rather than passed.
```
