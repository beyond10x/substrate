---
format: aep.planning-md/3
id: story:base-view-carries-etc-alternatives
kind: story
status: active
title: A confined run on a Debian-family host finds cc, awk and every other alternatives link (#223)
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-18T08:23:22Z", actor: "human:unknown", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-18T08:23:22Z", actor: "human:unknown", revision: 4}
---
# A confined run on a Debian-family host finds cc, awk and every other alternatives link (#223)

Issue #223 (friction F37, F38): the Linux base view binds `/usr`, `/bin`, `/lib`, `/lib64` and no
`/etc`, so every `/usr/bin` name that is a link into `/etc/alternatives` dangles in a sandbox;
`cc` among them, so `cargo` links nothing. Decided by
`adr/0049-the-base-view-carries-etc-alternatives-and-tmp-takes-an-operator-size.md` decision 1.

## Outcome

Every confined run's bubblewrap argv carries `--ro-bind-try /etc/alternatives /etc/alternatives`
beside the other base binds. Nothing else of `/etc` is added.

## Acceptance

- A unit test on the base view argv fails before the change and passes after: the argv holds
  `--ro-bind-try /etc/alternatives /etc/alternatives`.
- Spec: the ESS projections model neither the bubblewrap argv nor its mounts; `spec/` is
  unchanged and `cargo xtask check-ess` stays green.
- Full gate green; the pull request closes #223.
