//! What a release is made of: the version the binary prints, and
//! `packaging/release.sh`, whose steps the fleet takes to cut a version, on a
//! Mac, as the cut steps of this repo's config say.

use std::path::Path;
use std::process::{Command, Output};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/release.sh");

const README: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md"));

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
fn the_formula_installs_what_was_packaged_and_names_the_users_tools() {
    let dist = tempfile::tempdir().unwrap();
    let archive = |arch: &str| dist.path().join(format!("afkconvoy-{VERSION}-macos-{arch}.tar.gz"));
    std::fs::write(archive("arm64"), "the arm64 build").unwrap();

    let early = release(dist.path(), &["formula"]);
    assert!(!early.status.success(), "half a release is no formula");
    assert!(String::from_utf8_lossy(&early.stderr).contains("macos-x86_64.tar.gz is missing"));

    std::fs::write(archive("x86_64"), "the x86_64 build").unwrap();
    let formula = stdout(&release(dist.path(), &["formula"]));
    // from the tap: this repository is private, and Homebrew downloads with no login
    let base =
        format!("https://github.com/sunfmin/homebrew-tap/releases/download/afkconvoy-v{VERSION}");
    for arch in ["arm64", "x86_64"] {
        let block = format!(
            "    url \"{base}/afkconvoy-{VERSION}-macos-{arch}.tar.gz\"\n    sha256 \"{}\"\n",
            sha256(&archive(arch))
        );
        assert!(formula.contains(&block), "{arch}: {formula}");
    }
    assert_ne!(sha256(&archive("arm64")), sha256(&archive("x86_64")));
    assert!(formula.contains(&format!("  version \"{VERSION}\"\n")));
    assert!(formula.contains("  depends_on :macos\n"), "{formula}");
    for tool in ["git", "gh", "orca"] {
        assert!(!formula.contains(&format!("depends_on \"{tool}\"")), "{formula}");
    }
    assert!(formula.contains("Have `git` and `gh` on PATH."), "{formula}");
    assert!(formula.contains("`gh` must be logged in (`gh auth login`)."), "{formula}");
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
fn a_build_with_no_stable_toolchain_says_how_to_get_it() {
    let home = tempfile::tempdir().unwrap();
    let refused = Command::new(SCRIPT)
        .arg("build")
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(!refused.status.success(), "there is no rustup to build with");
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("no rustup is installed (https://rustup.rs)"), "{said}");
    let install = "rustup toolchain install stable --profile minimal \
                   --target aarch64-apple-darwin x86_64-apple-darwin";
    assert!(said.contains(install), "{said}");
}

#[test]
fn a_version_is_set_in_cargo_toml_and_in_cargo_lock_for_the_workspace_s_crates() {
    // A workspace shaped like this one: its version under [workspace.package],
    // a dependency's beside it, and a lock that says the version before.
    let root = tempfile::tempdir().unwrap();
    let at = |path: &str, text: &str| {
        let file = root.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    };
    at("packaging/release.sh", &std::fs::read_to_string(SCRIPT).unwrap());
    let script = root.path().join("packaging/release.sh");
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    at(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"crates/cli\"]\n\n[workspace.package]\n\
         version = \"0.5.16\"\nedition = \"2024\"\n\n[workspace.dependencies]\n\
         serde = { version = \"1\" }\n",
    );
    at(
        "crates/cli/Cargo.toml",
        "[package]\nname = \"afkconvoy\"\nversion.workspace = true\nedition.workspace = true\n",
    );
    at("crates/cli/src/lib.rs", "");
    at("Cargo.lock", "version = 4\n\n[[package]]\nname = \"afkconvoy\"\nversion = \"0.5.16\"\n");

    let set = |args: &[&str]| {
        Command::new(&script).args(args).env("CARGO_NET_OFFLINE", "true").output().unwrap()
    };
    stdout(&set(&["set", "0.5.17"]));
    assert_eq!(stdout(&set(&["version"])).trim(), "0.5.17");
    let read = |path: &str| std::fs::read_to_string(root.path().join(path)).unwrap();
    let lock = read("Cargo.lock");
    assert!(lock.contains("name = \"afkconvoy\"\nversion = \"0.5.17\"\n"), "{lock}");
    let toml = read("Cargo.toml");
    assert!(toml.contains("\nversion = \"0.5.17\"\n"), "{toml}");
    assert!(
        toml.contains("serde = { version = \"1\" }"),
        "a dependency's is not the version: {toml}"
    );
    assert!(!root.path().join("Cargo.toml.bak").exists(), "nothing is left beside it");
    assert!(!set(&["set"]).status.success(), "no version is set to nothing");
}

