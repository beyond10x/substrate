# Glossary

| Term | Meaning |
|---|---|
| substrate | One independently deployable execution data plane governing one machine or handed-over cluster scope |
| driver | Repository-owned implementation of the substrate contract for a host, container engine, or cluster API |
| capability fact | A property the running daemon has verified it can enforce, not a configured aspiration |
| workspace | A substrate-owned confined filesystem tree and execution context |
| exec | A bounded process run associated with a workspace |
| session | A leased interactive exec channel whose control is governed separately from its continuous bytes |
| workload | A long-lived substrate-managed application, normally image-backed |
| observation | State re-read from the serving driver after an action or probe |
| operation id | Caller-minted stable identifier used to reconcile retries and unanswered mutations |
| capability snapshot | Versioned facts probed from one driver/config generation and bound to admission |
| stream generation | Persisted event epoch that changes only when sequence continuity cannot be proved |
| reconciliation snapshot | Barriered resource and operation/tombstone view used after an event-history gap |
| contract bundle | Owner-issued canonical schemas, vectors, provenance, and hash manifest released by digest |
| subject | Authenticated local or foundation principal namespace owning resources and operation ids |
| lease | Renewable liveness assertion that turns abandonment into a typed observed transition |
| refusal | An answered rejection naming the unmet guard, capability, address, or limit without exposing a secret value |
| unserved | A declared operation or capability that the selected deployment does not implement |
| byte plane | Direct terminal, tunnel, or media bytes that flow after governed session establishment |
| placement | A higher-layer decision selecting an eligible substrate deployment; not a substrate scheduling function |
