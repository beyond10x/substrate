# daemonloom/substrate

Substrate is the standalone Daemonloom execution data plane. It turns one machine—or one handed-over
cluster scope—into a governed service for confined workspaces, bounded processes, workloads, images,
volumes, endpoints, leases, and observed state.

Substrate runs things and reports what it observed. It does not decide product policy, run agent
loops, understand connector vendors, or depend on Flux. Consumers choose whether to call its stable
API directly or through a higher-level Daemonloom service.

**Status:** design closure accepted; the minimum host slice is implementation-ready. No
implementation workspace has landed yet.

## Start here

1. [Vision](docs/VISION.md)
2. [Architecture overview](architecture/overview.md)
3. [Domain model](architecture/domain-model.md)
4. [Stack integration](architecture/stack-integration.md)
5. [API contract](docs/design/01-contract.md)
6. [Specification bundle and minimum wire](docs/design/07-specification-and-conformance.md)
7. [Roadmap](ROADMAP.md)

## Repository map

- [`architecture/`](architecture/) records the current accepted system boundary and dependency
  direction.
- [`docs/design/`](docs/design/) develops the wire, driver, lifecycle, security, session, and trust
  design. Each document states whether it is accepted or still under review.
- [`docs/plan/`](docs/plan/) turns the design into review gates and implementation slices without
  containing implementation.
- [`adr/`](adr/) records accepted repository decisions with YAML frontmatter.
- [`STATUS.md`](STATUS.md) records observed progress; [`ROADMAP.md`](ROADMAP.md) records ordered exit
  criteria.

## Relationships

- [daemonloom/connectors](https://github.com/daemonloom/connectors) may govern substrate operations
  as a first-party provider and may later use substrate to isolate an attested connector artifact.
- [Flux](https://github.com/codewandler/flux) may implement a remote execution adapter over the
  substrate API. The dependency never points back into Flux.
- [autodev](https://github.com/codewandler/autodev) may implement its `Executor` port over substrate.
- `daemonloom/agent` and future products consume bounded execution through their own ports.
- `daemonloom/cloud` composes and operates substrate deployments; it does not own substrate rules.

The product and binary name are `substrate`. Published packages will use the
`daemonloom-substrate-*` prefix.
