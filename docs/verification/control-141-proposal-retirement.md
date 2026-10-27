# Proposal retirement verification

Issue: [sunfmin/control#141](https://github.com/sunfmin/control/issues/141). Audit performed on the fleet host on 2026-10-27, before removing separate precedent injection. The decision and upgrade protocol are in ADR-0039.

## Configured teams

`afkconvoy checkouts` listed nine checkouts. Their `docs/agents/afkconvoy.md` files name:

| Checkout | Team |
|---|---|
| afkconvoy | unbound |
| bizkernel | sunfmin/product-team |
| tcmsaas | sunfmin/product-team |
| inspectvideo | unbound |
| hkj | sunfmin/product-team |
| agent-cli | sunfmin/product-team |
| whats-hot | unbound |
| hostvitals | sunfmin/product-team |
| product-team | unbound |

There is no other affected configured team. A read of hidden and branch-layout proposal claim refs on all five bound repos found no active proposal claim at audit time. Product-team migration [control#140](https://github.com/sunfmin/control/issues/140) landed in [product-team#37](https://github.com/sunfmin/product-team/pull/37), commit `c9f0facde97321db4c5977482ed8e89cab838fef`. Its team file has no proposal configuration and its separate precedent file is deleted; no compatibility stub remains to retire. All four roles receive the team rules through the existing rules path.

The installed `afkconvoy 0.5.24 check --team sunfmin/product-team` loaded that exact commit, with `rules: true`, `precedents: null`, and roles builder, lead, reviewer and timekeeper. The changed binary's same check loads it with the separate `precedents` field absent. A local `check --dir` without the team's repo identity cannot resolve its relative role paths; the verification uses the published team identity instead.

## Effective rules migrated by #140

Compared the five entries in product-team's pre-migration file with the landed rules, retaining applicability and links:

| Former entry | Authoritative destination and verification |
|---|---|
| Missing platform capability | Team rules, “Never fit the product to a missing capability”; builder's gap rule retains bizkernel#403 and forbids derived-value workarounds. |
| Failure no longer reproduces | Lead rules limit reproduction to two bounded attempts, keep raw output on the issue, and retain bizkernel#668's source and reopen instruction. |
| Unread background work | Builder rules, “Your run lasts as long as your turn”, retain all seven source links and require foreground completion or bounded checks through the result. |
| Cross-repo blocker | Lead rules retain tcmsaas#136 and #155, preserve the branch, require a dependency revision containing the fix and keep the complete issue. |
| Conflicting written decisions | Lead's conflict rule retains control#56 and #57, amends the decision that gives way in the same stage and preserves all four human-only boundaries. |

The team glossary and lead rules require actual human instruction or confirmation and its source; author identity, an agent ruling or a merge alone establishes none. The lead's ordinary `rule` skill directs changes to the owning document and no longer promotes rulings.

## Retained unmerged proposals

These PRs were open during the audit and were never part of the injected effective rules. Their recovery decisions remain on their source issues. Each now has an explicit retirement disposition comment: retain the PR, branch and source history for review; review any enduring content through ordinary work in its owning rules; no automatic merge or close, and no inferred human approval.

| PR | Content and disposition |
|---|---|
| [product-team#33](https://github.com/sunfmin/product-team/pull/33) | Recovery after operator-cancelled gates and a fixed worker start, sourced to control#73 comment 6418405966. Retain the issue-specific evidence; its historical `gate --land` command does not replace current landing help. [Disposition](https://github.com/sunfmin/product-team/pull/33#issuecomment-6430313160). |
| [product-team#35](https://github.com/sunfmin/product-team/pull/35) | Resume existing work after verified worker-start recovery, sourced to control#130 comment 6429310007. Retain for review through ordinary rule work if a durable change is needed. [Disposition](https://github.com/sunfmin/product-team/pull/35#issuecomment-6430313826). |
| [product-team#36](https://github.com/sunfmin/product-team/pull/36) | Similar worker-start recovery, sourced to control#134 comment 6429313319; overlaps #35 and is not a second authority. Retain both sources for review. [Disposition](https://github.com/sunfmin/product-team/pull/36#issuecomment-6430314390). |

Control ADR 0006's corresponding amendment is owned by [control#143](https://github.com/sunfmin/control/issues/143). That checkout is not configured for this host's fleet and has no fleet config: `afkconvoy flows /Users/sunfmin/Developments/control` reports the missing config. The task was filed with `repo:sunfmin/control`; this worker edited no control files.

## Regression evidence

Seven CLI integration scenarios in `crates/cli/tests/retired_proposals.rs` pass: repeated human ruling-to-resume with zero proposals; explicit legacy config refusal; owned worker stop with work/branch/history retained; live and stale peer protection; idempotent PR disposition and no ongoing polling; failed stop retry while an ordinary issue fills the concurrency slot; and same-host startup takeover followed by retirement without a replacement proposal.

Existing consult, operator/watch, rework, issue, patrol and cut tests remain in the full gate. The landing turn records the full gate result for the exact merge commit on the remote and in the PR landing receipt.
