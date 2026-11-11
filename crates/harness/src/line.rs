//! A shell line the tool writes for a shell to run: the one that starts a
//! worker, and those a brief tells a worker to run. Whatever differs from one
//! line to the next goes in as a word the shell reads as that text and nothing
//! more — but for the worker launch command, which is a shell string already
//! and goes in as it was given, once.

use flux_rs::attrs::*;

/// A shell line as it is put together. Text reaches it in no other way than
/// those below, so nothing that was read at run time is in it unquoted but
/// the worker launch command — and that at most once.
#[derive(Debug, Clone)]
#[refined_by(launched: bool)]
// The field is what the specs are stated on: nothing reads it back.
#[allow(dead_code)]
pub struct Line {
    text: String,
    /// Whether the worker launch command is in it.
    #[field(bool[launched])]
    launched: bool,
}

impl Line {
    #[spec(fn() -> Line[false])]
    pub fn new() -> Line {
        Line { text: String::new(), launched: false }
    }

    /// Adds text that every such line holds: it is written in this tool's
    /// source, so nothing read at run time can be passed for it.
    #[spec(fn(Line[@l], &str) -> Line[l])]
    pub fn fixed(mut self, text: &'static str) -> Line {
        self.text.push_str(text);
        self
    }

    /// Adds `text` as one word, whatever it holds.
    #[spec(fn(Line[@l], &str) -> Line[l])]
    pub fn word(mut self, text: &str) -> Line {
        self.text.push_str(&quoted(text));
        self
    }

    /// Adds `program` — what a command runs — as one word, left as it is
    /// where a shell reads every character of it as itself: a brief shows it
    /// to a worker.
    #[spec(fn(Line[@l], &str) -> Line[l])]
    pub fn program(mut self, program: &str) -> Line {
        let plain = !program.is_empty()
            && program.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-+".contains(&b));
        if plain {
            self.text.push_str(program);
        } else {
            self.text.push_str(&quoted(program));
        }
        self
    }

    /// Adds `number`.
    #[spec(fn(Line[@l], u64) -> Line[l])]
    pub fn number(mut self, number: u64) -> Line {
        self.text.push_str(&number.to_string());
        self
    }

    /// Adds the worker launch command `command` as it was given: the fleet
    /// never parses it. A line holds it once.
    #[spec(fn(Line[false], &str) -> Line[true])]
    pub fn launch(self, command: &str) -> Line {
        let mut text = self.text;
        text.push_str(command);
        Line { text, launched: true }
    }

    /// The line, of one that starts the worker launch command.
    #[spec(fn(Line[true]) -> String)]
    pub fn done(self) -> String {
        self.text
    }

    /// The line, of one that holds nothing unquoted: a command of this tool's
    /// own.
    #[spec(fn(Line[false]) -> String)]
    pub fn own(self) -> String {
        self.text
    }
}

impl Default for Line {
    #[spec(fn() -> Line[false])]
    fn default() -> Line {
        Line::new()
    }
}

/// `text` as one word of a shell line, whatever it holds.
pub fn quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A line that starts nothing, finished as one that starts a worker.
    #[should_fail]
    fn a_line_finished_with_no_launch_command(dir: &str) -> String {
        Line::new().fixed("cd ").word(dir).done()
    }

    /// A second text put in as it was given.
    #[should_fail]
    fn a_line_given_two_unquoted_texts(command: &str, model: &str) -> String {
        Line::new().launch(command).fixed(" --model ").launch(model).done()
    }

    /// A line that starts the worker launch command, finished as one that
    /// holds nothing unquoted.
    #[should_fail]
    fn a_launch_command_in_a_command_of_this_tools_own(command: &str) -> String {
        Line::new().launch(command).own()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// Any text a shell word can hold: what a shell reads as more than text,
    /// often, among any characters at all. Never a NUL, which no argument of
    /// a command holds.
    fn shell_text() -> impl Strategy<Value = String> {
        let read_by_a_shell = prop::sample::select(vec![
            '\'', '"', '\\', '\n', '\r', '\t', ' ', '$', '`', '!', '*', '?', '~', '#', ';', '&',
            '|', '<', '>', '(', ')', '{', '}', '[', ']', '=', '-', 'a', 'é', '文', '\u{a0}',
        ]);
        let any_other = any::<char>().prop_filter("a NUL", |c| *c != '\0');
        prop::collection::vec(prop_oneof![read_by_a_shell, any_other], 0..24)
            .prop_map(|chars| chars.into_iter().collect())
    }

    /// What `/bin/sh` printed running `line`.
    fn read(line: &str) -> Vec<u8> {
        let read = std::process::Command::new("/bin/sh").arg("-c").arg(line).output().unwrap();
        assert!(read.status.success(), "{line:?}: {read:?}");
        read.stdout
    }

    proptest! {
        /// `printf` writes its format once for each argument it is given: one
        /// NUL after the text says the shell read one word, and that word.
        #[test]
        fn a_shell_reads_the_quoted_form_of_any_text_as_that_one_word(text in shell_text()) {
            let line = format!("printf '%s\\0' {}", quoted(&text));
            let read = std::process::Command::new("/bin/sh").arg("-c").arg(&line).output().unwrap();
            prop_assert!(read.status.success(), "{line:?}: {read:?}");
            prop_assert_eq!(read.stdout, [text.as_bytes(), b"\0"].concat(), "{:?}", line);
        }

        #[test]
        fn a_shell_reads_a_program_of_any_text_as_that_one_word(text in shell_text()) {
            let line = Line::new().fixed("printf '%s\\0' ").program(&text).own();
            prop_assert_eq!(read(&line), [text.as_bytes(), b"\0"].concat(), "{:?}", line);
        }
    }

    #[test]
    fn a_line_holds_each_text_in_the_order_given() {
        let line = Line::new()
            .fixed("cd ")
            .word("/it's here")
            .fixed(" && ")
            .launch("kim --yolo")
            .fixed(" --issue ")
            .number(7)
            .done();
        assert_eq!(line, "cd '/it'\\''s here' && kim --yolo --issue 7");
    }

    #[test]
    fn a_program_is_quoted_only_where_a_shell_would_read_more_than_text() {
        let program = |text: &str| Line::new().program(text).own();
        assert_eq!(program("/opt/homebrew/bin/afkconvoy"), "/opt/homebrew/bin/afkconvoy");
        assert_eq!(program("/Users/a b/afkconvoy"), "'/Users/a b/afkconvoy'");
        assert_eq!(program(""), "''");
    }
}
