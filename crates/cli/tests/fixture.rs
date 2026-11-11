//! The fixture itself: what every later test stands on.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::common;

use common::{BASE, World, git};

#[test]
fn the_bare_repo_is_the_clones_remote() {
    let world = World::new();
    std::fs::write(world.clone.join("a.txt"), "a\n").unwrap();
    git(&world.clone, &["add", "."]);
    git(&world.clone, &["commit", "-q", "-m", "a"]);
    git(&world.clone, &["push", "-q", "origin", BASE]);
    assert_eq!(world.remote(&["rev-parse", BASE]), git(&world.clone, &["rev-parse", "HEAD"]));
    let began = std::time::Instant::now();
    drop(world);
    common::fixture_timing("world teardown", began);
}

#[test]
fn gh_and_orca_on_path_are_the_stand_ins() {
    let world = World::new();
    let gh = world.command("gh", &world.clone).args(["issue", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&gh.stderr).contains("fake gh"));
    assert_eq!(world.gh()["calls"], serde_json::json!([["issue", "list"]]));

    let orca =
        world.command("orca", &world.clone).args(["no-such-call", "--json"]).output().unwrap();
    let answer: serde_json::Value = serde_json::from_slice(&orca.stdout).unwrap();
    assert_eq!(answer["ok"], false);
    assert_eq!(world.orca()["calls"], serde_json::json!([["no-such-call", "--json"]]));
}

#[test]
fn repository_templates_leave_each_world_with_its_own_writable_git_history() {
    let first = World::new();
    let second = World::new();
    first.reuse_repository_templates();
    second.reuse_repository_templates();
    // A unique input makes each repetition exercise cold publication, as
    // well as concurrent readers. Only immutable seed data crosses Worlds.
    let original_text = format!("the original in {}\n", first.root.display());
    let files = [("a directory/skill.md", original_text.as_str())];
    let ready = std::sync::Barrier::new(2);
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            ready.wait();
            first.repo("acme/role with spaces", &files)
        });
        let b = scope.spawn(|| {
            ready.wait();
            second.repo("acme/role with spaces", &files)
        });
        (a.join().unwrap(), b.join().unwrap())
    });
    let original = git(&a, &["rev-parse", "HEAD"]);
    assert_eq!(git(&b, &["rev-parse", "HEAD"]), original);
    assert_eq!(git(&b, &["rev-list", "--count", "HEAD"]), "2");
    for relative in ["a directory/skill.md", ".git/index", ".git/refs/heads/main"] {
        assert_ne!(
            std::fs::metadata(a.join(relative)).unwrap().ino(),
            std::fs::metadata(b.join(relative)).unwrap().ino(),
            "{relative} must not be hard-linked"
        );
    }
    let object = format!(".git/objects/{}/{}", &original[..2], &original[2..]);
    assert_ne!(
        std::fs::metadata(a.join(&object)).unwrap().ino(),
        std::fs::metadata(b.join(&object)).unwrap().ino(),
        "Git objects must be independent too"
    );
    let seed = common::repository_template(&files);
    assert_read_only(seed.parent().unwrap());
    assert_eq!(std::fs::read_to_string(seed.join("a directory/skill.md")).unwrap(), original_text);
    let copied = std::fs::metadata(a.join("a directory/skill.md")).unwrap().permissions().mode();
    assert_ne!(copied & 0o200, 0, "a world's copy restores the write bit");
    common::commit(&a, "a directory/skill.md", "changed\n");
    std::fs::set_permissions(a.join(".git/index"), std::fs::Permissions::from_mode(0o400)).unwrap();
    drop(first);
    assert!(!a.exists(), "even the read-only index is cleaned up");
    assert_eq!(git(&b, &["rev-parse", "HEAD"]), original);
    assert_eq!(std::fs::read_to_string(b.join("a directory/skill.md")).unwrap(), original_text);
    let c = second.repo("acme/a later copy", &files);
    assert_eq!(git(&c, &["rev-parse", "HEAD"]), original, "the seed stays intact");
    assert_eq!(git(&c, &["status", "--porcelain"]), "");
    common::commit(&b, "a directory/skill.md", "independent\n");
    assert_eq!(git(&b, &["rev-list", "--count", "HEAD"]), "3");
    assert_eq!(git(&c, &["rev-list", "--count", "HEAD"]), "2");
    assert_eq!(
        std::fs::read_to_string(seed.join("a directory/skill.md")).unwrap(),
        original_text,
        "the seed stays intact"
    );
}

