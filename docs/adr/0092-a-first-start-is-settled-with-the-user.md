---
status: accepted
---

# A first start is settled with the user

A host setting that needs its user's decision should not arrive as a surprise. Before a first start the help has the agent explain what is needed, set up what requires no user decision, and ask the remaining questions together, recommending an answer and giving the line for each option.

Amended by ADR-0091: the tracker is no longer asked. A checkout that says none works its host's local tracker, which no stranger can write, so a first start no longer waits on that choice; a GitHub tracker named with `--tracker <owner>/<repo>` is still checked to be private.

An unanswered first start refuses before any writes or host-runner request, including the host kit, labels, checkout list and logs. Its one JSON object contains `error`, `asks` with every missing decision and its options and lines, and `see` pointing to `afkconvoy help first-start`. One question list generates both the topic and these asks. The operator's watch directory is asked about too, and defaults to none, which runs the stock watch (ADR-0033): a directory is chosen only by the user. Future host settings requiring a user decision add their question there when those settings exist. With the tracker no longer asked, no setting refuses a first start today: the watch directory keeps its default, and the refusal stays for a setting that will need it.

## Considered

- Keep the implicit defaults and explain them only after starting: a fleet could still start working the wrong issues before its user understood the choice.
- Prompt in the CLI: an unattended agent cannot answer for its user. The refusal gives the agent the questions to take back in one message, and explicit flags carry the answers.
- Require every setting as a flag: most settings have useful defaults and need explanation, not a mandatory flag. Only a setting with no useful default refuses a start here.
