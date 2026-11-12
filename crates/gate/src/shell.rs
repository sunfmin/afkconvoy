//! A shell line as a program reads it before running it: its simple
//! commands, each as its words with the quoting taken off, and whether one of
//! them runs a heavy command a repo declares (docs/adr/0057) outside the
//! host's exclusive queue. Nothing is expanded: a variable is the text that
//! names it.

/// What ends a simple command of a shell line.
const COMMAND_ENDS: [char; 7] = ['\n', ';', '|', '&', '(', ')', '`'];
/// What ends a word without ending its command.
const WORD_ENDS: [char; 2] = ['<', '>'];
/// The words a command may stand behind and still be the command run.
pub const WRAPPERS: [&str; 12] =
    ["sudo", "env", "command", "exec", "nohup", "time", "!", "{", "then", "do", "else", "if"];
/// Whether `word` sets a variable for the command after it.
pub fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// The words of `args` that are no option and no option's value, given the
/// options that take the next word as theirs.
pub fn positionals<'a>(args: &[&'a str], values: &[&str]) -> Vec<&'a str> {
    let mut found = vec![];
    let mut rest = args.iter().copied();
    while let Some(arg) = rest.next() {
        if values.contains(&arg) {
            rest.next();
        } else if !arg.starts_with('-') {
            found.push(arg);
        }
    }
    found
}

/// A shell line as it is read: the simple commands found so far, and the one
/// being read.
#[derive(Default)]
struct Reading {
    commands: Vec<Command>,
    words: Vec<String>,
    word: String,
    /// Whether a word has begun: a quoted one may be empty.
    begun: bool,
}

/// A simple command's argument words, with redirected input kept separately:
/// stdin data must never become an option's value or a program name.
pub struct Command {
    pub words: Vec<String>,
    pub input: String,
}

impl std::ops::Deref for Command {
    type Target = [String];

    fn deref(&self) -> &Self::Target {
        &self.words
    }
}

impl Reading {
    fn letter(&mut self, c: char) {
        self.word.push(c);
        self.begun = true;
    }

    /// Begins a word at a quote → where what follows it stands.
    fn quoted(&mut self, within: Within) -> Within {
        self.begun = true;
        within
    }

    fn end_word(&mut self) {
        if self.begun {
            self.words.push(std::mem::take(&mut self.word));
            self.begun = false;
        }
    }

    fn end_command(&mut self) {
        self.end_word();
        if !self.words.is_empty() {
            self.commands
                .push(Command { words: std::mem::take(&mut self.words), input: String::new() });
        }
    }
}

/// The simple commands of the shell line `line`, each as its words with the
/// quoting taken off. Nothing is expanded: a variable is the text that names
/// it. A status answer reads the same words when it names the program a
/// script runs (`exclusive::status_command`).
pub fn simple_commands(line: &str) -> Vec<Vec<String>> {
    commands(line).into_iter().map(|command| command.words).collect()
}

/// The simple commands of the shell line `line`, each as its words with the
/// quoting taken off. Nothing is expanded: a variable is the text that names
/// it.
pub fn commands(line: &str) -> Vec<Command> {
    let mut reading = Reading::default();
    let mut within = Within::Plain;
    let mut escaped = false;
    let mut chars = line.chars().peekable();
    let mut heredocs: Vec<(usize, String, bool)> = vec![];
    while let Some(c) = chars.next() {
        if escaped {
            escaped = false;
            // A line carried on outside quotes ends no word and adds nothing.
            if c != '\n' || within != Within::Plain {
                reading.letter(c);
            }
            continue;
        }
        if c == '\n' && matches!(within, Within::Plain | Within::Comment) {
            within = Within::Plain;
            reading.end_command();
            for (command, delimiter, tabs) in heredocs.drain(..) {
                let body = heredoc_body(&mut chars, &delimiter, tabs);
                // The body belongs to the receiving command as data. A
                // handwritten record sent to gh still counts as a write.
                if let Some(command) = reading.commands.get_mut(command) {
                    command.input = body;
                }
            }
            continue;
        }
        match within {
            Within::Comment if c == '\n' => within = Within::Plain,
            Within::Comment => {}
            Within::Single if c == '\'' => within = Within::Plain,
            Within::Double if c == '"' => within = Within::Plain,
            Within::Double if c == '\\' => escaped = true,
            Within::Single | Within::Double => reading.letter(c),
            Within::Plain if c == '\'' => within = reading.quoted(Within::Single),
            Within::Plain if c == '"' => within = reading.quoted(Within::Double),
            Within::Plain if c == '\\' => escaped = true,
            Within::Plain if c == '<' && chars.peek() == Some(&'<') => {
                chars.next();
                reading.end_word();
                if chars.peek() == Some(&'<') {
                    // A here-string's next word is already ordinary data.
                    chars.next();
                } else {
                    let tabs = chars.peek() == Some(&'-');
                    if tabs {
                        chars.next();
                    }
                    let delimiter = heredoc_delimiter(&mut chars);
                    heredocs.push((reading.commands.len(), delimiter, tabs));
                }
            }
            Within::Plain if c == '#' && !reading.begun => {
                reading.end_command();
                within = Within::Comment;
            }
            Within::Plain if COMMAND_ENDS.contains(&c) => reading.end_command(),
            Within::Plain if c.is_whitespace() || WORD_ENDS.contains(&c) => reading.end_word(),
            Within::Plain => reading.letter(c),
        }
    }
    reading.end_command();
    reading.commands
}