#[test]
fn a_repository_template_seed_is_kept_outside_the_checkout() {
    let files = [("a directory/skill.md", "outside the checkout\n")];
    let cache = afkconvoy::copies::cache().unwrap();
    let cache_existed = cache.exists();
    let seed = canonical(&common::repository_template(&files));
    let process_temp = canonical(&std::env::temp_dir());
    let checkout = this_checkout();
    let build = this_build_output();
    let cache_at = if cache_existed { canonical(&cache) } else { cache.clone() };

    assert!(!seed.starts_with(&checkout), "the seed is inside the checkout: {}", seed.display());
    assert!(!seed.starts_with(&build), "the seed is inside the build output: {}", seed.display());
    assert!(
        !seed.starts_with(&cache_at),
        "the seed is inside the fleet's cache directory: {}",
        seed.display()
    );
    if !cache_existed {
        assert!(!cache.exists(), "publishing a seed created the fleet's cache directory");
    }
    let temp_blocked = process_temp.starts_with(&checkout)
        || process_temp.starts_with(&build)
        || process_temp.starts_with(&cache_at);
    let allowed = if temp_blocked { canonical(Path::new("/tmp")) } else { process_temp };
    assert!(
        seed.starts_with(&allowed),
        "the seed {} is not under the process temp directory {}",
        seed.display(),
        allowed.display()
    );

    let templates = seed.parent().unwrap().parent().unwrap();
    assert_eq!(templates.file_name().unwrap(), "afkconvoy-repository-templates-v1");
    let held = std::fs::metadata(templates).unwrap();
    assert_eq!(held.uid(), std::fs::metadata(std::env::current_exe().unwrap()).unwrap().uid());
    assert_ne!(
        held.permissions().mode() & 0o200,
        0,
        "the seed directory is not writable by its owner"
    );
    assert_read_only(seed.parent().unwrap());
}

#[test]
fn a_seed_stays_outside_a_checkout_that_holds_the_process_temp() {
    let checkout = tempfile::tempdir().unwrap();
    let inside = tempfile::tempdir_in(checkout.path()).unwrap();
    let build = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();

    let blocked = common::repository_template_home(
        inside.path(),
        Some(checkout.path()),
        Some(build.path()),
        Some(cache.path()),
    );
    assert_eq!(blocked, PathBuf::from("/tmp/afkconvoy-repository-templates-v1"));
    assert!(!path_is_inside(&blocked, checkout.path()));
    assert!(!path_is_inside(&blocked, build.path()));
    assert!(!path_is_inside(&blocked, cache.path()));

    let kept = common::repository_template_home(outside.path(), Some(checkout.path()), None, None);
    assert!(path_is_inside(&kept, outside.path()));
    assert!(!path_is_inside(&kept, checkout.path()));

    let in_build = common::repository_template_home(build.path(), None, Some(build.path()), None);
    assert!(!path_is_inside(&in_build, build.path()));
    let in_cache = common::repository_template_home(cache.path(), None, None, Some(cache.path()));
    assert!(!path_is_inside(&in_cache, cache.path()));
}

#[test]
fn two_publishers_of_the_same_inputs_leave_one_seed() {
    let files = [("a directory/skill.md", "one published seed\n")];
    let ready = std::sync::Barrier::new(2);
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            ready.wait();
            common::publish_repository_template(&files, "the same builder")
        });
        let second = scope.spawn(|| {
            ready.wait();
            common::publish_repository_template(&files, "the same builder")
        });
        (first.join().unwrap(), second.join().unwrap())
    });
    assert_eq!(first, second, "two publishers left two seeds");
    assert!(first.is_dir());
    assert_read_only(first.parent().unwrap());
}

#[test]
fn a_seed_swept_of_its_manifest_is_rebuilt() {
    let files = [("a directory/skill.md", "swept of its manifest\n")];
    let builder = format!("a builder whose manifest is swept {}", std::process::id());
    let broken = common::publish_repository_template(&files, &builder);
    let kept = broken.parent().unwrap();
    sweep(kept, |path| path == kept.join("manifest.json"));
    assert!(!kept.join("manifest.json").exists());

    let rebuilt = common::publish_repository_template(&files, &builder);
    assert_complete_seed(&rebuilt, "swept of its manifest\n");
    assert!(kept.is_dir(), "the broken seed is left aside, not removed");
    assert_eq!(common::publish_repository_template(&files, &builder), rebuilt);
    std::fs::remove_dir_all(kept).unwrap();
}

