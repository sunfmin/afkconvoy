The team [`afkconvoy help example`](help:example) reads file by file, pushed, bound to a repo and run. First what must be in place; then one issue of its `fix` workflow, followed from its flow label to the base branch, with how long each stage took and what it cost.

## Before a fleet works with it

`check` accepts the team as written, once it is pushed: it reads a role from GitHub. Three skills that the fellow's and the attending's own skills say to load are other authors' and are not written. Each role's `skills-lock.json` names them and where each is from: add them in each of the two directories, and commit them.

```sh
npx skills add <source> --skill <name> --agent claude-code
```

Until then `skills-loaded.sh` refuses every `pass` and `reject` of those two roles, and the attending's `limits.ts` does under pi. The hooks call `jq`: every host of the fleet needs it.

The roster lets the attending run pi first, so a host whose host kit holds pi needs `pi`, logged in to the provider of its model: `openai-codex/gpt-5.5`, as written — `afkconvoy kit` adds it to the host kit as pi lists it, and research places it; until then pi runs its default model. Write another model pi lists — or take pi out of the attending's entry, and it runs Claude Code as the others do. A host kit holding pi on a host without it starts nothing: `plan`, `tick` and `run` refuse, `doctor` finds the same, and each names the range that calls for it and the line that installs it:

```text
pi is not installed, called for by the range of the role `attending`
```

## One issue, from its label to the base branch

A repo holding one small Go package was bound to the team; its gate formats, vets and tests. One issue was opened with the label `flow:fix`: a named form the package was to read, with what each call should give written out. Nobody touched it again. One fleet instance on one laptop worked it:

```text
stage         role             harness      model                 worked    cost
workup        fellow           claude-code  fable                 1m 44s   $1.39
plan-review   attending        pi           openai-codex/gpt-5.5     51s   $0.41
treatment     fellow           claude-code  opus                  1m 06s   $0.45
code-review   attending        claude-code  opus                  1m 02s   $0.47
discharge     discharge-nurse  claude-code  sonnet                   28s   $0.14
the landing   discharge-nurse  claude-code  its default              31s   $0.19
```

From opened to merged on the base branch: 9 minutes 29 seconds and $3.05, with no rework and no human. The repo's two patrols, `triage` and `rounds`, ran three times beside it, for $0.23 more.

`worked` is a worker's session, from its first word to its last. The rest of the nine minutes is the fleet's: one tick reads a verdict and starts the next stage's worker, and a worker's `wake` ends the sleep between two. `cost` is what each harness reckoned for its session at its provider's listed prices: under a subscription login nothing is billed by the call.

Every stage left its record on the issue, a comment that names the harness it ran on beside the commits of the role and of the team it ran under:

```text
<!--convoy:stage stage=plan-review verdict=pass … harness=pi-->
```

## What held the pi worker

The issue asked the plan's reviewer to try one commit, so that the refusal could be seen. pi made the call, it was refused, and the worker quoted the refusal in the text of its verdict:

```text
afkconvoy guard: the stage `plan-review` is marked `writes: false`: `git commit`
is refused. A stage that writes nothing leaves no commit and pushes nothing; say
what you found in the text of your verdict
```

That is the fleet's guard, which a pi worker asks through the extension the fleet adds to it ([`afkconvoy help worker-launch`](help:worker-launch)). It answered before the role's own limits were reached, so `limits.ts` refused nothing in this run: it is what refuses the same commit in a stage a workflow left unmarked. Its other limit was met and let the worker through — the reviewer read the `SKILL.md` of both skills its skill names before it declared `pass`.

## Where to read on

- [`afkconvoy help example`](help:example): the team's files, each with the decision it holds
- [`afkconvoy help workflows`](help:workflows): `harness` and `model`, and every other key of a stage
- [`afkconvoy help team`](help:team): where a role's limits for each harness go
