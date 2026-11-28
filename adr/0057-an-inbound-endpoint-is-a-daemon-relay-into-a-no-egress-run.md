---
status: accepted
date: 2026-11-28
---

# ADR 0057: an inbound endpoint is a daemon relay into a no-egress run

Decider: the operator, 2026-11-28 11:08 CET, relayed by the operator's session (D3, option A);
`approval-record:operator-answers-design-38-questions-2026-11-28`. Carries
[design 38](../docs/design/38-long-lived-service-on-the-host-driver.md) § 5 and decisions W16 and
W17. The design comes before code (invariant 8): this record adds a resource, two capability facts,
a start member, a daemon option, refusal codes and a successor bundle, and no code yet. It follows
[ADR 0056](0056-a-workload-is-a-confined-run-the-daemon-supervises-on-the-host-driver.md) for the
workload target; the exec target does not need it.

## Context

A service that runs confined has no way in. Every exec runs under bubblewrap's `--unshare-net`, a
namespace with loopback and nothing else, and that floor is the reason a process can be trusted with
a workspace ([ADR 0013](0013-egress-apertures-are-declared-by-the-operator.md)). Design 01 § 3.5
specifies endpoints (`loopback` or `lan`, `lan` refused unless daemon configuration allows it, the
answer carries the observed address). No released bundle holds an endpoint route, and none holds the
word `lan` (`contracts/substrate-wire/0.29.0/operations.json` and a search of that bundle).

The tree has the mirror image of what is needed. The egress aperture is a helper that joins the run's
namespace once (`NS_GET_USERNS`, `setns`), binds, listens and hands the listener out over
`SCM_RIGHTS`; a forwarder that never leaves the host namespace; a relay per connection; and the
helper cannot also be the forwarder. The child pid comes from the run's `--info-fd`. Apertures
listen on `127.0.0.1` to `127.0.0.8`, at most `MAX_EGRESS_APERTURES` (8) per run and
`MAX_DEPLOYMENT_EGRESS_APERTURES` (12) per deployment (module documentation of
`crates/substrate-host/src/egress.rs`; `crates/substrate-wire/src/lib.rs`). The applied network and
the `exec.egress-apertures` fact state what an aperture is
(`contracts/substrate-wire/0.29.0/schemas/resource.json`, `schemas/capability.json`).

## Decision

