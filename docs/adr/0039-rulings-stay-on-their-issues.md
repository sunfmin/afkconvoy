---
status: accepted
---

# Rulings stay on their issues; lasting rules use ordinary work

Source: [sunfmin/control#141](https://github.com/sunfmin/control/issues/141), which records Felix's approval to remove automatic precedents, and its [documentation instruction](https://github.com/sunfmin/control/issues/141#issuecomment-6429444871). This supersedes the precedent/proposal parts of ADR-0011 and the related team definitions in ADR-0008, ADR-0013 and ADR-0027.

The separate precedent authority repeated rules already owned by ADRs, teams and roles. Promoting a ruling added a worker, worktree, claim, PR and human merge lifecycle. We remove that lifecycle instead of moving it into another dedicated workflow. A ruling remains the decision on its issue; an enduring rule is amended through ordinary implementation in its owning document, with conflicting old wording amended and source links kept.

An actual human instruction or confirmation and its source establish human intent. A shared GitHub identity, an agent-authored ruling, or a merged document alone establishes none. Actual user decisions quoted in issues or ADRs remain binding. Ruling parsing, consult permissions, conflict resolution and the existing escalation boundaries remain: product-team's four human-only cases are neither widened nor narrowed.

## Upgrade

[control#140](https://github.com/sunfmin/control/issues/140) must migrate product-team's effective rules before this removal stops their separate injection. It landed as [product-team#37](https://github.com/sunfmin/product-team/pull/37), commit c9f0fac. The host audit is in `docs/verification/control-141-proposal-retirement.md`.

The team key `precedents`, including null, is refused with instructions to migrate effective content and its sources and remove the key. It is not a disabled feature flag. No proposer role or skill is validated or loaded. There is no separate precedent file requirement or brief injection. The hospital example's gate-recovery rule moved to its team rules; its hospital-demo-only interval rule and source moved to `docs/demo-rules.md` for incorporation into that repo's own rules.

Legacy run names are read only for retirement. They take no concurrency slot and cannot keep the fleet from idle or prevent ordinary issue, patrol or cut work. A tick rechecks a local claim's instance, host and remote sha before stopping its worker, keeping its worktree and branch. A dead peer's claim is released without stopping a worker; a live peer's is left alone. Release is leased to the sha read. A same-host start takes old claims as before and its first tick retires them. Failures retry without blocking ordinary work.

Once per checkout/team binding, the upgrade reads open PRs from the team's own branches. A legacy PR gets an idempotent disposition comment containing its link and directing content review through ordinary work in its owning rules. The remote records completion; no ongoing proposal polling, acceptance queue, auto-close or auto-merge remains. Branches, PRs, worktrees, ruling comments and historical decision evidence stay available for review.
