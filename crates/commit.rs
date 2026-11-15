//! The build script of every package of this workspace but the cli, whose own
//! script says the same: it tells apart each build by the commit the gate
//! names to it.
//!
//! A candidate's gate builds its merge in its worker's target directory, and
//! cargo calls a workspace crate fresh by the times of its files alone. The
//! worker's own next build, of files older than what the candidate built from
//! another tree, found that build fresh and linked a crate its tree does not
//! have (sunfmin/control#1114). Run again whenever the commit changes, this
//! script makes cargo build its package again, every target of it.

/// The commit a build is of, as `./gate.sh` names it to every build it runs.
const COMMIT_ENV: &str = "AFKCONVOY_BUILD_COMMIT";

fn main() {
    println!("cargo:rerun-if-env-changed={COMMIT_ENV}");
}
