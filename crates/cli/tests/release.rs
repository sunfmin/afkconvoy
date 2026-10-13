//! What a release is made of: the version the binary prints, and
//! `packaging/release.sh`, which the release workflow runs step by step.

use std::path::Path;
use std::process::{Command, Output};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/release.sh");

/// The build script, for what it makes of a checkout.
#[allow(dead_code)]
#[path = "../build.rs"]
mod build;

fn release(dist: &Path, args: &[&str]) -> Output {
    Command::new(SCRIPT).args(args).env("DIST", dist).output().unwrap()
}

fn stdout(out: &Output) -> String {
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn sha256(file: &Path) -> String {
    let out = Command::new("shasum").args(["-a", "256"]).arg(file).output().unwrap();
    stdout(&out).split_whitespace().next().unwrap().to_string()
}

#[test]
fn the_release_script_knows_the_version_a_release_is_tagged_with() {
    let dist = tempfile::tempdir().unwrap();
    assert_eq!(stdout(&release(dist.path(), &["version"])).trim(), VERSION);
    assert!(release(dist.path(), &["check", &format!("v{VERSION}")]).status.success());
    for wrong in [VERSION, "v0.0.0-not-this", "release"] {
        let refused = release(dist.path(), &["check", wrong]);
        assert!(!refused.status.success(), "{wrong} is not the tag of this version");
        let said = String::from_utf8_lossy(&refused.stderr);
        assert!(said.contains(&format!("is not v{VERSION}")), "{said}");
    }
}

#[test]
fn the_formula_installs_what_was_packaged_and_depends_on_git_and_gh() {
    let dist = tempfile::tempdir().unwrap();
    let archive = |arch: &str| dist.path().join(format!("afkconvoy-{VERSION}-macos-{arch}.tar.gz"));
    std::fs::write(archive("arm64"), "the arm64 build").unwrap();

    let early = release(dist.path(), &["formula"]);
    assert!(!early.status.success(), "half a release is no formula");
    assert!(String::from_utf8_lossy(&early.stderr).contains("macos-x86_64.tar.gz is missing"));

    std::fs::write(archive("x86_64"), "the x86_64 build").unwrap();
    let formula = stdout(&release(dist.path(), &["formula"]));
    let base = format!("https://github.com/sunfmin/afkconvoy/releases/download/v{VERSION}");
    for arch in ["arm64", "x86_64"] {
        let block = format!(
            "    url \"{base}/afkconvoy-{VERSION}-macos-{arch}.tar.gz\"\n    sha256 \"{}\"\n",
            sha256(&archive(arch))
        );
        assert!(formula.contains(&block), "{arch}: {formula}");
    }
    assert_ne!(sha256(&archive("arm64")), sha256(&archive("x86_64")));
    assert!(formula.contains(&format!("  version \"{VERSION}\"\n")));
    assert!(formula.contains("  depends_on \"gh\"\n  depends_on \"git\"\n"), "{formula}");
    assert!(formula.contains("bin.install \"afkconvoy\""));
    let orca = afkconvoy_core::tools::Tool::Orca.install();
    assert!(formula.contains(&format!("\n        {orca}\n")), "orca is the user's to install");
    assert!(!formula.contains("Homebrew does not"), "…with the line a refusal gives: {formula}");
    assert!(formula.contains("assert_equal \"afkconvoy #{version}\""), "{formula}");

    // it is Ruby, where there is a ruby to ask
    let file = dist.path().join("afkconvoy.rb");
    std::fs::write(&file, &formula).unwrap();
    if let Ok(checked) = Command::new("ruby").arg("-c").arg(&file).output() {
        assert!(checked.status.success(), "{}", String::from_utf8_lossy(&checked.stderr));
    }
}

#[test]
fn a_build_says_the_release_alone_only_at_its_tag_or_with_no_git() {
    use build::built_from;
    assert_eq!(built_from("0.2.0", None), "0.2.0", "a tarball");
    assert_eq!(built_from("0.2.0", Some("v0.2.0-0-g9f58035")), "0.2.0", "the tag's commit");
    assert_eq!(built_from("0.2.0", Some("v0.2.0-11-g9f58035")), "0.2.0+11.g9f58035");
    assert_eq!(built_from("0.2.0", Some("v0.2.0-11-g9f58035-dirty")), "0.2.0+11.g9f58035.dirty");
    assert_eq!(built_from("0.2.0", Some("v0.2.0-0-g9f58035-dirty")), "0.2.0+0.g9f58035.dirty");
    // a version set in Cargo.toml and not tagged yet is not the tag's release
    assert_eq!(built_from("0.3.0", Some("v0.2.0-0-g9f58035")), "0.3.0+0.g9f58035");
    assert_eq!(built_from("0.3.0-rc.1", Some("v0.3.0-rc.1-0-g9f58035")), "0.3.0-rc.1");
    assert_eq!(built_from("0.3.0", Some("v0.3.0-rc.1-4-g9f58035")), "0.3.0+4.g9f58035");
    // no tag is reachable: a shallow clone
    assert_eq!(built_from("0.2.0", Some("9f58035")), "0.2.0+g9f58035");
    assert_eq!(built_from("0.2.0", Some("9f58035-dirty")), "0.2.0+g9f58035.dirty");
}

#[test]
fn the_binary_prints_what_it_was_built_from() {
    let out = Command::new(env!("CARGO_BIN_EXE_afkconvoy")).arg("--version").output().unwrap();
    let said = stdout(&out);
    assert_eq!(said.trim(), format!("afkconvoy {}", afkconvoy::VERSION));

    // In a checkout it is the commit checked out, and with no git the release.
    let here = env!("CARGO_MANIFEST_DIR");
    let git = |args: &[&str]| Command::new("git").args(args).current_dir(here).output();
    let in_checkout = git(&["rev-parse", "--show-prefix"])
        .is_ok_and(|out| out.status.success() && out.stdout == b"crates/cli/\n");
    if !in_checkout {
        assert_eq!(afkconvoy::VERSION, VERSION);
        return;
    }
    let described = git(&["describe", "--tags", "--long", "--always", "--match", "v*"]).unwrap();
    let built = build::built_from(VERSION, Some(stdout(&described).trim()));
    let built_dirty =
        build::built_from(VERSION, Some(&format!("{}-dirty", stdout(&described).trim())));
    assert!(
        [built.as_str(), built_dirty.as_str()].contains(&afkconvoy::VERSION),
        "{} is neither {built} nor {built_dirty}",
        afkconvoy::VERSION
    );
}
