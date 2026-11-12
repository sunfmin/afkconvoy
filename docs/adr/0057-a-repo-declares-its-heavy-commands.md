---
status: accepted
---

# A repo declares its heavy commands

Amends ADR-0012 and ADR-0019 (sunfmin/control#577).

A worker's brief tells it to run heavy steps through `afkconvoy exclusive`, and workers kept running them bare: a cold `cargo nextest` build of bizkernel held 4.1 GiB and 6.6 cores beside a gate running in the exclusive queue, turned its worker hot and started a watch round (sunfmin/control#565, bizkernel #576). A written rule broken again and again needs a mechanism, and the guard already runs before every worker's tool call.

A repo declares its heavy commands in a `yaml heavy-commands` block of its config, which older releases read past: `commands`, each a pattern of words. A simple command of a shell call runs a pattern when its words, from its program on, begin with the pattern's; a pattern's program with no `/` matches a program by its file name. The guard refuses a call that runs one outside `afkconvoy exclusive`, for every worker, bound or unbound: its reason names the pattern and gives the line to run in its place, the same command behind `afkconvoy exclusive --`. A simple command `afkconvoy exclusive …` is in the queue, whatever it runs. The gate crate's embedder names that command (ADR-0059): another program holding the queue names its own, in place of `afkconvoy exclusive` in both. A repo that declares none sees no change.

The guard reads the declaration from the worker's worktree: the one whose own `TMPDIR` the worker's `TMPDIR` links to (ADR-0019), its config as it stands there. A session the fleet did not start, and the watch, which is on no worktree, have none. A block that cannot be read refuses every shell call, naming the fault, as any guard that cannot read what it judges allows nothing. Only that block is read: a fault elsewhere in the file is `afkconvoy config`'s to say.

What `afkconvoy gate`, `land` and `cut` run is no worker's tool call, and already queues through exclusive: it is never refused. A pattern names words, not how much a command builds: a repo where one test of one crate is light and the workspace's suite heavy declares the longer pattern, `cargo nextest run --workspace`, and the guard recognises it only so spelled.
