# CLI test maintenance

For sunfmin/control#152, against `456b24d1683252d0386a3b79c1ec98cb2c342ea7`.
Production behavior, gate scope, time limits, ignored tests, property generators,
and Flux specifications are unchanged. This change reduces maintenance work;
it does not claim to solve the test suite's several-minute runtime.

## Decisions and coverage ownership

| Candidate | Decision | What remains responsible |
| --- | --- | --- |
| `help::a_topic_of_a_teams_files_opens_with_what_it_is_and_ends_with_what_to_read_next` | Delete the opening sentence template, terminal section heading, minimum link count, and link-only layout. These editorial preferences cease to be gate contracts. | `help::the_manual_holds_the_topics_there_are_and_no_other` compares the actual topic contents; `help::no_help_text_names_a_command_or_a_topic_that_does_not_exist` resolves references throughout all help texts. The index/manual tests retain topic and command entry reachability. |
| `help::the_readme_explains_no_concept_and_sends_its_reader_to_the_help` | Replace with `the_readme_points_to_the_help_and_its_generated_manual`. Fixed heading order, banned substrings, and the retired translation filename cease to be gate contracts. | Check the actual `--help`, `help <topic>`, and generated manual links, including the manual file's existence. The skill installation assertions remain. |
| `help::no_paragraph_of_the_guide_a_command_or_a_topic_is_wrapped` | Delete, including its private `wrapped` implementation and its example assertion. Paragraph wrapping ceases to be a gate contract. | Existing content, command/flag, example parsing/execution, glossary, index size (20 KiB, the harness display bound), and short-help screen bounds remain. No content check is replaced by a length-only assertion. |
| `help::the_guide_and_each_subcommand_agree_on_who_runs_it_and_why` | Retain responsibility groups, actors, worker worktree context, hook ownership, automatic tick transitions, and infrequent plumbing use. Remove fixed sentence starts, punctuation, and `Do it yourself when`. | The same test checks actual actor/context words and exact command group membership. Other help tests still execute every visible command's help and check its advertised arguments. This does not attempt to automatically judge arbitrary English paraphrases. |
| `manual::the_gate_checks_the_repositorys_page` | Delete inspection of the exact shell command's source text. That implementation spelling ceases to be a contract. | The unchanged gate actually runs the manual example's `--check`. Three manual tests retain generation, exact agreement with `help all`, and rejection of stale/missing files. Staleness now appends known content and compares the entire unchanged file, instead of depending on a particular glossary phrase being present. |
| `probe::plan_help_says_when_it_probes` | Retain the documented probe conditions; launch the CLI directly instead of constructing a `World`. | The check now also asserts exit 0 and empty stderr. All real startup-probe I/O scenarios remain unchanged. |
| `config::a_bad_config_is_an_error_naming_the_key` | Replace ten CLI rows with four representative rows: required key, unknown key, fractional YAML value, and zero domain boundary. | `config_errors_wire_yaml_parsing_and_domain_validation_to_json` still passes through file loading, YAML decoding, `Config::new`, one JSON error, and a nonzero exit. The success/default/`--file`/checkout-discovery tests remain. |
| Missing `gate_command` and label collision CLI rows | Merge into core rule ownership, with parser error propagation retained. | Core `the_two_keys_without_a_default_are_required` now checks absent, empty, and whitespace-only values for **both** required keys, comparing exact `ConfigError::Required` variants/keys. Core `a_config_nothing_could_run_on_is_refused` retains `ZeroGateSeconds`, `EscalatesToReady`, and empty-label variants. CLI parser `a_missing_required_key_is_named` still checks both fields' error text. |
| Repeated invalid concurrency and gate-limit CLI rows | Consolidate into the existing parser tests, which require no Git repository, host runner, or worker. | `a_wrong_type_names_its_key` retains text and sequence rejection and gains zero concurrency. `a_gate_limit_is_a_whole_number_of_seconds_from_1` retains zero, negative, fractional, text, sequence, quoted number, and valid 1. `an_unknown_key_is_named_not_ignored` retains unknown-field rejection. |
| Pure-rule rows in `check::a_bound_repos_config_answers_its_three_keys_more_and_refuses_a_harness` | Remove invalid repo syntax, team-required patrol/intake, and the duplicate unbound-harness row. Retain actual bound/unbound output values, bound harness rejection, and distinct YAML type branches (team sequence, negative intake, scalar patrols, harness sequence). | Core `a_team_is_named_as_github_names_a_repo`, `a_key_of_a_bound_repo_is_refused_where_no_team_is_named`, and `a_config_that_names_a_harness_is_refused_with_the_line_to_delete` own those rules. `check`'s separate harness error path remains because it is another command's loading/error wiring. |
| Fourteen CLI DSH profile rejection cases | Move the entire seven-value × two-location matrix into the core; retain both CLI success/inheritance directions and representative rejection at both field locations. | New core `dsh_profiles_inherit_and_override_the_team_and_reject_other_values_at_each_key` checks actual enum values, exact fault count, `team.md`, nested key, and error text. CLI `dsh_profile_defaults_to_fleet_and_roles_override_the_team_in_either_direction` still checks serialized values, exit 1, and team/role field positions. |

