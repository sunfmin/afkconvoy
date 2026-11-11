//! Where on the remote the gate's records are kept: two ways of naming a ref
//! a host writes, and the kind gate records are kept under. A record is read
//! from either layout, and written on the one the checkout noted.

/// One way of naming every ref a host writes. There are two and no third.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Refs no branch rule and no push-triggered CI sees.
    Hidden,
    /// Ordinary branches, for a remote that takes nothing else.
    Branches,
}

struct Row {
    name: &'static str,
    root: &'static str,
}

impl Layout {
    /// Every layout, in the order a start asks the remote about them.
    pub const ALL: [Layout; 2] = [Layout::Hidden, Layout::Branches];

    /// The table: everything that differs between the layouts.
    fn row(self) -> Row {
        match self {
            Layout::Hidden => Row { name: "hidden", root: "refs/convoy/" },
            Layout::Branches => Row { name: "branches", root: "refs/heads/convoy-" },
        }
    }

    /// What the layout is written down as.
    pub fn name(self) -> &'static str {
        self.row().name
    }

    /// The layout written down as `name`.
    pub fn named(name: &str) -> Option<Layout> {
        Layout::ALL.into_iter().find(|layout| layout.name() == name)
    }

    /// The ref `rest` is kept at: `gate/<key>`, ….
    pub fn under(self, rest: &str) -> String {
        format!("{}{rest}", self.row().root)
    }

    /// The pattern every ref of `kind` matches (`gate/`), or with no kind
    /// every ref kept under the layout.
    pub fn every(self, kind: &str) -> String {
        self.under(&format!("{kind}*"))
    }

    /// What `under` was given to make `refname`; `None` for a ref that is not
    /// kept under the layout.
    pub fn kept(self, refname: &str) -> Option<&str> {
        refname.strip_prefix(self.row().root)
    }

    /// The ref one probe pushes to ask whether the remote takes refs of `kind`.
    /// `token` is the probe's own, so two starts probing at once never meet on
    /// one ref.
    pub fn probe(self, kind: &str, token: &str) -> String {
        self.under(&format!("{kind}{PROBE}{token}"))
    }

    /// Whether `refname` is a probe's: one left behind is litter, never a record.
    pub fn is_probe(self, refname: &str) -> bool {
        self.kept(refname).is_some_and(|kept| {
            kept.starts_with(PROBE) || kept.strip_prefix(GATE_KIND).is_some_and(probe_leaf)
        })
    }

    /// The other of the two layouts.
    pub fn other(self) -> Layout {
        match self {
            Layout::Hidden => Layout::Branches,
            Layout::Branches => Layout::Hidden,
        }
    }
}

const PROBE: &str = "probe-";
/// The kind gate records are kept under.
pub const GATE_KIND: &str = "gate/";

fn probe_leaf(leaf: &str) -> bool {
    leaf.starts_with(PROBE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_two_layouts_and_each_reads_back() {
        assert_eq!(Layout::Hidden.under("claim/7"), "refs/convoy/claim/7");
        assert_eq!(Layout::Branches.under("claim/7"), "refs/heads/convoy-claim/7");
        assert_eq!(Layout::Branches.every(""), "refs/heads/convoy-*");
        assert_eq!(Layout::Hidden.every(GATE_KIND), "refs/convoy/gate/*");
        for layout in Layout::ALL {
            assert_eq!(Layout::named(layout.name()), Some(layout));
            assert_eq!(layout.kept(&layout.under("train/line/main")), Some("train/line/main"));
        }
        assert_eq!(Layout::named("tags"), None);
        assert_eq!(Layout::Hidden.kept("refs/heads/convoy-claim/7"), None);
        assert_eq!(Layout::Branches.kept("refs/heads/main"), None);
    }

    #[test]
    fn a_probe_is_told_from_a_record() {
        for layout in Layout::ALL {
            assert!(layout.is_probe(&layout.probe("", "17-4")));
            assert!(layout.is_probe(&layout.probe(GATE_KIND, "17-4")));
            assert!(!layout.is_probe(&layout.under("claim/7")));
            // An instance may be named anything its grammar allows.
            assert!(!layout.is_probe(&layout.under("heartbeat/probe-1")));
            assert!(!layout.is_probe(&layout.under("gate/4b825dc-00ff")));
        }
        assert_ne!(Layout::Hidden.probe("", "1"), Layout::Hidden.probe("", "2"));
    }
}
