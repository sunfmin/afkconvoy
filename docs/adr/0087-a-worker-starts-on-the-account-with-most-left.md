---
status: accepted
---

# A worker starts on the account with the most left, and an account at 90% of a window takes no new worker

Extends ADR-0086 (sunfmin/control#1220).

A harness may hold several engine accounts in mytokens (ADR-0086). afkconvoy
spends them: a worker starts on one, and which one is admission's.

## Which account a worker starts on

Of the accounts of its harness that take a new worker, a worker starts on
the one with the most left of its most used window that still holds; of
equals, the one with the fewest workers of the host on it, then the first
mytokens lists. An account takes none while:

- a window of it is used to the host's `--account-limit` or more — 90% by
  default (Felix, 2026-11-16) — until that window resets. The engine reports
  the whole account, its owner's own use among it, so its figure stands as
  it is;
- it met a limit of its use, until the reset the limit names, or for an hour
  where it names none — whether its session recorded the limit
  (`Harness::limit_event`) or worker health read it on its screen;
- it runs as many of the host's workers as its `SESSIONS`;
- its login is on another machine, or still to be made here.

Running workers are never stopped for it. With no account of a harness left,
admission holds that harness's next worker (`Hold::Accounts`); it calls no
watch, since nothing on the host gives an account's use back before its
window resets, and `doctor` says it once, of the host, not once per issue
(ADR-0079). A quota a worker on an account meets rests that account, not its
harness: the harness on another account still takes work.

## What the host heard of each account is kept in one book

A worker's windows are read from its session, as before (sunfmin/control#729),
and its account is the one its worktree's note names. What was last heard of
each account — its windows, and the limit it met — is kept in the host's
account book (`~/.cache/afkconvoy/accounts/book.json`), so that an account
at 92% is still known to be so once its worker has ended.

## Only a Claude Code account is handed to a worker yet

A Claude Code session is given its account's token as
`CLAUDE_CODE_OAUTH_TOKEN`, which Claude Code takes before the login kept in
its configuration. The worker's line fetches it from mytokens as it starts —
`CLAUDE_CODE_OAUTH_TOKEN="$(mytokens get agent-account-<name> --field
CLAUDE_CODE_OAUTH_TOKEN)"` — so the line holds where the token is, never the
token, and nothing starts where mytokens gives none.

The worker is given no `CLAUDE_CONFIG_DIR` of the account's, which ADR-0086
first put beside the token, and which it and the harness crate no longer do
(Felix, 2026-11-16): the configuration directory is where Claude Code
keeps its sessions, its status line and the user's rules and skills, and the
fleet reads a worker's turns and windows there. Pointed at the account's
empty home, a worker would have none of them, and the fleet could not see it
take its brief.

A Codex, Grok or Kimi Code worker starts in the fleet's kept home, which
links the host user's own login; an account's login is not linked into it
yet, so their workers start on the host user's login as before, and `plan`
says of their accounts that they take no work for that reason.