Core team unknown-field/type/location tests, host-user settings, splitting,
workflow/roster/skill validation, and the aggregated CLI team-fault report are
retained. Similar names do not establish duplicate behavior: all Git races,
gate/landing, ownership/deletion, single-writer, termination, recovery,
idempotence, and permission tests remain. No real-model smoke test is disabled.
`every_file_here_is_a_module` remains to prevent an uncollected test file from
silently passing.

## Change size

Eight Rust files: 81 lines added, 136 removed. Every changed Rust statement is
inside a test or its private helper. Three CLI tests are deleted, one core test
is added; net two fewer tests. The README and config checks are replacements,
not additional test removals.

Static `assert!`/`assert_eq!`/`assert_ne!` call sites in those files change from
780 to 774: help 178→166, manual 15→14, probe 103→105, core team 228→233;
the other files' totals are unchanged. These are source sites, not dynamic
assertion counts: loops still exercise their complete specified domains.
The config CLI rejection table shrinks 10→4, bound-config rejection rows 9→5,
and DSH CLI rejection rows 14→2 with all 14 moved to core. Counts describe the
change; the ownership decisions above justify it.

## Verification method

The unchanged full gate runs in a detached worktree at the base above, inside
this worktree's ignored `target/issue-152/before`. Both full gates use the
repository's `./gate.sh`, through `afkconvoy exclusive`, with the existing
Flux, clippy, generated-manual check, nextest collection, six test threads,
deep TMPDIR, ignored-test policy, and time limits. Queue wait is separate from
gate execution. The comparison includes the gate's step times, test totals,
and reported slow tests; it is not a controlled speed benchmark.

Targeted fault injection builds the original and revised tests against the
same dormant, environment-selected faults. Each selected test first passes
with no fault enabled and must then fail with exactly one fault enabled.
A compiler error, an empty test selection, or a failing control is not a
detected fault. The real CLI's `manual`, `fake_gh`, and `fake_orca` examples
must be built first (`cargo build -p afkconvoy --examples`). Temporary
instrumentation is restored before either final gate or commit.

The compared faults are: accepting a missing/blank required key; accepting a
ready/escalate label collision; coercing zero gate seconds to one; accepting
invalid repo syntax; allowing patrols without a team; allowing intake without
a team; accepting a retired harness key; dropping unknown YAML fields;
coercing fractional gate seconds; accepting zero concurrency; bypassing YAML
and domain config loading; accepting invalid DSH profiles; losing a nested
profile key; returning the wrong actual profile enum; printing two JSON
errors; exiting zero on an error; accepting a stale manual; introducing an
unresolvable topic reference; and dropping the watch's documented actor.

The required-key, collision, zero, repo, team-required, and harness faults run
the same existing core tests before and after. Unknown-field and fraction
faults run the same parser tests. Zero concurrency moves from the CLI table
to `a_wrong_type_names_its_key`; invalid profile values and nested keys move
from the CLI matrix to the new core test. Config wiring, actual serialized
profiles, JSON/exit errors, manual rejection, help reference reachability, and
actor responsibility also retain real CLI checks. Editorial preferences
explicitly removed above are not counted as retained behavior.

Execution receipts and full-gate measurements are recorded in the PR evidence.
