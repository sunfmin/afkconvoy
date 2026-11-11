//! The gate as a repo declares it (docs/adr/0047, docs/adr/0057): the fast
//! gate and the slow gate, each with its command, its gate limit and its
//! gate budget, and the heavy commands that run only through the host's
//! exclusive queue. Each is read off a fenced block of its own in a markdown
//! text, which a program that knows no such block skips.

use serde::{Deserialize, Serialize};

/// The opening line of the block that declares a repo's heavy commands: one
/// of its own, which an older reader skips.
pub const HEAVY_BLOCK: &str = "```yaml heavy-commands";

/// The opening line of the block that splits a repo's gate in two: one of
/// its own, which an older reader skips, so the version that reads it can
/// still be landed and cut by the one before.
pub const GATES_BLOCK: &str = "```yaml gates";

/// The fast gate and the slow gate: the repo's gate as two, each checking
/// what the other does not, and together what `gate_command` checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gates {
    /// What a worker passes before it opens or updates its PR.
    pub fast: Gate,
    /// Everything else the gate checks.
    pub slow: Gate,
}

/// One of the two gates: what it runs, its limit and its warning budget,
/// as `gate_command`, `gate_seconds` and `gate_budget_seconds` are the
/// whole gate's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gate {
    pub command: String,
    /// The limit: a run still going after it is ended, and red.
    pub seconds: u64,
    /// The warning budget: a run past it is said to be slow, and is not red.
    pub budget_seconds: u64,
}

impl Gates {
    /// These gates, or why they cannot be run on, naming the key at fault.
    /// Neither may name nothing to run, have no time, or a budget not below
    /// its limit; together they are the gate split, so their limits may not
    /// add up past `gate_seconds`, nor their budgets past
    /// `gate_budget_seconds`, where it is declared.
    pub fn checked(
        self,
        gate_seconds: u64,
        gate_budget_seconds: Option<u64>,
    ) -> Result<Gates, String> {
        for (name, gate) in [("fast", &self.fast), ("slow", &self.slow)] {
            if gate.command.trim().is_empty() {
                return Err(format!("{name}.command: names nothing to run"));
            }
            if gate.seconds == 0 {
                return Err(format!(
                    "{name}.seconds: must be a whole number of seconds, at least 1"
                ));
            }
            if gate.budget_seconds == 0 || gate.budget_seconds >= gate.seconds {
                return Err(format!(
                    "{name}.budget_seconds: must be a whole number of seconds, at least 1 and below {name}.seconds"
                ));
            }
        }
        if self.fast.seconds.saturating_add(self.slow.seconds) > gate_seconds {
            return Err(format!(
                "fast.seconds and slow.seconds: together {}s, past gate_seconds, {gate_seconds}s: the two gates are the gate split, and neither limit is raised by it",
                self.fast.seconds.saturating_add(self.slow.seconds)
            ));
        }
        let budgets = self.fast.budget_seconds.saturating_add(self.slow.budget_seconds);
        if let Some(budget) = gate_budget_seconds.filter(|budget| budgets > *budget) {
            return Err(format!(
                "fast.budget_seconds and slow.budget_seconds: together {budgets}s, past gate_budget_seconds, {budget}s: the two gates are the gate split, and neither budget is raised by it"
            ));
        }
        Ok(self)
    }
}

/// One of the two gates a repo's gate is split into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    Fast,
    Slow,
}

impl Which {
    pub fn word(self) -> &'static str {
        match self {
            Which::Fast => "fast",
            Which::Slow => "slow",
        }
    }
}

/// Why a block of a repo's declaration cannot be run on.
#[derive(Debug)]
pub enum Fault {
    /// The block is not YAML of the shape it has.
    Yaml(serde_yaml::Error),
    /// The block reads, and says this, which cannot be run on.
    Said(String),
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fault::Yaml(error) => error.fmt(f),
            Fault::Said(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for Fault {}

/// Complete blocks with this opening, in their document order. The bodies
/// form one YAML mapping, so each setting still has exactly one home.
pub fn fenced_all(markdown: &str, opening: &str) -> Vec<String> {
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

/// The fast and the slow gate a markdown text declares in its ```` ```yaml
/// gates ```` blocks (`GATES_BLOCK`), held to the whole gate's limit of
/// `gate_seconds` and its budget of `gate_budget_seconds` (`Gates::checked`):
/// `None` where it has none.
pub fn gates(
    markdown: &str,
    gate_seconds: u64,
    gate_budget_seconds: Option<u64>,
) -> Result<Option<Gates>, Fault> {
    let blocks = fenced_all(markdown, GATES_BLOCK);
    if blocks.is_empty() {
        return Ok(None);
    }
    let gates: Gates = serde_yaml::from_str(&blocks.join("\n")).map_err(Fault::Yaml)?;
    gates.checked(gate_seconds, gate_budget_seconds).map(Some).map_err(Fault::Said)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HeavyCommands {
    commands: Vec<String>,
}

/// The heavy commands a markdown text declares in its ```` ```yaml
/// heavy-commands ```` blocks (`HEAVY_BLOCK`): none where it has none. Each
/// is a pattern of words, nonempty and declared once.
pub fn heavy_commands(markdown: &str) -> Result<Vec<String>, Fault> {
    let blocks = fenced_all(markdown, HEAVY_BLOCK);
    if blocks.is_empty() {
        return Ok(vec![]);
    }
    let heavy: HeavyCommands = serde_yaml::from_str(&blocks.join("\n")).map_err(Fault::Yaml)?;
    let mut commands: Vec<String> = vec![];
    for pattern in heavy.commands {
        let words = pattern.split_whitespace().collect::<Vec<_>>().join(" ");
        if words.is_empty() {
            return Err(Fault::Said("heavy-commands: a command is empty".into()));
        }
        if commands.contains(&words) {
            return Err(Fault::Said(format!("heavy-commands: `{words}` is declared twice")));
        }
        commands.push(words);
    }
    Ok(commands)
}

/// Whether a gate command puts the gate in the host's worker lanes: it runs
/// through `afkconvoy exclusive`, by any path to it, with `-C` or not.
pub fn heavy_gate(command: &str) -> bool {
    let words: Vec<_> = command.split_whitespace().collect();
    words.iter().any(|word| word.trim_matches(['\'', '"']).ends_with("afkconvoy"))
        && words.contains(&"exclusive")
}
