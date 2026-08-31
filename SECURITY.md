# Security policy

## Reporting a vulnerability

Please use [GitHub private vulnerability reporting](https://github.com/beyond10x/substrate/security/advisories/new)
for a suspected vulnerability. Do not disclose an unresolved vulnerability in a public issue,
discussion or pull request.

Include the affected revision or release, the deployment posture, reproduction steps, the observed
impact and any suggested mitigation. Never include live credentials or somebody else's private
data.

## Supported line

Security fixes target current `main` and the latest released daemon image. Older releases and
development contract bundles do not have long-term-support windows. A development bundle remains a
development contract even when the daemon image carrying it is signed.

Substrate makes missing confinement visible as a named refusal. A report that a documented
guarantee silently degrades, that request data can choose the authenticated subject, or that secret
bytes reach an observation, log or error is security-sensitive even when no credential has yet been
exposed.

No response or remediation SLA is promised.
