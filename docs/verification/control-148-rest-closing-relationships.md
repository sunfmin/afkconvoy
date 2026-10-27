# control#148: REST closing-relationship feasibility

Investigated on 2026-10-27 for sunfmin/control#148. The original investigation
below is retained as historical evidence. The watch amended the issue to
authorize supported quota fallback with explicit capability refusals; the
implementation and regression evidence follow at the end of this report.

## Original decision needed (resolved by the watch ruling)

The requested quota routing is feasible for many GitHub operations. There is
no documented REST equivalent for the native PR closing relationships that
the existing rebuild, dispatch and settlement require. The issue requires
these relationships to remain equivalent, explicitly prohibits replacing a
failed read with a body guess, and also requires ordinary issue landing and
settlement to continue while GraphQL is exhausted.

A REST PR listing alone cannot meet all three requirements. Continuing on an
incomplete relationship list can start a second worker for an issue with an
existing manually linked PR, or miss a PR during settlement. Refusing that
read preserves the existing semantics, but still stops the requested paths.

To proceed, the contract needs either a supported REST source of the complete
native relationship, or permission to change how the fleet identifies its
issue-associated PRs during GraphQL exhaustion. In the latter case, the
contract must say what happens to manually linked PRs, including links to an
issue in another tracker, when no authoritative relationship read is available.
This is a domain decision rather than a retry or quota-cache implementation
choice.

## Existing execution paths

- `GitHub::open_prs` in `crates/cli/src/gh.rs` asks `gh pr list` for
  `closingIssuesReferences`, retains each linked issue's repository identity,
  and passes those links and the PR body to `Tracker::closed_by`.
- `GitHub::open` reads both the issues and those PRs for the rebuild. A failed
  PR read fails the rebuild; it does not become an empty PR list.
- `crates/cli/src/dispatch.rs` also reads `open_prs` directly when recovering
  starts and taking up retained work.
- `settle` in `crates/cli/src/settle.rs` reads `open_prs` before closing an
  issue or PR. It uses the relationship to check what the claim actually
  landed.
- `pr_of_branch`, introduced for watch landing, intentionally returns no
  closing relationships. Its interface comment explicitly directs callers
  associating a PR with an issue to `open_prs` instead. Reusing it does not
  establish equivalence for ordinary issue operations.

## Primary-source evidence

GitHub documents `PullRequest.closingIssuesReferences` as the connection of
issues a PR may close. Its `excludeUserLinked` and `userLinkedOnly` arguments
explicitly distinguish manually linked issues. Such a link therefore need
not appear in the PR body.