/// Quote removal for the one word after << (or <<-). Unlike a command
/// argument, its value names where the following lines of data end.
fn heredoc_delimiter(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
        chars.next();
    }
    let mut delimiter = String::new();
    let mut within = Within::Plain;
    while let Some(&c) = chars.peek() {
        if within == Within::Plain
            && (c.is_whitespace() || COMMAND_ENDS.contains(&c) || WORD_ENDS.contains(&c))
        {
            break;
        }
        chars.next();
        match (within, c) {
            (Within::Plain, '\'') => within = Within::Single,
            (Within::Plain, '"') => within = Within::Double,
            (Within::Single, '\'') | (Within::Double, '"') => within = Within::Plain,
            (Within::Plain | Within::Double, '\\') => {
                if let Some(next) = chars.next() {
                    delimiter.push(next);
                }
            }
            _ => delimiter.push(c),
        }
    }
    delimiter
}

/// Consume one pending heredoc, leaving the next command (or heredoc) unread.
fn heredoc_body(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    delimiter: &str,
    tabs: bool,
) -> String {
    let mut body = String::new();
    while chars.peek().is_some() {
        let line: String = chars.by_ref().take_while(|c| *c != '\n').collect();
        let line = if tabs { line.trim_start_matches('\t') } else { &line };
        if line == delimiter {
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    body
}

/// Where in a shell line a character stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Within {
    Plain,
    /// Between two single quotes, where every character is itself.
    Single,
    /// Between two double quotes, where a backslash still escapes.
    Double,
    /// After a `#`, to the end of its line.
    Comment,
}

/// The command by which a shell line enters the host's exclusive queue, as
/// the embedder of the queue's holder names it: afkconvoy's own is
/// `afkconvoy exclusive`, another program's daemon names its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueCommand<'a> {
    /// The program it runs: one with no `/` matches a program by its file
    /// name.
    pub program: &'a str,
    /// The words its arguments begin with, options and their values aside.
    pub words: &'a [&'a str],
    /// The options before those words that take the next word as theirs.
    pub values: &'a [&'a str],
    /// The words a refusal puts before `--` in the line to run instead.
    pub instead: &'a [&'a str],
}

impl QueueCommand<'_> {
    /// Whether the command `program` run with `args` is this one: already
    /// in the queue, whatever it runs.
    fn runs(&self, program: &str, args: &[&str]) -> bool {
        let ran = if self.program.contains('/') { program } else { file_name(program) };
        ran == self.program && positionals(args, self.values).starts_with(self.words)
    }
}

/// Why the shell line `line` is refused for running one of a repo's heavy
/// commands, `patterns`,
/// outside the host's exclusive queue (sunfmin/control#577); `None` when it
/// runs none bare. A heavy command is a pattern of words: a simple command
/// runs it when its words, from its program on, begin with the pattern's — a
/// pattern's program with no `/` matches a program by its file name. A
/// simple command that runs `queue`, the embedder's way into the queue, is
/// already in the queue, whatever it runs. The argument after `sh -c` is
/// judged as the line it is. The reason names the pattern and gives the line
/// to run in its place, the same command behind `queue`'s `instead` words
/// and `--`.
pub fn heavy(line: &str, patterns: &[String], queue: &QueueCommand) -> Option<String> {
    if patterns.is_empty() {
        return None;
    }
    commands(line).iter().find_map(|command| runs_heavy(command, patterns, queue))
}

