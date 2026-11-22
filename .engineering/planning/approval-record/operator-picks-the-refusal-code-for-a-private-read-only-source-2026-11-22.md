---
format: aep.planning-md/3
id: approval-record:operator-picks-the-refusal-code-for-a-private-read-only-source-2026-11-22
kind: approval-record
status: draft
title: Operator picks the refusal code for a private read-only source (2026-11-22)
revision: 1
---
# Operator picks the refusal code for a private read-only source (2026-11-22)

## Who decided

The operator, on 2026-11-22. He typed "A" in this repository's session, in answer to the one open
question with lettered options.

## What was asked

For `story:read-only-mount-sources-refuse-the-daemons-private-paths`: which refusal code a start
gets when it names, as a read-only root or a read-only file, a path the daemon keeps private. No
published code means "the source is private".

- A: reuse `exec.read-only-root-absent` and `exec.read-only-file-absent` (422) with a message of
  their own. No successor bundle. A client tells a private source from a missing one only by the
  message.
- B: two new codes in a successor wire bundle, with the cost of a bundle cut, and the fix waits
  for it.

The question was also sent to session `host-operator` with A as the default after 15 minutes. The
operator's own answer came first.

## What follows

- The design note of the story states A in its Refusal section, with the message text.
- A code of its own may come with the successor bundle that
  `story:read-only-mount-sources-are-declared-by-the-operator` has to cut.
