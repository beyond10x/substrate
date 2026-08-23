# Vision: daemonloom/substrate

**Status:** founding document · **Date:** 2026-08-13

The accepted boundary is summarized in the [architecture overview](../architecture/overview.md).
The numbered [design documents](design/) now contain accepted v1 decisions or named later-phase
deferrals, and the [design-closure plan](plan/01-design-closure.md) unblocks the minimum host slice.

## What this is

A standalone daemon (`substrate`) and a versioned API contract that make one machine — or one
handed-over cluster scope — a governed **execution substrate**: confined workspaces, bounded
process execution, long-lived workloads, image build and movement, volumes and endpoints, and an
event stream in which every state transition is an observation, not a claim.

One contract, several drivers: `host` (native processes under OS sandboxing), `docker`, `k8s`.
Consumers never learn which driver served them except through declared capability facts.

## Why

Three sibling products each stopped at the same edge, deliberately:

- **Flux** solved guarded IO *in-process* (path confinement, argv-only exec, environment clearing,
  OS sandboxing) and left the remote wire explicitly open — its delegate seam says
  *"no wire format is chosen here, deliberately."*
- **autodev** designed a proposed `Executor` port and decided (decision-0002) that *confinement
  belongs to the substrate, not to harness flags* — extraction remains downstream work and the real
  substrate does not exist yet.
- **connectors** refuses behaviour hosting by principle (*"no foreign engine in the dispatch
  path"*); it governs invocation but must never contain the thing invoked.

All three point at the same missing product. "Run things in containers" conflates five concepts;
substrate is exactly one of them:

| Concept | Home |
|---|---|
| Guarded local IO prior art | Flux's existing implementation (informs threat cases; no dependency) |
| **Machine/cluster substrate service (daemon, API)** | **substrate — this repo** |
| Governed invocation (identity, credentials, grants, audit) | daemonloom/connectors |
| Turn execution & verification (harness × substrate) | autodev |
| Agent sandboxing / agent hosting | flux + autodev, as substrate clients |

The one-sentence boundary: **substrate runs things and reports what it observed; the platform
decides who may ask and remembers who did; flux and autodev are clients on both paths.**

## Principles

1. **Observe, never trust.** Every resource mutation answers with state re-read from the driver, never
   the request echoed back. Absence stays absence: unknown is never rendered as zero or as
   success. No verb's reported outcome can disagree with the artifact.
2. **Fail closed, refuse by name.** A refusal names the address (path, capability, limit),
   never the value. A sandbox that cannot be provided refuses the exec; nothing silently runs
   weaker than asked.
3. **Argv-only, everywhere.** No operation in the contract accepts a shell string. "Shell
   access" means a caller explicitly chose `argv = ["bash"]`, and that choice is the caller's.
4. **A data-only, catalog-declarable wire.** Plain HTTP+JSON plus transport-independent channel
   semantics with closed event sets. Substrate owns a canonical machine-readable specification;
   connectors deterministically translates a pinned bundle plus its projection manifest into the
   connector catalog and proves the result byte-for-byte.
5. **Thin authn, no policy engine.** Owner-permissioned Unix peer identity or expiring generated
   bearer tokens with coarse per-family scopes; no unauthenticated loopback and no reachable TCP
   without TLS/trusted transport. Rich authorization—grants over declared facts—is the platform's
   job; duplicating it here would recreate the split connectors was founded to end.
6. **One daemon, one trust domain and scope.** A daemon governs one tenant on one machine
   (host/docker) or one handed-over cluster namespace (k8s). Fleets, placement, and federation
   belong to consumers. substrate is not a scheduler.
7. **Capabilities are probed facts.** A daemon advertises what it verified (bubblewrap present,
   docker reachable), not what it hopes. Unadvertised operations answer `unserved`.
8. **Liveness is asserted, never assumed.** Workspaces, execs, and workloads can carry leases;
   expiry is a typed transition, emitted as an event, never a silent disappearance.
9. **Bounded everything.** Captured output is byte-capped with truncation notices, event
   retention is a stated window, file reads are ranged. Unbounded responses are a defect class.
10. **Own the guarded implementation; keep substrate Flux-free.** Substrate specifies and
    implements path confinement, argv-only spawn, environment clearing and OS sandbox enforcement
    behind its own driver ports. Flux's existing behavior supplies prior threat cases and
    conformance expectations only. No Flux crate or type appears in any dependency kind, public
    contract or private driver implementation.

## Non-goals

- **No grants engine, no identity provider, no vendor-credential broker.** Substrate still owns
  local auth, destination-bound source/registry secrets, workload secret delivery, and channel
  authority verification. Tokens here are blast-radius limiters, not rich policy.
- **No cross-machine scheduling.** Not Nomad, not a PaaS. One daemon, one scope.
- **No catalog and no plugin runtime.** substrate is *declared in* a catalog; it does not have
  one.
- **No behaviour of its own.** No workflows, no cron, no model calls. It runs what it is told
  and reports what it saw.
- **No ingress product.** Endpoint exposure is loopback/LAN/tunnel primitives; TLS termination,
  DNS, and public routing are out of scope for v1.

## Relationships

- **Flux.** Flux may implement its remote execution delegate over this API, giving it the guarded
  port on a remote substrate without reversing the dependency. A local daemon is a stronger sandbox
  option (exec with the working tree as a mounted workspace, egress off). Flux itself is deployable
  as a workload — which is also how connectors' future "supervised client runtime" tier can
  exist without embedding an engine.
- **autodev.** A future `RemoteExecutor` behind the proposed `Executor` port, after that port is
  extracted: checkout the pinned base
  as a workspace, exec the harness turn, snapshot back as a git bundle or a push to a
  coordinator-owned ref. Evidence semantics unchanged; liveness is the lease autodev already
  designed.
- **connectors.** One first-party provider declaration. Each enrolled daemon is a Connection;
  grants admit operations from the risk vocabulary this spec declares; substrate events flow
  through platform delivery with provenance. In the personal posture, flux and autodev may also
  dial a loopback daemon directly — the platform is the governor, not a mandatory hop. Two
  alignments with [connectors Design 03](https://github.com/beyond10x/connectors/blob/a8c393135478973a89c700d14478936eb0ae1df5/docs/design/03-beyond-http.md): a daemon on a private network is
  reached the way any LAN-bound endpoint is — a **satellite** deployment near it, dialing up —
  so substrate carries no reverse-tunnel surface; and duplex byte streams (sessions, tunnels)
  follow the **byte-plane split** — the platform brokers establishment, the bytes flow directly.
