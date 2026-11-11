//! The one encoding of every record the fleet keeps: on a ref, as a commit's
//! subject; in a comment, as a marker. A word, then `field=value` pairs.
//! Whatever was written reads back exactly.

use flux_rs::attrs::*;

/// A record's fields, as read: every one the text states, in its order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fields(Vec<(String, String)>);

impl Fields {
    /// The value of `name`, when the record states one.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.iter().find(|(field, _)| field == name).map(|(_, value)| value.as_str())
    }

    /// `name` as a number the fleet wrote (`fleet_number`).
    pub fn number(&self, name: &str) -> Option<u64> {
        self.get(name).and_then(fleet_number)
    }

    /// `name` as a comma-separated list of numbers; a hand may write `#3`.
    /// Anything else in the list is dropped.
    pub fn numbers(&self, name: &str) -> Vec<u64> {
        let Some(raw) = self.get(name) else { return vec![] };
        raw.split(',')
            .filter_map(|item| fleet_number(item.strip_prefix('#').unwrap_or(item)))
            .collect()
    }
}

/// A number as the fleet writes one: ASCII digits, no sign, no leading zero,
/// at most 18 of them. Anyone can write a ref name or a comment, so every other
/// spelling is not a number rather than another name for one.
pub fn fleet_number(raw: &str) -> Option<u64> {
    let plain = raw.bytes().all(|b| b.is_ascii_digit());
    let canonical = raw == "0" || !raw.starts_with('0');
    if raw.is_empty() || raw.len() > 18 || !plain || !canonical {
        return None;
    }
    raw.parse().ok()
}

/// A record → the one line that carries it. A field with an empty value is
/// left out: it reads back as not stated.
pub fn write(word: &str, fields: &[(&str, &str)]) -> String {
    let mut line = word.to_string();
    for (name, value) in fields {
        if !value.is_empty() {
            line.push(' ');
            line.push_str(name);
            line.push('=');
            line.push_str(&encode(value));
        }
    }
    line
}

/// The first line of `text` → the fields of the record of kind `word` it
/// carries; `None` when it is no such record.
pub fn read(word: &str, text: &str) -> Option<Fields> {
    let line = text.lines().next().unwrap_or("").trim();
    let rest = line.strip_prefix(word)?;
    if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    Some(fields(rest))
}

/// The marker that carries a record in a comment.
pub fn marker(word: &str, fields: &[(&str, &str)]) -> String {
    format!("<!--{}-->", write(word, fields))
}

/// A comment body → the fields of its first marker of kind `word`.
pub fn read_marker(word: &str, body: &str) -> Option<Fields> {
    markers(word, body).into_iter().next()
}

/// Every marker of kind `word` in `body`, in the order written. One comment
/// can carry several — a work account keeps each red gate run it has cited.
pub fn markers(word: &str, body: &str) -> Vec<Fields> {
    let mut found = vec![];
    let mut rest = body;
    while let Some((_, inside)) = rest.split_once("<!--") {
        let Some((text, after)) = inside.split_once("-->") else { break };
        if let Some(stated) = text.trim().strip_prefix(word)
            && (stated.is_empty() || stated.starts_with(char::is_whitespace))
        {
            found.push(fields(stated));
        }
        rest = after;
    }
    found
}

/// The record of kind `word` kept in a list of comment bodies, oldest first:
/// the latest marker wins → (its index, its fields).
pub fn latest_marker<'a>(
    word: &str,
    bodies: impl IntoIterator<Item = &'a str>,
) -> Option<(usize, Fields)> {
    let mut found = None;
    for (at, body) in bodies.into_iter().enumerate() {
        if let Some(fields) = read_marker(word, body) {
            found = Some((at, fields));
        }
    }
    found
}

/// A list field may run over several words: a word with no `=` continues the
/// field before it (`blocked_by=3, 4`).
fn fields(text: &str) -> Fields {
    let mut found: Vec<(String, String)> = vec![];
    for word in text.split_whitespace() {
        match word.split_once('=') {
            Some((name, value)) => found.push((name.to_string(), decode(value))),
            None => {
                if let Some((_, value)) = found.last_mut() {
                    if !value.ends_with(',') {
                        value.push(',');
                    }
                    value.push_str(word);
                }
            }
        }
    }
    Fields(found)
}