#[test]
fn a_version_is_cut_by_the_steps_the_config_gives_and_the_readme_points_to_them() {
    // This repository is private and its GitHub Actions has no budget: the
    // fleet cuts on a Mac, by the cut steps of this repo's config, and
    // nothing else runs a release.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(!root.join(".github/workflows").exists(), "a release is cut on a Mac alone");
    let config = afkconvoy::config::load(&root.join(afkconvoy::config::FILE)).unwrap();
    let steps = config.cut.expect("this repo turns cutting on: its config names cut steps");

    // Each step stops at the first line that fails: `sh` alone runs on past
    // it, and judges the step by its last line.
    for lines in [&steps.set, &steps.publish] {
        assert!(lines.starts_with("set -ex\n"), "{lines}");
    }
    assert!(steps.set.contains("\npackaging/release.sh set \"$VERSION\"\n"), "{}", steps.set);
    let mut last = 0;
    for line in [
        "packaging/release.sh check ",
        "packaging/release.sh build",
        "packaging/release.sh package",
        "packaging/release.sh publish",
        "brew update",
        "brew upgrade sunfmin/tap/afkconvoy",
        "--version)\" = \"afkconvoy $VERSION\"",
    ] {
        let at = steps.publish.find(line).unwrap_or_else(|| panic!("publish runs no {line}"));
        assert!(at > last, "{line} is out of order: {}", steps.publish);
        last = at;
    }

    let section = &README[README.find("**Cutting a release**").unwrap()..];
    let section = &section[..section.find("\n## ").unwrap_or(section.len())];
    assert!(section.contains("(docs/agents/afkconvoy.md)"), "the README points to the steps");
    for step in ["set", "check", "build", "package", "publish"] {
        let line = format!("release.sh {step}");
        assert!(!section.contains(&line), "the README repeats the cut step {step}");
    }
    let script = std::fs::read_to_string(SCRIPT).unwrap();
    let cut = format!("{}{}", steps.set, steps.publish);
    for (what, text) in [("the README", section), ("release.sh", &script), ("the cut steps", &cut)]
    {
        for named in ["workflow", "secret", "HOMEBREW_TAP_TOKEN"] {
            assert!(!text.contains(named), "{what} names a {named}: a release is cut on a Mac");
        }
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

/// A build from another worktree of the repo, or at another commit with the
/// same tree, sharing one target directory, says the commit it is built from
/// (sunfmin/control#838): a candidate's gate builds its merge in its worker's
/// target directory, and a merge may have its branch's tree.
#[test]
fn a_build_sharing_a_target_directory_says_the_commit_it_is_built_from() {
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    let other = tmp.path().join("other");
    let target = tmp.path().join("target");
    let git = |dir: &Path, args: &[&str]| -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        stdout(&out).trim().to_string()
    };
    // A package laid out as this one is, with this build script, saying only
    // what it was built from. It is a workspace of its own, wherever the
    // tests' TMPDIR is.
    let package = main.join("crates/cli");
    std::fs::create_dir_all(package.join("src/scaffold/examples/one")).unwrap();
    std::fs::write(
        package.join("Cargo.toml"),
        "[package]\nname = \"stamp\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(package.join("build.rs"), include_str!("../build.rs")).unwrap();
    std::fs::write(
        package.join("src/main.rs"),
        "fn main() { print!(\"{}\", env!(\"AFKCONVOY_VERSION\")); }\n",
    )
    .unwrap();
    std::fs::write(package.join("src/scaffold/examples/one/file"), "a file\n").unwrap();
    std::fs::write(main.join("README.md"), "the root\n").unwrap();
    git(&main, &["init", "-q", "-b", "main"]);
    git(&main, &["add", "."]);
    git(&main, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "one"]);
    let first = git(&main, &["rev-parse", "HEAD"]);
    // A merge that has its branch's tree.
    let merge = git(
        &main,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit-tree",
            "HEAD^{tree}",
            "-p",
            "HEAD",
            "-m",
            "merge",
        ],
    );
    // What a build in `dir` says, and what it should, with the commit the
    // gate names to it, or none.
    let build = |dir: &Path, commit: Option<&str>| -> (String, String) {
        let mut cargo = Command::new(env!("CARGO"));
        cargo
            .args(["run", "--quiet", "--manifest-path", "crates/cli/Cargo.toml"])
            .current_dir(dir)
            .env("CARGO_TARGET_DIR", &target)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE");
        match commit {
            Some(commit) => cargo.env(build::COMMIT_ENV, commit),
            None => cargo.env_remove(build::COMMIT_ENV),
        };
        let said = stdout(&cargo.output().unwrap());
        let described = git(dir, &["describe", "--tags", "--long", "--dirty", "--always"]);
        (said, build::built_from("0.1.0", Some(&described)))
    };
    let (said, built) = build(&main, None);
    assert_eq!(said, built, "the first build");

    // A worktree made since watches its own paths, though cargo last ran the
    // build script in another.
    git(&main, &["worktree", "add", "-q", "--detach", other.to_str().unwrap(), &merge]);
    let (said, built) = build(&other, None);
    assert_eq!(said, built, "a build in a new worktree, at a merge with its branch's tree");

    // Back and forth between two worktrees, neither changed since the last
    // build, as a worker's checkout and its candidate's are: the commit the
    // gate names tells them apart.
    for (dir, commit) in [(&main, &first), (&other, &merge), (&main, &first)] {
        let (said, built) = build(dir, Some(commit));
        assert_eq!(said, built, "a gate's build in {}", dir.display());
    }
}
