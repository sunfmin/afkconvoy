//! Stamps the binary with what it was built from, as `AFKCONVOY_VERSION`: the
//! release alone for a build of the release's tag, and otherwise the release,
//! the commits past the newest tag and the commit — `0.2.0+11.g9f58035`, with
//! `.dirty` for a tree that had changes. A build with no git around it (a
//! tarball) says the release alone.
//!
//! And lists the example teams for `scaffold --example`: each directory of
//! `src/scaffold/examples/` is one, named as the directory is, and holds the
//! files it writes exactly as it writes them. Adding a file there is all it
//! takes for the binary to hold it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Where this package is in the checkout it is built from.
const PACKAGE: &str = "crates/cli/";
/// Where the example teams are, in this package.
const EXAMPLES: &str = "src/scaffold/examples";
/// The commit a build is of, as `./gate.sh` names it to every build it runs.
pub const COMMIT_ENV: &str = "AFKCONVOY_BUILD_COMMIT";

fn main() {
    let release = env!("CARGO_PKG_VERSION");
    let described = checkout().then(describe).flatten();
    println!("cargo:rustc-env=AFKCONVOY_VERSION={}", built_from(release, described.as_deref()));
    // Asked again whenever the checkout moved or a tracked file changed. With
    // no git there is nothing to watch but this script.
    println!("cargo:rerun-if-changed=build.rs");
    // And whenever the commit the gate names changes: a candidate's gate
    // builds in its worker's target directory, where the paths watched are
    // those of the checkout this script last ran in, which a candidate whose
    // merge has its branch's tree leaves as they were (sunfmin/control#838).
    println!("cargo:rerun-if-env-changed={COMMIT_ENV}");
    println!("cargo:rerun-if-changed={EXAMPLES}");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(out.join("examples.rs"), examples()).expect("OUT_DIR can be written in");
    for path in watched().unwrap_or_default() {
        if Path::new(&path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

/// The example teams as a Rust expression: each one's name with its files,
/// each file's path in the team's directory with its text. Both are in the
/// order of their names, so that two builds of one tree hold the same.
fn examples() -> String {
    // The package's directory as cargo names it to this run, not as it was
    // when this script was built: a candidate's gate builds in its worker's
    // target directory, where cargo reuses the script built from another
    // worktree of the repo, one that may be gone (2026-10-30).
    let package = std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let root = Path::new(&package).join(EXAMPLES);
    let mut source = String::from("&[\n");
    for example in entries(&root) {
        let name = example.file_name().expect("an entry has a name").to_string_lossy().into_owned();
        let mut files = Vec::new();
        collect(&example, "", &mut files);
        writeln!(source, "    Example {{ name: {name:?}, files: &[").unwrap();
        for (path, file) in files {
            writeln!(source, "        ({path:?}, include_str!({:?})),", file.display()).unwrap();
        }
        source.push_str("    ] },\n");
    }
    source + "]\n"
}

/// What `dir` holds, in the order of the names.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let read = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    let mut entries: Vec<PathBuf> = read.map(|entry| entry.expect("an entry").path()).collect();
    entries.sort();
    entries
}

/// Every file under `dir`, each with its path from there, its segments joined
/// by `/` after `prefix`.
fn collect(dir: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) {
    for entry in entries(dir) {
        let name = entry.file_name().expect("an entry has a name").to_string_lossy();
        let path = format!("{prefix}{name}");
        if entry.is_dir() {
            collect(&entry, &format!("{path}/"), files);
        } else {
            files.push((path, entry));
        }
    }
}

/// What `--version` says after the name. `described` is what
/// `git describe --tags --long --dirty --always` said of the checkout: `None`
/// with no git, and the commit alone where no tag is reachable.
pub fn built_from(release: &str, described: Option<&str>) -> String {
    let Some(described) = described else {
        return release.to_string();
    };
    let (described, dirty) = match described.strip_suffix("-dirty") {
        Some(clean) => (clean, ".dirty"),
        None => (described, ""),
    };
    // `<tag>-<distance>-g<commit>`, and a tag may itself hold a `-`.
    let mut parts = described.rsplitn(3, '-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(commit), Some(distance), Some(tag)) => {
            if distance == "0" && dirty.is_empty() && tag.strip_prefix('v') == Some(release) {
                release.to_string()
            } else {
                format!("{release}+{distance}.{commit}{dirty}")
            }
        }
        _ => format!("{release}+g{described}{dirty}"),
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Whether the git around this build is afkconvoy's own checkout, and not a
/// repo a tarball of it was unpacked inside.
fn checkout() -> bool {
    git(&["rev-parse", "--show-prefix"]).is_some_and(|prefix| prefix == PACKAGE)
}

fn describe() -> Option<String> {
    git(&["describe", "--tags", "--long", "--dirty", "--always", "--match", "v*"])
}

/// What changes when the answer may: where git keeps the commit checked out,
/// the index and the tags, and every tracked entry of the checkout's root.
///
/// The checkout's own entries are named from this package, `../../<entry>`,
/// and `.git` with them: cargo looks for a relative path in the package it
/// builds, so a build from another worktree of the repo sharing the target
/// directory watches that worktree, and not the one this script last ran in.
/// Where git keeps a worktree's commit is outside it and is named whole.
fn watched() -> Option<Vec<String>> {
    if !checkout() {
        return None;
    }
    let dir = git(&["rev-parse", "--path-format=absolute", "--git-dir"])?;
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let mut paths = vec![
        format!("{dir}/HEAD"),
        format!("{dir}/logs/HEAD"),
        format!("{dir}/index"),
        format!("{common}/refs/tags"),
        format!("{common}/packed-refs"),
    ];
    let tracked = git(&["ls-tree", "--name-only", "--full-tree", "HEAD"])?;
    let up = "../".repeat(PACKAGE.matches('/').count());
    paths.push(format!("{up}.git"));
    paths.extend(tracked.lines().map(|entry| format!("{up}{entry}")));
    Some(paths)
}
