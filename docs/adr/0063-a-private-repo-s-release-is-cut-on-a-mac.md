---
status: accepted
---

# A private repo's release is cut on a Mac, not in GitHub Actions

This is the home of the decision for every private repo of this host's user; the others link here.

The operator's private repos have no GitHub Actions budget: no workflow of theirs starts. So a private repo's release is cut on a Mac, by the repo's own release script — here `packaging/release.sh`, run by the cut's steps in `docs/agents/afkconvoy.md` (ADR-0028) — and it has no release workflow. No platform is dropped for it: one Mac builds every platform the repo ships, Linux musl through `cargo-zigbuild` and Windows msvc through `cargo-xwin` where a repo ships those. Only the release moves: a repo's deploy and other CI workflows are not part of this decision.

## Consequences

- Tests run on the Mac alone: a test that fails only on Linux or Windows is not caught.
- A public repo may still call the same release script in CI: its Actions cost nothing.
