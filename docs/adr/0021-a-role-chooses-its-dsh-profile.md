---
status: accepted
---

# A role chooses its DSH profile

A team may need its DSH workers to have the plugins, MCP servers, patches, theme and settings its host user works with by hand. Its team file chooses `dsh_profile: host-user` to load the user's `dsh-tui` profile in place; `dsh_profile: fleet`, the default, retains the fleet's own profile. A role may override either team choice by naming its directory as a map, beside `host_user`. `check` refuses other values and answers the team default and each role's choice.

This deliberately departs from ADR-0017's exclusion of host settings, hooks, MCP servers, plugins and extensions, for DSH alone and only on the team's or role's explicit say-so. Copying a selected subset would make the fleet responsible for understanding arbitrary user plugins and would drift from the session the user runs by hand. Loading the profile in place preserves its dependencies and both its profile and home patch layers. Nothing in the user's DSH_HOME is written by the fleet.

The role's limits still apply. The role's overlay follows the user layers and the fleet's overlay follows that: it adds the guard and orca state report, and makes the TUI and agent loop depend on the guard's service. User settings cannot turn these off. The guard remains protection against a worker that errs, as ADR-0012 says.

The profile choice is independent of ADR-0017's `host_user` skills-and-rules choice. A host profile can discover skills and rules itself even when the fleet supplies none with `host_user: false`. Such a role may behave differently on two hosts; a team requiring the fleet's controlled profile chooses `fleet`. Other harnesses are unchanged.
