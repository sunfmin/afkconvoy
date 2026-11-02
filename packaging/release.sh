#!/bin/sh
# What a release is made of, one step per subcommand. The fleet cuts each
# version on a Mac by the cut steps of docs/agents/afkconvoy.md: `set` on the
# base branch, then `check`, `build`, `package`, `publish` at the tag's commit.
#
#   release.sh version                   the version Cargo builds
#   release.sh set <version>             <version> in Cargo.toml, and in Cargo.lock
#                                        for the workspace's own crates
#   release.sh check <tag>               fail unless <tag> is v<version>
#   release.sh build                     both macOS binaries, Apple Silicon and
#                                        Intel, on rustup's stable toolchain
#   release.sh package                   what was built, checked, to
#                                        dist/afkconvoy-<version>-macos-<arch>.tar.gz
#   release.sh formula                   the Homebrew formula for what is in dist/
#   release.sh publish                   what is in dist/ to the tap: the archives
#                                        to its release afkconvoy-v<version>, the formula
#                                        to Formula/afkconvoy.rb. `gh` and `git`
#                                        must be allowed to write to the tap.
#
# The archives are downloaded from the tap (sunfmin/homebrew-tap), not from
# this repository: this one is private, and Homebrew downloads with no login.
#
# DIST names another directory than dist/.
set -eu
cd "$(dirname "$0")/.."

REPO=https://github.com/sunfmin/afkconvoy
TAP=sunfmin/homebrew-tap
ARCHES="arm64 x86_64"
DIST=${DIST:-dist}

version() {
    cargo pkgid -p afkconvoy | sed 's/.*[#@]//'
}

# The Rust target that builds for an arch.
target() {
    case "$1" in
    arm64) echo aarch64-apple-darwin ;;
    *) echo "$1-apple-darwin" ;;
    esac
}

archive() {
    echo "afkconvoy-$(version)-macos-$1.tar.gz"
}

# The tap's release that holds this version's archives.
tap_tag() {
    echo "afkconvoy-v$(version)"
}

sha() {
    shasum -a 256 "$1" | cut -d' ' -f1
}

case "${1:-}" in
version)
    version
    ;;
set)
    to=${2:?usage: release.sh set <version>}
    sed -i.bak "s/^version = \".*\"\$/version = \"$to\"/" Cargo.toml
    rm Cargo.toml.bak
    cargo update --quiet --workspace
    if [ "$(version)" != "$to" ]; then
        echo "release: Cargo.toml says the version $(version), not $to" >&2
        exit 1
    fi
    ;;
check)
    tag=${2:?usage: release.sh check <tag>}
    if [ "$tag" != "v$(version)" ]; then
        echo "release: the tag $tag is not v$(version), the version in Cargo.toml" >&2
        exit 1
    fi
    ;;
build)
    # The toolchain's own bin goes first on PATH. A `cargo` that is ahead of
    # rustup's there — Homebrew's — has no Intel target, and `rustup run
    # stable cargo` is not enough: the cargo it starts still finds the
    # `rustc` that is first on PATH.
    targets=$(for arch in $ARCHES; do target "$arch"; done)
    rustup=$(PATH="$PATH:$HOME/.cargo/bin" command -v rustup) || rustup=
    stable=$([ -n "$rustup" ] && "$rustup" which --toolchain stable cargo 2>/dev/null) || stable=
    installed=$([ -n "$stable" ] && "$rustup" target list --installed --toolchain stable) || installed=
    for target in $targets; do
        echo "$installed" | grep -qx "$target" && continue
        # shellcheck disable=SC2086
        cat >&2 <<EOF
release: build needs rustup's stable toolchain with the target $target, and ${rustup:+that is not installed}${rustup:-no rustup is installed (https://rustup.rs)}. With rustup, get it with:
    rustup toolchain install stable --profile minimal --target $(echo $targets)