#[test]
fn a_seed_swept_of_its_repo_files_is_rebuilt() {
    let files = [("a directory/skill.md", "swept of its repo files\n")];
    let builder = format!("a builder whose repo files are swept {}", std::process::id());
    let broken = common::publish_repository_template(&files, &builder);
    let kept = broken.parent().unwrap().to_path_buf();
    sweep(&kept, |path| path.starts_with(&broken));
    assert!(broken.join(".git").is_dir(), "the sweep leaves directories");
    assert!(!broken.join("a directory/skill.md").exists());

    let rebuilt = common::publish_repository_template(&files, &builder);
    assert_ne!(rebuilt, broken);
    assert_complete_seed(&rebuilt, "swept of its repo files\n");
    assert!(broken.is_dir(), "the broken seed is left aside, not removed");
    std::fs::remove_dir_all(&kept).unwrap();
}

#[test]
fn a_world_takes_a_rebuilt_seed_in_place_of_a_swept_one() {
    let world = World::new();
    world.reuse_repository_templates();
    let text = format!("a world past a swept seed in {}\n", world.root.display());
    let files = [("a directory/skill.md", text.as_str())];
    let broken = common::repository_template(&files);
    let kept = broken.parent().unwrap().to_path_buf();
    sweep(&kept, |path| path.is_file());

    let repo = world.repo("acme/past a sweep", &files);
    assert_eq!(git(&repo, &["rev-list", "--count", "HEAD"]), "2");
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");
    assert_eq!(std::fs::read_to_string(repo.join("a directory/skill.md")).unwrap(), text);
    std::fs::remove_dir_all(&kept).unwrap();
}

/// Deletes the files under the seed `kept` that `swept` picks, as the OS's
/// `/tmp` sweep does, and leaves every directory. Thaws the seed to do so.
fn sweep(kept: &Path, swept: impl Fn(&Path) -> bool) {
    let thawed = Command::new("chmod").args(["-R", "u+w"]).arg(kept).status().unwrap();
    assert!(thawed.success(), "thawing the seed to sweep it");
    let mut pending = vec![kept.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if std::fs::symlink_metadata(&path).unwrap().is_dir() {
                pending.push(path);
            } else if swept(&path) {
                std::fs::remove_file(&path).unwrap();
            }
        }
    }
}

/// `seed` is a published, read-only repo holding `text`, whose history a
/// clone reads in full.
fn assert_complete_seed(seed: &Path, text: &str) {
    assert_eq!(std::fs::read_to_string(seed.join("a directory/skill.md")).unwrap(), text);
    assert_eq!(git(seed, &["rev-list", "--count", "HEAD"]), "2");
    assert_eq!(git(seed, &["fsck", "--no-progress"]), "");
    assert_read_only(seed.parent().unwrap());
}

#[test]
fn a_losing_publisher_thaws_its_scratch_and_that_scratch_is_removed() {
    let templates = tempfile::tempdir().unwrap();
    let kept = templates.path().join("kept");
    std::fs::create_dir(&kept).unwrap();
    std::fs::write(kept.join("manifest.json"), b"winner").unwrap();
    let scratch = templates.path().join("scratch");
    std::fs::create_dir_all(scratch.join("repo/a directory")).unwrap();
    std::fs::write(scratch.join("repo/a directory/skill.md"), "loser\n").unwrap();

    common::publish_scratch(&scratch, &kept);

    let nested = std::fs::metadata(scratch.join("repo/a directory")).unwrap().permissions().mode();
    assert_ne!(nested & 0o200, 0, "the losing scratch stays read-only");
    std::fs::remove_dir_all(&scratch).unwrap();
    assert!(!scratch.exists(), "the losing scratch is still there");
    assert_eq!(std::fs::read(kept.join("manifest.json")).unwrap(), b"winner");
    let held = std::fs::metadata(templates.path()).unwrap().permissions().mode();
    assert_ne!(held & 0o200, 0, "publishing took the write bit off the directory that holds seeds");
}

