//! Stamps the binary with what it was built from, as `AFKCONVOY_VERSION`: the
//! release alone for a build of the release's tag, and otherwise the release,
//! the commits past the newest tag and the commit — `0.2.0+11.g9f58035`, with
//! `.dirty` for a tree that had changes. A build with no git around it (a
//! tarball) says the release alone.

use std::path::Path;
use std::process::Command;

/// Where this package is in the checkout it is built from.
const PACKAGE: &str = "crates/cli/";

fn main() {
    let release = env!("CARGO_PKG_VERSION");
    let described = checkout().then(describe).flatten();
    println!("cargo:rustc-env=AFKCONVOY_VERSION={}", built_from(release, described.as_deref()));
    // Asked again whenever the checkout moved or a tracked file changed. With
    // no git there is nothing to watch but this script.
    println!("cargo:rerun-if-changed=build.rs");
    for path in watched().unwrap_or_default() {
        if Path::new(&path).exists() {
            println!("cargo:rerun-if-changed={path}");
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
    let root = git(&["rev-parse", "--show-toplevel"])?;
    let tracked = git(&["ls-tree", "--name-only", "--full-tree", "HEAD"])?;
    paths.extend(tracked.lines().map(|entry| format!("{root}/{entry}")));
    Some(paths)
}