EOF
        exit 1
    done
    PATH="$(dirname "$stable"):$PATH"
    export PATH
    for target in $targets; do
        cargo build --release --locked -p afkconvoy --target "$target"
    done
    ;;
package)
    for arch in $ARCHES; do
        built=target/$(target "$arch")/release/afkconvoy
        [ -f "$built" ] || {
            echo "release: $built is missing: build it first" >&2
            exit 1
        }
        # `afkconvoy --version` must print the released version. A binary this
        # machine cannot run (Intel, with no Rosetta) is only checked for its arch.
        lipo -archs "$built" | grep -qx "$arch" || {
            echo "release: $built is not an $arch binary" >&2
            exit 1
        }
        if arch "-$arch" /usr/bin/true 2>/dev/null; then
            said=$(arch "-$arch" "$built" --version)
            if [ "$said" != "afkconvoy $(version)" ]; then
                echo "release: $built --version says '$said', not 'afkconvoy $(version)'" >&2
                exit 1
            fi
        else
            echo "release: this machine cannot run $arch: --version was not checked" >&2
        fi
        mkdir -p "$DIST"
        tar -czf "$DIST/$(archive "$arch")" -C "$(dirname "$built")" afkconvoy
        echo "$DIST/$(archive "$arch")"
    done
    ;;
formula)
    for arch in $ARCHES; do
        [ -f "$DIST/$(archive "$arch")" ] || {
            echo "release: $DIST/$(archive "$arch") is missing: package it first" >&2
            exit 1
        }
    done
    base=https://github.com/$TAP/releases/download/$(tap_tag)
    cat <<FORMULA
class Afkconvoy < Formula
  desc "Fleet of AFK coding agents that works a GitHub backlog"
  homepage "$REPO"
  version "$(version)"

  depends_on :macos

  on_arm do
    url "$base/$(archive arm64)"
    sha256 "$(sha "$DIST/$(archive arm64)")"
  end
  on_intel do
    url "$base/$(archive x86_64)"
    sha256 "$(sha "$DIST/$(archive x86_64)")"
  end

  def install
    bin.install "afkconvoy"
  end

  def caveats
    <<~EOS
      afkconvoy starts its workers through orca. Install it by its tap's
      name (the cask Homebrew itself calls orca is another program):
        brew install --cask stablyai/orca/orca
      Have \`git\` and \`gh\` on PATH.
      \`gh\` must be logged in (\`gh auth login\`).
    EOS
  end

  test do
    assert_equal "afkconvoy #{version}", shell_output("#{bin}/afkconvoy --version").strip
  end
end
FORMULA
    ;;
publish)
    formula=$DIST/afkconvoy.rb
    "$0" formula > "$formula"
    ruby -c "$formula" >/dev/null
    archives=""
    for arch in $ARCHES; do
        archives="$archives $DIST/$(archive "$arch")"
    done
    # A release that is there already takes the archives again: a publish
    # that failed half way is run a second time.
    if gh release view "$(tap_tag)" -R "$TAP" >/dev/null 2>&1; then
        # shellcheck disable=SC2086
        gh release upload "$(tap_tag)" -R "$TAP" --clobber $archives
    else
        # shellcheck disable=SC2086
        gh release create "$(tap_tag)" -R "$TAP" --title "afkconvoy $(version)" \
            --notes "The binaries the formula \`afkconvoy\` installs." $archives
    fi
    tap=$(mktemp -d)
    gh repo clone "$TAP" "$tap" -- --quiet --depth 1
    cp "$formula" "$tap/Formula/afkconvoy.rb"
    if git -C "$tap" diff --quiet; then
        echo "release: the tap's formula is afkconvoy $(version) already"
    else
        git -C "$tap" commit --quiet -am "afkconvoy $(version)"
        git -C "$tap" push --quiet origin HEAD
    fi
    rm -rf "$tap"
    ;;
*)
    sed -n '2,23p' "$0" >&2
    exit 2
    ;;
esac
