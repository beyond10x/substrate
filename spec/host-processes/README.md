# Host processes (draft)

**Draft, pending ADR 0036.** This directory drafts the ESS domain for a proposed resource
family, the *host process* (supervised process): a long-lived process tree or container that a
session starts through the daemon, with an owner, a reason, a lease or a reasoned keep, an optional
hard deadline, one cgroup, observations, a bounded log and an event stream. Under the proposed
`host` profile the process runs unconfined, and the record says so (`confinement: none`). No
Substrate code, wire bundle or route implements any of this yet.

Until the ADR is accepted:

- Nothing is generated from these inputs, and nothing in the workspace depends on them.
- `cargo xtask check-ess` does not read this directory. It validates `spec` and `spec/sessions`
  by name, and `spec/ess-inputs.yaml` lists its files explicitly, so these files are outside it.
- Every rule the source plan does not settle is an `UNMAPPED:` marker beside the place it would
  go. Each one is a question for the owner and is resolved before anything is generated.

```console
ess specify validate --path spec/host-processes --strict-requires
ess verify conform author --path spec/host-processes --scenarios spec/host-processes --strict-requires
ess verify conform run --path spec/host-processes --scenarios spec/host-processes \
  --target interpreted --report-format 2 --strict-requires
```

The pin is ESS 0.60.0 (`ess-inputs.yaml`). The authored scenarios under `scenarios/` are the
acceptance set, one or more per command. A run against the interpreted reference checks the
model only. It is not evidence that any implementation conforms.

The kernel half of these claims belongs to a native suite. That covers killing the whole cgroup
on every stop, the process tree being gone before a record ends, the refusal of the `host` profile
under a hosted or static-bearer TCP posture, and the bound on the log file. Here a record reaches
`Ended` only through the supervisor's empty-cgroup report (`RecordExit`). Lease expiry and the
hard deadline are supervisor commands, because ESS has no clock-driven transition.
