---
status: accepted
---

# A fleet's checkout runs no automatic gc, and a fetch repairs lost objects

sunfmin/control#1090.

On Studio64 the afkconvoy checkout lost objects twice in one morning, and
every gate and land of every worktree on the host stopped on a bare error.
Every worktree shares the checkout's object store. git's own automatic gc
fired after a fetch once the store reached `gc.autoPackLimit` (50 packs).
It consolidated the packs while a dozen of the fleet's fetches were writing
new packs into the same store. Trees and blobs that reachable commits named
went missing, and the commit-graph and multi-pack-index then named objects
and packs that were gone.

## No automatic gc or maintenance

Every fetch afkconvoy runs carries `-c gc.auto=0 -c maintenance.auto=false`.
On its first fetch in a process, afkconvoy also writes those two values into
the repository's config, which all its worktrees share. That way a worker's
commit or merge starts no gc either. A value already there is not written
again. afkconvoy itself runs no `gc`, `repack` or `prune`. Packing the store
at a quiet moment, under `exclusive`, is its own follow-up.

## A fetch repairs what the remote still has

When a fetch fails because objects are missing, afkconvoy repairs the
checkout and fetches again. Missing objects show up as one of these
messages: a commit "in the commit graph file but not in the object
database", `could not load pack`, `unresolved deltas`, `did not send all
necessary objects`, and the like. The repair goes in two steps:

1. It drops the commit-graph and the multi-pack-index. Both only speed up
   reads, and git reads each pack's own `.idx` without them.
2. If objects are still missing, it fetches with `--refetch`, which sends
   every object anew and builds on no base the checkout lacks.

It deletes no ref. If even the refetch fails, the error names the checkout
and the repair a human runs: `git fsck`, delete the refs it names broken,
then `git fetch --refetch origin`.
