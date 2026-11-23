---
format: aep.planning-md/3
id: approval-record:operator-picks-dot-as-the-root-of-the-directory-page-2026-11-23
kind: approval-record
status: draft
title: 'Operator''s pick: the workspace root is the path . on the directory page (2026-11-23)'
relations:
- decides: story:root-directory-listing-pages-immediate-children
revision: 1
---
# Operator's pick: the workspace root is the path `.` on the directory page (2026-11-23)

Question D22, put by this session through session `host-operator`: how a client pages the
immediate children of a workspace's root (finding 2 of a review of release 0.24.0).

Options as put:

- A (recommended): the root is the path `.` on the existing directory page. No successor
  bundle; consumers pinned to bundle `0.27.0` need no upgrade; a daemon that does not serve it
  answers 422 `workspace.path-escape`. Two daemons that advertise the same bundle answer
  `/files/.` differently until the next bundle publishes it.
- B: the root is the empty path. Successor bundle `0.28.0`, every SDK client rebuilt with the
  daemon.
- C: paging on the v2 tree, or a new route. Successor bundle `0.28.0`.

The pick, as relayed by `host-operator`: the operator picked at 02:06:00 CET on 2026-11-23, in
the terminal of `host-operator`, D22 "A: path `.` (Recommended)", with "the root is `.` on the
existing directory page, no successor bundle, ADR first".

Recorded in ADR 0052. This session did not see the operator type it; the relay is the source.
