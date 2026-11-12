---
status: accepted
---

# A private repo's release is cut on a Mac, not in GitHub Actions

This ADR is the home of the decision for every private repo of this host's user; the others link here. Formerly the control repo's.

The operator's private repos have no GitHub Actions budget: no workflow of theirs starts ("The job was not started because an Actions budget is preventing further use"). So a private repo's release is cut on a Mac, by the repo's own release script — here `packaging/release.sh`, run by the cut's steps in `docs/agents/afkconvoy.md` (ADR-0028, ADR-0064) — and it has no release workflow. No platform is dropped for it: one Mac builds every platform the repo ships, Linux musl through `cargo-zigbuild` and Windows msvc through `cargo-xwin` where a repo ships those. Only the release moves: a repo's deploy and other CI workflows are not part of this decision.

Sources: sunfmin/afkconvoy#447 (this repo), sunfmin/inspectvideo#45, sunfmin/hkj#50, sunfmin/agent-cli#26.

## Consequences

- Tests run on the Mac alone: a test that fails only on Linux or Windows is not caught. sunfmin/hkj's ADR on running tests on three platforms in CI gives way to hkj's own ADR from sunfmin/hkj#50.
- A public repo may still call the same release script in CI: its Actions cost nothing.
- A repo's `HOMEBREW_TAP_TOKEN` secret is no longer used; removing it is the operator's, in the repo's settings.
