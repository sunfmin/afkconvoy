## Release once the fleet is idle

For an operator's session on this host only — never for a fleet worker.

When this repo's fleet is idle (the last line of `~/Library/Logs/afkconvoy-run.log` says `holding 0/` and `frontier 0`, and no PR is on the landing train) and `main` is ahead of the newest `v*` tag, cut a release and install it, without asking:

1. Version: a patch bump; a minor one if a PR merged since the tag carries the `release:minor` label.
2. On a branch `release-<version>`: set `version` in `Cargo.toml`, `cargo update -w`, commit `afkconvoy <version>`, open a PR listing the PRs since the tag, and squash-merge it (`gh pr merge --squash --delete-branch`). This rule is the go-ahead to merge that PR.
3. On `main`, pulled: `git tag -a v<version> -m 'afkconvoy <version>'`, push the tag, then `packaging/release.sh check v<version>`, `build`, `package`, `publish` (the release workflow has no `HOMEBREW_TAP_TOKEN`).
4. `brew update && brew upgrade sunfmin/tap/afkconvoy`, and check `afkconvoy --version`.
5. Restart this repo's fleet only — the `afkconvoy run` whose working directory is this checkout, never another repo's: SIGTERM it, wait for it to exit, then `nohup afkconvoy run </dev/null >>~/Library/Logs/afkconvoy-run.log 2>&1 &`, and wait for its `tick 1:` line.

Report the version, the PRs it holds, and the seconds each step took.

## Agent skills

### Issue tracker

Issues and specs live as GitHub issues in `sunfmin/afkconvoy` (via the `gh` CLI). See `docs/agents/issue-tracker.md`.

### Triage labels

The five default triage labels, each named after its role: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
