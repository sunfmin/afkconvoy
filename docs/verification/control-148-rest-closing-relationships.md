# control#148: REST closing-relationship feasibility

Investigated on 2026-10-27 for sunfmin/control#148. No runtime change is
implemented here. The incident remains unresolved.

## Decision needed

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
