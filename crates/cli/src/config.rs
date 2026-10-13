//! Reads a repo's config out of its markdown file.

use std::path::{Path, PathBuf};

use afkconvoy_core::config::{Config, Settings};
use anyhow::{Context, Result, anyhow};

use crate::git::Repo;

/// Where a repo keeps its config, relative to the checkout's top.
pub const FILE: &str = "docs/agents/afkconvoy.md";

/// The config file of the checkout the command was run in.
pub fn default_file() -> Result<PathBuf> {
    Ok(Repo::here()?.dir.join(FILE))
}

pub fn load(file: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read the config file {}", file.display()))?;
    parse(&text).with_context(|| format!("config file {}", file.display()))
}

/// The config in a markdown text: its first ```` ```yaml ```` block.
pub fn parse(markdown: &str) -> Result<Config> {
    let yaml = yaml_block(markdown).ok_or_else(|| anyhow!("has no ```yaml block"))?;
    let settings: Settings = serde_yaml::from_str(&yaml)?;
    Ok(Config::new(settings)?)
}

/// The text of the first ```` ```yaml ```` block of a markdown text: the one
/// carrier of every definition afkconvoy reads.
pub fn yaml_block(markdown: &str) -> Option<String> {
    let mut lines = markdown.lines().skip_while(|line| line.trim_end() != "```yaml");
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
    fn a_missing_required_key_is_named() {
        assert!(error("gate_command: make").contains("base_branch"));
        assert!(error("base_branch: main").contains("gate_command"));
    }
}