/// Bytes that are written as themselves; every other is `%XX`. A space would
/// end the value, `%` would read as an encoding, and `>` could close a marker.
fn plain(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~/:@+,#".contains(&byte)
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if plain(byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn decode(raw: &str) -> String {
    let mut out = Vec::new();
    let mut bytes = raw.bytes();
    while let Some(byte) = bytes.next() {
        let mut ahead = bytes.clone();
        match (byte, ahead.next().and_then(hex), ahead.next().and_then(hex)) {
            (b'%', Some(high), Some(low)) => {
                out.push(high * 16 + low);
                bytes = ahead;
            }
            _ => out.push(byte),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The value of one hex digit.
#[spec(fn(u8) -> Option<u8{v: v < 16}>)]
fn hex(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_written_reads_back_exactly() {
        for value in
            ["plain", "two words", "a=b", "100%", "%41", "tab\there", "-->", "日本語", "a\nb"]
        {
            let line = write("convoy:x", &[("v", value), ("n", "7")]);
            assert!(!line.contains("-->") && line.lines().count() == 1, "{line}");
            let back = read("convoy:x", &line).unwrap();
            assert_eq!(back.get("v"), Some(value), "{line}");
            assert_eq!(back.number("n"), Some(7));
        }
    }

    #[test]
    fn an_empty_field_is_not_stated() {
        let line = write("convoy:x", &[("a", ""), ("b", "1")]);
        assert_eq!(line, "convoy:x b=1");
        assert_eq!(read("convoy:x", &line).unwrap().get("a"), None);
    }

    #[test]
    fn another_kind_of_record_is_not_this_one() {
        assert!(read("convoy:claim", "convoy:claimed instance=a").is_none());
        assert!(read("convoy:claim", "something else").is_none());
        assert!(read("convoy:claim", "").is_none());
        assert!(read("convoy:claim", "convoy:claim").is_some());
    }

    #[test]
    fn only_the_fleets_spelling_is_a_number() {
        assert_eq!(fleet_number("0"), Some(0));
        assert_eq!(fleet_number("42"), Some(42));
        for odd in ["", "007", "+1", "-1", "1.0", "٧", "1234567890123456789", " 1"] {
            assert_eq!(fleet_number(odd), None, "{odd:?}");
        }
    }

    #[test]
    fn a_marker_is_found_among_prose_and_other_markers() {
        let body = "text <!--convoy:status--> more\n<!-- convoy:verdict phase=blocked blocked_by=#3, 4 -->\nwhy";
        let fields = read_marker("convoy:verdict", body).unwrap();
        assert_eq!(fields.get("phase"), Some("blocked"));
        assert_eq!(fields.numbers("blocked_by"), [3, 4]);
        assert!(read_marker("convoy:branch", body).is_none());
        assert_eq!(marker("convoy:branch", &[("name", "a/b")]), "<!--convoy:branch name=a/b-->");
    }

    #[test]
    fn every_marker_of_a_kind_is_read_back() {
        let body = "<!--convoy:account-red tree=a at=1-->\n<!--convoy:account end=2-->\n\
                    <!--convoy:account-red tree=b at=3-->";
        let reds = markers("convoy:account-red", body);
        assert_eq!(reds.len(), 2);
        assert_eq!(reds[0].get("tree"), Some("a"));
        assert_eq!(reds[1].get("tree"), Some("b"));
        assert_eq!(markers("convoy:account", body).len(), 1);
    }

    #[test]
    fn the_latest_marker_wins() {
        let bodies =
            ["<!--convoy:verdict phase=blocked-->", "chat", "<!--convoy:verdict phase=escalate-->"];
        let (at, fields) = latest_marker("convoy:verdict", bodies).unwrap();
        assert_eq!((at, fields.get("phase")), (2, Some("escalate")));
        assert!(latest_marker("convoy:branch", bodies).is_none());
    }
}
