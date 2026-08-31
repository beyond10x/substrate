---
title: Security
description: How to report a Substrate vulnerability without disclosing it publicly.
---

# Report security issues privately

Use [GitHub private vulnerability reporting](https://github.com/beyond10x/substrate/security/advisories/new)
for a suspected vulnerability. Do not put an unresolved vulnerability, live credential or private
data in a public issue or pull request.

Include the affected revision or release, deployment posture, reproduction steps and observed
impact. Security fixes target current `main` and the latest daemon release; older releases and
development contract bundles have no long-term-support window or response SLA.

The source is public under
[Apache-2.0](https://github.com/beyond10x/substrate/blob/main/LICENSE). A signed daemon image does
not make its development wire contract stable.
