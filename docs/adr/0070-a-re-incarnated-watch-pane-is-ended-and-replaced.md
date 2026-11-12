---
status: accepted
---

# A re-incarnated watch pane is ended and replaced

Amends ADR-0039 (sunfmin/control#807). ADR-0039 associates a watch
Terminal only when its saved public pane identity matches: both its
`ptyId` and its `incarnationId`. When Orca restarts or restores the
resident watch's pane, it keeps the `ptyId` and gives a new handle and a
new `incarnationId`. That pane was a discovery candidate by its `ptyId`
yet never owned, so the runner neither took it back nor started another:
the host stayed without a watch for good, and renaming the pane did not
help.

A pane whose saved `ptyId` matches and whose saved `incarnationId`
differs is ours, re-incarnated. The runner does not take it back: a new
incarnation proves neither that the resident harness survived nor what
its screen and input hold, and a saved Terminal request key binds the old
incarnation. It ends the session's marked executor, wherever it survived,
closes that pane, which is ours by `ptyId` and would otherwise block the
next creation by its title, and records the launch ended. Its confirmed
Terminal is then absent, as with any closed resident, and the next look
starts a fresh watch for the work pending.

This is not a repeated creation: the old creation was confirmed, its
executor has ended before anything starts, and nothing is resent into the
restored pane. A same-name pane with no saved identity, a changed
`ptyId`, or several candidates still pause as ADR-0039 says.
