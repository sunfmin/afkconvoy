---
status: accepted
---

# A noted ref layout moves after two probes

A checkout notes the ref layout a startup probe settled on. ADR 0001 keeps that as hidden refs, or as `convoy-*` branches only where the remote rejects hidden refs. One rejected probe push was read as the remote refusing the noted layout, and the note moved at once. On 2026-10-28 a single `(failed)` rejection moved a hidden note to branches; the next probe took hidden refs and moved it back. A recorded gate run left on the layout just left was no longer believed, so the same tree was gated twice.

A noted layout now moves only when two startup probes that actually run, one after the other, each see the noted layout rejected and the other taken. One rejection leaves the note and does not say the fleet has moved. A probe that takes the noted layout leaves the note and clears that streak. A probe that reaches no verdict still refuses the start and changes nothing. A checkout that has noted no layout is unchanged: hidden refs, or branches where the remote rejects those.

A turn believes a recorded gate run of one tree and one gate command from either layout while the run stands. A new record is written only on the noted layout. A red run drops the record on both, so it is not believed from the one it was left on. Claims, heartbeats and the candidate chain stay on the noted layout. A later tree, including the commit a cut lands, is believed only by its own green run.
