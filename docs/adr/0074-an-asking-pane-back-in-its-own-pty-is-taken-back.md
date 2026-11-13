---
status: accepted
---

# An asking pane back in its own pty is taken back

Amends ADR-0052 (sunfmin/control#836). The asking session's Terminal is
owned only when its saved `ptyId` and `incarnationId` both match the listed
row, or when the launch's own acknowledgement names its handle. When the
session inside restarts in the same pty — Claude Code's `/restart` after
an update — Orca lists the same `ptyId`, `tabId` and `leafId` under a new
handle and a new `incarnationId`. That pane was a discovery candidate
by its title and its `ptyId` yet never owned, so the asking session stayed
unavailable for good, and the questions behind it with it.

A row whose saved `ptyId`, `tabId` and `leafId` all match and whose saved
`incarnationId` differs is our pane, re-incarnated. Where the session's
marked executor still runs, the session survived in it: the journal takes
the new handle and public identity and the session goes on in that pane.
Nothing already delivered is sent again. A send left unresolved follows the
replay rule of ADR-0040 with what this pane can prove: where the session took a
turn since the question was offered, it is delivered; otherwise its saved
request key, which binds the old incarnation, is dropped and the question is
offered afresh once input is ready.

Where the marked executor is gone, the pane is ours but the session is not in
it. As ADR-0070 does for the watch, the pane is closed, which would otherwise
block the next creation by its title, and the session is recorded ended;
the current question is asked again by a fresh asking session.

A same-name row in another pty, or the same pty in another tab or leaf, is
not the asking session's and is still refused, as is a changed identity under
the saved handle itself.

The watch keeps ADR-0070: its round is bounded work whose screen and input a
new incarnation does not prove, and a fresh watch carries its pending work on;
it never stays unavailable after a re-incarnation.
