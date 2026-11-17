---
status: accepted
---

# Engine accounts live in mytokens, and a refreshing login is bound to one machine

Felix keeps several subscription accounts per engine, used by afkconvoy and by Guild alike, on several machines. A list of accounts kept by each program, or on each machine, would drift from the others.

## mytokens is the accounts' one home

Each **account** is one mytokens profile secret named `agent-account-<name>` (GLOSSARY: **Account**). mytokens already holds every credential of this user, keeps it in the keychain, and syncs it to every machine. The harness crate (ADR-0058) reads them (`afkconvoy_harness::account`), so afkconvoy and Guild read one format with one reader. It gives, for an account, the environment its session starts with, the machine it is bound to, if any, and the login line a person runs where its login is missing.

A credential's value goes only into the environment of the session's process (`Environment::apply`): never to a file, a command line or a log. No type of the crate prints or serializes it.

## A Claude Code account is a token; every other is a login bound to one machine

`claude setup-token` makes a year-long token that is never refreshed and may be used on several machines at once. It is the account's credential, given as `CLAUDE_CODE_OAUTH_TOKEN`, which Claude Code takes before the login kept in its configuration. The worker's line fetches it from mytokens as it starts (`CLAUDE_CODE_OAUTH_TOKEN="$(mytokens get agent-account-<name> --field CLAUDE_CODE_OAUTH_TOKEN)"`), so the line holds where the token is, never the token. The session is given no `CLAUDE_CONFIG_DIR` of the account's own: that directory is where Claude Code keeps its sessions, its status line and the user's rules and skills, which the fleet reads to see a worker's turns and windows. The token alone decides the account.

Codex, Grok and Kimi Code log in with a refresh token kept in a file, and a refresh on one machine invalidates the token the others hold (OpenAI: "Use one auth.json per runner"). Copying the file through mytokens would have each machine refresh it against the others, so mytokens holds no such credential. The secret names the one machine (`MACHINE`) its login lives on; there the login is in the account's home, `~/.agent-accounts/<name>`, made once by a person with the harness's device-code login, which works over SSH. Elsewhere the account is bound to that machine and no session starts on it.

A worker on such an account starts in a kept home of that account — `<harness>[-host-user]@<account>` — which holds what the fleet's kept home holds but links the login file in the account's home, not the host user's. The account is chosen, and named in the worktree's note (`convoy-account`), before the start line is made, so a worker never starts in a home made for another account. The line holds no credential and fetches none. Whoever reads a worker's turns, windows and limits reads every home kept for its harness, the account ones too.

## A worker starts on the account with the most left

Of the accounts of its harness that take a new worker, a worker starts on the one with the most left of its most used window; of equals, the one with the fewest of the host's workers on it, then the first mytokens lists. An account takes none while:

- a window of it is used to the host's `--account-limit` or more, until that window resets. The engine reports the whole account, its owner's own use among it, so its figure stands as it is;
- it met a limit of its use, until the reset the limit names, or for an hour where it names none;
- it runs as many of the host's workers as its `SESSIONS`;
- its login is on another machine, or still to be made here.

Running workers are never stopped for it. With no account of a harness left, admission holds that harness's next worker (`Hold::Accounts`) and calls no watch: nothing on the host gives an account's use back before its window resets. A quota a worker meets rests its account, not its harness; worker health's continuation keeps the harness when another account of it takes a worker now. What the host last heard of each account — its windows and the limit it met — is kept in the host's account book (`accounts/book.json` in the cache), so an account near its limit is still known to be once its worker has ended.

## A harness that reports no windows comes after those that do

Where the host kit chooses a worker's harness, its first harness — which `switch --harness` puts there — is tried first. Of the rest, in kit order, each that reports its account's usage windows (`Harness::reports_windows`: Claude Code, Codex, Kimi Code) comes before each that reports none (Grok, Pi, DSH): admission can judge a harness with no windows only by the host, as its account's use cannot be seen until a limit is met. The kit's own order is not rewritten (`Kit::range` and `Kit::first` are its one home). The first is exempt because the kit's order is the only record of the operator's priority: a strict rule would silently override `switch --harness pi`.
