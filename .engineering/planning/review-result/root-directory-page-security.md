---
format: aep.planning-md/3
id: review-result:root-directory-page-security
kind: review-result
status: active
title: Security review of the root on the v1 directory page (ADR 0052)
relations:
- reviews: story:root-directory-listing-pages-immediate-children
revision: 1
---
# Security review: the root of a workspace on the v1 directory page (ADR 0052)

Review of `story:root-directory-listing-pages-immediate-children` on the working tree of
`story/root-directory-listing` over `main` at `ca9cdc404`. The invariant under review: `.` is
admitted as the whole path of a directory-page read and nowhere else. Verdict: nothing found
that holds the change; three notes.

Counts as reported: the daemon's `http` target 24 to 31 cases, 0 red. Origin: 1 introduced,
2 pre-existing.

- Note, pre-existing. The directory cursor is an index bound to no directory. A replayed
  cursor pages only the directory its request names or is refused. Not acted on.
- Note, pre-existing. The page hides `.git` and nothing else, so a temporary entry the driver
  left in a directory is listed there, in the root as in a named directory. Not acted on;
  whether a keeper's check directory can lie in a workspace's root was not established.
- Note, introduced. A direct caller of the guarded crate that sends `.` with a malformed
  directory query is refused `request.schema-invalid`; through the daemon the refusal stays
  `workspace.path-escape`. Both are refusals. Not acted on.

Held by the reviewer's seven cases: 20 longer spellings of the root and 15 malformed queries
are refused; `.` is refused on nine other routes and the root is unchanged; the root page
equals a named directory's page with the same children; a cursor pages only the directory its
request names; another subject is answered as for a named directory; a root that became a link
is refused.

Read and not faulted: the root is opened on the workspace descriptor with the resolve flags of
a named directory; the route's scope is the one of every file read; an executor of an earlier
release refuses `.`; the `relative-path` pattern of bundle `0.27.0` admits `.`.

Not checked by the reviewer: the delegated lane; the namespace lane on a cluster; an adopted
workspace's root page at run time; hosted TLS admission end to end.

