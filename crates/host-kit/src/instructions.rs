//! The operator's instructions file, linked at each harness's global
//! instructions location (`Harness::instructions`) under the home.

use std::path::Path;

use afkconvoy_harness::Harness;
use anyhow::{Context, Result};

/// Why `harness`'s global instructions under `home` are not the declared
/// `file`; `None` where they are a link to it.
pub fn unlinked(home: &Path, harness: Harness, file: &Path) -> Option<String> {
    let at = home.join(harness.instructions());
    let shown = format!("~/{}", harness.instructions());
    match std::fs::symlink_metadata(&at) {
        Err(_) => Some(format!("{shown} is missing")),
        Ok(meta) if !meta.file_type().is_symlink() => Some(format!("{shown} is not a link")),
        Ok(_) => {
            let to = std::fs::canonicalize(&at).ok();
            let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
            (to.as_ref() != Some(&file))
                .then(|| format!("{shown} links elsewhere than {}", file.display()))
        }
    }
}

/// Links `harness`'s global instructions under `home` to `file` → whether it
/// changed anything. What stood there and was no link to it is kept beside
/// it, as `<name>.before-agent-setup`, never removed.
pub fn link(home: &Path, harness: Harness, file: &Path, now: u64) -> Result<bool> {
    if unlinked(home, harness, file).is_none() {
        return Ok(false);
    }
    let at = home.join(harness.instructions());
    let parent = at.parent().context("an instructions location has a directory")?;
    std::fs::create_dir_all(parent).with_context(|| format!("cannot make {}", parent.display()))?;
    if std::fs::symlink_metadata(&at).is_ok() {
        let name = at.file_name().unwrap_or_default().to_string_lossy();
        let mut aside = at.with_file_name(format!("{name}.before-agent-setup"));
        if std::fs::symlink_metadata(&aside).is_ok() {
            aside = at.with_file_name(format!("{name}.before-agent-setup.{now}"));
        }
        std::fs::rename(&at, &aside)
            .with_context(|| format!("cannot move {} aside", at.display()))?;
    }
    std::os::unix::fs::symlink(file, &at)
        .with_context(|| format!("cannot link {} to {}", at.display(), file.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_are_linked_and_what_stood_there_is_kept_aside() {
        let home = tempfile::tempdir().unwrap();
        let declared = home.path().join("dotfiles/CLAUDE.md");
        std::fs::create_dir_all(declared.parent().unwrap()).unwrap();
        std::fs::write(&declared, "principles\n").unwrap();
        let at = home.path().join(".codex/AGENTS.md");
        assert!(unlinked(home.path(), Harness::Codex, &declared).unwrap().contains("missing"));
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(&at, "mine\n").unwrap();
        assert!(unlinked(home.path(), Harness::Codex, &declared).unwrap().contains("not a link"));
        assert!(link(home.path(), Harness::Codex, &declared, 7).unwrap());
        assert_eq!(unlinked(home.path(), Harness::Codex, &declared), None);
        assert_eq!(std::fs::read_to_string(&at).unwrap(), "principles\n");
        let aside = home.path().join(".codex/AGENTS.md.before-agent-setup");
        assert_eq!(std::fs::read_to_string(aside).unwrap(), "mine\n");
        assert!(!link(home.path(), Harness::Codex, &declared, 8).unwrap(), "linked already");
        // a link elsewhere is moved aside too, beside the first kept
        let other = home.path().join("other.md");
        std::fs::write(&other, "other\n").unwrap();
        std::fs::remove_file(&at).unwrap();
        std::os::unix::fs::symlink(&other, &at).unwrap();
        assert!(unlinked(home.path(), Harness::Codex, &declared).unwrap().contains("elsewhere"));
        assert!(link(home.path(), Harness::Codex, &declared, 9).unwrap());
        assert!(home.path().join(".codex/AGENTS.md.before-agent-setup.9").exists());
    }
}
