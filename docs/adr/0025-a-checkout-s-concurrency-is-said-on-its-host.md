---
status: superseded by ADR-0072
---

# A checkout's concurrency is said on its host, and the repo config names none

How many workers a repo gets at once was the repo config's `concurrency`: a commit, a PR and a merge to the base branch to change it. Then a checkout could say its own on its host, `afkconvoy run <checkout> --concurrency N`, read at each tick, and two places said one number. An operator asked which one was in force, and the answer depended on a third thing, whether the checkout had said one.

How many workers run at once is what a host can carry — its cores, its memory, its logins, what else it is needed for this afternoon — and no fact of the repo. Two hosts that work one repo carry different numbers, and one host carries a different number tonight than at noon. So a checkout's concurrency has one home: the checkout list of the host that runs it, beside the checkout. It is 3 until that host says otherwise, and the repo config's `concurrency` is read no more.

## Retiring the key

The repos on the host this was decided on said 2, 3, 4, 5 and 8 in their configs. Refusing the key, as `harness` was refused (ADR-0020), would have stopped every one of those fleets at its next tick for a line nobody needed to change in a hurry. Ignoring it silently would have dropped each to 3 with nothing to say why.

- A config that names `concurrency` is read as before, but for that key: it is not an unknown key, and it changes nothing.
- `doctor` reports each checkout whose repo config names it as `info`, with the value it names and the concurrency in force: nothing is held up, even where the values differ. Its fix is `afkconvoy run <checkout> --concurrency <value>`, which keeps the number on this host, and then the key deleted; where the checkout says its own or the config names the number already in force, the deletion alone.
- `afkconvoy config` answers no `concurrency`.

## Considered options

- **Keep both, the checkout's laid over the repo's.** What ADR-0023's checkout list made possible first. Refused: two places to say one number, and which holds is a fact you cannot see in either.
- **The repo's as a ceiling the host may lower.** A repo could cap a fleet that hurts a shared service. Refused: nothing asked for it, and a ceiling is still a second place to look. A repo that needs one can say so to its hosts.
- **Refuse the key.** The cleanest end state. Refused for now: it stops fleets for a number whose only fix is a commit. Once no repo names it, a later change can refuse it with the line to delete, as `harness` is.

## Consequences

- A host that starts working a repo works it at 3 until it says otherwise, whatever another host says.
- A second host's checkout that says none drops to 3 once the key is deleted on the base branch: each host keeps its number with its own `run` before the deletion reaches it.
- Supersedes, of the checkout's concurrency added beside ADR-0023, that a checkout that says none goes by the repo config's.
