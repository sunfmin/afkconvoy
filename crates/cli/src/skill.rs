//! `afkconvoy skill`: the skill file an agent installs to use afkconvoy,
//! held in the binary.

/// The skill file `skill` prints, and the one `npx skills add` installs: a
/// user's own agent learns from it that afkconvoy is there.
pub(crate) const SKILL: &str = include_str!("../../../skills/afkconvoy/SKILL.md");
