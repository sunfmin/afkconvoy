---
status: accepted; its host-sleep question superseded by ADR-0050
---

# A first start is settled with the user

A private tracker decides whose issues a fleet works, and host sleep decides whether a host may go to sleep. Neither choice should arrive as a surprise. Before a first start the help has the agent explain what is needed, set up what requires no user decision, and ask the remaining questions together, recommending an answer and giving the line for each option. A new private tracker is created only on the user's yes to its owner and name.

This amends ADR-0027: a checkout not on the host's checkout list must explicitly say `--tracker <owner>/<repo>` or `--tracker default` on its first `run`. `default` still means its own repo, and the private-tracker checks still apply. A host with no saved host settings must say `--host-sleep`, `--no-host-sleep`, or `--host-sleep-hours`. Saved settings, even defaults, and existing checkout entries retain their meaning; there is no new migration or acknowledgment record. Concurrency is asked about in the help but may retain its default of three.

An unanswered first start refuses before any writes or host-runner request, including the host kit, labels, checkout list and logs. Its one JSON object contains `error`, `asks` with every missing decision and its options and lines, and `see` pointing to `afkconvoy help first-start`. One question list generates both the topic and these asks. The operator's watch directory is asked about too, and defaults to none: a directory is chosen only by the user. Future host settings requiring a user decision add their question there when those settings exist.

## Considered

- Keep the implicit defaults and explain them only after starting: a fleet could still start working the wrong issues or put a host to sleep before its user understood the choice.
- Prompt in the CLI: an unattended agent cannot answer for its user. The refusal gives the agent the questions to take back in one message, and explicit flags carry the answers.
- Require every setting as a flag: most settings have useful defaults and need explanation, not a mandatory flag. Only the tracker of a new checkout and the sleep choice of a new host refuse a start here.