1. **An endpoint exposes a port of a confined exec or workload at an address on the host.**
   `POST /v1/endpoints` (`endpoints.expose`) with `{target: {workload} | {exec}, port, exposure:
   loopback|node}`; `GET /v1/endpoints[/{id}]`; `DELETE /v1/endpoints/{id}`. Id `ep_...`. The answer
   carries the observed address `{host, port}`. A workload's spec may carry `endpoints: [{port,
   exposure}]`, the same endpoint made at deploy, and `GET /v1/workloads/{id}` lists the observed
   addresses. Every mutation is keyed and durable before dispatch
   ([ADR 0005](0005-operations-are-durable-before-driver-dispatch.md)).

2. **The exposure word is `node`** (W17), where design 01 § 3.5 says `lan`. This renames it. No
   released bundle holds either word or an endpoint route, so nothing breaks; design 01 is edited
   with the slice.

3. **The mechanism** (W16) is the aperture's mirror, with the same constraint that a helper inside
   the namespace cannot leave it:

   | role | namespace | does |
   |---|---|---|
   | listener | host | the daemon binds the observed address: `127.0.0.1` for `loopback`, the configured node address for `node` |
   | helper, one per accepted connection | joins the run's network namespace once | `setns`, one `connect` to `127.0.0.1:<port>` inside it, hands the connected socket back over `SCM_RIGHTS`, exits |
   | relay, one per connection | host | copies bytes between the accepted and the returned socket |

   The helper gets the namespace as the aperture helper does: the child pid from `--info-fd`, the
   owning user namespace through `NS_GET_USERNS`, waiting for loopback. A connected socket made in
   the namespace stays in it when passed out, so **the run gets no interface** and `exec.no-egress`
   stays `true` for a run that serves an endpoint. The helper's only destination is `127.0.0.1` and
   the declared port; no request supplies one. No byte is copied inside the namespace.

4. **Lifetime.** An exec's endpoint is closed when the exec is terminal (`endpoint.closed`); a later
   connection is refused by the kernel. A workload's endpoint is bound while desired is `running`:
   during `starting` and `backoff` a connection that cannot reach the child is closed after accept;
   on `stopped`, `exited` and `failed` the listener is closed; on `start` it binds again, to the
   same port if free, else another, which the workload's record shows. A connection ends with its
   run's namespace.

5. **Exposure.** `loopback` is `127.0.0.1` and a kernel-chosen port. `node` is refused unless the
   daemon is given `--endpoint-node-address <ip>`, a non-loopback, non-wildcard address of the host;
   without it the refusal is `endpoint.exposure-unserved` (501) naming `node`. With it the endpoint
   binds that address and a kernel-chosen port. The relay forwards no peer address; the service sees
   loopback.

6. **Bounds:** `MAX_ENDPOINTS_PER_RUN` (8), `MAX_ENDPOINTS_PER_DEPLOYMENT` (64),
   `MAX_ENDPOINT_CONNECTIONS` (64) per endpoint and `MAX_DEPLOYMENT_ENDPOINT_CONNECTIONS` (512). A
   connection over a bound is closed at accept and counted as `refused_connections` on the endpoint.
   There is no byte bound and no idle timeout; the run's end and the endpoint's close end a
   connection.

7. **The wire of the slice**, in a successor bundle that rewrites no released byte (invariant 6):
   - Facts: `endpoint.loopback`, published only after the mechanism ran once on a throwaway sandbox
     (a connection delivered, the namespace left with no interface); `endpoint.node`, only where
     `--endpoint-node-address` is set and `endpoint.loopback` was proved. Absent otherwise
     (invariant 3).
   - Fields: the endpoint resource; `endpoints` on the workload spec. Applied: the observed address;
     `exec.no-egress` unchanged.
   - Refusals, each a wire constant with a vector: `endpoint.unserved` (501),
     `endpoint.exposure-unserved` (501), `endpoint.target-not-found` (404: absent, another
     subject's or terminal), `endpoint.port-invalid` (422), `endpoint.limit` (429),
     `endpoint.bind-failed` (500, nothing left). The lexical `endpoint.port-invalid` is checked
     before the fact, as design 37 orders its own.

## Alternatives

- *A virtual ethernet pair with nftables, inbound only (D3 B).* Needs `CAP_NET_ADMIN`, and
  `exec.no-egress` becomes false.
- *`pasta` or `slirp4netns` port forwarding (D3 C).* A host dependency and a tap interface.

B and C give the child an interface, so `exec.no-egress` could not stay `true`; invariant 3 refuses a
silent downgrade, and each would need a new fact stating the narrower guarantee.

## Consequences

- **`exec.no-egress` stays `true`** for a run that serves an endpoint. The run gains no interface and
  no destination; the daemon dials into the namespace and the run cannot dial out.
- **A `node` endpoint is an unauthenticated TCP port.** Anyone who can reach it can connect: the
  control API's subject check, which derives the subject from kernel peer credentials, does not
  extend to that port. This is why `node` is off unless the operator names an address.
- The relay hides the peer address from the service, which sees loopback.
- A helper is forked per accepted connection. That it is fast enough for a service is inferred and
  not measured (design 38 § 16, I11); the egress helper is one per aperture, not per connection. A
  long-lived helper per run is the alternative if it is not.
- The helper is a process in a run's namespace with no destination of its own: a new surface.
- **What stays true of the isolation floor.** The run keeps the non-nestable user namespace, the
  seccomp program, the pids and memory+swap bounds and the cleared environment; `exec.no-egress`
  holds. An endpoint opens nothing in the run's network namespace but one loopback connection per
  accepted client.
- `AGENTS.md` invariant 6, the gate section and the lane counts of the runtime vectors move with the
  successor bundle, as with every successor.

## Not decided here

- A tunnel, a WebSocket stream, a byte bound or idle timeout on a relayed connection.
- Authentication of a `node` endpoint.
- The number of the successor bundle, taken when it is cut.
- The local endpoint, which gives the consumer's agent a Unix-socket destination attributed to its
  run (design 38 § 6), and volumes (§ 7): other records.
- Inferred in design 38 § 16, proved by the slice: the per-connection helper's speed (I11).
