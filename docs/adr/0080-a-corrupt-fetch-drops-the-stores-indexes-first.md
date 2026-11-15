---
status: accepted
---

# A corrupt fetch drops the store's indexes before it refetches

Amends ADR-0079 (sunfmin/control#1090).

On Studio64 the afkconvoy checkout lost objects after a repack was cut short.
Its commit-graph still named commits the store no longer had, and every land
of every worktree stopped on the fetch of `refs/convoy/candidates/main`:
`fatal: You are attempting to fetch <sha>, which is in the commit graph file
but not in the object database`. Later its multi-pack-index named a pack that
was gone (`could not load pack 0`), which breaks every read, not only fetches.

Both messages count as a corrupt fetch. Before a corrupt fetch is made again
with `--refetch`, afkconvoy drops the store's commit-graph (`objects/info/commit-graph`,
`commit-graphs/`) and its multi-pack-index (`objects/pack/multi-pack-index`,
`multi-pack-index.d/`). Both only speed up reads: git reads each pack's own
`.idx` without them, and housekeeping writes them again from what is there.
No ref is deleted. A fetch that still fails is noted and backs off as ADR-0079
says.
