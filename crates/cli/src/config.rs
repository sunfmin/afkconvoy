//! Reads a repo's config out of its markdown file.

use std::path::{Path, PathBuf};

use afkconvoy_core::config::{Config, Settings};
use afkconvoy_core::cut;
use anyhow::{Context, Result, anyhow};

use crate::git::Repo;

/// Where a repo keeps its config, relative to the checkout's top.
pub const FILE: &str = "docs/agents/afkconvoy.md";

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
/// cut steps of its first ```` ```yaml cut ```` block (`cut::BLOCK`), which
/// turn cutting on. An afkconvoy that reads only the first block reads past
/// the second, and refuses nothing for it.
pub fn parse(markdown: &str) -> Result<Config> {
    let yaml = yaml_block(markdown).ok_or_else(|| anyhow!("has no ```yaml block"))?;
    let settings: Settings = serde_yaml::from_str(&yaml)?;
    let steps = match fenced(markdown, cut::BLOCK) {
        Some(yaml) => Some(cut_steps(&yaml).context("its ```yaml cut block")?),
        None => None,
    };
    let config = Config::new(settings)?.cutting(steps);
    // An outer gate/land would ask for the lane a second time before it
    // could even reach the inherited gate wrapper.
    if config.gate_command.split_whitespace().any(|word| word.ends_with("afkconvoy"))
        && config.gate_command.split_whitespace().any(|word| matches!(word, "gate" | "land"))
    {
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
/// carrier of every definition afkconvoy reads.
pub fn yaml_block(markdown: &str) -> Option<String> {
    fenced(markdown, "```yaml")
}

/// The text of the first fenced block of a markdown text whose opening line
/// is `opening`, and nothing else but trailing space.
fn fenced(markdown: &str, opening: &str) -> Option<String> {
    let mut lines = markdown.lines().skip_while(|line| line.trim_end() != opening);
    lines.next()?;
    let mut block = String::new();
    for line in lines {
        if line.trim_end() == "```" {
            return Some(block);
        }
        block.push_str(line);
        block.push('\n');
    }
    None
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
        let e = error("base_branch: main\ngate_command: make\nconcurrency: many");
        assert!(e.contains("concurrency"), "{e}");
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
        // An afkconvoy that reads the first block alone reads none of it.
        let both = format!("{config}{steps}");
        assert_eq!(yaml_block(&both).unwrap(), "base_branch: main\ngate_command: make\n");
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
