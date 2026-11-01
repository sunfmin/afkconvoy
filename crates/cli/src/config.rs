//! Reads a repo's config out of its markdown file.

use std::path::{Path, PathBuf};

use afkconvoy_core::config::{Config, GATES_BLOCK, GateNeed, Gates, Settings};
use afkconvoy_core::cut;
use anyhow::{Context, Result, anyhow};

use crate::git::Repo;

/// Where a repo keeps its config, relative to the checkout's top.
pub const FILE: &str = "docs/agents/afkconvoy.md";

/// A warning budget can follow the main block so older afkconvoy versions
/// can still gate and land the change that introduces it.
pub const BUDGET_BLOCK: &str = "```yaml gate-budget";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GateBudget {
    gate_budget_seconds: u64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GateNeeds {
    checks: Vec<GateNeed>,
}

/// The config file of the checkout `repo`.
pub fn file_of(repo: &Repo) -> PathBuf {
    repo.dir.join(FILE)
}

pub fn load(file: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read the config file {}", file.display()))?;
    parse(&text).with_context(|| format!("config file {}", file.display()))
}

/// The config in a markdown text: its first ```` ```yaml ```` block, and the
/// optional gate-budget blocks, the fast and slow gate of its ```` ```yaml gates ```` block
/// (`GATES_BLOCK`) and the cut settings across its ```` ```yaml cut ```` blocks (`cut::BLOCK`). New
/// settings can follow the first cut block: older versions read only that
/// first block and skip additions, while duplicate keys still fail here.
pub fn parse(markdown: &str) -> Result<Config> {
    let yaml = yaml_block(markdown).ok_or_else(|| anyhow!("has no ```yaml block"))?;
    let mut settings: Settings = serde_yaml::from_str(&yaml)?;
    let budgets = fenced_all(markdown, BUDGET_BLOCK);
    if !budgets.is_empty() {
        let budget: GateBudget =
            serde_yaml::from_str(&budgets.join("\n")).context("its ```yaml gate-budget blocks")?;
        if settings.gate_budget_seconds.replace(budget.gate_budget_seconds).is_some() {
            return Err(anyhow!(
                "duplicate field `gate_budget_seconds`: declare it in one block only"
            ));
        }
    }
    let blocks = fenced_all(markdown, cut::BLOCK);
    let steps = match blocks.is_empty() {
        false => Some(cut_steps(&blocks.join("\n")).context("its ```yaml cut blocks")?),
        true => None,
    };
    let config = Config::new(settings)?.cutting(steps);
    let blocks = fenced_all(markdown, GATES_BLOCK);
    let gates = match blocks.is_empty() {
        false => Some(
            serde_yaml::from_str::<Gates>(&blocks.join("\n"))
                .map_err(anyhow::Error::from)
                .and_then(|gates| {
                    gates
                        .checked(config.gate_seconds(), config.gate_budget_seconds)
                        .map_err(|why| anyhow!(why))
                })
                .context("its ```yaml gates block")?,
        ),
        true => None,
    };
    let mut config = config.split(gates);
    for (block, target) in
        [("gate-needs", &mut config.gate_needs), ("cut-needs", &mut config.cut_needs)]
    {
        let blocks = fenced_all(markdown, &format!("```yaml {block}"));
        if blocks.is_empty() {
            continue;
        }
        let needs: GateNeeds = serde_yaml::from_str(&blocks.join("\n"))
            .with_context(|| format!("its ```yaml {block} block"))?;
        let mut names = std::collections::HashSet::new();
        for check in &needs.checks {
            anyhow::ensure!(
                !check.name.trim().is_empty()
                    && !check.command.trim().is_empty()
                    && check.seconds > 0,
                "{block}: each check needs a name, command and positive seconds"
            );
            anyhow::ensure!(
                names.insert(&check.name),
                "{block}: duplicate check name {}",
                check.name
            );
        }
        *target = needs.checks;
    }
    // An outer gate/land would ask for the lane a second time before it
    // could even reach the inherited gate wrapper.
    if config.gate_command.split_whitespace().collect::<Vec<_>>().windows(2).any(|pair| {
        pair[0].trim_matches(['\'', '"']).ends_with("afkconvoy")
            && matches!(pair[1].trim_matches(['\'', '"']), "gate" | "land")
    }) {
        return Err(anyhow!(
            "gate_command must name the build-and-test command, never `afkconvoy gate` or `land`; set `gate_command: afkconvoy exclusive -- ./gate.sh` for a heavy gate, or `gate_command: ./gate.sh` for a light gate"
        ));
    }
    Ok(config)
}

/// Whether the repo puts its gate in the host's worker lane. The wrapper
/// may use an absolute executable path or pass a directory with `-C`.
pub fn heavy_gate(command: &str) -> bool {
    let words: Vec<_> = command.split_whitespace().collect();
    words.iter().any(|word| word.trim_matches(['\'', '"']).ends_with("afkconvoy"))
        && words.contains(&"exclusive")
}

/// The cut steps a ```` ```yaml cut ```` block says: `set` and `publish`,
/// each something to run, and optionally `initial`, three version numbers.
fn cut_steps(yaml: &str) -> Result<cut::Steps> {
    let steps: cut::Steps = serde_yaml::from_str(yaml)?;
    steps.checked().map_err(|key| match key {
        "initial" => anyhow!("`initial` must be X.Y.Z, three whole numbers without leading zeroes or a v prefix; set `initial: 0.1.0` or remove it"),
        _ => anyhow!("`{key}` names nothing to run"),
    })
}

/// The text of the first ```` ```yaml ```` block of a markdown text: the one
/// main config block; gate-budget and cut settings may follow separately.
pub fn yaml_block(markdown: &str) -> Option<String> {
    fenced(markdown, "```yaml")
}

/// The text of the first fenced block of a markdown text whose opening line
/// is `opening`, and nothing else but trailing space.
fn fenced(markdown: &str, opening: &str) -> Option<String> {
    fenced_all(markdown, opening).into_iter().next()
}

/// Complete blocks with this opening, in their document order. The bodies
/// form one YAML mapping, so each setting still has exactly one home.
fn fenced_all(markdown: &str, opening: &str) -> Vec<String> {
    let mut lines = markdown.lines();
    let mut blocks = Vec::new();
    while lines.by_ref().any(|line| line.trim_end() == opening) {
        let mut block = String::new();
        for line in lines.by_ref() {
            if line.trim_end() == "```" {
                blocks.push(block);
                break;
            }
            block.push_str(line);
            block.push('\n');
        }
    }
    blocks
}

/// What a markdown text says above its first ```` ```yaml ```` block, with no
/// space around it; nothing where it has no such block.
pub fn prose(markdown: &str) -> String {
    let above: Vec<&str> =
        markdown.lines().take_while(|line| line.trim_end() != "```yaml").collect();
    if above.len() == markdown.lines().count() {
        return String::new();
    }
    above.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(yaml: &str) -> String {
        format!("{:#}", parse(&format!("intro\n```yaml\n{yaml}\n```\n")).unwrap_err())
    }

    #[test]
    fn only_the_yaml_block_is_read() {
        let text = "# title\nbase_branch: nope\n```yaml\nbase_branch: main\ngate_command: make\n```\nmore\n";
        let config = parse(text).unwrap();
        assert_eq!(config.base_branch, "main");
        assert_eq!(config.gate_command, "make");
    }

    #[test]
    fn existing_exclusive_gate_wrappers_use_the_worker_lanes_and_recursion_is_refused() {
        for command in [
            "afkconvoy exclusive -- ./gate.sh",
            "/opt/homebrew/bin/afkconvoy exclusive -- ./gate.sh",
            "'/opt/homebrew/bin/afkconvoy' exclusive -C . -- make test",
            "afkconvoy exclusive -- make gate",
        ] {
            let config = parse(&format!(
                "```yaml\nbase_branch: main\ngate_command: {}\n```\n",
                serde_json::to_string(command).unwrap()
            ))
            .unwrap();
            assert!(heavy_gate(&config.gate_command), "{command}");
        }
        assert!(!heavy_gate("./gate.sh"));
        for command in
            ["afkconvoy gate", "afkconvoy exclusive -- afkconvoy gate", "afkconvoy land --train"]
        {
            let why = error(&format!("base_branch: main\ngate_command: {command}"));
            assert!(why.contains("gate_command: afkconvoy exclusive -- ./gate.sh"), "{why}");
        }
        for key in ["gate_lanes", "train_lanes"] {
            assert!(
                error(&format!("base_branch: main\ngate_command: make\n{key}: 2")).contains(key)
            );
        }
    }

    #[test]
    fn the_prose_is_what_stands_above_the_yaml_block() {
        let text = "\n# Fix\n\nFor a defect.\n\n```yaml\nstages: []\n```\nnot this\n";
        assert_eq!(prose(text), "# Fix\n\nFor a defect.");
        assert_eq!(prose("```yaml\nstages: []\n```\nafter\n"), "");
        assert_eq!(prose("  \n```yaml\n```\n"), "");
        assert_eq!(prose("no block here\n"), "", "a file with no block says nothing");
    }

    #[test]
    fn a_file_with_no_block_is_no_config() {
        assert!(parse("base_branch: main\n").is_err());
        assert!(parse("```yaml\nbase_branch: main\n").is_err(), "an unclosed block");
    }

    #[test]
    fn an_unknown_key_is_named_not_ignored() {
        let e = error("base_branch: main\ngate_command: make\nretry: 2");
        assert!(e.contains("unknown field `retry`"), "{e}");
    }

    #[test]
    fn a_wrong_type_names_its_key() {
        for value in ["many", "0"] {
            let e = error(&format!("base_branch: main\ngate_command: make\nconcurrency: {value}"));
            assert!(e.contains("concurrency"), "{e}");
        }
        let e = error("base_branch: main\ngate_command: make\nepic_labels: epic");
        assert!(e.contains("epic_labels"), "{e}");
    }

    #[test]
    fn a_gate_limit_is_a_whole_number_of_seconds_from_1() {
        let limit =
            |value: &str| format!("base_branch: main\ngate_command: make\ngate_seconds: {value}");
        for wrong in ["0", "-5", "1.5", "soon", "[600]", "\"600\""] {
            let e = error(&limit(wrong));
            assert!(e.contains("gate_seconds"), "{wrong}: {e}");
        }
        let config = parse(&format!("```yaml\n{}\n```\n", limit("1"))).unwrap();
        assert_eq!(config.gate_seconds(), 1);
        // Budget rule enumeration belongs beside gate-limit parsing; the
        // CLI keeps one representative budget rejection to check its wiring.
        for wrong in ["0", "-1", "1.5", "soon", "[600]", "\"600\"", "3600"] {
            let e = error(&format!(
                "base_branch: main\ngate_command: make\ngate_budget_seconds: {wrong}"
            ));
            assert!(e.contains("gate_budget_seconds"), "{wrong}: {e}");
        }
        let e = error(
            "base_branch: main\ngate_command: make\ngate_seconds: 600\ngate_budget_seconds: 601",
        );
        assert!(e.contains("gate_budget_seconds"), "{e}");
        let valid = parse(
            "```yaml\nbase_branch: main\ngate_command: make\ngate_budget_seconds: 600\n```\n",
        )
        .unwrap();
        assert_eq!(valid.gate_budget_seconds, Some(600));
    }

    #[test]
    fn the_gate_split_in_two_is_a_block_of_its_own() {
        let main = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\ngate_seconds: 3600\ngate_budget_seconds: 600\n```\n";
        let gates = |fast: &str, slow: &str| {
            format!(
                "{main}```yaml gates\nfast:\n  command: ./gate.sh fast\n  {fast}\nslow:\n  command: ./gate.sh slow\n  {slow}\n```\n"
            )
        };
        let split = |fast: &str, slow: &str| parse(&gates(fast, slow));
        assert_eq!(parse(main).unwrap().gates, None);
        let config =
            split("seconds: 600\n  budget_seconds: 120", "seconds: 3000\n  budget_seconds: 480")
                .unwrap();
        let fast = config.gate(afkconvoy_core::config::Which::Fast).unwrap();
        assert_eq!(
            (fast.command.as_str(), fast.seconds, fast.budget_seconds),
            ("./gate.sh fast", 600, 120)
        );
        assert_eq!(config.gate(afkconvoy_core::config::Which::Slow).unwrap().seconds, 3000);
        // The whole gate is unchanged: a landing turn still runs it.
        assert_eq!((config.gate_command.as_str(), config.gate_seconds()), ("./gate.sh", 3600));
        for (fast, slow, key) in [
            (
                "seconds: 0\n  budget_seconds: 0",
                "seconds: 3000\n  budget_seconds: 480",
                "fast.seconds",
            ),
            (
                "seconds: 600\n  budget_seconds: 600",
                "seconds: 3000\n  budget_seconds: 480",
                "fast.budget_seconds",
            ),
            (
                "seconds: 600\n  budget_seconds: 120",
                "seconds: 3001\n  budget_seconds: 480",
                "past gate_seconds",
            ),
            (
                "seconds: 600\n  budget_seconds: 121",
                "seconds: 3000\n  budget_seconds: 480",
                "past gate_budget_seconds",
            ),
            ("seconds: 600", "seconds: 3000\n  budget_seconds: 480", "budget_seconds"),
            (
                "seconds: 600\n  budget_seconds: 120\n  retry: 2",
                "seconds: 3000\n  budget_seconds: 480",
                "retry",
            ),
        ] {
            let why = format!("{:#}", split(fast, slow).unwrap_err());
            assert!(why.contains(key) && why.contains("```yaml gates"), "{key}: {why}");
        }
    }

    #[test]
    fn the_cut_steps_are_a_block_of_their_own_which_turns_cutting_on() {
        let config = "```yaml\nbase_branch: main\ngate_command: make\n```\n";
        assert_eq!(parse(config).unwrap().cut, None, "no block: cutting is off");
        let steps =
            "```yaml cut\nset: ./bump \"$VERSION\"\npublish: |\n  ./ship\n  brew upgrade x\n```\n";
        let cut = parse(&format!("{config}\nTo cut:\n\n{steps}")).unwrap().cut.unwrap();
        assert_eq!(cut.set, "./bump \"$VERSION\"");
        assert_eq!(cut.publish, "./ship\nbrew upgrade x\n");
        assert_eq!(cut.busy_after_seconds, cut::BUSY_AFTER_SECONDS);
        assert_eq!(
            parse(&format!("{config}{steps}\n```yaml cut\nbusy_after_seconds: 0\n```\n"))
                .unwrap()
                .cut
                .unwrap()
                .busy_after_seconds,
            0,
            "zero is kept"
        );
        let steps = format!("{steps}\n```yaml cut\nbusy_after_seconds: 120\n```\n");
        assert_eq!(
            parse(&format!("{config}{steps}")).unwrap().cut.unwrap().busy_after_seconds,
            120
        );
        // An afkconvoy that reads the first block alone reads none of it.
        let both = format!("{config}{steps}");
        assert_eq!(yaml_block(&both).unwrap(), "base_branch: main\ngate_command: make\n");
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct OlderCut {
            set: String,
            publish: String,
        }
        let older: OlderCut = serde_yaml::from_str(&fenced(&both, cut::BLOCK).unwrap()).unwrap();
        assert_eq!(older.set, cut.set);
        assert_eq!(older.publish, cut.publish);
        let duplicate = format!("{both}\n```yaml cut\nbusy_after_seconds: 240\n```\n");
        assert!(format!("{:#}", parse(&duplicate).unwrap_err()).contains("duplicate field"));
        assert!(prose(&format!("{steps}{config}")).starts_with("```yaml cut"));
        let wrong = |steps: &str| {
            format!("{:#}", parse(&format!("{config}```yaml cut\n{steps}\n```\n")).unwrap_err())
        };
        let e = wrong("set: ./bump\npublish: ./ship\ndeploy: now");
        assert!(e.contains("```yaml cut") && e.contains("unknown field `deploy`"), "{e}");
        let e = wrong("set: ./bump");
        assert!(e.contains("publish"), "{e}");
        let e = wrong("set: ' '\npublish: ./ship");
        assert!(e.contains("`set` names nothing to run"), "{e}");
    }

    #[test]
    fn a_missing_required_key_is_named() {
        assert!(error("gate_command: make").contains("base_branch"));
        assert!(error("base_branch: main").contains("gate_command"));
    }

    #[test]
    fn an_initial_cut_declares_three_version_numbers() {
        let config = "```yaml\nbase_branch: main\ngate_command: make\n```\n";
        let declared = |initial: &str| {
            parse(&format!(
                "{config}```yaml cut\ninitial: {initial}\nset: bump\npublish: ship\n```\n"
            ))
        };
        for initial in ["0.0.0", "0.1.0", "12.34.56"] {
            let cut = declared(initial).unwrap().cut.unwrap();
            assert_eq!(cut.initial.as_deref(), Some(initial));
            assert_eq!(cut.initial_version().unwrap().to_string(), initial);
        }
        for wrong in [
            "v0.1.0",
            "0.1",
            "0.1.0-rc1",
            "0.01.0",
            "-1.2.3",
            "''",
            "[0, 1, 0]",
            "18446744073709551616.0.0",
        ] {
            let error = format!("{:#}", declared(wrong).unwrap_err());
            assert!(error.contains("initial"), "{wrong}: {error}");
        }
    }
}
