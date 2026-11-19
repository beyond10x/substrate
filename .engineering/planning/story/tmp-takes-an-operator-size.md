---
format: aep.planning-md/3
id: story:tmp-takes-an-operator-size
kind: story
status: implemented
title: An operator sizes every confined run's /tmp with --tmp-bytes (#224)
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-11-18T08:23:23Z", actor: "human:unknown", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-18T08:23:23Z", actor: "human:unknown", revision: 4}
- {from: "active", to: "implemented", at: "2026-11-19T15:12:01Z", actor: "human:unknown", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
# An operator sizes every confined run's /tmp with --tmp-bytes (#224)

Issue #224 (friction F16, F50): a confined run's `/tmp` is a tmpfs at the kernel default, half the
RAM, and nothing sets it. Decided by
`adr/0049-the-base-view-carries-etc-alternatives-and-tmp-takes-an-operator-size.md` decisions 2
to 4. The sandbox root holds only bubblewrap's mount points and gets no size option.

## Outcome

`--tmp-bytes N` (`SUBSTRATE_TMP_BYTES`) on the daemon sets `HostConfig::tmp_bytes`; every
confined run's argv then carries `--size N --tmpfs /tmp`. Absent keeps the kernel default.
Startup refuses a size under one MiB as `config.tmp-bytes-invalid` and a bubblewrap whose
`--help` does not name `--size` as `config.tmp-bytes-unserved`. No wire change.

## Acceptance

- Unit tests fail before and pass after: the argv carries `--size N` immediately before
  `--tmpfs /tmp` when set and no `--size` when absent; the validation refuses 0 and under one MiB
  by code; the daemon parses `--tmp-bytes` into the host configuration.
- Spec: the ESS projections model neither daemon flags nor the argv; `spec/` is unchanged.
- README documents the flag; full gate green; the pull request closes #224.
