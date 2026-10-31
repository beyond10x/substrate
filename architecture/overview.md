# Architecture overview

Substrate is a standalone data-plane service with one versioned contract and multiple drivers. Its
domain is deliberately smaller than a platform: it owns bounded resources, guarded mutations,
verified capabilities, and observations. It does not own the intent that led a caller to request
them.

```text
products / agent / Flux / autodev
                 │ direct API or higher-level port
                 ▼
      connectors / cloud composition
                 │ optional governance and placement
                 ▼
        substrate service contract
                 │
       ┌─────────┼──────────────┐
       ▼         ▼              ▼
     host      Kubernetes      Docker
     driver    namespace       driver
               driver          (later)
```

| driver | state | serves |
|---|---|---|
| host (`substrate-host`) | released | workspaces, confined exec, sessions, host processes |
| Kubernetes namespace (`substrate-kubernetes`, [ADR 0047](../adr/0047-a-kubernetes-namespace-driver-serves-workspaces-on-pvcs-and-execs-as-pods.md)) | released in 0.16.1 as a development profile | empty workspaces on a PVC with a keeper Pod; from 0.18.0 an exec is admitted only on the confinement its probe observed, otherwise `exec.sandbox-unavailable` |
| Docker | proposed | none |

A daemon runs exactly one driver, chosen by `--driver` and never by a request. The namespace
driver reaches its Pods only through the Kubernetes API server, and its keeper and exec Pods run
the static `substrate-executor`, which links neither the host driver nor the store.

## Control flow

1. A caller chooses a substrate deployment directly or through a higher-level placement decision.
2. The daemon authenticates the request and checks coarse local scope and preconditions.
3. Domain logic validates a driver-independent command and requires named capability facts.
4. One driver performs the bounded action.
5. The daemon re-reads observed state and records the operation outcome.
6. The response and event stream report observations, including refusal, truncation, expiry, and
   uncertainty explicitly.

## Internal layers

| Layer | Owns | Must not own |
|---|---|---|
| wire | versioned requests, responses, events, pagination, channel frames | driver or product types |
| domain | resources, commands, errors, leases, capability requirements | HTTP, Docker, Kubernetes, connector grants |
| service | admission, idempotency, lifecycle, reconciliation, observation sequencing | vendor or agent behavior |
| driver ports | generic filesystem, process, workload, image, volume, endpoint operations | placement or rich authorization |
| adapters | host/container/cluster enforcement and probes | weakened fallback semantics |
| composition | listener, authentication, storage, selected drivers, telemetry | cloud-only product rules |

Continuous session bytes use a separate byte plane after control-plane establishment. Substrate may
terminate the direct endpoint, but connectors' ordinary invoke/event path never becomes the byte
proxy.