- [GraphQL PR fields](https://docs.github.com/en/graphql/reference/pulls)
- [Manual linking, including links from another repository's issue sidebar](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/linking-a-pull-request-to-an-issue)

The published REST OpenAPI description was inspected at revision
`755ee33b4a81d51f6f07a902cce26cbec3b822f2`:

- [REST OpenAPI description](https://github.com/github/rest-api-description/blob/755ee33b4a81d51f6f07a902cce26cbec3b822f2/descriptions/api.github.com/api.github.com.json)
- `components.schemas.pull-request` has no closing-issue relationship field.
- `components.schemas.timeline-cross-referenced-event` contains `event`,
  `actor`, `created_at`, `updated_at`, and `source`. It does not expose the
  GraphQL `willCloseTarget` boolean. A cross-reference alone is not a closing
  relationship.
- `components.schemas.timeline-connected-event` and
  `timeline-disconnected-event` expose event identity, actor, timestamps,
  commit identity and application identity, but no linked issue/PR target.
  They cannot reconstruct the current connection by replaying the timeline.
- The repository paths for events/timeline do not add a closing-relationship
  listing endpoint.

These documented limitations also appear in the
[REST issue event definitions](https://docs.github.com/en/rest/using-the-rest-api/issue-event-types).
This finding concerns the supported REST contract; it does not claim that
GitHub's private web endpoints have no other data.

Read-only live checks corroborated the schema:

```sh
gh api repos/sunfmin/afkconvoy/pulls/561 --jq 'keys'
gh api repos/sunfmin/afkconvoy/issues/561/timeline --paginate \
  --jq '.[] | select(.event == "connected" or .event == "disconnected" or .event == "cross-referenced") | {event, fields: keys, source: .source.issue.html_url}'
```

The PR had no closing-relationship field. Its returned cross-reference events
contained only `actor`, `created_at`, `event`, `source`, and `updated_at`.
No live connected/disconnected sample was observed in that timeline; the
claims about their fields above come from the published schema.

A separate read of the open PRs through GraphQL returned repository-qualified
closing references, including PR 553 to `sunfmin/control#131`, PR 560 to
`sunfmin/control#139`, and PR 564 to `sunfmin/control#141`. These particular
PRs also contain explicit body references; they are not evidence of the
manual-link case.

At the time of a separate quota observation, REST core remaining was 4676 and
GraphQL remaining was 4906. These are observations during this investigation,
not the quotas at the incident's failed requests. No account quota was
deliberately exhausted, and no test wrote to live issues or PRs.

## Isolated CLI reproduction

The temporary probe source is preserved in
[commit 13be11fc](https://github.com/sunfmin/afkconvoy/blob/13be11fc/crates/cli/tests/quota_feasibility_probe.rs).
It uses the existing `World` fixture, real CLI binary and stand-in `gh`/`orca`.
A wrapper rejects only `api graphql` or `pr list` with the incident's primary
quota error, while forwarding other calls to the normal stand-in. The native
link probe supplies `linked: [146]` and a body with no closing keyword.

Commands, run on that probe tree using the gate's installed toolchain:

```sh
cargo +nightly-2026-09-25 build -p afkconvoy --examples --quiet
cargo +nightly-2026-09-25 test -p afkconvoy --test cli quota_feasibility_probe -- --nocapture
```

The three probes ran in 0.55 seconds after compilation:

```text
graphql_exhaustion_should_still_allow_a_rebuild ... FAILED
  {"error":"gh api graphql failed: GraphQL: API rate limit already exceeded"}
pr_list_exhaustion_should_still_allow_dispatch ... FAILED
  {"error":"gh pr list failed: GraphQL: API rate limit already exceeded"}
a_native_link_without_a_body_reference_still_holds_an_issue_off ... ok

test result: FAILED. 1 passed; 2 failed
```

The first two failures establish the original symptom; they are not a
completed fix. The third establishes existing behavior a fallback must retain.
The probes were removed from the final tree rather than leaving deliberately
red acceptance tests in its suite. Only this report differs from the initial
tree. The full gate was not run: no implementation or landing is proposed.

## Implementation after the watch ruling

`GitHub::read` and `GitHub::gh` now share primary-quota routing. The adapters
in `gh_equivalent` preserve each operation's projection and complete its
collections. `gh_quota` holds only expiring observations, separated by GitHub
host, authenticated login, and core/GraphQL quota category. No issue, blocker,
comment, PR relationship or credential is cached. A shared secondary cooldown
holds both APIs; numeric and HTTP-date Retry-After values are honored. Failed
writes are never replayed after an ambiguous receipt. Confirmed single REST
write rejections have explicit GraphQL adapters where authoritative node
identities can be read first; other writes name their capability boundary.

| Exhausted API | Supported continuation | Required refusal |
| --- | --- | --- |
| GraphQL | Complete REST issues, comments/stage records, blockers, labels, PR listings without native relationships, and eligible dispatch with an authoritative absence of relevant open PRs | Any same-repository open PR whose native/manual closing relationships a rebuild, dispatch, ordinary issue turn or settlement needs |
| REST core | GraphQL tracker visibility, issues, comments, native blockers, labels/locks, PR heads, watch records, branch protection facts, repository heads/text blobs, and supported issue/comment/label mutations | No equivalent adapter or authoritative node identity, incomplete/binary blobs, ambiguous write receipts, or unsafe high-level multi-request writes |
| Both, or secondary cooldown | A bounded refusal with remaining/reset and next attempt information; subsequent commands recover after expiry | No healthy empty queue, alternate-login attempt, private endpoint or relationship inference |

The ordinary GraphQL rebuild retains its batched read. REST degradation needs
separate full comments/blocker collections per issue. The old unconditional
fixed-request-count sentence in the glossary was removed; the domain invariant
is that missing facts cannot produce a working set. The existing ordinary
rebuild-cost tests still verify the normal batched path.

`gh pr list` and `gh issue list` can cap their output. At 1,000 rows, or at an
embedded 100-closing-reference boundary, the common reader replaces that
listing with complete cursor pagination. Native closing references retain
repository-qualified identities. Nested connections use their own cursor,
including blockers and labels. A repeated/missing cursor, null required
resource or missing raw collection receipt is an error.

A read-only schema probe confirmed Issue.fullDatabaseId, Issue.blockedBy,
PullRequest.baseRepository and the IssueOrPullRequest union. A live read of
this issue's id and one comment id confirmed that fullDatabaseId is serialized
as a decimal string, so the adapters explicitly parse it to the existing
u64 domain identity. The stand-in models that string representation too.
No quota was deliberately exhausted and no live issue/PR was changed by tests.

## Real CLI regression evidence

Before implementation, the two initial regressions failed in 0.46 seconds:

```text
GraphQL exhaustion -> dispatch: gh pr list failed: gh: API rate limit already exceeded
REST exhaustion -> plan: gh api repos/{owner}/{repo} failed: gh: API rate limit already exceeded
2 failed; 0 passed
```

The retained tests are `crates/cli/tests/quota.rs`, registered in the existing
real-CLI integration suite. They use its real binary, git remote, stand-in gh
and Orca, full JSON pages and actual command outcomes. The scenarios cover:

- Both directions, initial rejection and quota known before the first call.
- No repeated exhausted-backend requests across CLI invocations, reset expiry
  without restart, and separate authentication identities and GitHub hosts.
- Both APIs exhausted, numeric and HTTP-date secondary cooldowns, response
  reset headers, ordinary 403/auth failures, and transport failures.
- Eligible issue 146 dispatch with a full REST zero-open-PR listing.
- Manual/native links absent from bodies, qualified cross-tracker links after
  page one, and a relevant PR beyond the 1,000-row listing limit.
- Blockers past page one in both directions, fleet locks, and a latest stage
  record past 100 comments in a bound repo.
- Watching in both directions retains every ending across 101 PRs and an
  intake record beyond the first 100 comments.
- One successful GraphQL comment mutation after a confirmed REST rejection,
  and one applied mutation with a lost receipt that is never sent twice.
- An ordinary issue turn with REST exhausted that gates, records green,
  fast-forwards the remote base branch and closes the issue.
- An ordinary turn with GraphQL exhausted that refuses the native relationship
  read before running its gate or closing the issue.

Reproduction commands (rustup's cargo, rather than Homebrew cargo):

```sh
~/.cargo/bin/cargo +nightly-2026-09-25 build -p afkconvoy --examples --quiet
~/.cargo/bin/cargo +nightly-2026-09-25 test -p afkconvoy --test cli quota:: -- --nocapture
```

Latest focused run: **22 passed, 0 failed in 4.05 seconds**, following the
initial two red reproductions. The stand-in emits string GraphQL database
identities and multiple REST/GraphQL pages; the assertions check dispatch,
stage/lock/blocker positions, mutation counts and the gated remote commit.

The additional authentication scenarios exercise overriding environment
credentials. `gh auth status` would itself query GraphQL for such a token, so
the common entry resolves the login through an available API and binds an
opaque one-way credential/profile fingerprint to that identity. Token values
never enter argv, disk notes or diagnostics. A changed token gets a separate
identity lookup; a known token does not hit an exhausted identity API each
tick. With both quotas zero before identification, an expiring refusal is
remembered under that fingerprint. Keychain/configured users are read locally,
and quota-aware startup tool checks defer API authentication to the common
entry before GitHub effects instead of mistaking a quota stop for logout.

The full repository gate is run by the required landing turn, after the PR is
open and all changes committed. Its receipt and final outcome are recorded on
the PR, together with the exact integration-test result.

The complete workspace suite was also run through the host's exclusive queue
with fail-fast disabled, after integrating the base branch and updating the
read-only fixture assertions for local identity/quota observations:

```sh
afkconvoy exclusive -- ~/.cargo/bin/cargo +nightly-2026-09-25 nextest run \
  --workspace --no-fail-fast --status-level fail --final-status-level slow \
  --show-progress none
```

Result: **1,649 passed, 0 failed, 16 skipped in 359.307 seconds**. All 22 quota
regressions are part of that run. The skipped cases retain their existing
external-service requirements; no passing count relies on skipping a quota
fixture. The final landing turn still gates and records its exact merge tree.

## Original incident still requiring GraphQL reset

This implementation does not establish that the original live #131/#139/#141
landings or #146 dispatch can all proceed while GraphQL is exhausted. Existing
same-repository open PRs can still make their native relationships necessary.
Those reads now report the operation, missing capability, known quotas and
reset/retry boundary instead of guessing. The successful fixture for eligible
issue 146 proves only the supported zero-open-PR case. Installation is a
separate fleet cut; a landed source change alone does not establish that the
installed 0.5.25 host runner has been fixed.
