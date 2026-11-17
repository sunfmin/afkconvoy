---
status: accepted, amended by ADR-0037 and ADR-0088
---

# A gate turn merges, gates and records in one command

A worker used to merge before waiting twenty to forty-five minutes for its gate, and the combined tree was then gated again. One red tip also made many workers repeat the same failure. A turn now holds one command through merge, gate and recording, with the PR open before it asks. Red or a merge conflict frees the turn and lands nothing; conflicts stay in progress for the worker to resolve and commit.

One PR at a time gets a turn. “Landing turn” stays avoided. Whatever the host runs at once, one repo holds at most one turn at a time; an earlier second request of a running repo waits while another repo may run. Light gates take turns in a line of their own repo. The contention bound of ADR-0031 covers the complete turn.

Existing gate commands that wrap the build in `afkconvoy exclusive` inherit the owning command's lease. They do not ask for another place behind themselves. Config and check refuse gate commands that call `afkconvoy gate` or `land`, with the build-and-test command to set instead. A standalone gate before workflow review lands nothing; only the workflow's landing worker takes the turn.
