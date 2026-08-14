# Roadmap

The roadmap is ordered. A later phase does not begin while an earlier exit criterion remains open.

| Phase | Outcome | Exit criterion | State |
|---:|---|---|---|
| 0 | Private design repository | private `daemonloom/substrate`, bot-authored `main`, portable docs | complete |
| 1 | Design closure | contract questions are decided or explicitly deferred; canonical schema/translation, trust-domain, destination-security, capability-snapshot, and driver guarantees are reviewable | complete |
| 2 | Minimum host slice | one confined workspace, bounded argv-only exec, observed result, named refusals, and machine facts | complete — portable and delegated-host lanes green |
| 3 | Lifecycle and recovery | operation ledger, events, leases, cancellation, and unanswered-outcome reconciliation | in progress — reopened 39-finding closure review; durable dispatch/terminal/event-effect boundary green, lifecycle/capacity/recovery/transport/schema evidence open |
| 4 | Direct byte plane | PTY/session establishment and bounded channel authority without routing bytes through connectors | pending behind phase-3 closure |
| 5 | Docker driver | the same contract serves container-backed execs and workloads with truthful capability facts | pending |
| 6 | Stack adoption | connectors projection, one Flux adapter, and one autodev `Executor` adapter prove the public contract independently | externally gated — Flux/autodev must first record adoption in their own repositories |
| 7 | Hosted composition | identity/cloud trust and placement operate substrate without moving domain rules into cloud | pending |

Kubernetes, image builds, cross-machine scheduling, external connector artifacts, and a generic AI
agent platform are not prerequisites for the minimum host slice. Their pressure must be demonstrated
against the stable contract before they enter an implementation phase.
