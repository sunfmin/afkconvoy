# Claude Code ancestor skill discovery

Verified on 26 October 2026 with the installed, logged-in Claude Code 2.1.310
for sunfmin/control#111. The host's configuration was never edited. All
profiles, skills, hooks, copies and debug logs belonged to disposable fixtures.

## Owning discovery path

The role's native project skill loader walks its parents. A controlled
profile's `.claude/skills` above a role is consequently a **project** source,
even with `host_user: false` and `--setting-sources project`.

The first regression run, before the fix, failed against the native
`system/init.skills` list:

```text
role-ancestor, host_user=false:
  convoy-ancestor-role, convoy-ancestor-user, convoy-ancestor-project
assertion failed: left: true, right: false
```

A second controlled run removed only the copied role's discovery boundary
before starting the unchanged launch line. Claude's own debug log identified
the owning loader (fixture paths abbreviated):

```text
Loading skills from:
  user=$FIXTURE/host/.claude/skills
  project=[$ROLE/.claude/skills, $FIXTURE/host/.claude/skills, ...]
Loaded 6 unique skills (... user: 0, project: 4, additional: 2, ...)
system/init.skills includes convoy-ancestor-user
assertion failed: left: true, right: false
```

That experiment is reproducible with:

```sh
AFKCONVOY_SMOKE_COMMAND='chmod u+w . && mv .git .git-disabled && claude --dangerously-skip-permissions' \
  cargo test -p afkconvoy --test cli smoke_claude_skills:: -- --ignored --nocapture
```

The wrapper acts only inside the disposable role copy. No tracked source is
changed. The test is expected to fail with that wrapper.

## Bound copies

The fleet puts an empty `.git` directory in each Claude role copy before
making it read-only. The installed harness recognises this entry as a
project discovery boundary, so its skill search stops at the role. It holds
no Git repository, configuration, hooks or history. Whole-role and
subdirectory copies have new cache names; the stock consult's copy is also
versioned again. Existing immutable copies are preserved.

With the boundary present, the same native debug output is:

```text
Loading skills from:
  user=$FIXTURE/host/.claude/skills
  project=[$ROLE/.claude/skills]
host_user=false: user: 0, project: 2, additional: 2
host_user=true:  user: 0, project: 2, additional: 4
```

The positive run reaches host skills through the fleet's existing added
directory of links. It never enables user settings. Claude's documented
[project discovery and added-directory behavior](https://code.claude.com/docs/en/skills#load-skills-in-monorepos-and-subdirectories)
explains the distinction between those sources; the empty boundary's effect
is verified against the installed harness by the test above.

## Coverage

```sh
cargo test -p afkconvoy --test cli smoke_claude_skills:: -- --ignored --nocapture
```

This remains outside the gate because it requires a login and sends paid
prompts. It exercises the real `Copies::role`, `keep_under` and
`Harness::ClaudeCode.in_role` paths with host skills above a role, above only
the added project, and above the normal `.cache/afkconvoy/copies` layout.
Each layout runs with host-user opt-in off and on. Assertions check:

- Host sentinel absence/presence in native `system/init.skills`, with role
  and project sentinels present in both runs.
- Invocation of the unprefixed duplicate-named skill yields the role's
  sentinel, even with both project and host copies available.
- The native role hook and fleet guard each refuse their designated Bash
  command; native hook events count the role hook, guard and status report.
- Project and host settings hooks never fire; the worker keeps its role's
  working directory and the host's login remains usable.

The ordinary gate test
`role::claude_role_copies_bound_discovery_without_reusing_older_unbounded_copies`
checks whole-role, subdirectory and stock-consult boundaries, read-only
permissions, cache reuse, and preservation of older copies. Existing role,
patrol and proposal scenarios check the paths used by fleet dispatch.

Ancestor **rule** discovery is handled separately by sunfmin/control#107.
This change bounds the observed native skill-discovery path; rerun the real
harness test when upgrading Claude Code because that path is harness-owned.