#[test]
fn a_changed_input_or_builder_publishes_a_new_seed_and_an_unchanged_one_is_reused() {
    let files = [("a directory/skill.md", "the inputs\n")];
    let seed = common::publish_repository_template(&files, "builder one");
    let head = seed.join(".git/HEAD");
    let inode = std::fs::metadata(&head).unwrap().ino();
    let again = common::publish_repository_template(&files, "builder one");
    assert_eq!(again, seed, "an unchanged builder published a second seed");
    assert_eq!(
        std::fs::metadata(&head).unwrap().ino(),
        inode,
        "an unchanged builder rebuilt the seed"
    );

    let rebuilt = common::publish_repository_template(&files, "builder two");
    assert_ne!(rebuilt, seed, "a changed builder reused the seed");
    let changed = common::publish_repository_template(
        &[("a directory/skill.md", "other inputs\n")],
        "builder one",
    );
    assert_ne!(changed, seed, "changed inputs reused the seed");
    assert_ne!(changed, rebuilt);

    let wired =
        common::repository_template(&[("a directory/skill.md", "wired to the builder source\n")]);
    let same = common::publish_repository_template(
        &[("a directory/skill.md", "wired to the builder source\n")],
        include_str!("common/mod.rs"),
    );
    assert_eq!(wired, same, "the published seed hashes the builder source");
    let templates = seed.parent().unwrap().parent().unwrap();
    let held = std::fs::metadata(templates).unwrap().permissions().mode();
    assert_ne!(held & 0o200, 0, "a later seed cannot be published");
    assert!(rebuilt.is_dir() && changed.is_dir());
}

#[test]
fn a_worktree_removes_after_a_repository_template_is_published() {
    let base = tempfile::tempdir().unwrap();
    let main = base.path().join("main");
    let linked = base.path().join("linked");
    std::fs::create_dir(&main).unwrap();
    git(&main, &["init", "-q", "-b", BASE]);
    git(&main, &["commit", "-q", "--allow-empty", "-m", "initial"]);
    git(&main, &["worktree", "add", "--detach", linked.to_str().unwrap()]);

    let build = linked.join("target/debug/deps");
    std::fs::create_dir_all(&build).unwrap();
    std::fs::write(build.join("cli-test"), b"binary").unwrap();
    let seed = canonical(&common::repository_template(&[(
        "a directory/skill.md",
        "published from a worktree\n",
    )]));
    assert!(!seed.starts_with(canonical(&linked)), "the seed landed in the worktree");
    assert!(!seed.starts_with(canonical(&main)), "the seed landed in the main checkout");
    assert!(!seed.starts_with(this_checkout()), "the seed landed in this checkout");
    assert!(!seed.starts_with(this_build_output()), "the seed landed in the build output");

    let removed =
        Command::new("rm").args(["-rf", "--"]).arg(linked.join("target")).status().unwrap();
    assert!(removed.success(), "removing the worktree's build output");
    assert!(!linked.join("target").exists());
    git(&main, &["worktree", "remove", "--force", linked.to_str().unwrap()]);
    assert!(!linked.exists(), "the worktree is still there");
}

/// Every file and directory under `dir` has no write bit. A symlink's mode
/// is not a permission on its target.
fn assert_read_only(dir: &Path) {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(path) = pending.pop() {
        let meta = std::fs::symlink_metadata(&path).unwrap();
        if meta.file_type().is_symlink() {
            continue;
        }
        assert_eq!(
            meta.permissions().mode() & 0o222,
            0,
            "{} is writable ({:o})",
            path.display(),
            meta.permissions().mode()
        );
        if meta.is_dir() {
            for entry in std::fs::read_dir(&path).unwrap() {
                pending.push(entry.unwrap().path());
            }
        }
    }
}

fn this_checkout() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let top = git(exe.parent().unwrap(), &["rev-parse", "--show-toplevel"]);
    canonical(Path::new(&top))
}

fn this_build_output() -> PathBuf {
    let exe = canonical(&std::env::current_exe().unwrap());
    let mut dir = exe.parent().unwrap().to_path_buf();
    loop {
        let profile = dir.file_name().is_some_and(|name| name == "debug" || name == "release");
        if profile {
            let target = dir.parent().unwrap().to_path_buf();
            assert_eq!(target.file_name().unwrap(), "target");
            assert!(exe.starts_with(&target));
            return target;
        }
        assert!(dir.pop(), "the test binary is outside a cargo target directory");
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether `path` is `root` or under it, following symlinks of the parts
/// that exist. `/tmp` is `/private/tmp` once the real directory is named.
fn path_is_inside(path: &Path, root: &Path) -> bool {
    resolved(path).starts_with(resolved(root))
}

fn resolved(path: &Path) -> PathBuf {
    let mut acc = PathBuf::new();
    for component in path.components() {
        acc.push(component);
        if let Ok(canon) = std::fs::canonicalize(&acc) {
            acc = canon;
        }
    }
    acc
}
