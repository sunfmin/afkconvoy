---
status: accepted
---

# A settled worker's worktree is removed

Settling a claim removes that worker's worktree (sunfmin/control#215). The gate runs inside it and leaves read-only role copies there. Git deletes the worktree's registration and then stops on a directory the owner cannot write. Orca records that failure and still answers success, so a settle that trusts the answer releases the claim and leaves the directory.

Removal gives the owner write on the directories it has to delete, and clears no other mode bit. A role copy stays read-only while a test holds it. A directory still there after orca answers success is removed. A later tick finishes one that remains, including one git has already unregistered, and orca drops the failure when it is next asked for the list. Asking orca to remove that directory again leaves it: orca refuses a folder git no longer lists.

The worker the fleet started for the issue, stopped at its prompt once it landed, is the fleet's own session and keeps nothing: settling ends it — its terminals closed and its process tree confirmed gone — before the removal (sunfmin/control#235). A worker still working keeps the worktree, and so does any other process in it but a stopped orphan, which settling ends (ADR-0074): the worktree stays, and the claim stays with it, until a later tick sees that process gone. A worktree that holds a commit the remote does not stays, and the claim is released: nothing records a failed removal, so a later tick does not take it. Dirt the gate left, with every commit already on the remote, does not keep it. A stopped candidate keeps the removal it already has (ADR-0037). That removal does not touch an issue's, watch's, or cut's worktree; settling is what removes the worker's.