fn runs_heavy(words: &[String], patterns: &[String], queue: &QueueCommand) -> Option<String> {
    let mut before: Vec<&str> = vec![];
    let mut rest = words.iter().map(String::as_str);
    let mut wrapped = false;
    let program = loop {
        let word = rest.next()?;
        if WRAPPERS.contains(&word) {
            wrapped = true;
        } else if !(is_assignment(word) || wrapped && word.starts_with('-')) {
            break word;
        }
        before.push(word);
    };
    let args: Vec<&str> = rest.collect();
    if queue.runs(program, &args) {
        return None;
    }
    if let "sh" | "bash" | "zsh" | "dash" | "ksh" = file_name(program) {
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            if arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c') {
                return rest.next().and_then(|line| heavy(line, patterns, queue));
            }
        }
        return None;
    }
    let pattern = patterns.iter().find(|pattern| runs_pattern(pattern, program, &args))?;
    let mut instead: Vec<String> = before.iter().map(|word| shell_word(word)).collect();
    instead.extend(queue.instead.iter().map(|word| shell_word(word)));
    instead.push("--".to_string());
    instead.push(shell_word(program));
    instead.extend(args.iter().map(|word| shell_word(word)));
    Some(format!(
        "`{pattern}` is a heavy command of this repo (its `yaml heavy-commands` block in \
         docs/agents/afkconvoy.md): it runs through the host's exclusive queue, never bare, so \
         it waits for the gates and builds already running. Run `{}` instead",
        instead.join(" ")
    ))
}

/// Whether the command `program` run with `args` runs `pattern`: its words
/// begin with the pattern's, a pattern's program with no `/` matching
/// `program` by its file name.
fn runs_pattern(pattern: &str, program: &str, args: &[&str]) -> bool {
    let mut said = pattern.split_whitespace();
    let Some(first) = said.next() else { return false };
    let ran = if first.contains('/') { program } else { file_name(program) };
    if ran != first {
        return false;
    }
    let mut args = args.iter();
    for word in said {
        if args.next() != Some(&word) {
            return false;
        }
    }
    true
}

/// The file name of the program `word` names: what follows its last `/`.
fn file_name(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// `word` as a shell line would say it: as it is where nothing in it means
/// anything to a shell, else quoted.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word.chars().all(|c| c.is_ascii_alphanumeric() || "_-./=:,+@%^".contains(c));
    if plain { word.to_string() } else { format!("'{}'", word.replace('\'', "'\\''")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shell_line_is_read_as_its_simple_commands_and_their_words() {
        let read = |line: &str| -> Vec<String> {
            commands(line).iter().map(|words| words.join("·")).collect()
        };
        assert_eq!(read("cd /wt && git  push"), ["cd·/wt", "git·push"]);
        assert_eq!(read("a 'b c' \"d \\\" e\" f\\ g ''"), ["a·b c·d \" e·f g·"]);
        assert_eq!(read("a; b | c\nd $(e) `f`"), ["a", "b", "c", "d·$", "e", "f"]);
        assert_eq!(read("a # not; this\nb#c >out 2>&1"), ["a", "b#c·out·2", "1"]);
        assert_eq!(read("a \\\n b"), ["a·b"]);
        assert_eq!(read("  \n ;; "), [] as [String; 0]);
    }

    const GUILD: QueueCommand = QueueCommand {
        program: "guild",
        words: &["exclusive"],
        values: &[],
        instead: &["guild", "exclusive", "--fast"],
    };

    fn patterns() -> Vec<String> {
        ["cargo test", "./gate.sh"].map(String::from).to_vec()
    }

    #[test]
    fn an_embedders_queue_command_is_already_in_the_queue() {
        for line in [
            "guild exclusive -- cargo test",
            "guild exclusive --slow -- cargo test -p x",
            "/opt/bin/guild exclusive --fast -- ./gate.sh",
            "sh -c 'guild exclusive -- cargo test'",
            "RUST_LOG=debug env guild exclusive -- cargo test",
        ] {
            assert_eq!(heavy(line, &patterns(), &GUILD), None, "{line}");
        }
        for line in [
            "cargo test -p x",
            "afkconvoy exclusive -- true; cargo test",
            "guild hook && ./gate.sh",
            "guild-exclusive; cargo test",
            "sh -c 'cargo test'",
        ] {
            assert!(heavy(line, &patterns(), &GUILD).is_some(), "allowed: {line}");
        }
    }

    #[test]
    fn a_refusal_gives_the_line_behind_the_embedders_queue_command() {
        let why =
            heavy("cd x && FOO=1 cargo test -E 'a b'", &patterns(), &GUILD).expect("a refusal");
        assert!(why.contains("`cargo test` is a heavy command"), "{why}");
        assert!(
            why.contains("Run `FOO=1 guild exclusive --fast -- cargo test -E 'a b'` instead"),
            "{why}"
        );
    }
}
