---
name: afkconvoy
description: Operate afkconvoy, the CLI that runs an unattended fleet of coding agents against a repo's ready GitHub issues and lands their PRs. Use when the user mentions afkconvoy, wants to configure, plan, run, watch, stop, diagnose or debug an afkconvoy fleet, its workers, claims or landing train, or wants issues opened for the fleet to work.
---

# afkconvoy

Run `afkconvoy --help` and read all of it before anything else. It is the tool's own guide, and it says which command's help to read next.

When the user wants issues opened for the fleet to work, run `afkconvoy flows` in a checkout of their repo before opening the first one.

When the user asks whether the fleet is working, or something about it looks wrong, run `afkconvoy doctor` in that checkout first: it says what is wrong and the fix of each.
