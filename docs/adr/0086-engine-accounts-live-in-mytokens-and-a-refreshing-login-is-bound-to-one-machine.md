---
status: accepted
---

# Engine accounts live in mytokens, and a refreshing login is bound to one machine

Extends ADR-0058 (sunfmin/control#1219).

Felix wants several subscription accounts per engine, used by afkconvoy and by Guild alike (control session, 2026-11-16). Both programs run on several machines, and a list of accounts kept by each program, or on each machine, would drift from the others.

## mytokens is the accounts' one home

Each **account** is one mytokens profile secret named `agent-account-<name>` (GLOSSARY: **Account**). mytokens already holds every credential of this user, keeps it in the keychain, and syncs it to every machine; a program lists the accounts with `mytokens list` and reads one with `mytokens get <service> --json`. The harness crate (ADR-0058) reads them (`afkconvoy_harness::account`), so afkconvoy and Guild read one format with one reader. The crate gives, for an account, the environment its session starts with (a Claude Code account's token; a login's home), the machine it is bound to, if any, and the login line a person runs where its login is missing.

A credential's value goes only into the environment of the session's process (`Environment::apply`): never to a file, a command line or a log. No type of the crate prints or serializes it, and an error of reading never holds what mytokens printed.

## A Claude Code account is a token; every other is a login bound to one machine

Claude Code's `claude setup-token` makes a token valid for a year that is never refreshed and may be used on several machines at once (code.claude.com/docs/en/authentication, 2026-11-16): it is the account's credential, given as `CLAUDE_CODE_OAUTH_TOKEN`, which Claude Code takes before the login kept in its configuration. The session is given no `CLAUDE_CONFIG_DIR` of the account's own: its configuration directory is where Claude Code keeps its sessions, its status line and the user's rules and skills, which the program that started it reads, and the token alone decides the account (ADR-0087, amending this ADR's first wording).

Codex, Grok and Kimi Code log in with a refresh token kept in a file. OpenAI says not to share the same file across machines — "Use one auth.json per runner" (learn.chatgpt.com/docs/auth and its ci-cd-auth page, 2026-11-16): a refresh on one machine invalidates the token the others hold. Grok and Kimi say nothing (github.com/xai-org/grok-build user guide 02-authentication; kimi.com/code/docs, 2026-11-16), so they are treated the same. Copying the file through mytokens would make each machine refresh it against the others; so mytokens holds no such credential. The secret names the one machine (`MACHINE`) its login lives on; on that machine the login is in the account's home, `~/.agent-accounts/<name>`, made there once by a person with the harness's device-code login (`codex login --device-auth`, `grok login --device-auth`, `kimi login`), which works over SSH with nobody at that machine's screen. Elsewhere the account is said to be bound to that machine, and no session starts on it there.
